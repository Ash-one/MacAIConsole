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
macai pull qwen3
macai run qwen3

macai transcribe meeting.wav

macai speak "Hello world"

macai list
macai ps
macai stop qwen3
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
        ┌──────────┬──────────┬──────────┬──────────┐
        │          │          │          │          │
        ▼          ▼          ▼          ▼          ▼
   llama.cpp    MLX-LM    whisper.cpp  MLX-ASR   MLX-Audio
        │          │          │          │          │
        ▼          ▼          ▼          ▼          ▼
       LLM        LLM        STT        STT         TTS
```

Realtime / extra Backend：

```text
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
macai
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
│   ├── mlx-asr/
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
enum IsolationMode {
    InProcess,
    Worker,
}

struct ProviderDescriptor {
    id: String,
    capabilities: Vec<Capability>,
    isolation: IsolationMode,
    supported_devices: Vec<String>,
}

struct ProviderStatus {
    available: bool,
    ready: bool,
    effective_device: Option<String>,
    resident_models: Vec<String>,
    reason: Option<String>,
    install_hint: Option<String>,
}

#[async_trait]
pub trait Provider {
    fn id(&self) -> &'static str;

    fn capabilities(&self) -> Vec<Capability>;

    fn descriptor(&self) -> ProviderDescriptor;

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

    async fn status(
        &self,
    ) -> Result<ProviderStatus>;
}
```

Provider 元数据必须描述真实能力，而不是 UI 展示意图。至少包括：

```text
capabilities
supported_devices
isolation mode
availability + reason
effective device
install hint
resident models
```

`available`、`ready`、`resident` 是不同状态：已安装不等于可用，可用不等于已就绪，已就绪不等于模型常驻。

Provider 列举必须 best-effort：一个 Provider 探测失败时，将该项报告为 unavailable 并给出原因，不得让整个 `/api/providers` 返回 500。

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

MLX-ASR
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
  ├── mlx-asr worker
  │
  ├── mlx-audio worker
  │
  └── whisper worker
```

Python Backend 采用 persistent worker。第三方原生库、容易崩溃的 C/C++ binding、依赖冲突明显的引擎也优先使用隔离 worker。

进程边界按故障域划分：

```text
稳定且可控的 Rust/C API
→ 可在 aiworkd 内运行

Python / 第三方 native runtime / 易崩溃 backend
→ 独立 persistent worker
```

默认按 Provider 复用 worker 与环境；只有依赖确实冲突时，才为该 Provider 使用独立 venv。不要为每个模型复制一套 Python 环境。

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

worker 应支持惰性启动、模型常驻、空闲回收和幂等 shutdown。worker 崩溃只影响其承载的 Provider；`aiworkd` 保持存活并将请求标记为 `backend_crashed`。

这里吸收 VoiceStudio 的 sidecar 故障隔离经验，但不采用它的 FastAPI/PyTorch 单体作为 Runtime Authority。

---

# 9. Inter-Process Communication

第一版本优先使用 Unix Domain Socket 上的 length-prefixed JSON。

```text
4-byte big-endian length
JSON body
```

协议必须具备：

```text
protocol_version
request_id
method / event
deadline_ms
bounded frame size
operation allowlist
```

控制帧保持小型；音频和大型模型数据通过受控临时文件、memory map 或后续共享内存传递。单帧设置硬上限（初始 64 MiB），在分配 buffer 前校验长度，防止损坏 worker 触发超大内存分配。

第一版允许的方法固定为：

```text
hello
capabilities
health
load
unload
infer
cancel
shutdown
```

worker 可主动发送：

