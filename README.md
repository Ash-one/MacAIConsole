# MacAI

<p align="center">
  <img src="logo.png" alt="MacAI logo" width="280">
</p>

MacAI 是面向 Apple Silicon 的本地 AI Runtime。Rust daemon `aiworkd` 统一管理模型、推理 worker、内存调度和 HTTP API；CLI 与原生 SwiftUI 应用都通过同一个本地服务工作。

> 项目处于活跃开发阶段，当前适合本地开发、实验和个人工作流。发布安装、API 稳定性和向后兼容尚未承诺。

## 架构

```text
macai CLI ────────────┐
MacAIConsole (SwiftUI) ├── HTTP ──> aiworkd ──┬── llama.cpp ──> GGUF / Metal
OpenAI-compatible SDK ┘                      ├── mlx-lm ──────> MLX / Metal
                                             ├── whisper.cpp ─> Core ML / Metal
                                             ├── Kokoro MLX ──> local TTS
                                             └── Qwen3-TTS MLX ─> local TTS

                                             ├── SQLite model registry
                                             ├── memory budget / LRU / keep-alive
                                             ├── bounded task history
                                             └── local logs
```

`aiworkd` 是唯一的 Runtime Authority。GUI 和 CLI 不直接加载模型，也不维护独立的运行状态。

## 当前能力

### Runtime

- 默认监听 `127.0.0.1:11435`
- OpenAI-compatible Chat、STT 和 TTS endpoints
- STT 上传支持 wav / mp3 / flac / ogg / m4a；daemon 在入口解码归一化为 PCM WAV 后交给 provider
- Chat Completion 与逐 token SSE streaming
- SQLite 模型注册表，重启后保存
- 模型 load / unload、busy guard 和请求期 model lease
- 可配置内存预算、LRU 逐出、keep-alive 与空闲自动卸载
- worker RSS、加速设备和活跃请求状态
- Chat / STT / TTS 的有界会话内任务历史
- GUI / daemon 本地日志

### Provider

| 能力 | Provider | 模型/输入 | 加速 |
| --- | --- | --- | --- |
| LLM | llama.cpp | GGUF | Metal / Accelerate |
| LLM | mlx-lm | MLX 模型目录（safetensors） | MLX / Metal |
| STT | whisper.cpp | `.bin` + PCM WAV | Core ML 优先，Metal 回退 |
| TTS | Kokoro MLX | Kokoro 模型目录 | MLX / Metal GPU |
| TTS | Qwen3-TTS MLX | Qwen3-TTS CustomVoice 模型目录 | MLX / Metal GPU |

Provider 以独立进程承载高风险或第三方推理运行时。某个 worker 退出时，`aiworkd` 保持运行并向客户端返回结构化错误。

### 客户端

CLI `macai` 当前提供：

```text
status      list        ps          providers   tasks
load        start       unload      remove      rename
keep-alive  pull        chat        run         transcribe
speak       voice       logging     serve
```

运行 `macai --help` 查看完整命令索引，运行 `macai <命令> --help` 查看参数和示例。`start`、`unload`、`rename`、`keep-alive` 和 `remove` 操作 daemon 的运行状态或注册表；`remove` 会保留磁盘上的模型文件。

MacAIConsole 当前提供：

- Runtime 状态和系统内存压力
- 已加载模型、驻留内存和有效加速设备
- 模型仓库、注册、加载、卸载、改名和详细设置
- Chat / STT / TTS 任务记录与详情
- GUI / daemon 最近日志，支持 Info / Debug 过滤和级别着色
- 菜单栏状态与 daemon 启停
- 修复 kokoro tts bug，支持中英混读和长音频自动拼接

## 系统要求

- Apple Silicon Mac
- macOS 14 或更高版本
- Rust stable toolchain
- Xcode Command Line Tools
- CMake
- Python 3.12（仅 Kokoro TTS 需要）

仓库不包含模型权重、编译后的第三方推理引擎或 Python 虚拟环境。

## 快速开始

### 1. 构建 workspace

在仓库根目录运行：

```bash
cargo build --release --workspace
```

### 2. 启动 daemon

```bash
./target/release/aiworkd
```

`aiworkd` 启动后会监听：

```text
http://127.0.0.1:11435
```

另开一个终端检查状态：

