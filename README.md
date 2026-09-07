# MacAI

<p align="center">
  <img src="logo.png" alt="MacAI logo" width="280">
</p>

MacAI 是面向 Apple Silicon 的本地 AI 运行时。系统以 Rust 守护进程 `aiworkd` 为核心权威，统一管理模型注册、推理 worker 进程、内存调度与 HTTP API；CLI 与 SwiftUI 应用（MacAIConsole）均为无状态客户端。

> 项目处于活跃开发阶段，适合本地开发、实验与个人工作流；目前尚未承诺 API 稳定性和跨版本兼容。

当前行为以本 README、代码和测试为准。工作提案、决策与契约草案位于
[`docs/decisions/`](docs/decisions/README.md)。

## 架构

```text
macai CLI ────────────┐
MacAIConsole (SwiftUI) ├── HTTP ──> aiworkd ──> Runners ─┬── whisper.cpp / llama.cpp（受管原生引擎）
OpenAI-compatible SDK ┘                                  └── MLX / Kokoro / Qwen3 / sherpa-onnx

                                             ├── SQLite model registry
                                             ├── memory budget / LRU / keep-alive
                                             ├── bounded task history
                                             └── local logs
```

架构坚持单守护进程管理全部状态，避免客户端持有模型生命周期：若由 GUI 或 CLI 直接加载模型，客户端异常会导致推理服务中断，多个客户端并存时亦容易引发状态分歧与内存竞争。在 MacAI 中，客户端不维护常驻运行状态，所有界面数据均从 daemon 的 HTTP API 实时拉取。客户端退出或重载不会影响后台已驻留的推理实例。

## 当前能力

### Runtime

- 默认监听 `127.0.0.1:11435`
- OpenAI-compatible Chat、STT 和 TTS endpoints
- Chat Completion 支持逐 token SSE streaming
- STT 音频上传支持 wav / mp3 / flac / ogg / m4a，由 daemon 在入口统一解码为 PCM WAV，下游 Provider 仅接收标准 PCM WAV 数据流
- SQLite 模型注册表，重启后注册记录持久化保留
- 模型 load / unload、busy guard 和请求期 model lease：模型卸载操作绝不打断正在处理中的请求
- 内存预算默认 `min(RAM×0.75, RAM−8GB)`，支持 LRU 自动逐出、keep-alive 策略与空闲自动卸载
- 支持查看 worker RSS 驻留内存、硬件加速设备与活跃请求状态
- 维护 Chat / STT / TTS 的有界会话内任务历史
- 提供 GUI 与 daemon 的本地持久化日志

### Provider

| 能力 | Provider | 模型/输入 | 加速 |
| --- | --- | --- | --- |
| LLM | org.macai.llama.cpp（Runner） | GGUF | Metal / Accelerate |
| LLM | org.macai.mlx-lm（Runner） | MLX 模型目录（safetensors） | MLX / Metal |
| STT | org.macai.whisper.cpp（Runner） | `.bin` + PCM WAV | Core ML 优先，Metal 回退 |
| STT | org.macai.sherpa-onnx（Runner） | zh-int8-2025 model directory + PCM WAV | CPU |
| STT | org.macai.qwen3-asr（Runner） | Qwen3-ASR MLX 模型目录 + PCM WAV | MLX / Metal GPU |
| TTS | org.macai.qwen3-tts（Runner） | Qwen3-TTS CustomVoice 模型目录 | MLX / Metal GPU |
| TTS | org.macai.kokoro（Runner） | Kokoro 模型目录 | MLX / Metal GPU |

生产推理引擎已全部收敛到 [Runner 架构](docs/decisions/2026-09-02-runner-plugin-architecture.md)：daemon 自动发现 `runners/` 下的 Runner 包并装配为动态 Provider。Python 环境与原生 C++ 引擎共用同一套安装接口；`/api/model-profiles` 向客户端暴露数据化 catalog，按 Profile ID 下载时由 daemon 展开源地址、产物路径与 Runner 绑定。已注册模型会固化当时的 Profile 快照，不受后续 catalog 变更影响。

