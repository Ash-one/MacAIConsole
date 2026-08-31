# MacAI

<p align="center">
  <img src="logo.png" alt="MacAI logo" width="280">
</p>

MacAI 是一个跑在你 Mac 上的本地 AI Runtime。它的核心只有一个东西：Rust daemon `aiworkd`。模型注册、推理 worker、内存调度、HTTP API，全部归它管；CLI 和 SwiftUI 应用都是它的客户端。

> 项目还在活跃开发中。现在适合本地开发、实验和个人工作流——发布安装、API 稳定性和向后兼容都还没承诺。

## 架构

```text
macai CLI ────────────┐
MacAIConsole (SwiftUI) ├── HTTP ──> aiworkd ──┬── llama.cpp ──> GGUF / Metal
OpenAI-compatible SDK ┘                      ├── mlx-lm ──────> MLX / Metal
                                             ├── whisper.cpp ─> Core ML / Metal
                                             ├── sherpa-onnx ─> ONNX Runtime / CPU or Core ML
                                             └── Kokoro MLX ──> local TTS

                                             ├── SQLite model registry
                                             ├── memory budget / LRU / keep-alive
                                             ├── bounded task history
                                             └── local logs
```

为什么要坚持「一个 daemon 管所有事」？因为客户端拥有模型状态的代价太高了：GUI 崩了模型跟着没，两个客户端看到的状态互相打架，内存调度各管各的最后一起爆。所以 MacAI 把这条路径砍掉了——GUI 和 CLI 不加载模型、不维护运行状态，屏幕上显示的每个状态值都来自 daemon 的 HTTP API。客户端崩了无所谓，模型还在 daemon 里驻着。

## 当前能力

### Runtime

- 默认监听 `127.0.0.1:11435`
- OpenAI-compatible Chat、STT 和 TTS endpoints
- Chat Completion 支持逐 token SSE streaming
- STT 上传支持 wav / mp3 / flac / ogg / m4a，daemon 在入口统一解码为 PCM WAV——provider 只见 WAV，格式转换的脏活都在门口干完
- SQLite 模型注册表，重启后注册记录还在
- 模型 load / unload、busy guard 和请求期 model lease：卸载永远打断不了正在跑的请求
- 内存预算默认 `min(RAM×0.75, RAM−8GB)`，LRU 逐出、keep-alive、空闲自动卸载都可配置
- worker RSS、加速设备、活跃请求状态可查
- Chat / STT / TTS 的有界会话内任务历史
- GUI / daemon 本地日志

### Provider

| 能力 | Provider | 模型/输入 | 加速 |
| --- | --- | --- | --- |
| LLM | llama.cpp | GGUF | Metal / Accelerate |
| LLM | mlx-lm | MLX 模型目录（safetensors） | MLX / Metal |
| STT | whisper.cpp | `.bin` + PCM WAV | Core ML 优先，Metal 回退 |
| STT | sherpa-onnx | zh-int8-2025 model directory + PCM WAV | CPU（可选 Core ML） |
| TTS | Kokoro MLX | Kokoro 模型目录 | MLX / Metal GPU |

每个 Provider 都是独立进程。这不是设计洁癖，是故障隔离的实际需要：某个推理引擎崩了——llama.cpp 段错误、Python worker OOM——`aiworkd` 本身保持存活，客户端收到的是 `backend_crashed` 这样的结构化错误，其他模型照常服务。另外有一点是刻意的：显式指定 `--provider` 是硬选择，provider 不可用就直接失败，绝不悄悄换一个。静默 fallback 会让「为什么变慢了」这种问题永远查不出原因。

### 客户端

CLI `macai` 当前提供：

```text
status      list        ps          providers   tasks
load        start       unload      remove      rename
keep-alive  pull        chat        run         transcribe
speak       voice       logging     serve
```

完整命令索引看 `macai --help`，参数和示例看 `macai <命令> --help`。几个容易混淆的：`start`、`unload`、`rename`、`keep-alive`、`remove` 操作的是 daemon 的运行状态或注册表；`remove` 删注册记录，磁盘上的模型文件保留——注册错了删掉重来，模型不用重新下载。

MacAIConsole 当前提供：

- Runtime 状态和系统内存压力
- 已加载模型、驻留内存和有效加速设备
- 模型仓库、注册、加载、卸载、改名和详细设置
- Chat / STT / TTS 任务记录与详情
- GUI / daemon 最近日志，支持 Info / Debug 过滤和级别着色
- 菜单栏状态与 daemon 启停

## 系统要求

- Apple Silicon Mac
- macOS 14 或更高版本
- Rust stable toolchain
- Xcode Command Line Tools
- CMake
- Python 3.12（Kokoro TTS 与 sherpa-onnx STT 需要）

仓库不含模型权重、编译好的第三方推理引擎和 Python 虚拟环境——这些都留在本机，仓库保持干净。

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

### 3. 构建 llama.cpp worker

构建脚本固定了经过验证的 llama.cpp revision，升版本之前先跑通这里的版本：

```bash
./scripts/build-llama-server.sh
```