```bash
./target/release/macai status
./target/release/macai list
./target/release/macai ps
./target/release/macai --help
```

### 3. 构建 llama.cpp worker

仓库构建脚本固定了经过验证的 llama.cpp revision：

```bash
./scripts/build-llama-server.sh
```

默认产物：

```text
.build/llama.cpp/bin/llama-server
```

也可以指定已有二进制：

```bash
export AIWORK_LLAMA_SERVER=/absolute/path/to/llama-server
```

注册并运行本地 GGUF：

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

已注册但未运行的模型可用 `macai start <model>` 重新加载。`macai keep-alive <model> 30m` 调整空闲驻留时间；`macai rename` 修改模型 ID，运行中的模型会先停止；`macai remove` 删除注册记录。TTS 模型可通过 `macai voice <model>` 查看音色，并用 `macai voice <model> <voice>` 设置默认音色。

### 4. 构建 whisper.cpp worker

```bash
./scripts/build-whisper-cli.sh
./scripts/download-whisper-model.sh base
```

默认产物：

```text
.build/whisper.cpp/bin/whisper-cli
.build/models/ggml-base.bin
```

可用环境变量覆盖路径：

```bash
export AIWORK_WHISPER_CLI=/absolute/path/to/whisper-cli
```

Core ML encoder 为可选项。将编译后的 `.mlmodelc` 目录放到 `.bin` 同目录，并保持对应名称：

```text
ggml-large-v3-turbo.bin
ggml-large-v3-turbo-encoder.mlmodelc/
```

缺少 Core ML encoder 时会使用 Metal 路径。

### 5. 准备 Qwen3-ASR 0.6B

Qwen3-ASR 通过 MLX 8-bit Provider 提供服务（Apple Silicon Metal 加速）。

Python 环境也可以在 MacAIConsole「设置 → Python 运行环境」中一键安装（需本机有 Python 3.12，依赖版本与本节一致）；以下为手动步骤。

MLX 8-bit 环境与权重：

```bash
uv venv --python 3.12 .build/qwen3-asr-mlx-venv
uv pip install --python .build/qwen3-asr-mlx-venv/bin/python 'mlx-audio==0.5.0'

hf download mlx-community/Qwen3-ASR-0.6B-8bit \
  --local-dir "$HOME/Library/Application Support/MacAIConsole/Models/stt/Qwen3-ASR-0.6B-MLX-8bit"
```

注册时选择 `qwen3-asr-mlx`；daemon 会校验模型配置确实为 8-bit：

```bash
./target/release/macai load \
  "$HOME/Library/Application Support/MacAIConsole/Models/stt/Qwen3-ASR-0.6B-MLX-8bit" \
  --id qwen3-asr-mlx-8bit \
  --type stt \
  --provider qwen3-asr-mlx \
  --keep-alive always

./target/release/macai transcribe meeting.wav \
  --model qwen3-asr-mlx-8bit \
  --language zh
```

MLX Provider 固定使用 Metal，并可通过 `AIWORK_QWEN3_ASR_MLX_PYTHON` 指向其他隔离环境。

### 6. 准备 Kokoro TTS

创建 Python 环境（也可在 MacAIConsole「设置 → Python 运行环境」中一键安装）：

```bash
python3.12 -m venv .build/kokoro-venv
.build/kokoro-venv/bin/pip install \
  mlx-audio \
  "misaki[zh]" \
  "misaki[en]" \
  phonemizer-fork \
  espeakng-loader
```