`/api/models/load` 将调用方请求的 Provider、daemon 选定的 Provider 与裁决理由一并持久化记录；
`/v1/models` 和 `/api/runtime` 暴露 `requested_provider`、`provider`、
`provider_selection_reason` 及 `effective_device`，便于审计选择逻辑与兼容别名。

本地目录通过 `POST /api/models/inspect` 由 daemon 信任的 Runner manifest 静态探测；只有
唯一匹配时才会颁发短期 routing token。CLI 可使用 `macai inspect <directory>` 查看检测结果，并通过
`macai load <directory> --routing-token <token>` 完成注册。GUI 采用相同诊断，不自行推断 Runner。

当前 Runner Protocol v1 针对单实例、单活动推理设计。信任特定 Runner 意味着允许其以
`aiworkd` 用户权限运行本地代码；digest 校验、环境变量白名单和输出路径检查不构成操作系统沙箱。

所有生产 Provider 均运行于独立 Runner 进程中，实现进程级故障隔离：当推理引擎发生段错误或 Python worker 发生 OOM 时，`aiworkd` 保持稳定存活并向客户端返回结构化的 `backend_crashed` 错误，其余模型服务不受影响。此外，系统坚持确定性调度：显式通过 `--provider` 指定引擎属于硬选择，若目标 Provider 不可用将直接失败并返回具体原因，不进行静默回退，确保推理延迟与硬件开销完全透明可溯。

### 客户端

CLI `macai` 当前提供：

```text
status      list        ps          providers   tasks
load        start       unload      remove      rename
keep-alive  pull        chat        run         transcribe
speak       voice       logging     serve
```

完整命令索引可通过 `macai --help` 查询，具体参数与示例参见 `macai <命令> --help`。关键语义说明：`start`、`unload`、`rename`、`keep-alive` 与 `remove` 统一操作 daemon 的运行时状态或 SQLite 注册表；`remove` 仅注销模型条目，磁盘上的模型源文件保持不变。由 Model Profile 绑定的模型 ID 具有固定命名约束，不支持 rename。

MacAIConsole 当前提供：

- Runtime 状态和系统内存压力
- 已加载模型、驻留内存和有效加速设备
- 模型仓库、注册、加载、卸载、改名和详细设置
- 模型页从 daemon Profile catalog 展示并一键下载推荐模型；环境未就绪时行内提示安装入口，环境完成后可注册启动，无需修改或重启 GUI
- Chat / STT / TTS 任务记录与详情
- GUI / daemon 最近日志，支持 Info / Debug 过滤和级别着色
- 菜单栏状态与 daemon 启停
- 设置页支持切换 Hugging Face 官方源、`hf-mirror.com` 或自定义 Hugging Face 兼容源

## 系统要求

- Apple Silicon Mac
- macOS 14 或更高版本
- Rust stable toolchain
- Xcode Command Line Tools
- CMake
- Python 3.12（Runner 的 uv 受管环境以 3.12 为基础解释器）

仓库源码不包含模型权重、预编译第三方推理引擎及 Python 虚拟环境，所有运行时产物均在本地受管目录生成或存储。

## 快速开始

### 1. 构建 workspace

```bash
cargo build --release --workspace
```

### 2. 启动 daemon

```bash
./target/release/aiworkd
```

监听 `http://127.0.0.1:11435`。另开一个终端：

```bash
./target/release/macai status
./target/release/macai list
./target/release/macai ps
```

### 3. 安装 llama.cpp 引擎（Runner）

llama.cpp 已收敛至 Runner 架构（`org.macai.llama.cpp`）。引擎安装通过 daemon 统一入口执行——可经由 GUI「设置 → 引擎」操作，或直接调用管理 API：

```bash
curl -X POST http://127.0.0.1:11435/api/runners/org.macai.llama.cpp/install
```

安装流程会自动同步 uv 受管的适配器环境，并下载验证过的 llama.cpp 预编译二进制（依赖 manifest `[engine]` 固定 tag 与 sha256 校验）到：
`~/Library/Application Support/MacAIConsole/Engines/org.macai.llama.cpp/`。

