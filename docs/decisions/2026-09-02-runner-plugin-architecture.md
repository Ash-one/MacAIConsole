# Decision: Runner 插件架构

Status: implemented

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
| 架构权威 | README、代码及历史设计材料混合 | 本决策拥有架构方向与理由；代码与 README 继续拥有当前行为 | replace |
| 当前 Provider 行为 | 已上线且被客户端使用 | 迁移期间保留，完成后由 Runner 路径接管 | retain then revise |
| 安全边界 | daemon 管理隔离 worker | 保留并扩展到外部 Runner，禁止模型目录自动执行代码 | keep |
| Phase 1 completion | fake Runner 组合测试曾被当作 generic foundation 完成证据 | 该测试只证明实验性骨架；完成必须包含启动失败清理、执行时信任绑定、显式版本解析，以及 environment 和 Runtime/scheduler adapter | replace |
| 2026-09-03 架构复核 | 已把 built-in TTS/STT 切片与 isolated tests 视为完整开放架构 | 首次下载必须无需重启即可加载；注册模型必须绑定 immutable Profile snapshot；status 必须来自真实 Worker Instance；未实现的并发/cancel/capacity 不能宣称 current | replace |
| Runner 权限模型 | manifest 的 network/filesystem 字段曾被描述为运行时沙箱 | v1 显式信任 Runner 等同授予 daemon 用户权限；MacAI 只强制身份、环境变量、模型/输出路径契约和进程监督。OS 级沙箱需独立安全决策 | replace |

架构概念自 Phase 1A 起逐步落地；本决策以现在时描述当前已运行的架构，尚未落地的
部分在 Migration 与 Implementation status 中显式标注，保持单一事实来源。

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

精确契约由 [`model-profile-v1.md`](../specs/model-profile-v1.md) 拥有。

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

Runner 包的精确元数据契约由
[`runner-manifest-v1.md`](../specs/runner-manifest-v1.md) 拥有。

### Runtime Environment

Runtime Environment 是 Runner 使用的可安装依赖集合。Python Runner 的环境由
[`2026-09-02-uv-python-environments.md`](2026-09-02-uv-python-environments.md)
独立拥有。原生 Runner 可以引用受管二进制 artifact。

Runner 和环境是多对一或一对多关系。只有环境身份完全相同时才可复用；依赖冲突
产生独立环境，不由 GUI 或 Provider ID 手工推断。

### Worker Instance

Worker Instance 是加载某个模型后的实际进程和运行状态。daemon 保存独立于协议 I/O
锁的 instance snapshot；environment ready 不能代替 worker alive 或 model resident。

每个实例至少暴露：

- runner、model 和 instance ID；
- PID、状态、effective device 和 resident RSS；
- 当前活跃请求；
- load/inference timeout；
- graceful shutdown 与 crash/reload 状态。

当前 v1 只接受 `max_instances = 1`、`max_concurrency_per_instance = 1`，并在解析期
拒绝更高容量。多实例/多并发需要进程 actor、request dispatcher 与 instance pool，
通过后续协议决策引入，不能由 manifest 提前宣称。

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
- 引擎内流式输出；主动取消在后续协议决策中与 daemon 消费路径共同定义；
- 引擎特有健康检查和设备报告。

### GUI and CLI consume

- GUI 与 CLI 继续只调用 daemon；
- GUI 不扫描插件目录、不执行 `uv`、不推断 Runner 可用性；
- 安装进度、可用性、ready/resident、设备和失败原因全部来自 daemon；
- 通用安装与状态 UI 由 daemon descriptor 驱动；新 Runner 不要求新增 Swift 枚举。

## Discovery and trust

Runner discovery 当前实现只读取一个来源：

1. 应用或仓库随附的 built-in Runner（`runners/` 目录，由 daemon 启动时的
   `bootstrap_runners` 发现、持久化 Model Profile 并装配）。

`~/Library/Application Support/MacAIConsole/Plugins/<runner-id>/<version>/` 下由用户
显式安装的第三方 Runner 属于设计边界，尚未实现（见 Migration）。

发现不等于信任或执行。daemon 在启动 Runner 前验证：

- manifest schema 和 Runner Protocol 版本；
- Runner ID、版本、artifact digest 和路径约束；
- capability 与 Model Profile 的一致性；
- 已记录的显式信任；
- Runtime Environment 已安装且探测通过。

模型权重目录只能提供数据型 Model Profile。它不能通过 `config.json`、模型仓库脚本
或等价机制让 daemon 自动执行任意代码。需要代码的模型必须通过显式安装的 Runner
交付。

v1 的 Runner 信任是本地代码信任：built-in 或用户显式信任的 Runner 以 aiworkd
用户权限运行。daemon 强制 digest、无 shell argv、环境变量 allowlist、受管输入输出
路径和越界结果拒绝；这些机制不构成 OS 级文件系统或网络沙箱。若产品要求不可信插件
隔离，需另立安全决策并以真实逃逸测试验收。