产物在 `.build/llama.cpp/bin/llama-server`。本机已经有合适的二进制的话：

```bash
export AIWORK_LLAMA_SERVER=/absolute/path/to/llama-server
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

几个常用的后续操作：已注册但没在跑的模型用 `macai start <model>` 重新加载；`macai keep-alive <model> 30m` 调整空闲驻留时间；`macai rename` 改模型 ID，运行中的会先停；`macai remove` 删注册记录。TTS 模型可以用 `macai voice <model>` 看音色，`macai voice <model> <voice>` 设默认音色。

### 4. 构建 whisper.cpp worker

```bash
./scripts/build-whisper-cli.sh
./scripts/download-whisper-model.sh base
```

产物：

```text
.build/whisper.cpp/bin/whisper-cli
.build/models/ggml-base.bin
```

路径可以用环境变量覆盖：

```bash
export AIWORK_WHISPER_CLI=/absolute/path/to/whisper-cli
```

Core ML encoder 是可选项，有它更快。把编译好的 `.mlmodelc` 目录放到 `.bin` 同目录、保持对应名称：

```text
ggml-large-v3-turbo.bin
ggml-large-v3-turbo-encoder.mlmodelc/
```

没有 encoder 就走 Metal 路径，能用。

### 5. 准备 Qwen3-ASR 0.6B

Qwen3-ASR 通过 MLX 8-bit Provider 提供服务，Apple Silicon 上走 Metal 加速。

Python 环境也可以在 MacAIConsole「设置 → Python 运行环境」里一键安装（需要本机有 Python 3.12，依赖版本与本节一致）。手动装的话：

```bash
uv venv --python 3.12 .build/qwen3-asr-mlx-venv
uv pip install --python .build/qwen3-asr-mlx-venv/bin/python 'mlx-audio==0.5.0'

hf download mlx-community/Qwen3-ASR-0.6B-8bit \
  --local-dir "$HOME/Library/Application Support/MacAIConsole/Models/stt/Qwen3-ASR-0.6B-MLX-8bit"
```

注册时选 `qwen3-asr-mlx`。daemon 会校验模型配置确实是 8-bit——选错 provider 会直接报错，这是故意的：

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

MLX Provider 固定使用 Metal。`AIWORK_QWEN3_ASR_MLX_PYTHON` 可以指向其他隔离环境。

### 6. 准备 sherpa-onnx zh-int8-2025

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

安装与模型下载：

```bash
./scripts/setup-sherpa-onnx.sh
curl -L -o sherpa-onnx-model.tar.bz2 \
  https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30.tar.bz2
tar xf sherpa-onnx-model.tar.bz2
rm sherpa-onnx-model.tar.bz2
```

注册并运行。`aiworkd` 会校验完整目录，加载时启动一个常驻 Python worker，
识别时复用同一个 sherpa-onnx recognizer：

```bash
./target/release/macai load \
  ./sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30 \
  --id sherpa-onnx-zh-int8-2025 \
  --type stt \
  --provider sherpa-onnx \
  --keep-alive always

./target/release/macai transcribe meeting.m4a \
  --model sherpa-onnx-zh-int8-2025 \
  --language zh
```

上传入口会把 wav / mp3 / flac / ogg / m4a 统一解码为 PCM WAV；worker 再将
多声道输入下混为单声道并按 streaming chunk 解码，因此长录音不会因一次性
把全部音频交给模型而改变内存行为。该模型只支持中文；`--language` 省略或
使用 `zh` / `zh-CN` / `Chinese` 均可。

默认使用 CPU，显式选择 Core ML 可设置：

```bash
export AIWORK_SHERPA_ONNX_DEVICE=coreml
```

也可以用 `AIWORK_SHERPA_ONNX_PYTHON` 指向已安装 `sherpa-onnx==1.13.6` 与
`numpy>=1.26,<3` 的其他 Python 解释器。`AIWORK_SHERPA_ONNX_THREADS` 控制 ONNX Runtime 线程数；
`AIWORK_SHERPA_ONNX_LOAD_TIMEOUT_SECS` 与
`AIWORK_SHERPA_ONNX_INFERENCE_TIMEOUT_SECS` 分别控制加载和识别超时。

### 8. 准备 Kokoro TTS

```bash
python3.12 -m venv .build/kokoro-venv
.build/kokoro-venv/bin/pip install \
  mlx-audio \
  "misaki[zh]" \
  "misaki[en]" \
  phonemizer-fork \
  espeakng-loader