```text
ready
progress
heartbeat
result
error
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

IPC 应先保证边界、取消、超时和错误语义；只有 profiling 证明 JSON framing 成为瓶颈时，才迁移 MessagePack / Protobuf。

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
MLX-native ASR worker
    ├── mlx-whisper
    └── parakeet-mlx
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

### MLX-native ASR

适合：

```text
Apple Silicon
Whisper → mlx-whisper
Parakeet → parakeet-mlx
```

MLX ASR 与 MLX TTS 使用不同 Provider capability；允许共用基础 MLX 环境，但不能假定 `mlx-audio` 包负责所有 ASR 模型。

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
macai list
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
macai pull qwen3
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
macai run qwen3
```

进入：

```text
interactive chat
```

---

## Chat

```bash
macai chat qwen3 "Explain transformers"
```

---

## STT

```bash
macai transcribe audio.wav
```

可选：

```bash
macai transcribe audio.wav \
    --model whisper-large-v3 \
    --language zh
```

---

## TTS

```bash
macai speak "Hello world"
```

或者：

```bash
macai speak \
    "你好" \
    --model qwen3-tts \
    --output output.wav
```

---

## Runtime

```bash
macai ps
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
macai stop qwen3-8b
```

---

## Server

```bash
macai serve
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
effective device
resident / unloadable
active lease count
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

Runtime 必须提供单一的 loaded-model inventory。注册表中的模型与内存中真实常驻的模型分开表示；`loaded_models` 只能列出实际 resident 的模型。

每个常驻模型至少报告：

```text
model id
provider id
effective device
state
estimated memory
measured memory（可用时）
loaded_at
last_used_at
active lease count
unloadable
```

推理开始前获取 model lease，结束或客户端断开时释放。手动 unload、LRU eviction 和 idle reaper 必须跳过 lease count > 0 的模型，避免在请求中途释放权重。

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

Apple Silicon / MPS 第一版每个 accelerator lane 默认并发为 1。只有 benchmark 与稳定性测试证明同模型并发能提升吞吐且不会增加内存峰值时，才提高并发。

队列等待与实际执行使用两个独立时钟：

```text
queue timeout
→ 任务尚未开始，返回可重试的 busy / 503

execution timeout
→ 任务已经开始，返回 inference timeout
```

队列必须支持 position、cancel、queued/running 计数。长任务的进度 heartbeat 可以在有界范围内延长执行 deadline；heartbeat 不能无限续期。

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

auto 初期先按模型格式生成候选，再验证 Provider availability 与设备能力：

```text
MLX weights
→ MLX

GGUF
→ llama.cpp
```

选择结果必须显式记录：

```text
requested provider
selected provider
effective device
accelerated / cpu fallback / unavailable
reason
```

禁止静默 CPU fallback。某个 Provider 在当前 Mac 上只能走 CPU 时，API、CLI 与 GUI 必须展示原因；对于预计内存或时延明显不合理的大模型，可由策略直接拒绝并提示选择更轻模型。

显式指定 `--provider` 时，将其视为硬选择：不可用就失败，不再悄悄切换到其他 Provider。

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

`aiworkd` 自身先绑定端口并提供轻量 `/health`；Provider worker 与重型 ML import 在后台启动。对外区分：

```text
liveness  = daemon event loop 正常
readiness = 所需 registry / provider 已可服务
deep health = 至少一个真实轻量操作可完成
```

启动进度通过 `/api/startup` 或事件流暴露，GUI 不依赖固定 sleep 猜测模型导入时间。

Worker 崩溃：

```text
detect
restart
restore state if necessary
```

重启必须有次数上限和退避。只有幂等 load 可以自动恢复；正在执行的 inference 不得无条件重放，避免重复生成或重复写文件。

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

协议还必须定义：

```text
cancel acknowledgement
progress sequence
heartbeat lease
worker generation / restart id
```

收到未知 operation、超限 frame、错误 protocol version 或重复终态时，关闭该 worker 连接并记录结构化错误；不得执行自由形式命令或由客户端提供可执行路径。

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

OpenAI-compatible SSE 最后发送：

```text
data: [DONE]
```

长任务事件带单调递增 `seq`。进入 SQLite 持久化阶段后，客户端可用 `after_seq` 重连并回放有界事件尾部；实时分发保持内存内，磁盘写失败只降低可恢复性，不中断当前 stream。

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

GUI 连接已有 daemon 时必须同时检查：

```text
service identity
API version compatibility
deep health
```

端口被其他进程占用时只报告冲突，不得直接杀死未知进程。已有兼容 `aiworkd` 时附着；版本不兼容时提示用户停止旧 daemon 或执行受控升级。

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

日志使用 rotation，并为每次 daemon / worker run 记录独立 run id 或 byte boundary。崩溃诊断只读取本次 run 的 stderr 尾部，避免把下一次健康启动误归因到上一次崩溃。

诊断包可以包含版本、Provider 状态、routing decision、内存快照与脱敏日志；不得包含 prompt、音频、API token 或完整本地路径。

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

queue depth / running workers

selected provider / effective device

provider restart count

progress heartbeat age
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

Provider metadata truthfulness

no-silent-fallback routing

model lease vs unload race

queue timeout vs execution timeout

framed IPC size / op validation
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

worker crash and bounded restart

SSE reconnect from after_seq

daemon liveness vs readiness vs deep health
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

macai CLI

health API
```