## Protocol

Runner Protocol v1 使用 daemon 监督的本地子进程。stdout 只承载 length-prefixed
UTF-8 JSON frame，stderr 只承载日志。协议包含：

- `hello` / `initialize`；
- `load` / `unload` / `shutdown`；
- `infer`；
- `health`；
- `accepted` / `progress` / `delta` / `result` / `metrics` / `error`。

精确 frame、状态机和错误映射由
[`runner-protocol-v1.md`](../specs/runner-protocol-v1.md) 拥有。

Runner 可返回 `cancelled` 终态；daemon→Runner 主动 cancel 不属于 current v1。
客户端断开只终止 daemon 任务记录，不宣称底层计算已中止。主动取消必须与多路 I/O
dispatcher 和真实 HTTP/task abort 消费方一起设计。

## Management surface

最终管理面至少要让客户端分别观察：

- 已发现与已信任 Runner；
- Runner 使用的环境及安装状态；
- 当前 Worker Instance；
- Model Profile 选择的 Runner 和 adapter；
- requested / selected / effective device / reason。

当前实现：`GET /api/runners`（Runner、环境与 Worker Instance snapshot）与
`POST /api/runners/{runner}/install`（显式环境安装，幂等）。`/api/providers` 组合视图
继续服务 GUI/CLI，Runner-backed provider 以 `org.macai.*` 出现在同一视图与
`/v1/models` 的 `owned_by` 中；Provider 全量迁移完成后按
[`runner-migration-roadmap.md`](../plans/runner-migration-roadmap.md) 收尾，不留下
无限期双 owner。

## Kokoro reference slice

Kokoro 已作为第一个完整 Runner 落地（2026-09-02/03），覆盖 Python 环境、TTS、临时
WAV、长请求、模型家族 workaround、常驻 worker、RSS、故障隔离和 GUI 安装状态；
第二个切片 qwen3-asr（STT）复用同一边界并已删除 legacy。实施证据由
[`kokoro-runner-reference.md`](../plans/kokoro-runner-reference.md)（Kokoro）与
[`runner-migration-roadmap.md`](../plans/runner-migration-roadmap.md)（迁移）拥有。

本决策不预先认定 Kokoro 与其他 `mlx-audio` 模型必须共享同一个 Runner 或环境。
后续只有在上游 API、依赖锁和 capability 行为确实一致时才合并。

## Migration

已落地的 Phase 与剩余工作的边界：

### Phase 1: generic foundation ✅（2026-09-02）

- manifest/Profile 解析、Runner registry、Protocol codec、supervisor、instance state；
- fake Runner composition test；启动失败 kill/wait 并从 digest-bound staging 副本启动；
- SemVer 显式版本解析，多版本命中返回结构化 ambiguity；
- daemon-owned uv environment manager（`resolve_uv`、单飞安装锁、staging sync、
  probe、原子提升、重启恢复，`tests/runner_environment_manager.rs` 8 tests）；
- Runtime/scheduler Runner Instance adapter（`runners/instance.rs` +
  `RunnerProvider`，lease/busy guard/LRU/keep-alive 由既有 Runtime 裁决）；
- Model Profile catalog 与 per-model immutable snapshot（SQLite `model_profiles` +
  `registered_model_profiles`）。

### Phase 2–3: Kokoro ✅（2026-09-02，legacy 删除 2026-09-03）

`runners/kokoro/` + adapter 迁移 + daemon 装配 + HTTP 管理面 + GUI 安装状态 +
真实接线验收（zh/mixed/长文本 WAV），随后删除 `KokoroMlxProvider` 全消费面。
证据由 [`kokoro-runner-reference.md`](../plans/kokoro-runner-reference.md) 拥有。

### Phase 3+（qwen3-asr 扩展切片）✅（2026-09-02/03）

`runners/qwen3-asr/`（STT capability）复用同一边界；推荐模型、注册路径与
`ModelRepository.directoryProvider` 均指向 Runner，legacy 已删除。证据由
[`runner-migration-roadmap.md`](../plans/runner-migration-roadmap.md) 拥有。

### 剩余 working 范围

- **逐引擎迁移**：qwen3-tts ✓ → sherpa-onnx ✓ → mlx-lm ✓；每迁一个删一个
  legacy Provider 与 GUI `PythonEnvironmentSpec` 条目（roadmap owner）；
- **未来主动取消/并发**：作为新的协议决策实现 worker 线程化、进程 actor、request
  dispatcher、容量调度与 HTTP abort；current v1 保持 single-flight；
- **Plugins 目录与显式信任**：第三方 Runner 的用户显式安装来源；
- **收尾**：mock / macos-say 保留为测试与 CLI 兜底能力（2026-09-03 用户拍板）：
  生产装配不含它们，GUI 无注册入口；删除 GUI `PythonEnvironmentManager`
  （.build 一键安装路径，全部 Python worker 已迁移 Runner，仅剩 llama.cpp
  二进制安装）。

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