```

下载 [Kokoro-82M-zh-MLX](https://huggingface.co/1038lab/Kokoro-82M-zh-MLX)，模型目录放进：

```text
~/Library/Application Support/MacAIConsole/Models/tts/
```

同样，`AIWORK_KOKORO_PYTHON` 可以指向其他 Python 环境。

### 9. 准备 MLX-LM

```bash
python3.12 -m venv .build/mlx-lm-venv
.build/mlx-lm-venv/bin/pip install "mlx-lm==0.31.3"
```

推荐模型（Qwen3 8B · MLX 4-bit）可以在 MacAIConsole 模型页一键下载；或者手动把 MLX 格式模型目录（含 `config.json` 与 safetensors 权重，例如 [mlx-community/Qwen3-8B-4bit](https://huggingface.co/mlx-community/Qwen3-8B-4bit)）放进：

```text
~/Library/Application Support/MacAIConsole/Models/llm/
```

注册时显式选 provider `mlx-lm`。GGUF 和 MLX 格式互不通用：GGUF 走 llama.cpp，MLX 走 mlx-lm。`AIWORK_MLX_LM_PYTHON` 指向其他 Python 环境；加载与推理超时分别由 `AIWORK_MLX_LM_LOAD_TIMEOUT_SECS` 与 `AIWORK_MLX_LM_INFERENCE_TIMEOUT_SECS` 控制。

## MacAIConsole

```bash
cargo build --release -p ai-daemon
cd apps/MacAIConsole
scripts/build-app.sh release
open build/MacAIConsole.app
```

产物在 `apps/MacAIConsole/build/MacAIConsole.app`。

GUI 可以连接已经在跑的 `aiworkd`。GUI 自动启动 daemon 时按顺序探测：`AIWORKD_PATH` 环境变量 → 仓库 `target/release/aiworkd` → `target/debug/aiworkd`。

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

现有 OpenAI SDK 代码改个 `base_url` 就能接入，就这么简单。

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

`/v1/*` 面向推理，`/api/*` 面向运行时管理。

## 使用案例：为 Hermes Agent 提供本地 TTS 与 STT

[Hermes Agent](https://hermes-agent.nousresearch.com/docs/) 支持命令型语音 Provider。MacAI 通过本地 OpenAI-compatible API 给 Hermes 提供：

| Hermes 能力 | MacAI endpoint | MacAI Provider |
| --- | --- | --- |
| TTS（文字转语音） | `POST /v1/audio/speech` | Kokoro MLX |
| STT（语音转文字） | `POST /v1/audio/transcriptions` | whisper.cpp |

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

两个适配脚本的分工：TTS 脚本用 Python 标准库读取 Hermes 创建的 UTF-8 文本文件，调用 MacAI 后把音频写入 `{output_path}`；STT 脚本先把 Hermes 收到的 WAV、OGG、Opus 或其他 ffmpeg 支持的音频统一转成 MacAI 当前要求的 PCM WAV，再返回纯文本。

STT Provider 单独命名为 `macai` 还有另一个原因：Hermes 内置的 `openai` STT 路径可能把 `whisper-large-v3-turbo` 认成 Groq 模型名，改写成 `whisper-1`。命令型 Provider 会把 MacAI 的模型 ID 原样传递，绕开这个坑。

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

### 常见问题

- **`model_not_found`**：跑一下 `curl http://127.0.0.1:11435/v1/models`，把脚本配置里的模型 ID 改成返回的真实 `id`，或者重新跑一键配置。
- **`Connection refused`**：先通过 MacAIConsole 启动 daemon，或者在仓库里跑 `./target/release/aiworkd`。
- **`Required command is missing: ffmpeg`**：装好 ffmpeg 重新跑配置；STT 适配器靠它统一音频格式。
- **Hermes 还显示旧 Provider**：重启 Hermes Desktop 或 Gateway。Hermes profile 相互隔离，确认跑配置脚本和启动 Hermes 用的是同一个 profile。
- **模型第一次请求慢**：whisper.cpp 的 Core ML encoder 第一次做 ANE 特化需要额外时间，之后会复用缓存。

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

日志到 5 MB 轮换，保留一份 `.1` 文件。日志页面默认显示 Info 及以上级别；启用 Debug 会同时调整 GUI 和 daemon 的运行时日志级别。

## 安全边界

`aiworkd` 现在只监听 loopback，也没有做局域网认证——所以请把它留在 `127.0.0.1` 上，走端口转发或反向代理暴露到不受信任网络之前，先想清楚。

任务历史可能包含 prompt、转写文本和 TTS 输入。原始音频不写入任务历史，日志也不记录 API token 或模型内容。

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
└── ai-cli/        # macai 命令行客户端

apps/
└── MacAIConsole/  # 原生 SwiftUI 控制台

scripts/           # llama.cpp、whisper.cpp 与 Kokoro worker 工具
samples/           # 第三方集成示例（含 Hermes TTS/STT 适配器）
handoff.md         # 目标架构与长期路线
```

## 路线图

handoff 里排队中的主要工作：

- MLX-native ASR（mlx-whisper / parakeet-mlx）
- LLM / STT / TTS benchmark 框架（MLX vs llama.cpp 对比实验）
- per-device 请求队列、独立 queue/execution deadline、持久任务和事件回放
- GUI Chat、Speech 和 Hugging Face 下载界面
- VAD、streaming STT 和 streaming TTS
- readiness / deep-health、启动进度与完整可观测性
- Homebrew、正式签名、公证与安装包

完整目标设计见 [`handoff.md`](handoff.md)。它描述的是长期架构，当前实现状态以本 README 和代码为准。
