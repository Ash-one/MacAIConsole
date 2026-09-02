# Proposal: Runner 插件架构

Status: proposed

Class: architecture

Owner: this file

## Problem

MacAI 当前把推理引擎适配、模型家族逻辑、Python 或原生运行环境、worker
生命周期、模型校验和能力路由集中在 `Provider` 实现及其调用方。新增一种后端
通常需要同时修改 `ai-core` trait、daemon 静态注册、HTTP 白名单、模型默认值、
SwiftUI 环境清单、worker、测试和文档。

这个结构使兼容现有引擎的新权重接入成本较低，使新模型家族或推理库的接入成本
接近修改 MacAI 核心。第三方贡献者也无法在不重新编译 daemon 和 GUI 的情况下
交付扩展。

问题保持成立，无论最终选择动态插件、静态模块、远端服务或其他实现机制。

## Requirement delta

| Item | Previous accepted state | New accepted state | Disposition |
| --- | --- | --- | --- |
| 扩展目标 | 新后端进入 daemon 内部 Provider 集合 | 新权重优先数据化，新后端可由独立 Runner 扩展 | replace |
| Python 环境 | 各 Provider 自行寻找或安装 venv | 由独立环境管理层统一拥有，详见 uv 提案 | replace |
| 首个迁移对象 | 未指定 | Kokoro 是首个完整 Runner 与真实验收样板 | add |
| 架构权威 | README、代码及历史设计材料混合 | 本提案拥有未落地架构方向；代码与 README 继续拥有当前行为 | replace |
| 当前 Provider 行为 | 已上线且被客户端使用 | 迁移期间保留，完成后由 Runner 路径接管 | retain then revise |
| 安全边界 | daemon 管理隔离 worker | 保留并扩展到外部 Runner，禁止模型目录自动执行代码 | keep |
| Phase 1 completion | fake Runner 组合测试曾被当作 generic foundation 完成证据 | 该测试只证明实验性骨架；完成必须包含启动失败清理、执行时信任绑定、显式版本解析，以及 environment 和 Runtime/scheduler adapter | replace |

本次 bounded change 修订 isolated Runner skeleton 的信任与启动行为；它不改变既有
Provider 运行行为、API 或模型注册数据。

## Proposal

MacAI 将扩展边界拆成五个概念：

```text
Capability Contract
        │
        ▼
Model Profile ─────── selects ──────> Runner Plugin
                                           │
                                      references
                                           │
                                           ▼
                                  Runtime Environment
                                           │
                                           ▼
                                    Worker Instance

aiworkd owns discovery, trust, registry, process supervision, leases,
memory scheduling, task history, errors, observability and cleanup.
```

### Capability Contract

Capability Contract 定义 daemon 和客户端理解的稳定语义，如 `chat.v1`、
`stt.v1` 和 `tts.v1`。它拥有请求字段、流式事件、取消、结果和结构化错误。

新增权重和 Runner 不得私自改变这些语义。图像、视频或双向实时音频等新能力需要
独立的核心协议决策，因为它们会改变公共 API 和用户可观察行为。

### Model Profile

Model Profile 是数据型模型说明，声明模型 artifact、来源、能力、Runner、adapter、
资源估算和默认参数。兼容现有 Runner 的新权重只需新增或导入 Model Profile。

精确草案由 [`model-profile-v1.md`](../specs/model-profile-v1.md) 拥有。

### Runner Plugin

Runner Plugin 是可独立分发的推理引擎适配器。它实现版本化 Runner 协议，并声明
真实能力、运行环境、设备、并发和模型匹配范围。Runner 表示可复用推理边界，例如：

- `llama.cpp`；
- `mlx-lm`；
- `mlx-audio`；
- `sherpa-onnx`；
- 后续的 Core ML、ONNX Runtime 或其他本地引擎。

模型家族确实需要特殊调用代码时，该 adapter 位于 Runner 插件内。MacAI 核心只
理解 Capability Contract 和 Runner Protocol。

Runner 包的精确元数据草案由
[`runner-manifest-v1.md`](../specs/runner-manifest-v1.md) 拥有。

### Runtime Environment

