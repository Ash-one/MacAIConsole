下面这份可以直接交给 Codex / Claude Code / Cursor 等开发 Agent 作为项目 handoff，也适合作为仓库根目录的 `HANDOFF.md`。

# macOS Local AI Workbench — Development Handoff

## 0. Document Purpose

本文档用于指导一个 macOS 本地一站式 AI Runtime / Workbench 的设计与实现。

项目目标是在 Apple Silicon Mac 上提供统一的本地 AI 推理环境，支持：

* LLM
* VLM（后续）
* Speech-to-Text
* Text-to-Speech
* Embedding / Reranker（后续）
* Image / Video Generation（后续）

系统必须同时提供：

1. macOS 原生 GUI
2. CLI
3. HTTP API
4. 可插拔推理 Backend
5. 本地模型管理
6. 统一内存管理
7. Streaming
8. 后台 Daemon

项目定位接近：

* Ollama 的 CLI / Model UX
* LocalAI 的多 Backend 架构
* Jan 的 Desktop + Runtime 架构
* MLX / llama.cpp 的本地推理能力

核心原则：

> GUI、CLI 和 API 都只是同一个 Local AI Runtime 的客户端。

---

# 1. Product Goal

用户安装应用后，应能够执行：

```bash
ai pull qwen3
ai run qwen3

ai transcribe meeting.wav

ai speak "Hello world"

ai list
ai ps
ai stop qwen3
```

同时能够通过：

```bash
curl http://127.0.0.1:11435/v1/chat/completions
```

访问本地模型。

Python 应兼容：

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:11435/v1",
    api_key="local",
)

response = client.chat.completions.create(
    model="qwen3",
    messages=[
        {"role": "user", "content": "Hello"}
    ],
)
```

macOS GUI 则提供：

* 模型下载
* 模型管理
* Chat
* STT
* TTS
* Runtime 状态
* Memory 使用情况
* API Server 管理
* Backend 设置

三种入口：

```text
SwiftUI GUI
CLI
HTTP API
    │
    ▼
aiworkd
```

共享完全相同的模型和运行状态。

---

# 2. Target Platform

第一阶段只支持：

```text
Apple Silicon Mac
```

目标架构：

```text
arm64
```

推荐最低系统：

```text
macOS 14+
```

如果部分 MLX API 需要更高系统版本，可进一步调整。

Intel Mac 暂不进入第一阶段。

---

# 3. Primary Architecture

系统由四层组成：

```text
┌──────────────────────────────────────────────┐
│                  Clients                     │
│                                              │
│ SwiftUI.app      CLI          OpenAI Client │
└───────────────┬───────────────┬──────────────┘
                │               │
                ▼               ▼
        ┌────────────────────────────┐
        │          aiworkd           │
        │                            │
        │ Runtime Manager            │
        │ Model Manager              │
        │ Scheduler                  │
        │ OpenAI API                 │
        │ Provider Manager           │
        └─────────────┬──────────────┘
                      │
        ┌─────────────┼────────────────────┐
        │             │                    │
        ▼             ▼                    ▼
   llama.cpp         MLX              MLX-Audio
                                        │
                                  ┌─────┴─────┐
                                  ▼           ▼
                                 STT         TTS
