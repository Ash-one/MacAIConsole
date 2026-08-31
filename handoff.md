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

Realtime 是 `aiworkd` 内的协议与会话编排层，不预设为某一个额外 Backend。
第一版复用现有 STT / LLM / TTS Provider；sherpa-onnx、Parakeet 等候选只有在
benchmark 证明能改变延迟、准确率或功耗决策时才加入。

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

```text
WS input audio
  → VAD
  → 当前 turn 的累计音频
  → 既有 STT Provider
  → transcription.delta / transcription.completed
```

第一版允许用“累计音频重新转写”生成 progressive transcript：同一 session 最多
一个 progressive 请求在途，final 请求独立提交，迟到或旧 revision 的 partial
必须丢弃。最终文本是 authoritative，客户端用 final 替换对应 item 的 partial。
真正的 streaming decoder 只有在 benchmark 证明累计重传成为瓶颈后再引入。

TTS realtime：

```text
TTS worker PCM chunk
  → daemon bounded channel
  → response.output_audio.delta
  → client playback queue
```

Realtime TTS 的流式边界必须从模型/worker 的首个可播放 PCM chunk 开始；把已经
完整生成的 WAV 拆成 HTTP/WebSocket 小块不算 streaming。现有
`POST /v1/audio/speech` 完整文件响应继续保留，Realtime 使用独立的 chunk path。
详细会话、取消和兼容性决策见 §73。

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

目标：在不改变 `aiworkd` Runtime Authority 的前提下，从无状态的 STT / Chat /
TTS 请求扩展到有状态的实时语音会话。

按以下顺序加入：

```text
0. 用 Hugging Face speech-to-speech sidecar 调用现有三个 OpenAI-compatible endpoint 做 PoC
1. WS /v1/realtime 的窄 OpenAI Realtime 事件集
2. session state + VAD + speech_started / speech_stopped
3. progressive STT（初期可累计音频重传）
4. turn revision + cancellation generation + stale output suppression
5. worker 原生 PCM chunk → streaming TTS + playback buffering
6. 可选 Smart Turn semantic endpointing
7. Qwen3-TTS-MLX Provider、tool calling 与 WebRTC（按需求后置）
```

第一版 Realtime 复用当前已验证的 Qwen3-ASR / whisper.cpp、MLX-LM /
llama.cpp 与 Kokoro-MLX Provider，不预先绑定新的推理引擎。sherpa-onnx、Parakeet
或其他 streaming backend 只在最小 benchmark 证明它能改变延迟或准确率决策时加入。

内部音频基线：

```text
mono PCM16
pipeline sample rate 16 kHz
512 samples per internal audio chunk
bounded queues with explicit backpressure
```

协议、turn 状态、打断传播、sidecar 边界与后置项见 §73。

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

Realtime     OpenAI Realtime WS + VAD + turn revision / cancellation

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

---

# 71. Decision: 测试套件精简与 keep_alive 解析 owner 唯一化

## 问题

测试随功能增长积累了若干零防护力或重复固化的条目，维护成本为正、回归防护为零：

* ai-core 的 `ModelSpec::keep_alive_secs` 与 `parse_duration` 没有任何生产
  消费方（daemon 实际使用 scheduler 的 `parse_keep_alive`），两个单元测试在
  保护死代码；仓库同时存在两套 keep_alive 解析器，语义 owner 不唯一。
* scheduler 的内存预算测试在测试内用与生产代码相同的公式重算期望值，从不
  调用 `memory_budget()`，改坏生产公式时测试照常通过（自证）。
* macos-say 的测试断言宿主机存在 `say`，是环境断言而非逻辑断言，CI runner
  变化会误报。
* 同一契约被双份固化：kokoro 音色清单在 DaemonAPIRequestTests 与
  RecommendationPullTests 各固化一份；`wav_duration_ms` 的测试写在 HTTP 层
  模块（daemon main.rs）且与 audio.rs 的覆盖重复。