若本地已有兼容版本的可执行文件，可通过环境变量显式覆盖：

```bash
export MACAI_LLAMA_SERVER=/absolute/path/to/llama-server
```

注册并运行一个本地 GGUF：

```bash
./target/release/macai load /path/to/model.gguf \
  --id local-model \
  --type llm \
  --keep-alive 5m \
  --context-length 4096

./target/release/macai chat local-model "Hello" \
  --system "Answer concisely" \
  --temperature 0.2 \
  --max-tokens 256

./target/release/macai tasks --limit 10
./target/release/macai providers
./target/release/macai unload local-model
```

常用生命周期管理：
- 加载已注册模型：`./target/release/macai start <model>`
- 调整空闲驻留时间：`./target/release/macai keep-alive <model> 30m`
- 重命名模型 ID（运行中模型将先安全停用）：`./target/release/macai rename <old_id> <new_id>`
- 注销模型记录（保留文件）：`./target/release/macai remove <model>`
- TTS 音色查看与默认设置：`./target/release/macai voice <model>` 或 `./target/release/macai voice <model> <voice>`

### 4. 安装 whisper.cpp 引擎（Runner）

```bash
curl -X POST http://127.0.0.1:11435/api/runners/org.macai.whisper.cpp/install
./scripts/download-whisper-model.sh base
```

安装流程会同步轻量适配器环境，校验 manifest 声明的 whisper.cpp 官方源码归档，并在临时暂存区构建静态链接的常驻 `whisper-server`，随后原子替换至：

```text
~/Library/Application Support/MacAIConsole/Engines/org.macai.whisper.cpp/build/bin/whisper-server
.build/models/ggml-base.bin
```

若本机已有对应版本的 server 二进制，可通过环境变量覆盖：

```bash
export MACAI_WHISPER_SERVER=/absolute/path/to/whisper-server
```

注册 `.bin` 格式 STT 模型时若省略 `--provider`，将默认路由至 `org.macai.whisper.cpp`；旧别名 `whisper.cpp` 会被自动规范化并持久化记录选择依据。常驻 server 在加载时初始化模型，后续转写请求均复用该内存实例。

Core ML encoder 为可选性能优化组件。将编译好的 `.mlmodelc` 目录置于与 `.bin` 模型同级的路径下并保持对应命名：

```text
ggml-large-v3-turbo.bin
ggml-large-v3-turbo-encoder.mlmodelc/
```

若未提供 encoder 目录，引擎将自动走 Metal 计算路径。

### 5. 准备 Qwen3-ASR 0.6B

Qwen3-ASR 由 daemon 的 Qwen3-ASR Runner（`org.macai.qwen3-asr`）提供服务，Apple Silicon 上走 MLX/Metal 加速。Python 环境由 uv 受管：在 MacAIConsole「设置 → 引擎」里对 `org.macai.qwen3-asr` 执行安装，或手动：

```bash
uv sync --project runners/qwen3-asr --locked --no-dev
```

推荐模型（4-bit，Hugging Face: `mlx-community/Qwen3-ASR-0.6B-4bit`）可以在 MacAIConsole「管理」页一键下载；模型目录放进 `~/Library/Application Support/MacAIConsole/Models/stt/`。

注册时显式选择 Runner provider `org.macai.qwen3-asr`，模型 ID 必须与目录名一致：

```bash
./target/release/macai load \
  "$HOME/Library/Application Support/MacAIConsole/Models/stt/Qwen3-ASR-0.6B-MLX-4bit" \
  --id Qwen3-ASR-0.6B-MLX-4bit \
  --type stt \
  --provider org.macai.qwen3-asr \
  --keep-alive always

./target/release/macai transcribe meeting.wav \
  --model Qwen3-ASR-0.6B-MLX-4bit \
  --language zh
```

### 6. 准备 sherpa-onnx zh-int8-2025（Runner）