```

额外 Backend：

```text
whisper.cpp
sherpa-onnx
```

后续：

```text
Stable Diffusion
FLUX
Embedding
Reranker
VLM
Video
```

---

# 4. Technology Stack

## 4.1 macOS GUI

使用：

```text
Swift
SwiftUI
AVFoundation
```

GUI 负责：

* Chat
* 模型管理
* 下载状态
* Runtime 状态
* 麦克风输入
* STT
* TTS
* API 设置
* Preferences
* Menu Bar
* Global Shortcut

GUI 不直接负责模型推理。

所有 inference 请求发送给：

```text
aiworkd
```

---

# 4.2 Core Daemon

推荐：

```text
Rust
Tokio
Axum
Serde
Clap
SQLite
```

Daemon 名称：

```text
aiworkd
```

职责：

```text
HTTP Server
Model lifecycle
Provider lifecycle
Model scheduler
Memory management
Download manager
Model registry
Config
Logging
Streaming
Health monitoring
```

---

# 4.3 CLI

CLI 名称：

```text
ai
```

Rust 实现。

使用：

```text
clap
```

CLI 不启动自己的 inference runtime。

调用：

```text
aiworkd
```

---

# 5. Repository Structure

建议：

```text
ai-workbench/
│
├── README.md
├── HANDOFF.md
├── Cargo.toml
│
├── crates/
│   │
│   ├── ai-core/
│   │   ├── model.rs
│   │   ├── provider.rs
│   │   ├── request.rs
│   │   ├── response.rs
│   │   └── errors.rs
│   │
│   ├── ai-daemon/
│   │   ├── server/
│   │   ├── runtime/
│   │   ├── scheduler/
│   │   ├── registry/
│   │   ├── download/
│   │   └── main.rs
│   │
│   └── ai-cli/
│       └── main.rs
│
├── providers/
│   │
│   ├── llama-cpp/
│   │
│   ├── mlx-lm/
│   │
│   ├── whisper-cpp/
│   │
│   ├── mlx-audio/
│   │
│   └── sherpa-onnx/
│
├── macos/
│   └── AIWorkbench/
│       ├── App/
│       ├── Models/
│       ├── Services/
│       ├── Views/
│       └── Components/
│
├── schemas/
│
├── scripts/
│
├── tests/
│
└── benchmarks/
```

---

# 6. Core Runtime Design

核心对象：

```rust
struct Runtime {
    model_registry: ModelRegistry,
    provider_registry: ProviderRegistry,
    scheduler: Scheduler,
    memory_manager: MemoryManager,
}
```

Runtime 应负责：

```text
model resolution
provider selection
model load/unload
request dispatch
memory scheduling
health check
```

---

# 7. Provider Abstraction

这是项目最重要的 abstraction。

所有 inference engine 必须实现统一 Provider interface。

示例：

```rust
#[async_trait]
pub trait Provider {
    fn id(&self) -> &'static str;

    fn capabilities(&self) -> Vec<Capability>;

    async fn load(
        &self,
        model: &ModelSpec,
    ) -> Result<ModelHandle>;

    async fn unload(
        &self,
        handle: &ModelHandle,
    ) -> Result<()>;

    async fn health_check(
        &self,
    ) -> Result<ProviderHealth>;
}
```

Capability：

```rust
enum Capability {
    Chat,
    Completion,
    Vision,
    Embedding,
    SpeechToText,
    TextToSpeech,
    Rerank,
    ImageGeneration,
}
```

LLM Provider：

```rust
trait ChatProvider: Provider {
    async fn chat(
        &self,
        request: ChatRequest,
    ) -> Result<ChatStream>;
}
```

STT：

```rust
trait STTProvider: Provider {
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse>;
}
```

TTS：

```rust
trait TTSProvider: Provider {
    async fn synthesize(
        &self,
        request: SpeechRequest,
    ) -> Result<AudioStream>;
}
```

---

# 8. Provider Process Model

不同 Backend 不需要使用同一种语言。

例如：

```text
llama.cpp
    C++

whisper.cpp
    C++

MLX-LM
    Python

MLX-Audio
    Python

sherpa-onnx
    C++ / Rust
```

推荐结构：

```text
aiworkd
  │
  ├── llama provider
  │
  ├── mlx-lm worker
  │
  ├── mlx-audio worker
  │
  └── whisper worker
```

Python Backend 采用 persistent worker。

禁止每次请求：

```text
spawn python
load model
inference
exit
```

应该：

```text
spawn worker
    ↓
load model
    ↓
keep alive
    ↓
request 1
request 2
request 3
```

---

# 9. Inter-Process Communication

第一版本优先：

```text
Unix Domain Socket
```

推荐消息：

```text
JSON
```

例如：

```json
{
  "id": "req_123",
  "method": "load_model",
  "params": {
    "model": "/models/qwen"
  }
}
```

后续性能需要时可迁移：

```text
MessagePack
Protobuf
gRPC
```

第一阶段不要因为 RPC 技术提前增加系统复杂度。

---

# 10. Initial Providers

## 10.1 LLM

第一阶段：

```text
llama.cpp
MLX-LM
```

llama.cpp 支持：

```text
GGUF
Metal
```

MLX 支持：

```text
MLX models
Hugging Face MLX weights
```

模型配置：

```toml
id = "qwen3-8b"

