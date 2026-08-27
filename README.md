# MacAI

<p align="center">
  <img src="logo.png" alt="MacAI logo" width="280">
</p>

MacAI 是面向 Apple Silicon 的本地 AI Runtime。Rust daemon `aiworkd` 统一管理模型、推理 worker、内存调度和 HTTP API；CLI 与原生 SwiftUI 应用都通过同一个本地服务工作。

> 项目处于活跃开发阶段，当前适合本地开发、实验和个人工作流。发布安装、API 稳定性和向后兼容尚未承诺。

## 架构

```text
ai CLI ───────────────┐
MacAIConsole (SwiftUI) ├── HTTP ──> aiworkd ──┬── llama.cpp ──> GGUF / Metal
OpenAI-compatible SDK ┘                      ├── whisper.cpp ─> Core ML / Metal
                                             └── Kokoro MLX ──> local TTS

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
| STT | whisper.cpp | `.bin` + PCM WAV | Core ML 优先，Metal 回退 |
| TTS | Kokoro MLX | Kokoro 模型目录 | MLX / Metal GPU |

Provider 以独立进程承载高风险或第三方推理运行时。某个 worker 退出时，`aiworkd` 保持运行并向客户端返回结构化错误。

### 客户端

CLI `ai` 当前提供：

```text
status      list        ps          providers   tasks
load        start       unload      remove      rename
keep-alive  pull        chat        run         transcribe
speak       voice       logging     serve
```

运行 `ai --help` 查看完整命令索引，运行 `ai <命令> --help` 查看参数和示例。`start`、`unload`、`rename`、`keep-alive` 和 `remove` 操作 daemon 的运行状态或注册表；`remove` 会保留磁盘上的模型文件。

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
./target/release/ai status
./target/release/ai list
./target/release/ai ps
./target/release/ai --help
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
./target/release/ai load /path/to/model.gguf \
  --id local-model \
  --type llm \
  --keep-alive 5m \
  --context-length 4096

./target/release/ai chat local-model "Hello" \
  --system "Answer concisely" \
  --temperature 0.2 \
  --max-tokens 256

./target/release/ai tasks --limit 10
./target/release/ai providers
./target/release/ai unload local-model
```

已注册但未运行的模型可用 `ai start <model>` 重新加载。`ai keep-alive <model> 30m` 调整空闲驻留时间；`ai rename` 修改模型 ID，运行中的模型会先停止；`ai remove` 删除注册记录。TTS 模型可通过 `ai voice <model>` 查看音色，并用 `ai voice <model> <voice>` 设置默认音色。

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

### 5. 准备 Kokoro TTS

创建 Python 环境：

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

GUI 可以连接已经运行的 `aiworkd`。如需由 GUI 自动启动 daemon，请在设置中选择 `target/release/aiworkd` 的实际路径。

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
handoff.md         # 目标架构与长期路线
```

## 路线图

当前 handoff 中仍未完成的主要工作：

- MLX-LM Provider 与 MLX-native ASR
- LLM / STT / TTS benchmark 框架
- per-device 请求队列、独立 queue/execution deadline、持久任务和事件回放
- GUI Chat、Speech 和 Hugging Face 下载界面
- VAD、streaming STT 和 streaming TTS
- readiness / deep-health、启动进度与完整可观测性
- Homebrew、正式签名、公证与安装包

完整目标设计见 [`handoff.md`](handoff.md)。它描述长期架构，当前实现状态以本 README 和代码为准。