sherpa-onnx 由 daemon 的 sherpa-onnx Runner（`org.macai.sherpa-onnx`）提供服务，
Python 环境由 uv 受管：在 MacAIConsole「设置 → 引擎」里对
`org.macai.sherpa-onnx` 执行安装，或手动：

```bash
uv sync --project runners/sherpa-onnx --locked --no-dev
```

sherpa-onnx 使用官方 streaming Zipformer 中文 int8 模型
`sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`。模型目录必须保留以下
四个文件；模型权重不提交到仓库：

```text
sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/
├── tokens.txt
├── encoder.int8.onnx
├── decoder.onnx
└── joiner.int8.onnx
```

模型下载（GitHub release 包或维护者 HF 镜像均可，布局一致）：

```bash
curl -L -o sherpa-onnx-model.tar.bz2 \
  https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30.tar.bz2
tar xf sherpa-onnx-model.tar.bz2
rm sherpa-onnx-model.tar.bz2
```

注册时显式选择 Runner provider `org.macai.sherpa-onnx`，模型 ID 与目录名一致：

```bash
./target/release/macai load \
  ./sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30 \
  --id sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30 \
  --type stt \
  --provider org.macai.sherpa-onnx \
  --keep-alive always

./target/release/macai transcribe meeting.m4a \
  --model sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30 \
  --language zh
```

上传入口会把 wav / mp3 / flac / ogg / m4a 统一解码为 PCM WAV；Runner 再将
多声道输入下混为单声道并按 streaming chunk 解码，因此长录音不会因一次性
把全部音频交给模型而改变内存行为。该模型只支持中文；`--language` 省略或
使用 `zh` / `zh-CN` / `Chinese` 均可。当前 Runner 固定 CPU 推理（Core ML
provider 需 runner 包内显式启用后另行声明）。

### 7. 准备 Kokoro TTS

Kokoro 由 daemon 的 Kokoro Runner（`org.macai.kokoro`）提供服务，Python 环境由 uv 受管：在 MacAIConsole「设置 → 引擎」里对 `org.macai.kokoro` 执行安装，或手动：

```bash
uv sync --project runners/kokoro --locked --no-dev
```