type = "llm"

provider = "auto"

[source]
repo = "mlx-community/Qwen3-8B-4bit"

[requirements]
memory = 6000000000
```

`provider = auto` 时：

```text
MLX model
    → MLX

GGUF
    → llama.cpp
```

---

# 11. Speech-to-Text

第一阶段实现：

```text
whisper.cpp
MLX-Audio
```

第二阶段：

```text
sherpa-onnx
```

用途：

### whisper.cpp

适合：

```text
stable transcription
offline transcription
low dependency
```

### MLX-Audio

适合：

```text
Apple Silicon
new ASR models
Parakeet
Whisper
Qwen audio models
```

### sherpa-onnx

适合：

```text
streaming
VAD
keyword spotting
speaker recognition
```

---

# 12. Text-to-Speech

第一版本优先：

```text
MLX-Audio
```

Backend 应允许多个模型，例如：

```text
Qwen3-TTS
Kokoro
Dia
Chatterbox
```

统一 API：

```text
POST /v1/audio/speech
```

---

# 13. OpenAI-Compatible API

API Server 默认：

```text
127.0.0.1:11435
```

初始实现：

```text
GET  /health

GET  /v1/models

POST /v1/chat/completions

POST /v1/audio/transcriptions

POST /v1/audio/speech
```

第二阶段：

```text
POST /v1/responses

POST /v1/embeddings

WS /v1/realtime
```

---

# 14. Chat API

Example：

```http
POST /v1/chat/completions
```

Request：

```json
{
  "model": "qwen3",
  "messages": [
    {
      "role": "user",
      "content": "Hello"
    }
  ],
  "stream": true
}
```

Response streaming：

```text
SSE
```

必须支持：

```text
stream=true
```

Streaming 是第一版必需功能。

---

# 15. STT API

遵循：

```text
POST /v1/audio/transcriptions
```

支持 multipart：

```text
file
model
language
response_format
```

示例：

```bash
curl \
  http://127.0.0.1:11435/v1/audio/transcriptions \
  -F file=@meeting.wav \
  -F model=whisper-large-v3
```

---

# 16. TTS API

```text
POST /v1/audio/speech
```

示例：

```json
{
  "model": "qwen3-tts",
  "input": "你好，这是本地模型。",
  "voice": "default",
  "format": "wav"
}
```

返回：

```text
audio/wav
```

后续支持 streaming PCM。

---

# 17. Internal Management API

额外提供本地 Runtime API：

```text
GET    /api/runtime

GET    /api/models

POST   /api/models/pull

POST   /api/models/load

POST   /api/models/unload

DELETE /api/models/:id
```

例如：

```json
POST /api/models/load

{
  "model": "qwen3"
}
```

---

# 18. CLI Specification

## Models

```bash
ai list
```

输出：

```text
NAME                 TYPE    SIZE     STATUS
qwen3-8b             llm     5.2 GB   loaded
whisper-large-v3     stt     3.1 GB   idle
qwen3-tts            tts     4.8 GB   idle
```

---

## Pull

```bash
ai pull qwen3
```

行为：

```text
resolve model
download files
verify files
register model
```

---

## Run

```bash
ai run qwen3
```

进入：

```text
interactive chat
```

---

## Chat

```bash
ai chat qwen3 "Explain transformers"
```

---

## STT

```bash
ai transcribe audio.wav
```

可选：

```bash
ai transcribe audio.wav \
    --model whisper-large-v3 \
    --language zh
```

---

## TTS

```bash
ai speak "Hello world"
```

或者：

```bash
ai speak \
    "你好" \
    --model qwen3-tts \
    --output output.wav