* 若干测试镜像声明或系统框架：CLI help 文本包含性检查与 chat 参数解析镜像
  clap derive 声明（`debug_assert` 已覆盖结构有效性）；UserDefaults 写入
  读出往返测的是系统框架；Runtime 任务计数测试只覆盖两个薄委托；
  qwen3-asr feature flag 的表格测试镜像 `matches!` 字面量——flag 缺省开启
  的行为已由 `exposes_all_provider_descriptors`（注册全量集合断言）端到端兜住。

## 决策

删除上述死代码与测试。keep_alive 字符串解析的唯一 owner 是
`ai-daemon::scheduler::parse_keep_alive`；ai-core 不再暴露
`keep_alive_secs` / `parse_duration`。kokoro 音色清单与下载体积口径的唯一
固化点是 DaemonAPIRequestTests 的 pull payload 测试（文件数 105 + 抽样 +
`estimatedSizeBytes`）；`wav_duration_ms` 的垃圾输入分支并入 audio.rs 的
时长指标测试。保留 CLI 的 `clap_definition_is_valid`、`--help` 退出行为、
remove/rename 的 HTTP 契约测试，以及 AppSettings 的缺省值契约测试。

## 被放弃的方案

* 为内存预算公式提取纯函数 `budget_for(total)` 再测：会引入无生产消费方的
  测试专用 API，与删除 `keep_alive_secs` 的理由自相矛盾；公式本身已由
  handoff §23 与 scheduler 模块文档承载。
* 在 RecommendationPullTests 保留具名/编号音色全集合枚举：文件数（105）
  加抽样断言已是充分的 tripwire，全集合枚举与 count 断言重复。

## 后果与已知边界

* keep_alive 语义（单位后缀、从宽处理、缺省 always）的回归防护完全依赖
  `parse_keep_alive` 的现有测试；ai-core 层不再有独立解析路径。
* macos-say 在宿主机缺 `say` 时的行为不再有测试观察点，由 `status()` 的
  `available=false` 运行时路径兜底。
* 音色清单增删文件时只需同步 pull payload 测试的 count 断言，单一修改点。
* CLI help 文本与 chat 参数声明不再有测试观察点；结构有效性由
  `debug_assert` 兜底。

## 验证

* `cargo fmt --all -- --check` 通过。
* `cargo test --workspace` 通过：Rust 测试 83 → 74（macai 6→4、ai_core
  10→8、aiworkd 67→62）。
* `swift test --enable-xctest`（apps/MacAIConsole）通过：26 → 24。
* 负向搜索：`keep_alive_secs`、`parse_duration` 及全部被删测试名在
  crates/apps/scripts 零命中；`qwen3_asr_enabled_value` 与
  `MacOSSayProvider::available()` 的生产引用保持不变。

---

# 72. Decision: 设置合并进主窗口并裁剪冗余设置项

## 问题

设置以独立 `Settings` scene（Cmd+, 弹窗）存在，与主窗口四大页面割裂：
修改内存预算要从运行状态页弹到另一个窗口。同时设置表单混入了三项不
 earn 维护成本的入口：aiworkd 路径选择器与自动探测（`AIWORKD_PATH` env
→ `target/release` → `target/debug`）构成两套指定机制；「打开日志文件夹」
与日志页「在访达中显示」重复；「关于 · 版本 0.1.0」是永不更新的硬编码
假版本（daemon 真实版本已展示在运行状态页与菜单栏）。

## 决策

设置的主 owner 是主窗口侧边栏「设置」页（`RootView` 的 `Page.settings`，
经 `AppRouter` 共享导航状态）；独立 `Settings` scene、菜单栏「设置…」
入口（`SettingsLink`）删除，Cmd+, 快捷键保留在主窗口内。运行状态页
内存预算卡片的「修改」经 `AppRouter` 跳转设置页。

逐项裁决：

* 移除「aiworkd 可执行文件路径」选择器：`DaemonController.resolveBinary`
  的探测链（env → release → debug）是路径指定的唯一 owner；需要自定义
  二进制时用 `AIWORKD_PATH`。旧版本存入 UserDefaults 的路径偏好不再
  被读取。保留「自动探测结果」展示作为探测链的可见性窗口。