下载 [Kokoro-82M-zh-MLX](https://huggingface.co/1038lab/Kokoro-82M-zh-MLX)，并将模型目录放入：

```text
~/Library/Application Support/MacAIConsole/Models/tts/
```

也可以通过 `AIWORK_KOKORO_PYTHON` 指向其他 Python 环境。

### 7. 准备 Qwen3-TTS CustomVoice

Qwen3-TTS 与 Kokoro 复用同一个 `mlx-audio` Python 环境。推荐使用
`mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-4bit`，它通过常驻 worker
运行在 MLX/Metal 上：

```bash
.build/kokoro-venv/bin/pip install -U mlx-audio
hf download mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-4bit \
  --local-dir "$HOME/Library/Application Support/MacAIConsole/Models/tts/Qwen3-TTS-0.6B-CustomVoice-4bit"
```

注册时显式选择 `qwen3-tts`：

```bash
./target/release/macai load \
  "$HOME/Library/Application Support/MacAIConsole/Models/tts/Qwen3-TTS-0.6B-CustomVoice-4bit" \
  --id qwen3-tts-customvoice-4bit \
  --type tts \
  --provider qwen3-tts \
  --keep-alive always

./target/release/macai speak qwen3-tts-customvoice-4bit "你好，这是本地模型。"
```

可用 `AIWORK_QWEN3_TTS_PYTHON` 覆盖 Python 路径，`AIWORK_QWEN3_TTS_SCRIPT`
覆盖 worker 脚本路径。请求只支持 WAV；`voice` 传递 Qwen3-TTS 的 speaker，
例如 `Vivian`。首版支持用逗号携带情感指令（`Vivian, very happy`）。
`speed` 参数仍按 `0.25..=4.0` 校验，但当前 `mlx-audio` 的
`generate_custom_voice` 没有 speed 参数，因此通过校验后不改变合成速度；后续
若上游提供原生支持再透传。首版不包含流式、声音克隆或 VoiceDesign。

### 8. 准备 MLX-LM

创建 Python 环境（也可在 MacAIConsole「设置 → Python 运行环境」中一键安装）：

```bash
python3.12 -m venv .build/mlx-lm-venv
.build/mlx-lm-venv/bin/pip install "mlx-lm==0.31.3"
```

推荐模型（Qwen3 8B · MLX 4-bit）可在 MacAIConsole 模型页一键下载；或手动把
MLX 格式模型目录（含 `config.json` 与 safetensors 权重，例如
[mlx-community/Qwen3-8B-4bit](https://huggingface.co/mlx-community/Qwen3-8B-4bit)）放入：

```text
~/Library/Application Support/MacAIConsole/Models/llm/
```

注册时显式选择 provider `mlx-lm`（GGUF 与 MLX 格式互不通用：GGUF 走
llama.cpp，MLX 走 mlx-lm）。可通过 `AIWORK_MLX_LM_PYTHON` 指向其他 Python
环境；加载与推理超时分别由 `AIWORK_MLX_LM_LOAD_TIMEOUT_SECS` 与
`AIWORK_MLX_LM_INFERENCE_TIMEOUT_SECS` 控制。

## MacAIConsole

```bash
cargo build --release -p ai-daemon
cd apps/MacAIConsole
scripts/build-app.sh release
open build/MacAIConsole.app
```

应用产物位于：

```text
apps/MacAIConsole/build/MacAIConsole.app
```

GUI 可以连接已经运行的 `aiworkd`。GUI 自动启动 daemon 时按顺序探测 `AIWORKD_PATH` 环境变量、仓库 `target/release/aiworkd` 与 `target/debug/aiworkd`。

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

流式请求将 `stream` 设为 `true`，响应以 `data: [DONE]` 结束。

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

### 主要 endpoints

```text
GET  /health
GET  /v1/models
POST /v1/chat/completions
POST /v1/audio/transcriptions
POST /v1/audio/speech

GET  /api/runtime
GET  /api/providers
GET  /api/tasks
GET  /api/tasks/{id}
GET  /api/logging
POST /api/logging
POST /api/models/pull
POST /api/models/load
POST /api/models/{id}/load
POST /api/models/{id}/unload
DELETE /api/models/{id}
POST /api/models/{id}/rename
POST /api/models/{id}/keep-alive
GET  /api/models/{id}/voices
POST /api/models/{id}/voice
```

## 使用案例：为 Hermes Agent 提供本地 TTS 与 STT

[Hermes Agent](https://hermes-agent.nousresearch.com/docs/) 支持命令型语音 Provider。MacAI 可以通过本地 OpenAI-compatible API 为 Hermes 提供：

| Hermes 能力 | MacAI endpoint | MacAI Provider |
| --- | --- | --- |
| TTS（文字转语音） | `POST /v1/audio/speech` | Kokoro MLX |
| STT（语音转文字） | `POST /v1/audio/transcriptions` | whisper.cpp |

接入后的数据流如下：

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

先检查 MacAI：

```bash
curl --fail http://127.0.0.1:11435/health
curl --fail http://127.0.0.1:11435/v1/models
```

`/v1/models` 的 `data` 数组中应包含 `type: "tts"` 和 `type: "stt"` 的模型。

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

可以通过环境变量覆盖自动发现结果：

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
> 配置中保存了 `samples` 脚本的绝对路径。移动 MacAI 仓库后，请重新运行配置脚本。

### 生成的 Hermes 配置

脚本通过 `hermes config set` 写入当前 profile。生成的核心结构如下，其中脚本路径与模型 ID 会替换为本机真实值：

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

TTS 适配脚本使用 Python 标准库读取 Hermes 创建的 UTF-8 文本文件，调用 MacAI 后将音频写入 `{output_path}`。STT 适配脚本先把 Hermes 收到的 WAV、OGG、Opus 或其他 ffmpeg 支持的音频统一转换为 MacAI 当前要求的 PCM WAV，再返回纯文本转写结果。

命名为独立的 `macai` STT Provider 还有一个兼容性作用：Hermes 的内置 `openai` STT 路径可能把 `whisper-large-v3-turbo` 识别为 Groq 模型名并改写为 `whisper-1`；命令型 Provider 会将 MacAI 模型 ID 原样传递。

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

配置完成后重启 Hermes Desktop。Gateway 用户可以执行 `/restart`。随后可使用 Hermes 的 `text_to_speech` 工具、CLI `/voice tts` 模式或消息平台语音消息验证完整链路。

### 常见问题

- **`model_not_found`**：运行 `curl http://127.0.0.1:11435/v1/models`，把脚本配置中的模型 ID 改成返回的真实 `id`，或重新运行一键配置。
- **`Connection refused`**：先通过 MacAIConsole 启动 daemon，或在仓库中运行 `./target/release/aiworkd`。
- **`Required command is missing: ffmpeg`**：安装 ffmpeg 后重新运行配置；STT 适配器依赖它统一音频格式。
- **Hermes 仍显示旧 Provider**：重启 Hermes Desktop 或 Gateway。Hermes profile 相互隔离，请确认运行配置脚本和启动 Hermes 时使用的是同一个 profile。
- **模型首次请求较慢**：whisper.cpp Core ML encoder 首次进行 ANE 特化时可能需要额外时间，后续请求会复用缓存。

Hermes 命令型语音 Provider 的完整说明见 [Voice & TTS](https://hermes-agent.nousresearch.com/docs/user-guide/features/tts)。

## 本地数据

MacAIConsole 使用以下目录：

```text
~/Library/Application Support/MacAIConsole/
├── Models/
│   ├── llm/
│   ├── stt/
│   └── tts/
├── models.db
├── model-settings.json
└── logs/
    ├── aiworkd.log
    └── gui.log
```

日志文件达到 5 MB 时轮换，并保留一份 `.1` 文件。日志页面默认显示 Info 及以上级别；启用 Debug 后会同时调整 GUI 和 daemon 的运行时日志级别。

## 安全边界

`aiworkd` 当前只监听 loopback 地址，并且没有为局域网访问提供认证。请保持服务绑定在 `127.0.0.1`，不要通过端口转发或反向代理直接暴露到不受信任网络。

任务历史可能包含 prompt、转写文本和 TTS 输入。原始音频不会写入任务历史，日志也不应记录 API token 或模型内容。

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
├── ai-daemon/     # aiworkd、Provider、调度与管理 API
└── ai-cli/        # ai 命令行客户端

apps/
└── MacAIConsole/  # 原生 SwiftUI 控制台

scripts/           # llama.cpp、whisper.cpp 与 Kokoro worker 工具
samples/           # 第三方集成示例（含 Hermes TTS/STT 适配器）
handoff.md         # 目标架构与长期路线
```

## 路线图

当前 handoff 中仍未完成的主要工作：

- MLX-native ASR（mlx-whisper / parakeet-mlx）
- LLM / STT / TTS benchmark 框架（MLX vs llama.cpp 对比实验）
- per-device 请求队列、独立 queue/execution deadline、持久任务和事件回放
- GUI Chat、Speech 和 Hugging Face 下载界面
- VAD、streaming STT 和 streaming TTS
- readiness / deep-health、启动进度与完整可观测性
- Homebrew、正式签名、公证与安装包

完整目标设计见 [`handoff.md`](handoff.md)。它描述长期架构，当前实现状态以本 README 和代码为准。