```

---

## Runtime

```bash
ai ps
```

例如：

```text
MODEL              PROVIDER      MEMORY      STATE
qwen3-8b           mlx           5.7 GB      running
whisper-large-v3   whisper.cpp   3.2 GB      idle
```

---

## Stop

```bash
ai stop qwen3-8b
```

---

## Server

```bash
ai serve
```

如果 daemon 已运行：

```text
Server already running
http://127.0.0.1:11435
```

---

# 19. Model Registry

使用：

```text
SQLite
```

数据库：

```text
~/Library/Application Support/AIWorkbench/models.db
```

模型文件：

```text
~/Library/Application Support/AIWorkbench/models/
```

推荐 schema：

```sql
CREATE TABLE models (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    provider TEXT,
    source TEXT,
    path TEXT NOT NULL,
    format TEXT,
    size_bytes INTEGER,
    installed_at INTEGER,
    last_used_at INTEGER
);
```

---

# 20. Model Source

第一阶段：

```text
Hugging Face
```

支持：

```text
repo
revision
filename
```

示例：

```text
mlx-community/Qwen3-8B-4bit
```

以及：

```text
repo/model-Q4_K_M.gguf
```

下载必须支持：

```text
resume
progress
checksum
cancel
```

---

# 21. Model Manifest

模型目录：

```text
models/qwen3/
```

内部：

```text
model.toml
```

示例：

```toml
id = "qwen3"
name = "Qwen3 8B"

type = "llm"

format = "mlx"

provider = "mlx"

source = "mlx-community/Qwen3-8B-4bit"

memory_estimate = 6000000000

[parameters]
context_length = 32768
```

---

# 22. Memory Manager

Apple Silicon 使用 Unified Memory。

系统必须主动管理模型常驻状态。

需要跟踪：

```text
system memory
available memory
estimated model memory
actual RSS
last_used
model state
```

模型生命周期：

```text
Unloaded
↓
Loading
↓
Ready
↓
Busy
↓
Idle
↓
Unloading
```

状态定义：

```rust
enum ModelState {
    Unloaded,
    Loading,
    Ready,
    Busy,
    Idle,
    Unloading,
    Failed,
}
```

---

# 23. Memory Budget

默认：

```text
AI memory budget =
min(
    system_memory * 0.75,
    system_memory - 8GB
)
```

该值必须可配置。

例如：

```text
Mac RAM:

64 GB

AI Budget:

48 GB
```

---

# 24. Model Eviction

需要支持：

```text
LRU
```

基本策略：

当：

```text
requested_memory >
available_AI_memory
```

则：

```text
找出 idle model

按照：

last_used ascending

逐个 unload
```

直到满足加载条件。

---

# 25. Keep Alive

每个模型：

```text
keep_alive
```

例如：

```text
5m
30m
always
0
```

语义：

```text
0
→ request 完成即可卸载

5m
→ idle 5 分钟后卸载

always
→ 不自动卸载
```

---

# 26. Model Scheduler

Scheduler 第一版保持简单。

单模型请求：

```text
request
   ↓
model loaded?
   ├ yes
   │
   └ no
      ↓
memory check
      ↓
eviction
      ↓
load
      ↓
inference
```

暂不实现复杂：

```text
continuous batching
multi-GPU scheduler
distributed scheduling
```

这些方向需要真实负载证明有必要。

---

# 27. Backend Selection

Provider 设置：

```text
auto
mlx
llama.cpp
```

auto 初期按照格式：

```text
MLX weights
→ MLX

GGUF
→ llama.cpp
```

未来可基于 benchmark 数据选择。

不要在第一阶段设计复杂自动 benchmark scheduler。

---

# 28. Benchmark Framework

项目必须包含：

```text
benchmarks/
```

目标是支持 Backend 技术选择。

## LLM

测量：

```text
Model loading time

TTFT

Prompt processing speed

Generation tokens/s

Peak memory

Energy impact
```

比较：

```text
MLX
vs
llama.cpp
```

使用相同模型尺寸和量化等级。

---

# 29. STT Benchmark

指标：

```text
RTF

cold start latency

warm latency

peak memory
```

准确率测试可使用固定小型语音样本。

第一阶段主要用于：

```text
whisper.cpp
vs
MLX implementation
```

的工程选择。

---

# 30. TTS Benchmark

指标：

```text
TTFA
Time to first audio

RTF

Peak memory
```

第一阶段重点验证实时性。

---

# 31. Experimental Principle

只执行能够改变工程决策的实验。

例如：

## Experiment A

假设：

```text
MLX 比 llama.cpp 在 Apple Silicon 上有明显优势。
```

实验：

```text
同一模型
同一量化等级
同一机器

比较：

TTFT
TPS
Memory
Energy
```

决策：

```text
如果 MLX 综合表现更优
→ 默认 provider = MLX