Runtime Environment 是 Runner 使用的可安装依赖集合。Python Runner 的环境由
[`2026-09-02-uv-python-environments.md`](2026-09-02-uv-python-environments.md)
独立拥有。原生 Runner 可以引用受管二进制 artifact。

Runner 和环境是多对一或一对多关系。只有环境身份完全相同时才可复用；依赖冲突
产生独立环境，不由 GUI 或 Provider ID 手工推断。

### Worker Instance

Worker Instance 是加载某个模型后的实际进程和运行状态。Runner descriptor/factory
不持有单一全局模型状态；daemon 根据 Runner 声明的容量创建或复用实例。

每个实例至少暴露：

- runner、model 和 instance ID；
- PID、状态、effective device 和 resident RSS；
- 最大并发及当前活跃请求；
- load/inference timeout；
- cancellation 和 graceful shutdown 能力。

一个 Runner 只允许一个 resident model 时，由 manifest 声明 `max_instances = 1`
或等价容量约束，scheduler 据此执行，而不是把限制固化进 Provider 单例结构。

## Ownership boundaries

### aiworkd keeps

- 唯一 Runtime Authority；
- Runner 发现、信任、版本选择和实例监督；
- Model Profile 注册和持久化；
- lease、busy guard、LRU、keep-alive 和内存预算；
- capability 路由、no-silent-fallback 和结构化错误；
- 任务历史、日志脱敏、RSS/device/status 聚合；
- 临时输入输出目录的创建、授权和清理。

### Runner owns

- 特定引擎或模型 API 的加载与调用；
- capability 请求到引擎调用的转换；
- tokenizer、G2P、prompt template 或模型家族 workaround；
- 引擎内流式输出与取消的实现；
- 引擎特有健康检查和设备报告。

### GUI and CLI consume

- GUI 与 CLI 继续只调用 daemon；
- GUI 不扫描插件目录、不执行 `uv`、不推断 Runner 可用性；
- 安装进度、可用性、ready/resident、设备和失败原因全部来自 daemon；
- 通用安装与状态 UI 由 daemon descriptor 驱动；新 Runner 不要求新增 Swift 枚举。

## Discovery and trust

Runner discovery 首版只读取两个来源：

1. 应用或仓库随附的 built-in Runner；
2. `~/Library/Application Support/MacAIConsole/Plugins/<runner-id>/<version>/` 下由用户显式安装的 Runner。

发现不等于信任或执行。daemon 在启动 Runner 前验证：

- manifest schema 和 Runner Protocol 版本；
- Runner ID、版本、artifact digest 和路径约束；
- capability 与 Model Profile 的一致性；
- 已记录的显式信任；
- Runtime Environment 已安装且探测通过。

模型权重目录只能提供数据型 Model Profile。它不能通过 `config.json`、模型仓库脚本
或等价机制让 daemon 自动执行任意代码。需要代码的模型必须通过显式安装的 Runner
交付。

## Protocol

Runner Protocol v1 使用 daemon 监督的本地子进程。stdout 只承载 length-prefixed
UTF-8 JSON frame，stderr 只承载日志。协议包含：

- `hello` / `initialize`；
- `load` / `unload` / `shutdown`；
- `infer` / `cancel`；
- `health`；
- `accepted` / `progress` / `delta` / `result` / `metrics` / `error`。

精确 frame、状态机和错误映射由
[`runner-protocol-v1.md`](../specs/runner-protocol-v1.md) 拥有。

## Management surface

最终管理面至少要让客户端分别观察：

- 已发现与已信任 Runner；
- Runner 使用的环境及安装状态；
- 当前 Worker Instance；
- Model Profile 选择的 Runner 和 adapter；
- requested / selected / effective device / reason。

具体 endpoint 名称在实现切片中确定。迁移期可保留 `/api/providers` 组合视图，直到
GUI、CLI 和文档完成切换。兼容期限要在实现提案中明确，不能留下无限期双 owner。

## Kokoro reference slice

Kokoro 作为第一个完整 Runner，覆盖 Python 环境、TTS、临时 WAV、长请求、模型家族
workaround、常驻 worker、RSS、故障隔离和 GUI 安装状态。它比只做 mock Runner 更能
暴露真实组合问题。

实施步骤和证据矩阵由
[`kokoro-runner-reference.md`](../plans/kokoro-runner-reference.md) 拥有。

