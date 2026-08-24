# MacAI Workbench Runtime

<p align="center">
  <img src="logo.png" alt="MacAI logo" width="320">
</p>

这是 `handoff.md` 中 Local AI Runtime 架构的可运行实现。SwiftUI、CLI 与 OpenAI-compatible 客户端最终都连接同一个 Rust daemon：

```text
ai CLI ─┐
        ├─ HTTP ─> aiworkd ─┬─> llama.cpp worker  ─> Metal / GGUF
OpenAI ─┘                   ├─> whisper.cpp worker ─> Metal 离线转写
                            ├─> kokoro-mlx worker ─> mlx-audio / 中文 TTS
MacAIConsole ────────────── └─> SQLite model registry / 内存调度
```

## 当前实现

- Rust workspace：`ai-core`、`ai-daemon`、`ai-cli`
- `aiworkd` 默认只监听 `127.0.0.1:11435`
- Provider Registry 与统一 `ProviderDescriptor` / `ProviderStatus`（管理面只暴露真实引擎；mock/macos-say 仅作测试能力）
- LlamaCppProvider：管理持久 `llama-server` 子进程，GGUF load/unload 与 Metal offload
- WhisperCppProvider：本地 `whisper-cli` + Metal 离线转写
- KokoroMlxProvider：Kokoro-82M-zh 中文 TTS，常驻 Python worker（mlx-audio / Metal GPU），中英混说与 OOV 专名已修复
- MacOSSayProvider：macOS `say` 兜底 TTS（测试能力，不进生产注册表）
- **SQLite 模型注册表**：daemon 重启自动恢复模型清单（`~/Library/Application Support/MacAIConsole/models.db`）
- **内存调度**：AI 预算 `min(ram×0.75, ram−8GB)` 可配置；预算不足按 LRU 逐出空闲模型；keep_alive 到期后台 reaper 自动卸载
- 模型 lease / busy guard：推理期间 unload 返回 503，流结束或断开后自动释放
- stale handle 自恢复：worker 崩溃后下一次请求自动重建
- 普通 Chat Completion 与逐 token SSE streaming
- OpenAI-compatible endpoints：
  - `GET /health`
  - `GET /v1/models`
  - `POST /v1/chat/completions`
  - `POST /v1/audio/transcriptions`
  - `POST /v1/audio/speech`
- Runtime 管理 endpoints：
  - `GET /api/runtime`（含 memory_budget）
  - `GET /api/providers`
  - `POST /api/models/load`（model_type 路由 llm/stt/tts）
  - `POST /api/models/pull`（HuggingFace 单文件下载，断点续传）
  - `POST /api/models/{id}/load`
  - `POST /api/models/{id}/unload`
- 统一 API / Provider 错误结构；活跃请求计数，流结束或客户端断开时自动释放
- CLI：`status`、`list`、`pull`、`chat`、`load`、`unload`、`run`、`transcribe`、`speak`、`ps`、`serve`
- **MacAIConsole**（`apps/MacAIConsole`）：SwiftUI 原生 GUI——运行状态页（含内存预算）、模型管理页（llm/tts/stt 分组、仓库扫描、上下文长度 K 单位热调、TTS 试听）、菜单栏状态摘要、GUI 掌管 daemon 生命周期

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

TTS HTTP API（macos-say 系统语音）：

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

### Kokoro 中文 TTS（MLX，Metal GPU 加速）

[1038lab/Kokoro-82M-zh-MLX](https://huggingface.co/1038lab/Kokoro-82M-zh-MLX) 提供 103 个声音
（55 中文女声 zf_*、45 中文男声 zm_*、3 英文），由常驻 Python worker（mlx-audio）推理。

一次性环境准备：

```bash
# Python venv + 依赖（Python 3.12）
/opt/homebrew/bin/python3.12 -m venv .build/kokoro-venv
.build/kokoro-venv/bin/pip install mlx-audio "misaki[zh]" "misaki[en]" phonemizer-fork espeakng-loader

# 模型文件放入模型仓库的 tts/ 分区
mkdir -p "$HOME/Library/Application Support/MacAIConsole/Models/tts/Kokoro-82M-zh-MLX/voices"
cd "$HOME/Library/Application Support/MacAIConsole/Models/tts/Kokoro-82M-zh-MLX"
curl -LO https://huggingface.co/1038lab/Kokoro-82M-zh-MLX/resolve/main/model.safetensors
curl -LO https://huggingface.co/1038lab/Kokoro-82M-zh-MLX/resolve/main/config.json
# 下载 voices/*.safetensors 到 voices/ 目录（103 个文件，每个约 0.5MB）
```

注册并加载（GUI「模型管理 → 添加模型」选类型 TTS，或直接调 API）：

```bash
curl http://127.0.0.1:11435/api/models/load \
  -H 'Content-Type: application/json' \
  -d '{
    "path": "'"$HOME"'/Library/Application Support/MacAIConsole/Models/tts/Kokoro-82M-zh-MLX",
    "id": "kokoro-zh",
    "model_type": "tts"
  }'
```

合成中文语音（本机实测 RTF≈0.55，快于实时）：

```bash
curl http://127.0.0.1:11435/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Kokoro-82M-zh-MLX",
    "input": "主人你好，我是Kokoro中文语音合成。",
    "voice": "zf_001",
    "response_format": "wav"
  }' \
  -o kokoro.wav && afplay kokoro.wav
```

> 排错提示：请求失败时 daemon 返回 JSON 错误体，`curl -o` 会把它写进输出文件——
> 若 `afplay` 报 AudioFileOpenURL failed，先 `cat kokoro.wav` 查看实际错误
> （最常见是 model ID 与注册时不一致）。

说明：`model_type` 决定 provider 路由——`llm`→llama.cpp（`.gguf`）、`stt`→whisper.cpp（`.bin`）、`tts`→kokoro-mlx（含 `model.safetensors` 的目录）。加载 = 拉起对应 worker 进程，卸载 = 结束该进程。

> 已知兼容性修复：mlx-audio 的 KokoroPipeline 调用 `misaki.zh.ZHG2P()` 时不带
> `version` 参数，默认输出 IPA 音素（`tu↗ʂu→`），与 v1.1-zh 模型的注音符号 vocab
> 不匹配——声调会被静默丢弃、中文听不清。`scripts/kokoro_worker.py` 启动时会
> 把默认 version 钉为 `'1.1'`（与官方 kokoro 一致），输出 `ㄉㄨ2ㄕㄨ1…` 注音符号，
> 100% 命中 vocab。若更新 mlx-audio 后中文异常，优先检查该 patch 是否仍生效。

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

当前版本已完成 handoff 的 Milestone 1（Runtime 架构）与 Milestone 2（SQLite model registry、`ai pull` HuggingFace 下载、内存预算/LRU/keep-alive、busy guard），以及 Milestone 3 的一部分：whisper.cpp 离线 STT 与 MLX-Audio 高质量中文 TTS（Kokoro-82M-zh）。GUI（MacAIConsole）已提供运行状态、模型管理、TTS 试听与菜单栏摘要。

尚未实现：MLX LLM Provider、LLM/STT/TTS benchmark 框架、long-running job metadata 与有界事件回放、GUI Chat/Speech 页面、实时 STT/VAD/streaming TTS。

完整产品与架构说明见 [`handoff.md`](handoff.md)。