否则
→ 保留 llama.cpp 为默认
```

---

## Experiment B

假设：

```text
Python persistent worker 的 IPC overhead 可以忽略。
```

实验：

测量：

```text
IPC round trip
vs
model inference latency
```

如果：

```text
IPC overhead < 1%
```

停止优化。

不要为了消除 Python 而 rewrite backend。

---

# 32. Python Worker Architecture

例如：

```text
providers/mlx-audio/
```

运行：

```text
python worker.py \
    --socket /tmp/aiworkd-mlx-audio.sock
```

Daemon：

```text
spawn worker

wait health

send:
load

send:
infer

send:
unload
```

Worker 崩溃：

```text
detect
restart
restore state if necessary
```

---

# 33. Worker Protocol

请求：

```json
{
  "id": "001",
  "method": "transcribe",
  "params": {
    "model": "parakeet",
    "file": "/tmp/input.wav"
  }
}
```

响应：

```json
{
  "id": "001",
  "result": {
    "text": "Hello world"
  }
}
```

错误：

```json
{
  "id": "001",
  "error": {
    "code": "MODEL_LOAD_FAILED",
    "message": "..."
  }
}
```

---

# 34. Streaming Protocol

LLM streaming：

```text
Provider
 ↓
Rust stream
 ↓
SSE
 ↓
Client
```

STT realtime：

后续：

```text
WebSocket
```

TTS realtime：

后续：

```text
WebSocket / chunked PCM
```

---

# 35. macOS Application

SwiftUI App 第一阶段包含：

```text
Sidebar

Chat
Models
Speech
Runtime
Settings
```

---

# 36. Models View

展示：

```text
model name

type

format

provider

disk size

RAM estimate

status

last used
```

支持：

```text
Load
Unload
Delete
Open folder
```

---

# 37. Runtime View

显示：

```text
System RAM

AI Runtime RAM

Loaded Models

CPU

GPU / Metal activity

Active requests
```

不要求第一阶段实现复杂 GPU telemetry。

---

# 38. Speech View

提供：

```text
Microphone input

Record

Transcribe

Model selection

Language

Copy result
```

TTS：

```text
Text input

Voice

Model

Generate

Play

Save
```

---

# 39. Menu Bar

提供：

```text
Runtime Running

Loaded Models

Start / Stop API

Open App

Quit
```

后续增加：

```text
global speech-to-text shortcut
```

---

# 40. Process Architecture

建议：

```text
AIWorkbench.app
       │
       │ IPC / HTTP
       ▼
    aiworkd
       │
       ├── provider process
       ├── provider process
       └── provider process
```

关闭 GUI 后：

```text
aiworkd
```

仍然可以运行。

---

# 41. Daemon Lifecycle

GUI 启动：

```text
check aiworkd

if absent:
    launch
```

CLI：

```text
check aiworkd

if absent:
    optionally launch
```

Daemon 支持：

```text
launchd
```

后续可提供：

```text
Start AI Workbench at login
```

---

# 42. Configuration

路径：

```text
~/Library/Application Support/AIWorkbench/config.toml
```

示例：

```toml
[server]
host = "127.0.0.1"
port = 11435

[memory]
budget_ratio = 0.75
reserve_bytes = 8589934592

[models]
keep_alive = "5m"

[providers]
mlx = true
llama_cpp = true
whisper_cpp = true
mlx_audio = true
```

---

# 43. Security

默认：

```text
bind 127.0.0.1
```

不得默认：

```text
0.0.0.0
```

如果用户启用 LAN API：

必须：

```text
token authentication
```

并明确提示。

---

# 44. Logging

使用：

```text
tracing
tracing-subscriber
```

日志：

```text
~/Library/Logs/AIWorkbench/
```

结构：

```text
daemon.log
providers.log
```

每个 request 必须具备：

```text
request_id
```

---

# 45. Error Model

统一错误：

```rust
enum AIError {
    ModelNotFound,
    ModelLoadFailed,
    ProviderUnavailable,
    OutOfMemory,
    InvalidRequest,
    DownloadFailed,
    BackendCrashed,
    Internal,
}
```

API 返回：

```json
{
  "error": {
    "type": "model_load_failed",
    "message": "..."
  }
}
```

---

# 46. Observability

第一版本统计：

```text
request count

request latency

model loading latency