* 移除「打开日志文件夹」：日志页是日志目录入口的 owner。
* 移除「关于 · 版本」：daemon 版本由运行状态页与菜单栏从 `/api/runtime`
  展示；保留「API 地址」（应用内唯一的本地 endpoint 展示位）。
* 保留自动拉起开关（重启 GUI 时不强行复活已手动停止的 daemon）、内存
  预算（§23 要求可配置）、Qwen3-ASR 0.6B 开关（重型 Provider 的显式
  opt-in；daemon 端缺省开启，GUI 传 0 是唯一关闭途径）、网络代理三模式
  （模型下载前置条件，契约已由 DaemonControllerConnectionTests 固化）。

## 被放弃的方案

* 保留独立 Settings scene + 侧边栏页双入口：同一表单两个 surface，无
  独有能力，制造展示不一致（520pt 窗口 vs 全宽页面）。
* 菜单栏保留「设置…」并做跨 scene 导航：需要把路由状态穿透进
  MenuBarExtra，收益只是省一次点击；「打开主窗口」已可达。

## 后果与已知边界

* 曾在旧设置窗口指定过自定义 aiworkd 路径的安装：偏好被静默忽略，
  需改用 `AIWORKD_PATH`（`launchctl setenv` 或终端启动）。
* `aiworkdPath` 键残留在个别用户的 UserDefaults 中，应用不再读写；
  不做迁移。
* 设置无独立窗口后，菜单栏小窗不能直达设置，需先打开主窗口。

## 验证

* `swift build` 通过；`swift test --enable-xctest` 通过（24 个测试，
  数量与 §71 记录一致；AppSettingsTests 固化的 qwen3 缺省、代理缺省与
  归一化契约不受影响）。
* 负向搜索：`aiworkdPath`、`aiworkdPathKey`、`SettingsLink`、
  `openSettings`、「打开日志文件夹」在 apps/MacAIConsole 源码零命中。
* 运行 MacAIConsole.app：侧边栏出现「设置」页，Cmd+, 聚焦设置页，
  运行状态页内存预算「修改」跳转设置页。

---

# 73. Decision: Realtime Audio 参考 Hugging Face speech-to-speech 的协议与状态机

参考项目：

```text
https://github.com/huggingface/speech-to-speech
inspected commit: 3986f453012a131632eee4731995474046846794
inspected date: 2026-08-28
license: Apache-2.0
```

Smart Turn 参考：

```text
https://github.com/pipecat-ai/smart-turn
inspected commit: 4786657e242dfe77dd138699ac564ee074a2a543
inspected date: 2026-01-29
model: pipecat-ai/smart-turn-v3 (v3.2)
license: BSD-2-Clause
```

## 问题

MacAI 当前提供 `/v1/audio/transcriptions`、`/v1/chat/completions` 与
`/v1/audio/speech`，已经具备完整的单次 STT → LLM → TTS 推理能力；API 仍是
无状态请求，TTS 以完整 `Vec<u8>` 返回，缺少实时语音会话所需的：

* session / conversation state；
* 连续音频输入与 VAD 事件；
* progressive transcript 与 authoritative final transcript；
* 用户说话打断、客户端取消和跨 STT/LLM/TTS 的取消传播；
* turn reopen 后丢弃旧 STT、LLM、TTS 与待播放音频；
* 从 TTS worker 首个 PCM chunk 到客户端播放的真实流式路径。

原 §34、§56 只写了“WebSocket / streaming STT / streaming TTS”，没有定义协议
事件、turn identity、取消语义和 stale output 边界。直接逐 endpoint 增加流式参数
会形成多套不一致的会话状态。

Hugging Face `speech-to-speech` 已实现并用 stock OpenAI Agents SDK 验证了一组窄
OpenAI Realtime WebSocket/WebRTC 协议，包含 VAD、转写事件、barge-in、取消、
tool result 和音频输出；其 Python runtime、线程模型和 Backend Registry 不适合
成为 MacAI 的 Runtime Authority，但协议边界、turn revision 与取消状态机可作为
Phase 8 的可运行参考实现。