完成标准：

```bash
macai status
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
macai run model.gguf
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

macai list

macai pull

model folder
```

完成标准：

```bash
macai pull MODEL
macai list
macai run MODEL
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
macai transcribe
```

---

# 53. Phase 5 — MLX Speech Workers

加入：

```text
MLX ASR worker
MLX Audio TTS worker
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

model leases

per-device job queue

queue / execution deadlines

job metadata + bounded event replay
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

初始职责：

```text
streaming STT → sherpa-onnx
streaming TTS → MLX-Audio
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

Remote GPU workers（V1 后）
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

## Constraint 6

Provider 必须诚实报告能力、可用性、设备与 fallback。UI 显示的状态必须来自 Runtime routing decision，不能维护第二套推断逻辑。

---

## Constraint 7

模型卸载必须经过 lease / busy guard。任何 idle reaper 或手动 unload 都不能中断正在运行的请求。

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
macai pull qwen3
```

然后：

```bash
macai run qwen3
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
macai run qwen3 --provider mlx
```

---

# 62. Recommended First Development Task

第一项开发任务只实现：

```text
Rust workspace

aiworkd

macai CLI

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
macai chat mock hello
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
* `macai` 可以发现 daemon
* Provider interface 已建立
* Provider descriptor / status 可查询
* Mock Provider 可运行
* llama.cpp Provider 可运行
* 模型可以 load / unload
* `/v1/models` 可用
* `/v1/chat/completions` 可用
* SSE streaming 可用
* CLI Chat 可用
* Provider 崩溃不会导致 daemon 崩溃
* worker IPC 能拒绝未知 operation 与超限 frame
* liveness、readiness、deep health 语义分离
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
* `macai pull`
* `macai list`
* MLX Provider
* LLM benchmark
* Memory estimate
* keep_alive
* LRU eviction
* model lease / busy guard
* no-silent-fallback routing
* queue wait 与 execution deadline 分离

此时形成完整：

```text
Local LLM Runtime
```

---

# 65. Third Milestone Acceptance Criteria

Milestone 3：

* whisper.cpp
* MLX-native ASR（mlx-whisper / parakeet-mlx）
* MLX-Audio TTS
* STT API
* TTS API
* CLI STT
* CLI TTS
* long-running job metadata 与有界事件回放

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
macai pull qwen3

macai run qwen3
```

可以：

```text
Chat
```

运行：

```bash
macai transcribe audio.wav
```

可以：

```text
STT
```

运行：

```bash
macai speak "Hello"
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
     MLX   llama.cpp MLX-ASR whisper.cpp MLX-Audio sherpa
```

核心技术选择：

```text
GUI          SwiftUI

Daemon       Rust

CLI          Rust

API          Axum

Registry     SQLite

LLM          MLX + llama.cpp