inference latency

TTFT

tokens generated

memory usage
```

这些数据主要用于本地调试。

无需建立复杂 telemetry infrastructure。

---

# 47. Testing Strategy

## Unit tests

覆盖：

```text
Model Registry

Manifest parsing

Memory scheduler

LRU eviction

API serialization

Provider selection
```

---

## Integration Tests

模拟 Provider：

```text
MockProvider
```

测试：

```text
load model

chat

stream response

unload

OOM eviction
```

---

## Provider Smoke Tests

每个真实 provider：

```text
load tiny model

one inference

unload
```

不在普通 CI 下载大型模型。

---

# 48. Development Phases

## Phase 0 — Skeleton

目标：

```text
Rust workspace

aiworkd

ai CLI

health API
```

完成标准：

```bash
ai status
```

可以得到：

```text
AI Runtime running
```

---

# 49. Phase 1 — llama.cpp LLM

实现：

```text
llama.cpp provider

model load

chat

stream

OpenAI API
```

完成标准：

```bash
ai run model.gguf
```

可聊天。

同时：

```python
OpenAI(base_url=...)
```

可以调用。

---

# 50. Phase 2 — Model Registry

实现：

```text
SQLite

model manifest

ai list

ai pull

model folder
```

完成标准：

```bash
ai pull MODEL
ai list
ai run MODEL
```

完整工作。

---

# 51. Phase 3 — MLX LLM

加入：

```text
MLX-LM provider
```

完成 benchmark：

```text
MLX
vs
llama.cpp
```

根据结果设置默认策略。

---

# 52. Phase 4 — STT

实现：

```text
whisper.cpp
```

API：

```text
/v1/audio/transcriptions
```

CLI：

```bash
ai transcribe
```

---

# 53. Phase 5 — MLX Audio

加入：

```text
MLX Audio worker
```

支持：

```text
STT
TTS
```

API：

```text
/v1/audio/transcriptions

/v1/audio/speech
```

---

# 54. Phase 6 — Memory Scheduler

实现：

```text
memory budget

model memory estimate

LRU

keep_alive

automatic unload
```

---

# 55. Phase 7 — macOS GUI

实现：

```text
Chat

Models

Speech

Runtime

Settings
```

GUI 只调用 daemon。

---

# 56. Phase 8 — Realtime Audio

加入：

```text
VAD

streaming STT

streaming TTS
```

候选 Backend：

```text
sherpa-onnx
MLX-Audio
```

根据实验结果选择。

---

# 57. Future Capabilities

架构应允许：

```text
VLM

Embedding

Reranker

Image generation

Video generation

Agent Runtime

MCP

Realtime conversation
```

通过：

```text
new Provider
+
new Capability
```

加入。

---

# 58. Explicit Non-Goals for V1

第一阶段不实现：

```text
Windows

Linux

Intel Mac

distributed inference

multiple Mac cluster

training

fine-tuning

multi-user authentication

cloud synchronization

complex agent platform

plugin marketplace

workflow editor
```

这些能力暂时不会改变核心 Runtime 架构。

---

# 59. Important Engineering Constraints

## Constraint 1

GUI 不拥有模型。

```text
GUI
→ daemon
→ model
```

---

## Constraint 2

CLI 不拥有模型。

```text
CLI
→ daemon
→ model
```

---

## Constraint 3

Python Backend 必须常驻。

禁止：

```text
one request
→ one process
```

---

## Constraint 4

Backend failure 不得导致：

```text
aiworkd
```

崩溃。

---

## Constraint 5

所有 Provider 必须支持：

```text
load
health
unload
```

---

# 60. Performance Priorities

优先优化：

```text
model load

prompt processing

TTFT

tokens/s

RTF

Metal utilization

memory usage

cache reuse
```

低优先级：

```text
JSON parsing

Unix socket overhead

CLI startup
```

除非 profiling 显示其成为瓶颈。

---

# 61. UX Principle

目标体验：

```bash
ai pull qwen3
```

然后：

```bash
ai run qwen3
```

用户无需理解：

```text
MLX

GGUF

Metal

Python

ONNX

quantization backend
```

高级用户可以手动设置：

```bash
ai run qwen3 --provider mlx
```

---

# 62. Recommended First Development Task

第一项开发任务只实现：

```text
Rust workspace

