# MacAI Workbench Runtime

这是 `handoff.md` 中 Local AI Runtime 架构的可运行实现。SwiftUI、CLI 与 OpenAI-compatible 客户端最终都连接同一个 Rust daemon：

```text
ai CLI ─┐
        ├─ HTTP ─> aiworkd ─┬─> MockProvider
OpenAI ─┘                   └─> llama.cpp worker ─> Metal / GGUF
```

## 当前实现

- Rust workspace：`ai-core`、`ai-daemon`、`ai-cli`
- `aiworkd` 默认只监听 `127.0.0.1:11435`
- Provider Registry 与统一 `ProviderDescriptor` / `ProviderStatus`
- MockProvider：内置回显测试路径
- LlamaCppProvider：管理持久 `llama-server` 子进程
- GGUF load / unload 与 Apple Silicon Metal offload
- 模型 lease / busy guard：推理期间 unload 返回 503，流结束或断开后自动释放
- stale handle 自恢复：worker 崩溃后下一次请求自动重建 `llama-server`
- 普通 Chat Completion 与逐 token SSE streaming
- WhisperCppProvider：本地 `whisper-cli` + Metal 离线转写
- MacOSSayProvider：macOS `say` + `afconvert` 输出 16 kHz mono PCM WAV
- OpenAI-compatible endpoints：
  - `GET /health`
  - `GET /v1/models`
  - `POST /v1/chat/completions`
  - `POST /v1/audio/transcriptions`
  - `POST /v1/audio/speech`
- Runtime 管理 endpoints：
  - `GET /api/runtime`
  - `GET /api/providers`
  - `POST /api/models/load`
  - `POST /api/models/{id}/load`
  - `POST /api/models/{id}/unload`
- 统一 API / Provider 错误结构
- 活跃请求计数；流结束或客户端断开时自动释放
- CLI：`status`、`list`、`chat`、`load`、`unload`、`run`、`transcribe`、`speak`、`ps`、`serve`

## 构建与验证

需要：

- Apple Silicon Mac
- Rust stable toolchain
- CMake
- Apple Clang / Xcode Command Line Tools

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
```

## 构建 llama.cpp Worker

项目固定 llama.cpp commit：

```text
8086439a4cea94c71a5dfb8fe4ad1546aebd640f
```

运行：

```bash
./scripts/build-llama-server.sh
```

产物：

```text
.build/llama.cpp/bin/llama-server
```

该 server 只作为内部推理 worker，因此构建脚本关闭嵌入式 WebUI，并启用 Metal 与 Accelerate。`.build/` 不进入 Git。

也可使用已有二进制：

```bash
export AIWORK_LLAMA_SERVER=/absolute/path/to/llama-server
```

内部 worker 默认监听 `127.0.0.1:11436`，可通过 `AIWORK_LLAMA_PORT` 修改。端口被其他进程占用时，daemon 会明确报错，不会终止未知进程。

## 构建 whisper.cpp STT

项目固定 whisper.cpp `v1.9.3` commit：

```text
371b5a7561823ab2bb32142d2751e35e7534727b
```

构建 Metal 版 `whisper-cli` 并下载 base 模型：

```bash
./scripts/build-whisper-cli.sh
./scripts/download-whisper-model.sh base
```

产物：

```text
.build/whisper.cpp/bin/whisper-cli
.build/models/ggml-base.bin
```

本机实际模型路径：

```text
/path/to/MacAI/.build/models/ggml-base.bin
```

可用环境变量覆盖默认路径：

```bash
export AIWORK_WHISPER_CLI=/absolute/path/to/whisper-cli
export AIWORK_WHISPER_MODEL=/absolute/path/to/ggml-base.bin
```

STT 当前接受 PCM WAV。TTS 通过 macOS 自带的 `/usr/bin/say` 与 `/usr/bin/afconvert` 输出 16 kHz、mono、PCM16 WAV，不需要额外下载 TTS 权重。系统 TTS Provider ID 为 `macos-say`，后续 MLX-Audio Provider 会沿用同一 API 契约。

## 启动

终端 1：

```bash
cargo run -p ai-daemon --bin aiworkd
```

终端 2：

```bash
cargo run -p ai-cli --bin ai -- status
cargo run -p ai-cli --bin ai -- list
cargo run -p ai-cli --bin ai -- chat mock hello
cargo run -p ai-cli --bin ai -- ps
```

构建后也可直接使用：

```bash
./target/debug/aiworkd
./target/debug/ai chat mock hello
```

## 运行 GGUF 模型

### 本机 SmolLM2 测试示例

以下示例使用已经下载到本机 `.build/models/` 的测试模型。`.build/` 已被 Git 忽略，模型权重不会进入仓库。

终端 1 启动 daemon：

```bash
cd /path/to/MacAI
./target/debug/aiworkd
```

终端 2 加载模型、对话、检查状态并卸载：

```bash
cd /path/to/MacAI