## 决策

### 1. 先用 sidecar 验证，再把稳定语义落入 aiworkd

第一步以 `speech-to-speech` 作为仅开发/实验使用的 sidecar：VAD、会话、打断和
播放留在 sidecar，STT、LLM、TTS 分别调用 MacAI 现有的 OpenAI-compatible
endpoint。

```text
microphone / speaker
        ↕
HF speech-to-speech sidecar
        ├─ POST /v1/audio/transcriptions → aiworkd
        ├─ POST /v1/chat/completions      → aiworkd
        └─ POST /v1/audio/speech          → aiworkd
```

sidecar 不是产品依赖，也不拥有模型；它用于确认端到端延迟、打断体验、协议缺口
和真实瓶颈。PoC 有价值后，`aiworkd` 用 Rust/Axum 实现稳定的协议语义，SwiftUI
仍只调用 daemon。

MacAI 的 TTS 请求类型应兼容 OpenAI 字段 `response_format`（当前内部字段是
`format`；可用 serde alias 保持兼容）。PoC 初期使用 WAV 非流式响应；这只验证
会话编排，不代表已经实现 streaming TTS。

### 2. 第一版实现窄 OpenAI Realtime WebSocket 子集

`aiworkd` 新增：

```text
WS /v1/realtime
```

第一版客户端事件：

```text
session.update
input_audio_buffer.append
conversation.item.create       # 先支持 input_text
response.create
response.cancel
conversation.item.truncate     # 为 stock SDK 打断兼容，可先作受控 no-op
```

第一版服务端事件：

```text
session.created / session.updated
input_audio_buffer.speech_started / speech_stopped
conversation.item.input_audio_transcription.delta / completed
response.created
response.output_audio.delta / done
response.output_audio_transcript.delta / done
response.done
error
```

该列表是显式兼容面；没有列出的 OpenAI Realtime 事件不承诺支持。第一阶段只做
WebSocket，不把 WebRTC、完整 Responses API 或未来 SDK 行为隐含进兼容声明。

### 3. turn identity 与取消是统一状态，不由各 Provider 自行猜测

每个输入轮次携带：

```text
session_id
turn_id
turn_revision
cancel_generation
```

Silero VAD 发现静音后可推测启动 STT/LLM；用户在 commit 前继续说话时，同一
`turn_id` 增加 revision，旧 revision 的 partial transcript、LLM token、TTS chunk
与排队音频全部失效。取消使用单调递增 generation；各阶段只发布自己启动时所持
generation 仍有效且 revision 仍为 latest 的输出。

commit 发生在第一段可播放音频准备发布时，而不是 LLM 开始生成或 TTS 请求开始
时。客户端 `response.cancel`、server VAD barge-in、session teardown 与客户端断连
都必须进入同一取消路径，并清空尚未播放的本地/服务端音频。

该状态的 owner 是 daemon Realtime session；Provider 只接收带取消上下文的工作，
不得各自维护平行的 turn truth。

### 4. progressive STT 先复用现有 Provider

第一版 progressive STT 在 VAD 停顿或节流点重新提交当前 turn 的累计音频：

* 每个 session 最多一个 progressive 请求在途；新的 progressive 更新可丢弃；
* final 请求独立提交，不等待尚未完成的 progressive 请求；
* 迟到、旧 revision 或已取消的结果不发布；
* wire 上的 delta 只追加稳定前缀，`completed.transcript` 是权威结果；
* 客户端按 `item_id` 用 final 替换 partial。

该方案允许先复用 Qwen3-ASR-MLX / whisper.cpp。只有在实测的 STT 首字延迟、重复
计算或功耗成为主要瓶颈后，才引入真正的 streaming decoder Provider。

### 5. streaming TTS 必须贯通 worker 到播放队列

保留现有 `POST /v1/audio/speech` 完整文件 API；Realtime 新增独立的 PCM chunk
路径：

```text
TTS worker first PCM chunk
  → provider stream
  → daemon bounded channel
  → response.output_audio.delta
  → client playback buffer
```