aiworkd

ai CLI

provider abstraction

MockProvider

OpenAI chat API
```

流程：

```text
CLI
   ↓
daemon
   ↓
MockProvider

HTTP
   ↓
daemon
   ↓
MockProvider
```

验收：

```bash
ai chat mock hello
```

输出：

```text
hello
```

同时：

```bash
curl \
http://127.0.0.1:11435/v1/chat/completions \
-H "Content-Type: application/json" \
-d '{
  "model":"mock",
  "messages":[
    {
      "role":"user",
      "content":"hello"
    }
  ]
}'
```

成功返回 OpenAI-compatible response。

该阶段通过以后，再接 llama.cpp。

---

# 63. First Milestone Acceptance Criteria

Milestone 1 完成时，必须满足：

* `aiworkd` 可以独立运行
* `ai` 可以发现 daemon
* Provider interface 已建立
* Mock Provider 可运行
* llama.cpp Provider 可运行
* 模型可以 load / unload
* `/v1/models` 可用
* `/v1/chat/completions` 可用
* SSE streaming 可用
* CLI Chat 可用
* Provider 崩溃不会导致 daemon 崩溃
* 基础 logging 可用

无需：

* GUI
* STT
* TTS
* Hugging Face download
* MLX

Milestone 1 的作用是验证：

```text
Runtime architecture
```

---

# 64. Second Milestone Acceptance Criteria

Milestone 2：

* Model Registry
* Hugging Face download
* `ai pull`
* `ai list`
* MLX Provider
* LLM benchmark
* Memory estimate
* keep_alive
* LRU eviction

此时形成完整：

```text
Local LLM Runtime
```

---

# 65. Third Milestone Acceptance Criteria

Milestone 3：

* whisper.cpp
* MLX-Audio
* STT API
* TTS API
* CLI STT
* CLI TTS

此时项目达到：

```text
Local AI Runtime
```

而不再局限于 Local LLM Runtime。

---

# 66. Definition of Done

项目第一版完成时：

用户可以：

```bash
brew install aiworkbench
```

或者安装：

```text
AIWorkbench.app
```

然后：

```bash
ai pull qwen3

ai run qwen3
```

可以：

```text
Chat
```

运行：

```bash
ai transcribe audio.wav
```

可以：

```text
STT
```

运行：

```bash
ai speak "Hello"
```

可以：

```text
TTS
```

第三方应用使用：

```text
http://localhost:11435/v1
```

即可访问。

GUI 可以管理相同的所有模型和 runtime 状态。

---

# 67. Core Architectural Decision Summary

最终架构：

```text
                     macOS
                       │
          ┌────────────┼────────────┐
          │            │            │
       SwiftUI        CLI         HTTP
          │            │            │
          └────────────┼────────────┘
                       │
                       ▼
                    aiworkd
                       │
           ┌───────────┼───────────┐
           │           │           │
           ▼           ▼           ▼
        LLM          STT          TTS
           │           │           │
      ┌────┴────┐ ┌────┴────┐ ┌────┴────┐
      │         │ │         │ │         │
     MLX   llama.cpp MLX whisper MLX   sherpa
```

核心技术选择：

```text
GUI          SwiftUI

Daemon       Rust

CLI          Rust

API          Axum

Registry     SQLite

LLM          MLX + llama.cpp

STT          whisper.cpp + MLX-Audio

TTS          MLX-Audio

Realtime     sherpa-onnx / MLX-Audio

Model Store  Hugging Face
```

整个项目围绕：

```text
aiworkd
```

构建。

`aiworkd` 是唯一的 Runtime Authority。

所有 UI、CLI 和 API 都只是它的客户端。

---

# 68. Instruction for Coding Agents

开发时遵循以下顺序：

```text
1. Runtime correctness
2. Provider isolation
3. API compatibility
4. Model lifecycle
5. Streaming
6. Memory management
7. Performance
8. GUI
```

每个阶段都必须：

```text
build
test
run smoke test
```

之后才进入下一阶段。

涉及 Backend 技术选型时，通过最小 benchmark 回答具体假设。

当额外实验已经无法改变：

```text
provider choice
architecture
performance strategy
```

时停止实验，继续实现。

优先交付可以实际运行的端到端路径。