后续落地（2026-09-02/03，全部带直接证据）：

- Phase 1B uv environment manager 与 Phase 1C Runtime/scheduler 组合（证据见
  [`uv-python-environments.md`](2026-09-02-uv-python-environments.md) 与
  [`kokoro-runner-reference.md`](../plans/kokoro-runner-reference.md)）；
- Model Profile 持久 authority：catalog 写入 `model_profiles`，首次注册复制到
  `registered_model_profiles`；重启优先恢复 per-model snapshot；
- daemon 装配：`Runtime::attach_runner` / `register_runner_profile` +
  main.rs `bootstrap_runners`（built-in discovery → profile 持久化 → 无论 artifact
  是否存在都先 bind/attach，首次下载无需重启）；
- HTTP 管理面：`GET /api/runners` 与 `POST /api/runners/{runner}/install`；
- GUI 消费端：设置页「Runner 引擎（实验）」区块（`runners()`/`installRunner`）；
- Kokoro 真实接线验收（zh/mixed/长文本）与 legacy 删除；qwen3-asr（STT）
  Runner 落地与 legacy 删除。

下方验收表记录本决策的核心证据义务；`Result` 为空表示尚未执行的承诺。

| Acceptance | Failure surface | Direct evidence | Result |
| --- | --- | --- | --- |
| 已验证 package 的 fake Runner 可在 isolated test 中完成发现、信任、加载、调用和卸载；启动失败不留下进程，源码 package 修改后不可执行 | manifest、trust、supervision、composition | `cargo test -p ai-daemon --test runner_plugin_composition` | passed at `cadfa31`（5 tests） |
| 新 Model Profile 可选择 Runner，且首次下载前 Runner 已装配 | profile resolution、registration | `builtin_runner_attaches_before_its_model_artifact_exists`；动态 provider 按 descriptor 能力判定 | passed（2026-09-03） |
| 已注册模型恢复注册时 Profile，catalog 更新不改写它 | persistence、restart | registry binding roundtrip + `registered_runner_model_restores_its_immutable_profile_snapshot` | passed（2026-09-03） |
| Worker 崩溃返回 `backend_crashed`、清空 resident，并可重新 load | process supervision、status、recovery | `crashed_worker_clears_residency_and_can_be_reloaded` | passed（2026-09-03） |
| lease、busy guard、LRU 和 keep-alive 对动态 Provider 走同一 Runtime owner | lifecycle composition | `active_model_lease_blocks_unload` + `attach_runner_registers_provider_and_shutdown_handle`；直接 Runner+Runtime 生命周期测试仍应补充 | partial |
| status snapshot 不等待 Runner I/O 锁，停止后无 resident | observability concurrency | `status_reflects_environment_phase_without_resident_worker` + crash/reload composition | passed（2026-09-03） |
| 未信任 Runner 不执行；已信任 Runner 以 daemon 用户权限执行 | trust boundary | discovery/trust 单测 + digest-bound staging；OS 权限边界由文档明确，不宣称沙箱 | passed for declared v1 boundary |
| GUI 仅根据 daemon descriptor 展示 Runner 与环境状态 | real client composition | MacAIConsole `DaemonAPI` decoding/request tests + swift test 40 passed | passed（2026-09-03） |
| Kokoro 经新路径产生有效 WAV，并保持当前中文、英文、长文本、voice 和 speed 契约 | real model behavior | `MACAI_KOKORO_SMOKE_MODEL=… scripts/tests/kokoro_runner_smoke.sh` + `MACAI_KOKORO_WIRING_MODEL=… cargo test --test runner_kokoro_real_wiring` | passed（2026-09-02，见 kokoro-runner-reference 证据表） |
| 旧 Kokoro 路径在迁移完成后完全不可达 | source、registration、GUI、docs、tests | 负向搜索（零残留）+ 全套测试绿（legacy 删除 bounded change，2026-09-03） | passed |

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
- 插件发现扩大本地代码执行面。v1 把显式信任定义为完整本地代码权限；digest、环境
  变量 allowlist、路径契约和进程监督不提供 OS 沙箱。开放第三方安装前 UI 必须明确
  展示这一授权边界；更弱权限模型需要独立安全设计。
- 两套运行路径并存会产生状态和错误语义漂移。每个迁移切片都要有明确删除阶段。
- Kokoro 的成功只能证明 TTS/Python Runner 路径，不能证明 token streaming、视觉
  输入或实时双向音频。
- 插件协议一旦供第三方使用会形成兼容负担。v1 已经过 fake Runner 与 Kokoro 两种
  实现验证；在 Plugins 第三方安装路径落地前，对第三方作者的稳定性承诺保持克制。