TTFA 定义为从 TTS 文本可提交到客户端收到第一段可播放音频的时间。完整生成 WAV
后再切块不满足该指标。barge-in 关闭当前 TTS 工作并阻止旧 generation 的音频继续
发布；若底层模型无法中止计算，至少必须立即停止对外发布并在 worker 协议支持后
补齐真正的 server-side cancellation。

客户端允许配置小型启动 buffer，用少量 TTFA 换取首段 chunk 不均匀时的连续播放；
buffer 属于播放策略，不改变 TTS 请求、生成和重采样。

### 6. Smart Turn 与 Qwen3-TTS 按依赖顺序后置

Realtime WebSocket、VAD、turn revision 和取消路径稳定后，再把 Smart Turn v3.2
作为可选 semantic endpointing：Silero 先检测静音，Smart Turn 对当前 turn 最近
最多约 8 秒的 16 kHz mono PCM 判断 complete / incomplete。它是轻量 auxiliary
ONNX 模型，不接管 STT，也不用于普通文件转写。是否纳入统一 Model Registry，等
实现时根据内存记账、下载和启停需求决定，不提前建立新的特殊生命周期。

Qwen3-TTS-MLX 作为新的常驻 Python worker Provider 接入，与 Kokoro-MLX 并存；
GUI 不直接加载模型。是否成为默认 TTS 由 Apple Silicon 上的 TTFA、实时系数、
中文自然度、内存占用和取消行为 benchmark 决定。

### 7. 保留 MacAI 现有 Runtime 架构

以下现有 owner 不改变：

* `aiworkd` 是唯一 Runtime Authority；
* SQLite registry、scheduler、lease / busy guard、keep_alive 与 LRU 继续管理模型；
* Python/第三方模型继续运行在常驻隔离 worker；
* GUI、CLI、Realtime 客户端不拥有模型，也不推断 Provider 可用性；
* Provider 明确选择和 no-silent-fallback 规则继续适用。

HF 的 FastAPI/uvicorn、每阶段线程 + Python Queue、Backend Registry 和本地模型生命
周期代码不迁入 MacAI。Rust 实现使用 Tokio task、bounded channel 与统一取消状态。

## 被放弃或后置的方案

* 把 HF Python server 嵌入 MacAIConsole 或长期作为产品 Runtime：形成第二个
  Runtime Authority，模型生命周期和错误状态会分裂。
* 直接移植 HF 的线程/Queue pipeline：与 aiworkd 的 Tokio、worker 隔离、lease
  和 scheduler 重复。
* 第一版直接做 WebRTC：本机 `127.0.0.1` 的 Swift 客户端用 WebSocket 已足够；
  ICE、STUN/TURN、SDP、RTP/Opus 只在浏览器或远程网络需求出现后加入。
* 把完整 WAV 拆小块冒充 streaming TTS：不会改善模型 TTFA，也无法及时中止生成。
* 先引入 sherpa-onnx、Parakeet 或新的大规模 Backend 矩阵：复用已有 Provider 完成
  会话闭环，再用 benchmark 决定新增引擎。
* 第一版实现 tool calling：当前 `ChatMessage.content` 只支持字符串，先完成纯文本
  语音闭环；工具所需的结构化 content、tool schema、function result 和 Responses
  API 作为后续兼容面单独设计。

## 后果与已知边界

* Realtime session 会同时持有 STT、LLM、TTS lease；内存预算不足时必须在 session
  建立或模型加载阶段返回明确错误，不能会话中途静默换 Provider。
* 累计音频 progressive STT 会重复计算，属于用已有 Provider 换取更快落地的明确
  成本；监控请求次数、音频秒数、RTF 和功耗后再决定是否更换 decoder。
* 标准 `/v1/audio/speech` 没有可移植的 server-side cancel；在 worker 支持取消前，
  断开消费者只能保证停止发布，未必立刻停止模型计算。
* 内部 pipeline 固定 16 kHz mono PCM16 / 512 samples 是第一版会话契约；WebRTC
  或高采样率 TTS 接入时在 transport/provider 边界做有状态重采样，不让各阶段使用
  不同的隐式采样率。