下载 [Kokoro-82M-zh-MLX](https://huggingface.co/1038lab/Kokoro-82M-zh-MLX)（推荐模型可在 MacAIConsole 模型页一键下载），模型目录放进：

```text
~/Library/Application Support/MacAIConsole/Models/tts/
```

注册时显式选择 Runner provider `org.macai.kokoro`，模型 ID 与目录名一致：

```bash
./target/release/macai load \
  "$HOME/Library/Application Support/MacAIConsole/Models/tts/kokoro-82m-zh" \
  --id kokoro-82m-zh \
  --type tts \
  --provider org.macai.kokoro \
  --keep-alive always
```

### 8. 准备 Qwen3-TTS CustomVoice（Runner）

Qwen3-TTS 由 daemon 的 Qwen3-TTS Runner（`org.macai.qwen3-tts`）提供服务，
Apple Silicon 上走 MLX/Metal 加速。Python 环境由 uv 受管：在 MacAIConsole
「设置 → 引擎」里对 `org.macai.qwen3-tts` 执行安装，或手动：

```bash
uv sync --project runners/qwen3-tts --locked --no-dev
```

推荐模型（Qwen3-TTS 0.6B · CustomVoice 4-bit）可以在 MacAIConsole 模型页一键下载；
或者手动下载 [mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-4bit](https://huggingface.co/mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-4bit)，
模型目录放进 `~/Library/Application Support/MacAIConsole/Models/tts/`。

注册时显式选择 Runner provider `org.macai.qwen3-tts`，模型 ID 与目录名一致：

```bash
./target/release/macai load \
  "$HOME/Library/Application Support/MacAIConsole/Models/tts/Qwen3-TTS-0.6B-CustomVoice-4bit" \
  --id Qwen3-TTS-0.6B-CustomVoice-4bit \
  --type tts \
  --provider org.macai.qwen3-tts \
  --keep-alive always

./target/release/macai speak Qwen3-TTS-0.6B-CustomVoice-4bit "你好，这是本地模型。"
```

请求只支持 WAV；`voice` 传递 Qwen3-TTS 的 speaker，
例如 `Vivian`。首版支持用逗号携带情感指令（`Vivian, very happy`）。
`speed` 参数仍按 `0.25..=4.0` 校验，但当前 `mlx-audio` 的
`generate_custom_voice` 没有 speed 参数，因此通过校验后不改变合成速度；后续
若上游提供原生支持再透传。详细设置中的默认音色提供 Qwen3-TTS 官方内置的
`Vivian`、`Serena`、`Uncle_Fu`、`Dylan`、`Eric`、`Ryan`、`Aiden`、
`Ono_Anna`、`Sohee`；首版不包含流式、声音克隆或 VoiceDesign。

### 9. 准备 MLX-LM（Runner）

MLX LLM 由 daemon 的 mlx-lm Runner（`org.macai.mlx-lm`）提供服务，Apple Silicon 上走
MLX/Metal 加速。Python 环境由 uv 受管：在 MacAIConsole「设置 → 引擎」里
对 `org.macai.mlx-lm` 执行安装，或手动：

```bash
uv sync --project runners/mlx-lm --locked --no-dev
```

推荐模型（SmolLM2-135M-Instruct-8bit、Qwen3 8B · MLX 4-bit）可以在 MacAIConsole
模型页一键下载；或者手动把 MLX 格式模型目录（含 `config.json` 与 safetensors 权重，
例如 [mlx-community/Qwen3-8B-4bit](https://huggingface.co/mlx-community/Qwen3-8B-4bit)）
放进：

```text
~/Library/Application Support/MacAIConsole/Models/llm/
```

注册时显式选择 Runner provider `org.macai.mlx-lm`，模型 ID 与目录名一致：

```bash
./target/release/macai load \
  "$HOME/Library/Application Support/MacAIConsole/Models/llm/SmolLM2-135M-Instruct-8bit" \
  --id SmolLM2-135M-Instruct-8bit \
  --type llm \
  --provider org.macai.mlx-lm \
  --keep-alive 5m
```

模型权重格式与 Runner 对应关系明确：GGUF 格式由 llama.cpp 加载，MLX 格式模型目录由 mlx-lm Runner 加载。

## MacAIConsole

```bash
cd apps/MacAIConsole
scripts/build-app.sh release
```

该脚本停止旧 GUI / daemon、重建同一配置的前后端并启动新的 app；日常本地联调应使用它。

产物在 `apps/MacAIConsole/build/MacAIConsole.app`。

GUI 可以连接已经在跑的 `aiworkd`。GUI 自动启动 daemon 时按顺序探测：`AIWORKD_PATH` 环境变量 → 仓库 `target/release/aiworkd` → `target/debug/aiworkd`。

模型下载默认使用 `https://huggingface.co`。MacAIConsole 的「设置 → 模型下载源」可切换
到 `https://hf-mirror.com` 或填写自定义 HTTP(S) 地址；修改后点击「应用设置并重启
aiworkd」。下载由 daemon 统一执行，使用 HTTP/1.1、`.part` 断点续传，并在响应体中断
时自动重试。手动启动 daemon 时可设置：

```bash
AIWORKD_HF_ENDPOINT=https://hf-mirror.com ./target/release/aiworkd
```

自定义源只支持 HTTP(S) 主机和可选路径，不支持凭据、查询参数或片段。

## HTTP API

### Health

```bash
curl http://127.0.0.1:11435/health
```

### Chat Completion

```bash
curl http://127.0.0.1:11435/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "local-model",
    "messages": [{"role": "user", "content": "Hello"}],
    "stream": false
  }'
```

流式请求把 `stream` 设为 `true`，响应以 `data: [DONE]` 结束。

### OpenAI Python SDK

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:11435/v1",
    api_key="local",
)

response = client.chat.completions.create(
    model="local-model",
    messages=[{"role": "user", "content": "Hello"}],
)
print(response.choices[0].message.content)
```

支持任何标准 OpenAI SDK，配置对应的 `base_url` 与模型 ID 即可接入。

### 主要 endpoints

```text
GET  /health
GET  /v1/models
POST /v1/chat/completions
POST /v1/audio/transcriptions
POST /v1/audio/speech

GET  /api/runtime
GET  /api/providers
GET  /api/runners
POST /api/runners/{runner}/install
GET  /api/tasks
GET  /api/tasks/{id}
GET  /api/logging
POST /api/logging
POST /api/models/pull
POST /api/models/inspect
POST /api/models/load
POST /api/models/{id}/load
POST /api/models/{id}/unload
DELETE /api/models/{id}
POST /api/models/{id}/rename
POST /api/models/{id}/keep-alive
GET  /api/models/{id}/voices
POST /api/models/{id}/voice
```

`/v1/*` 面向推理，`/api/*` 面向运行时管理。

## 使用案例：为 Hermes Agent 提供本地 TTS 与 STT

[Hermes Agent](https://hermes-agent.nousresearch.com/docs/) 支持命令型语音 Provider。MacAI 通过本地 OpenAI-compatible API 给 Hermes 提供：

| Hermes 能力 | MacAI endpoint | MacAI Provider |
| --- | --- | --- |
| TTS（文字转语音） | `POST /v1/audio/speech` | Kokoro（org.macai.kokoro Runner） |
| STT（语音转文字） | `POST /v1/audio/transcriptions` | org.macai.whisper.cpp Runner |

接入后的数据流：

```text
Hermes text_to_speech
  -> samples/hermes_macai_tts.py
  -> MacAI /v1/audio/speech
  -> WAV audio

Hermes voice message
  -> samples/hermes_macai_stt.sh
  -> ffmpeg: input audio -> 16 kHz mono PCM WAV
  -> MacAI /v1/audio/transcriptions
  -> UTF-8 transcript
```

### 前置条件

1. `aiworkd` 正在 `http://127.0.0.1:11435` 运行。
2. MacAI 已注册至少一个 `tts` 模型和一个 `stt` 模型。
3. 本机已安装 `hermes`、`python3`、`curl` 和 `ffmpeg`。
4. Hermes 版本支持 `tts.providers.<name>` 与 `stt.providers.<name>` 命令型 Provider。

先确认 MacAI 这边就绪：

```bash
curl --fail http://127.0.0.1:11435/health
curl --fail http://127.0.0.1:11435/v1/models
```

`/v1/models` 的 `data` 数组里应该有 `type: "tts"` 和 `type: "stt"` 的模型。

### 一键配置

在 MacAI 仓库根目录运行：

```bash
./samples/configure-hermes-audio.sh
```

脚本会：

1. 检查 MacAI、Hermes、Python、curl 与 ffmpeg。
2. 从 `/v1/models` 自动选择第一个 TTS 和 STT 模型。
3. 向当前 Hermes profile 写入名为 `macai` 的 TTS/STT Provider。
4. 启用 Hermes 的 `tts` toolset。
5. 读回 `tts.provider` 与 `stt.provider`，确认配置已落盘。

自动发现的结果可以用环境变量覆盖：

```bash
MACAI_BASE_URL=http://127.0.0.1:11435 \
MACAI_TTS_MODEL=Kokoro-82M-zh \
MACAI_TTS_VOICE=zf_001 \
MACAI_STT_MODEL=whisper-large-v3-turbo \
MACAI_STT_LANGUAGE=zh \
./samples/configure-hermes-audio.sh
```

可用变量：

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `HERMES_BIN` | `hermes` | 指定 Hermes 可执行文件 |
| `MACAI_BASE_URL` | `http://127.0.0.1:11435` | MacAI 根地址或 `/v1` API 地址 |
| `MACAI_TTS_MODEL` | 自动发现 | MacAI TTS 模型 ID |
| `MACAI_TTS_VOICE` | 空 | 可选的 Kokoro 音色 ID；为空时使用模型默认音色 |
| `MACAI_STT_MODEL` | 自动发现 | MacAI STT 模型 ID |
| `MACAI_STT_LANGUAGE` | `zh` | STT 语言提示 |

> [!NOTE]
> 配置里保存了 `samples` 脚本的绝对路径。移动 MacAI 仓库后，重新跑一遍配置脚本。

### 生成的 Hermes 配置

脚本通过 `hermes config set` 写入当前 profile。核心结构如下，脚本路径与模型 ID 会替换为本机真实值：

```yaml
tts:
  provider: macai
  providers:
    macai:
      type: command
      command: >-
        python3 "/absolute/path/to/MacAI/samples/hermes_macai_tts.py"
        --input "{input_path}"
        --output "{output_path}"
        --base-url "http://127.0.0.1:11435/v1"
        --model "{model}"
        --voice "{voice}"
        --format "{format}"
        --speed "{speed}"
      model: Kokoro-82M-zh
      voice: ""
      output_format: wav
      timeout: 150
      max_text_length: 3000
      voice_compatible: true

stt:
  enabled: true
  echo_transcripts: true
  provider: macai
  language: zh
  providers:
    macai:
      type: command
      command: >-
        bash "/absolute/path/to/MacAI/samples/hermes_macai_stt.sh"
        "{input_path}"
        "{output_path}"
        "http://127.0.0.1:11435/v1"
        "{model}"
        "{language}"
      model: whisper-large-v3-turbo
      language: zh
      format: txt
      timeout: 300
```

两个适配脚本的分工：TTS 脚本使用 Python 标准库读取 Hermes 生成的 UTF-8 文本文件，请求 MacAI `/v1/audio/speech` 并将音频流写入 `{output_path}`；STT 脚本则将 Hermes 接收到的多格式音频（WAV、OGG、Opus 等）通过 ffmpeg 归一化为 PCM WAV，交由 MacAI 转写并输出纯文本。

STT Provider 显式命名为 `macai` 亦可规避模型名称改写：Hermes 原生 `openai` STT 逻辑可能将 `whisper-large-v3-turbo` 映射为特定云端厂商的 `whisper-1`。采用独立 command Provider 可确保本地模型 ID 被原样透传。

### 验证适配脚本

直接验证 TTS：

```bash
printf '你好，这是 MacAI 提供给 Hermes 的语音合成测试。' > /tmp/macai-tts.txt

python3 samples/hermes_macai_tts.py \
  --input /tmp/macai-tts.txt \
  --output /tmp/macai-tts.wav \
  --base-url http://127.0.0.1:11435 \
  --model Kokoro-82M-zh \
  --format wav

file /tmp/macai-tts.wav
```

直接验证 STT：

```bash
bash samples/hermes_macai_stt.sh \
  /path/to/input.ogg \
  /tmp/macai-transcript.txt \
  http://127.0.0.1:11435 \
  whisper-large-v3-turbo \
  zh

cat /tmp/macai-transcript.txt
```

检查 Hermes 的最终选择：

```bash
hermes config get tts.provider
hermes config get stt.provider
hermes config get tts.providers.macai
hermes config get stt.providers.macai
```

配置完成后重启 Hermes Desktop；Gateway 用户执行 `/restart`。之后用 Hermes 的 `text_to_speech` 工具、CLI `/voice tts` 模式或消息平台的语音消息验证整条链路。

### 常见排错

- **`model_not_found`**：请求 `curl http://127.0.0.1:11435/v1/models` 确认已注册模型的准确 `id`，并将其填入脚本配置，或重新执行一键配置。
- **`Connection refused`**：守护进程未启动。请在 MacAIConsole 中开启服务，或在终端运行 `./target/release/aiworkd`。
- **`Required command is missing: ffmpeg`**：系统缺少 `ffmpeg` 依赖。请安装后重新执行配置脚本（STT 适配器依赖其归一化音频格式）。
- **Hermes 仍沿用旧 Provider**：重启 Hermes Desktop 或在 Gateway 中执行 `/restart`；同时确认配置脚本写入的 profile 与运行中的 Hermes profile 一致。
- **首次推理延迟偏高**：whisper.cpp 的 Core ML encoder 在首次调用时需完成 Apple Neural Engine (ANE) 架构特化，完成编译后将复用缓存。

Hermes 命令型语音 Provider 的完整说明见 [Voice & TTS](https://hermes-agent.nousresearch.com/docs/user-guide/features/tts)。

## 本地数据

MacAIConsole 使用以下目录：

```text
~/Library/Application Support/MacAIConsole/
├── Models/
│   ├── llm/
│   ├── stt/
│   └── tts/
├── model-settings.json
├── models.db
└── logs/
    ├── aiworkd.log
    └── gui.log
```

日志到 5 MB 轮换，保留一份 `.1` 文件。日志页面默认显示 Info 及以上级别；启用 Debug 会同时调整 GUI 和 daemon 的运行时日志级别。

## 安全边界

`aiworkd` 默认仅监听本地回环地址（`127.0.0.1`），且未内置身份鉴权机制。如需通过端口转发、反向代理或局域网访问，务必在前端配置健全的安全隔离与身份认证。

任务历史可能包含 Prompt、转写结果与 TTS 文本输入。系统遵循数据最小化原则：原始音频数据不落盘写入任务历史，日志系统亦会主动过滤 API Token 与模型上下文内容。

## 开发与验证

Rust：

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo build --workspace
```

MacAIConsole：

```bash
cd apps/MacAIConsole
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
  swift test --enable-xctest
scripts/build-app.sh release
```

## 仓库结构

```text
crates/
├── ai-core/       # 共享模型、Provider、请求与响应类型
├── ai-daemon/     # aiworkd、Provider、调度与管理 API；Runner discovery/environment/instance bridge
└── ai-cli/        # macai 命令行客户端

apps/
└── MacAIConsole/  # 原生 SwiftUI 控制台

scripts/           # 模型下载、可选开发构建与真实 Runner smoke
runners/           # daemon 自动发现的七个 Runner 包（含 llama.cpp 与 whisper.cpp）
samples/           # 第三方集成示例（含 Hermes TTS/STT 适配器）
docs/              # 当前工作提案、已落地决策、精确契约与验证参考
```

## 路线图

当前优先方向：

- [Runner 插件架构决策](docs/decisions/2026-09-02-runner-plugin-architecture.md)：把新模型接入从 daemon/GUI 硬编码迁到可发现 Runner 与数据化 Model Profile。
- [uv Python 环境决策](docs/decisions/2026-09-02-uv-python-environments.md)：所有 Python Runner 使用可复现、可探测的 `uv` 环境。
- [Kokoro Runner 验证参考](docs/reference/kokoro-runner-verification.md)：保留首个真实 Runner 的可重复证据路径。
- [whisper.cpp Runner 迁移决策](docs/decisions/2026-09-05-whisper-runner-migration.md)：官方 source build、常驻 server、兼容别名与验证证据。
- readiness / deep-health、启动进度与完整可观测性
- Homebrew、正式签名、公证与安装包

## 社区与贡献

我们欢迎社区贡献！无论是新模型 Runner 插件、功能建议还是缺陷修复：
- 贡献指南请参见 [CONTRIBUTING.md](CONTRIBUTING.md)；
- 行为准则请参见 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。

## 安全政策

MacAI 专注于本机私有推理：`aiworkd` 默认仅监听本地 `127.0.0.1:11435` 且无鉴权机制，切勿直接公网暴露。完整威胁模型与漏洞报告指引请参见 [SECURITY.md](SECURITY.md)。

## 开源许可证与模型版权

- **项目许可证**：MacAI 依据 [MIT License](LICENSE) 开源；
- **第三方组件通知**：使用的第三方推理引擎与依赖库协议声明参见 [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)；
- **AI 模型免责声明**：MacAI 仅提供本地运行时与任务调度能力，不拥有亦不分发模型权重；下载与使用各开源模型需严格遵守其原始权利人的授权许可协议。