MODEL_PATH="/path/to/MacAI/.build/models/SmolLM2-135M-Instruct-Q4_K_M.gguf"

test -f "$MODEL_PATH"

./target/debug/ai load "$MODEL_PATH" \
  --id smollm2 \
  --context-length 2048

./target/debug/ai chat smollm2 "请用一句中文介绍你自己"
./target/debug/ai ps
./target/debug/ai unload smollm2
```

预期关键输出：

```text
Loaded smollm2 with llama.cpp
MODEL              PROVIDER      STATE
smollm2            llama.cpp     ready
Unloaded smollm2
```

也可以把同一个本地路径直接交给交互命令，CLI 会自动注册并加载模型：

```bash
./target/debug/ai run \
  /path/to/MacAI/.build/models/SmolLM2-135M-Instruct-Q4_K_M.gguf
```

### 通用用法

显式加载任意本地 GGUF：

```bash
./target/debug/ai load /path/to/model.gguf \
  --id local-model \
  --context-length 4096

./target/debug/ai chat local-model "Hello"
./target/debug/ai ps
./target/debug/ai unload local-model
```

也可把 GGUF 路径直接交给 `run`。CLI 会自动注册并加载模型：

```bash
./target/debug/ai run /path/to/model.gguf
```

`ai models` 保留为 `ai list` 的兼容别名。

## STT / TTS 测试

先生成一段标准 WAV：

```bash
./target/debug/ai speak \
  "你好，这是 Mac AI 的语音识别测试。" \
  --voice Tingting \
  --output .build/tts-stt-test.wav
```

再使用本地 whisper.cpp 模型转写：

```bash
./target/debug/ai transcribe \
  .build/tts-stt-test.wav \
  --model whisper-base \
  --language zh
```

本机已验证的关键输出：

```text
Wrote 99824 bytes to .build/tts-stt-test.wav
你好,這是MegaE的語音時別測試。
```

`ggml-base` 体积较小，示例重点验证完整链路。需要更高转写精度时，可通过同一下载脚本选择更大的 whisper.cpp 模型，并将 `AIWORK_WHISPER_MODEL` 指向对应 `.bin` 文件。

TTS HTTP API：

```bash
curl http://127.0.0.1:11435/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "macos-say",
    "input": "你好，这是本地语音合成。",
    "voice": "Tingting",
    "format": "wav",
    "speed": 1.0
  }' \
  -o speech.wav
```

STT HTTP API：

```bash
curl http://127.0.0.1:11435/v1/audio/transcriptions \
  -F file=@speech.wav \
  -F model=whisper-base \
  -F language=zh \
  -F response_format=json
```

## HTTP API

非流式：

```bash
curl http://127.0.0.1:11435/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "local-model",
    "messages": [{"role": "user", "content": "hello"}],
    "stream": false
  }'
```

流式：

```bash
curl --no-buffer http://127.0.0.1:11435/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "local-model",
    "messages": [{"role": "user", "content": "hello"}],
    "stream": true
  }'
```

流式响应以 OpenAI-compatible `data: [DONE]` 结束。

## 当前边界

当前版本完成真实 llama.cpp Chat、whisper.cpp 离线 STT 与 macOS 系统 TTS 的端到端路径。SQLite model registry、Hugging Face 自动模型管理、MLX Provider、MLX-Audio 高质量 TTS、实时 STT/VAD、内存预算/LRU/keep-alive 和 SwiftUI GUI 在后续里程碑实现。

完整产品与架构说明见 [`handoff.md`](handoff.md)。