* HF `speech-to-speech` 是 Apache-2.0。直接复制代码必须保留许可证、NOTICE 和修改
  说明；MacAI 优先参考协议、状态机与测试行为并在 Rust 中独立实现。Smart Turn
  BSD-2-Clause 代码/权重及 Qwen3-TTS 权重仍分别核对并保留各自许可。

## 验证与阶段验收

Sidecar PoC 至少记录：

* VAD speech stop → final transcript；
* final transcript → LLM first token；
* TTS text ready → first playable audio（TTFA）；
* 用户在 assistant 播放期间说话，旧音频停止且旧 generation 不再发布；
* session 结束后无残留 worker 请求、running task 或播放队列音频。

`aiworkd` Realtime 实现至少验证：

* 协议测试覆盖上述显式 client/server 事件与错误事件；
* 同一 turn reopen 后旧 revision 的 transcript/token/audio 全部被抑制；
* `response.cancel`、server VAD barge-in、断连走同一取消状态机；
* bounded channel 在慢客户端下不无限增长，session teardown 能释放全部 lease；
* progressive final 不被迟到 partial 覆盖；
* stock OpenAI Agents SDK 的 pinned WebSocket transport 可完成 session update、麦克风
  输入、转写、音频输出与取消；
* 现有 `/v1/chat/completions`、`/v1/audio/transcriptions`、
  `/v1/audio/speech` 与管理 API 回归测试继续通过。

---

# 74. Decision: 模型下载源由 daemon 统一执行并支持可恢复切换

## 问题

大文件模型从 Hugging Face 官方 resolve 地址下载时，响应体可能在传输中途发生
解码错误。旧实现把该错误直接返回为 HTTP 500；已有 `.part` 文件虽会保留，但
当 HEAD 没有返回 Content-Length 时会放弃断点并从头请求。GUI 也没有下载源选择，
用户无法在官方链路不稳定时切换到 Hugging Face 兼容镜像。

## 决策

模型下载继续由 `aiworkd` 作为唯一 owner 执行。MacAIConsole 设置页新增模型下载源
选择：Hugging Face 官方、`hf-mirror.com` 与自定义 HTTP(S) 地址；GUI 在拉起 daemon
时通过 `AIWORKD_HF_ENDPOINT` 传递选择，切换后由「应用设置并重启 aiworkd」生效。
手动启动 daemon 也可以直接设置同名环境变量。

下载客户端固定使用 HTTP/1.1。响应体中断、短包和 408/429/5xx 会最多重试三次，
每次根据 `.part` 的实际大小发送 Range；服务器忽略 Range 时安全地从头覆盖临时
文件。源地址只允许 HTTP(S) 主机和可选路径，不接受凭据、查询参数或片段。

## 被放弃的方案

* 在 GUI 中直接下载模型：会让客户端拥有下载状态，破坏 daemon 的 Runtime Authority
  和统一错误语义。
* 只增加镜像下拉框而保留原有单次请求：网络中断仍会反复失败，不能利用已下载的
  大量权重。
* 依赖外部 `aria2` 或 `huggingface_hub`：增加本机依赖与版本漂移，现有 Rust 流式
  下载已足够实现 HTTP/1.1、Range 和有限重试。

## 后果与已知边界

* `hf-mirror.com` 是第三方公益镜像；需要登录或 gated 权限的仓库仍需先在官方
  Hugging Face 侧完成授权，镜像不替代访问权限。
* 自定义源必须提供与 `/{repo}/resolve/main/{filename}` 兼容的路径语义。
* 设置变更需重启 aiworkd；已存在的 `.part` 文件会在重启后按新源继续尝试。

## 验证

* `pull::tests::download_file_resumes_after_interrupted_response_body` 使用本地 HTTP
  服务截断第一次响应，验证第二次 Range 请求与最终文件拼接。
* `cargo fmt --all -- --check` 通过。
* `cargo test --workspace` 通过（85 个 Rust 测试）。
* `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest`
  通过（31 个 Swift 测试）。