STT          whisper.cpp + MLX-native ASR

TTS          MLX-Audio

Realtime     sherpa-onnx / MLX-Audio

Model Store  Hugging Face

Provider IPC Unix Domain Socket + framed JSON

Routing      capability + availability + effective device

Job Model    per-device queue + cancellation + bounded event replay
```

整个项目围绕：

```text
aiworkd
```

构建。

`aiworkd` 是唯一的 Runtime Authority。

所有 UI、CLI 和 API 都只是它的客户端。

---

# 68. VoiceStudio Reference Decisions

参考项目：

```text
https://github.com/debpalash/VoiceStudio
inspected commit: afa361913cbfd2549421b15e54a2550592b46e58
license: AGPL-3.0-only
```

VoiceStudio 证明了「桌面 UI 只是本地 Runtime 客户端」「多引擎必须有统一能力矩阵」「高风险推理 backend 应使用常驻隔离进程」这些方向有效。

本项目只迁移架构规律与接口思想，不直接复制 VoiceStudio 的 AGPL 实现代码。

## 保留当前路线

| 决策 | 原因 |
|---|---|
| SwiftUI 而非 Tauri/React | 第一阶段只支持 Apple Silicon；原生菜单栏、全局快捷键、AVFoundation 与系统权限链路更直接 |
| Rust `aiworkd` 而非 Python FastAPI 作为 Runtime Authority | daemon 生命周期、内存调度、协议边界与 CLI 可保持轻量稳定；Python 只承载需要 Python 生态的 Provider |
| Axum + OpenAI-compatible API | 当前原型已验证 chat 与 SSE；后续扩展 audio endpoints 即可 |
| 先少量 Provider 再扩展 | VoiceStudio 的大量引擎带来显著依赖、安装、兼容与测试成本；MacAI 先验证最优 Apple Silicon 路径 |

## 吸收的机制

| VoiceStudio 中的有效机制 | MacAI 中的落点 |
|---|---|
| 引擎能力、可用性、设备与隔离元数据 | `ProviderDescriptor` / `ProviderStatus` |
| no-silent-fallback routing | Backend Selection 与统一 routing decision |
| 长驻 sidecar + crash isolation | Python / 第三方 native persistent worker |
| lazy load、idle unload、loaded-model inventory | Runtime model lease、LRU 与 idle reaper |
| early bind + startup progress + deep health | `aiworkd` liveness/readiness/deep-health API |
| serial GPU queue、取消、queue position | Apple Silicon accelerator lane，初始 concurrency=1 |
| queue wait 与 execution timeout 分离 | Scheduler 两类 deadline 与可重试 busy 错误 |
| job metadata + SSE event tail | SQLite jobs/events 与 `after_seq` 重连 |
| per-run crash evidence 与日志脱敏 | daemon/worker run id、rotation、诊断包 |

## V1 继续排除

```text
Tauri / React frontend
Python monolithic backend
remote GPU workers
distributed scheduling
plugin marketplace
大规模引擎矩阵
```

远程 worker 的 capability negotiation、warm-model affinity、TLS enrollment 与 circuit breaker 只作为未来设计参考；V1 不实现网络任务调度。

关键证据（固定到 inspected commit）：

* [TTS backend contract and registry](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/services/tts_backend.py)
* [ASR backend contract and registry](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/services/asr_backend.py)
* [No-silent-fallback routing](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/services/engine_routing.py)
* [Persistent subprocess backend](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/services/subprocess_backend.py)
* [Loaded-model inventory and unload](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/services/model_lifecycle.py)
* [GPU queue and execution guards](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/services/model_manager.py)
* [Resource-gating job queue](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/core/job_queue.py)
* [Job metadata and SSE event persistence](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/core/job_store.py)
* [Early-bind backend startup](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/backend/main.py)
* [Desktop backend supervision](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/frontend/src-tauri/src/backend.rs)
* [Remote-worker design](https://github.com/debpalash/VoiceStudio/blob/afa361913cbfd2549421b15e54a2550592b46e58/docs/remote-workers.md)

---

# 69. Instruction for Coding Agents

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

---

# 70. Decision: STT 音频格式在 Daemon 入口归一化

## 问题

最初接入 STT 时（`666d01e`）端点只接收扩展名为 `.wav` 的 PCM WAV 上传。
用户持有 mp3 / m4a / flac / ogg 等常见格式时必须先手动转码。该限制并非推理
引擎的真实能力边界（pinned 版 whisper.cpp 的 miniaudio 本身支持
wav/mp3/flac/ogg；mlx-audio 自带 miniaudio 解码），而是实现期约束，且与
§15 承诺的 OpenAI `/v1/audio/transcriptions` 契约（接受
flac/m4a/mp3/mp4/oga/ogg/wav/webm）相矛盾。当时 `.wav` 校验散布在 CLI、
daemon 端点与两个 provider 共四处，格式裁决没有唯一 owner。

## 决策

`/v1/audio/transcriptions` 在 daemon 入口把上传音频完整解码并重写为规范的
16-bit PCM WAV（保原始采样率与声道数，不重采样、不下混），再交给 STT
provider（`crates/ai-daemon/src/audio.rs`，symphonia 纯 Rust 解码）。CLI
删除客户端 `.wav` 校验，multipart 透传真实文件名；provider 层的扩展名
校验一并删除。格式裁决的 owner 只有 daemon 入口一处；provider 契约
始终保持"只接收 PCM WAV"。

支持的容器/编解码：wav、mp3、flac、ogg（Vorbis）、m4a（AAC-LC）、ALAC，
与 `ai-daemon` 的 symphonia features 一一对应。探测与解码以文件内容为准，
扩展名只作容器探测提示，改名伪装的文件可正确解码。

## 被放弃的方案

* 外部 ffmpeg 转码：覆盖 opus/HE-AAC/WebM，但为本地单用户应用引入系统级
  二进制依赖与版本漂移，违背低依赖原则。
* 逐 provider 放宽校验、各用已有解码器：零依赖，但三条 provider 路径的
  格式集合不一致（m4a 仅部分路径可用），API 无法给出单一格式承诺；
  whisper.cpp 需重编译才支持 ffmpeg 回退格式。

## 后果与已知边界

* opus、HE-AAC、WebM 音轨不受支持（symphonia 未实现），返回明确的
  `InvalidRequest` 并列出支持格式；未来若确有需求，可在入口归一化之上
  增补"系统存在 ffmpeg 时额外支持"，无需改动 provider。
* 解码发生在 daemon 进程内（blocking 线程池），symphonia 为安全 Rust；
  上传体量与解码时长对内存的压力与既有 wav 路径同级。
* 非 WAV 压缩格式的时长指标来自归一化后的 WAV，对 mp3/m4a 含编码器帧
  填充（±1~2 帧）；RTF 指标因此对所有受支持格式可用。
* 临时文件固定写 `.wav` 现在与内容一致；`file_name` 任务详情仍记录用户
  上传的原始文件名。

## 验证

* 单元：`cargo test -p ai-daemon audio::` 覆盖规范 PCM WAV 往返、时长
  指标、垃圾/截断输入拒绝，以及 mp3/flac/m4a/ogg 真实编码 fixtures 的
  完整探测+解码路径（fixtures 由 `scripts/tests/generate_audio_fixtures.py`
  生成并随仓库提交）。
* 端到端：Kokoro TTS 生成语音 → 转码 mp3/m4a/flac/ogg → `POST
  /v1/audio/transcriptions`（whisper.cpp provider）五种格式转写文本一致，
  任务详情 `audio_duration_ms` 全部有值；垃圾文件返回 400
  `invalid_request` 而非 `backend_crashed`；`macai transcribe` 对 m4a
  成功、对垃圾文件转发 daemon 的 400。