本提案不预先认定 Kokoro 与其他 `mlx-audio` 模型必须共享同一个 Runner 或环境。
首个切片先验证通用边界；后续只有在上游 API、依赖锁和 capability 行为确实一致时
才合并。

## Migration

### Phase 1: generic foundation

- 实现 manifest/Profile 解析和 Runner registry；
- 实现 Runner Protocol codec、process supervisor 和 instance state；
- 建立 fake Runner composition test；
- 启动失败必须 kill/wait 子进程并回收 stderr capture；Runner 执行的 package 必须从
  已验证 digest 的私有 staging 副本启动；
- Model Profile 必须以 SemVer 范围显式选择可信 Runner；多版本命中或不匹配必须失败，
  不得按发现顺序选择；
- 建立 daemon-owned uv environment 和 Runtime/scheduler 的 Runner Instance adapter，
  使上述基础走入真实的 lifecycle/lease/内存裁决路径。
- 保留当前 Provider 路径，避免在通用基础尚未自证时同时迁移全部模型。

### Phase 2: Kokoro

- 建立自包含 `runners/kokoro/` Runner project；
- 用 `uv` lock 和受管环境替代 Kokoro 手工 venv；
- 迁移现有 Kokoro worker 行为及测试；
- 由 Model Profile 注册并通过 Runner registry 加载；
- GUI 从 daemon 读取环境状态和安装动作。

### Phase 3: remove the parallel Kokoro path

真实模型路径通过后，删除旧 `KokoroMlxProvider` 装配、Kokoro 专属环境枚举和
provider 白名单分支。删除必须覆盖源代码、注册、GUI、测试、文档和持久数据迁移，
不能仅留下不可达旧代码。

### Phase 4: next Runner

第二个 Runner 应选择与 Kokoro 不同的 failure surface。推荐 MLX-LM，用来验证
token streaming、finish reason、usage 和长上下文；完成后再判断是否迁移其余
Python Provider。

## Alternatives considered

**继续在 daemon 内增加 Provider。** 当前路径具有强类型和直接调试优势。它要求
每个新后端修改核心组合、GUI 和发布物，无法满足独立分发扩展的目标。

**从模型仓库直接执行 remote code。** 它降低作者发布门槛，也把模型下载扩大为代码
执行授权，并引入依赖冲突、升级漂移和不可审计启动路径。MacAI 选择显式安装、固定
版本和进程隔离的 Runner。

**所有 Runner 只暴露 OpenAI-compatible HTTP。** Chat 生态成熟，STT/TTS、load、
unload、RSS、设备、取消和进程监督的语义并不完整。Runner Protocol 保留完整的本地
Runtime Authority；以后可单独设计 unmanaged endpoint bridge。

**一次迁移全部 Provider。** 可以更快删除旧抽象，也会把协议、环境、GUI、持久化和
多种 capability 的失败混在一个不可诊断变更里。Kokoro 先建立完整样板，后续迁移
复用已验证边界。

## Implementation status

2026-09-02：merge `232f010` 引入了实验性 Runner 骨架（原实现 commit `f0bc102`）；
commit `cadfa31` 收敛了已发现的启动、契约与信任边界：

- `crates/ai-daemon/src/runners/`：manifest、profile、protocol、registry、
  supervisor、environment 六模块与 `lib.rs` 导出；
- `crates/ai-daemon/src/bin/fake-runner.rs`：协议参考实现；
- `tests/runner_plugin_composition.rs`：发现 → 信任 → 握手 → load → infer →
  unload 的 isolated fake Runner 测试。

它没有接入 `main.rs`、Runtime 或 scheduler，也没有 uv sync/probe/environment staging、
真实 Kokoro Runner 或 GUI 状态路径。本次安全与契约修订已使实验性骨架：

- 在 boot deadline、hello identity 和 malformed protocol 三类启动失败后 kill/wait
  子进程并 abort/await stderr capture；
- 接受 `project = "."`，为 Profile defaults/resources 建立类型和校验，并以 SemVer
  range 显式解析 Runner 版本；
- 对同 ID 的多个可信匹配版本返回结构化 ambiguity，不再使用发现顺序；
- discovery 时解析 Model Profile snapshot，启动前复核 package digest，并只执行
  daemon-owned staging 副本。

本提案保持 `proposed`：Phase 1A isolated foundation、Phase 1B daemon-owned uv
environment manager 与 Phase 1C Runtime/scheduler Runner Instance 组合均已完成
（分别见 uv 提案与 [`kokoro-runner-reference.md`](../plans/kokoro-runner-reference.md)
的 Implementation status）。当前下一步依次是 Kokoro 迁移基线（Phase 0 completion）、
uv-lock Kokoro Runner（Phase 2）、adapter 迁移（Phase 3）和最终产品 cutover
（Phase 4+5），以及 Model Profile 持久 snapshot 与 HTTP 管理面接入。

Model Profile 持久 snapshot/digest 已先行落地（2026-09-02，Phase 4 首块）：
`ModelProfile::canonical_json`/`digest` + `RegistryStore::model_profiles` 表与
upsert/get/load。daemon 装配第二块同日本地：`Runtime::attach_runner`/
`register_runner_profile` + main.rs `bootstrap_runners`（built-in Runner
discovery → profile 持久化 → artifact 存在才 bind/attach）。HTTP 管理面端点、
Plugins 目录与显式信任仍未接入（见
[`kokoro-runner-reference.md`](../plans/kokoro-runner-reference.md) Phase 4）。

下表是提案完成后的直接证据要求。`Result` 在实施前保持 `not run`。

| Acceptance | Failure surface | Direct evidence | Result |
| --- | --- | --- | --- |
| 已验证 package 的 fake Runner 可在 isolated test 中完成发现、信任、加载、调用和卸载；启动失败不留下进程，源码 package 修改后不可执行 | manifest、trust、supervision、composition | `cargo test -p ai-daemon --test runner_plugin_composition` | passed at `cadfa31`（5 tests） |
| 新 Model Profile 可选择 Runner，且不需要修改 `main.rs` provider 白名单 | profile resolution、registration | `cargo test -p ai-daemon model_profile_runner_resolution`；负向搜索旧白名单 | not run |
| Worker 崩溃返回 `backend_crashed`，aiworkd 仍可通过 health/status 访问 | process supervision、error mapping | `cargo test -p ai-daemon runner_crash_isolated` | not run |
| lease、busy guard、LRU 和 keep-alive 对 Runner Instance 生效 | lifecycle composition | `cargo test -p ai-daemon runner_lifecycle` | not run |
| 推理进行时 status/RSS 查询不等待 Runner I/O 锁 | observability concurrency | 确定性并发测试；Kokoro 长合成期间轮询 `/api/runtime` | not run |
| 未信任 Runner 和模型目录代码不会被执行 | trust boundary | manifest/discovery 单测和拒绝路径集成测试 | not run |
| GUI 仅根据 daemon descriptor 展示 Runner 与环境状态 | real client composition | MacAIConsole API decoding/request tests，加运行应用检查 | not run |
| Kokoro 经新路径产生有效 WAV，并保持当前中文、英文、长文本、voice 和 speed 契约 | real model behavior | `docs/plans/kokoro-runner-reference.md` 的 unit、integration、real smoke matrix | not run |
| 旧 Kokoro 路径在迁移完成后完全不可达 | source、registration、GUI、docs、tests | 负向搜索加全套测试 | not run |

完整变更还必须运行：

```bash
cargo fmt --all -- --check
cargo test --workspace
cd apps/MacAIConsole
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest
```

Python 与真实 Kokoro 命令由 uv 决策和 Kokoro 计划拥有。

## Risks

- 通用协议可能演变成透传任意 JSON 的弱类型接口。v1 只接受已版本化 capability
  contract，模型特有参数必须命名空间化且不能改变核心语义。
- 插件发现扩大本地代码执行面。显式信任、digest、环境变量 allowlist、路径约束和
  进程隔离属于首版验收，不作为后续加固项。
- 两套运行路径并存会产生状态和错误语义漂移。每个迁移切片都要有明确删除阶段。
- Kokoro 的成功只能证明 TTS/Python Runner 路径，不能证明 token streaming、视觉
  输入或实时双向音频。
- 插件协议一旦供第三方使用会形成兼容负担。v1 发布前必须先经过 fake Runner 和
  Kokoro 两种实现验证；在此之前保持 draft。
