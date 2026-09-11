# Decision: Runner 插件架构

Status: implemented

Class: architecture

Owner: this file

## Problem

本决策引入前，MacAI 把推理引擎适配、模型家族逻辑、运行环境、worker
生命周期和能力路由集中在 daemon 内部 Provider 及其调用方。新后端因此需要
同时修改核心装配、HTTP、GUI、worker、测试和文档。

这个结构使兼容现有引擎的新权重接入成本较低，使新模型家族或推理库的接入成本
接近修改 MacAI 核心。第三方贡献者也无法在不重新编译 daemon 和 GUI 的情况下
交付扩展。

问题保持成立，无论最终选择动态插件、静态模块、远端服务或其他实现机制。

## Requirement delta

| Item | Previous accepted state | New accepted state | Disposition |
| --- | --- | --- | --- |
| 扩展目标 | 新后端进入 daemon 内部 Provider 集合 | 新权重优先数据化，新后端可由独立 Runner 扩展 | replace |
| Python 环境 | 各 Provider 自行寻找或安装 venv | 由独立环境管理层统一拥有，详见 uv 决策 | replace |
| 首个迁移对象 | 未指定 | Kokoro 是首个完整 Runner 与真实验收样板 | add |
| 架构权威 | README、代码及历史设计材料混合 | 本决策拥有架构方向与理由；代码与 README 继续拥有当前行为 | replace |
| 生产 Provider 形态 | 静态 Provider 为主 | 七个生产引擎全部由 Runner 接管 | replace |
| 安全边界 | daemon 管理隔离 worker | 保留并扩展到外部 Runner，禁止模型目录自动执行代码 | keep |
| Phase 1 completion | fake Runner 组合测试曾被当作 generic foundation 完成证据 | 该测试只证明实验性骨架；完成必须包含启动失败清理、执行时信任绑定、显式版本解析，以及 environment 和 Runtime/scheduler adapter | replace |
| 2026-09-03 架构复核 | 已把 built-in TTS/STT 切片与 isolated tests 视为完整开放架构 | 首次下载必须无需重启即可加载；注册模型必须绑定 immutable Profile snapshot；status 必须来自真实 Worker Instance；未实现的并发/cancel/capacity 不能宣称 current | replace |
| Runner 权限模型 | manifest 的 network/filesystem 字段曾被描述为运行时沙箱 | v1 显式信任 Runner 等同授予 daemon 用户权限；MacAI 只强制身份、环境变量、模型/输出路径契约和进程监督。OS 级沙箱需独立安全决策 | replace |
| GUI 模型目录 | Swift 内置推荐模型、artifact 清单并按目录内容推断 Provider | daemon 从 Model Profile 输出 catalog，并按 Profile ID 拥有下载、Runner 选择与注册；GUI 只传用户意图 | replace |
| GUI 引擎管理用语 | 设置页同时保留旧「运行环境」与实验性 Runner 区块，运行状态称为「Provider 状态」 | 旧管理器已退役；设置页仅以「引擎」展示 daemon 管理的 Runner，运行状态以「Runner」标示同一生产引擎集合 | replace |
| 2026-09-07 Worker 驻留内存恢复 | 迁移 Runner 后因旧模块删除导致 resident RSS 缺失（UI 降级显示「内存不可测」） | Worker Instance 快照由 daemon 统一基于 PID 与子进程树实时采样物理内存（RSS），不阻塞推理，卸载后清空 | replace |

whisper.cpp 的具体 source build、常驻 server 与兼容迁移由
[`2026-09-05-whisper-runner-migration.md`](2026-09-05-whisper-runner-migration.md) 拥有。

## Decision

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
资源估算和默认参数。兼容现有 Runner 的 bundled 新权重通过新增 Profile 接入；
第三方导入入口尚未实现。

精确契约由 [`model-profile-v1.md`](../specs/model-profile-v1.md) 拥有。

### Runner Plugin

Runner Plugin 是独立版本化的推理引擎适配器包。它实现版本化 Runner 协议，并声明
真实能力、运行环境、设备、并发和模型匹配范围。当前产品只装配仓库随附的 built-in
包，第三方分发/安装入口尚未实现。Runner 表示可复用推理边界，例如：

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
- 推荐模型、artifact 完整清单和 Runner 绑定由 daemon Model Profile catalog 驱动；新增 bundled Profile 不要求修改 Swift 清单。

## Discovery and trust

Runner discovery 当前实现读取两个来源：

1. 应用或仓库随附的 built-in Runner（`runners/` 目录，由 daemon 启动时的
   `bootstrap_runners` 发现、持久化 Model Profile 并装配）。
2. MacAIConsole 创建并显式信任的单文件 Script Runner（受管 `Plugins/` 目录，由 daemon
   生成标准 package、记录 digest，并在下次启动时装配）。

任意完整第三方 Runner package 的导入、更新和分发尚未提供 UI；当前插件入口只接受
[`单文件 Script Runner`](2026-09-08-single-file-script-runner.md) 的受限 metadata 与 hook 契约。

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

当前管理面让客户端分别观察：

- 已发现与已信任 Runner；
- Runner 使用的环境及安装状态；
- 当前 Worker Instance；
- Model Profile 选择的 Runner 和 adapter；
- requested / selected / effective device / reason。Provider 选择存入模型注册表，
  并由 `/v1/models` 和 `/api/runtime` 暴露。

当前实现：`GET /api/runners`（Runner、环境与 Worker Instance snapshot）、
`POST /api/runners/{runner}/install`（显式环境安装，幂等）与同路径 `DELETE`（显式卸载）。
卸载拒绝仍有加载模型或活动请求的 Runner，关闭空闲 worker 后删除 daemon-owned Python
环境和原生引擎产物；状态回到 `missing` 后可重新安装。`/api/providers` 组合视图
继续服务 GUI/CLI，Runner-backed provider 以 `org.macai.*` 出现在同一视图与
`/v1/models` 的 `owned_by` 中。原生引擎的下载、校验、可选 source build 与就绪状态
也由同一 Runner install/status 管理面拥有。

`GET /api/model-profiles` 提供 GUI 所需的 Profile catalog 投影；
`POST /api/model-profiles/{id}/pull` 只接收 `auto_load` 用户意图，由 daemon 从可信
Profile 取 source、artifact、能力和 Runner。GUI 不再展开 pull manifest，也不扫描模型
目录来推断 Provider。本地单文件注册仍只提交 path、model type 与用户参数，最终选择由
daemon 的注册契约拥有。

## Kokoro reference slice

Kokoro 已作为第一个完整 Runner 落地（2026-09-02/03），覆盖 Python 环境、TTS、临时
WAV、长请求、模型家族 workaround、常驻 worker、RSS、故障隔离和 GUI 安装状态；
第二个切片 qwen3-asr（STT）复用同一边界并已删除旧实现。Kokoro 的目标机证据由
[`kokoro-runner-verification.md`](../reference/kokoro-runner-verification.md) 保存；
其余 built-in Runner 的当前自动化证据位于各包 `tests/` 与 Runtime 组合测试。

本决策不预先认定 Kokoro 与其他 `mlx-audio` 模型必须共享同一个 Runner 或环境。
后续只有在上游 API、依赖锁和 capability 行为确实一致时才合并。

## Current scope

- built-in Runner 已包含 llama.cpp、whisper.cpp、mlx-lm、Kokoro、Qwen3-ASR、
  Qwen3-TTS 和 sherpa-onnx；它们全部经 `bootstrap_runners` 动态装配；
- GUI 通过 `/api/runners` 观察和安装环境，通过 `/api/model-profiles` 获取推荐目录并按 Profile ID 下载；不存在本地 venv/脚本安装 manager、Swift Profile catalog 或目录到 Provider 推断；
- mock / macos-say 仅由测试 Runtime 注入，不进入生产 Provider 表；
- 单文件 Script Runner 已提供受管 Plugins 与显式 digest trust；任意完整第三方 package
  导入、OS 级沙箱、多实例/多并发和 daemon→Runner 主动 cancel 仍需独立决策。

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

## Verification

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
  [`kokoro-runner-verification.md`](../reference/kokoro-runner-verification.md)）；
- Model Profile 持久 authority：catalog 写入 `model_profiles`，首次注册复制到
  `registered_model_profiles`；重启优先恢复 per-model snapshot；
- daemon 装配：`Runtime::attach_runner` / `register_runner_profile` +
  main.rs `bootstrap_runners`（built-in discovery → profile 持久化 → 无论 artifact
  是否存在都先 bind/attach，首次下载无需重启）；
- HTTP 管理面：`GET /api/runners` 与 `POST`/`DELETE /api/runners/{runner}/install`；
- GUI 消费端：管理页「引擎」区块（`runners()`/`installRunner`/`uninstallRunner`）；
- 七个 built-in Runner 包的 adapter 单测和 lock freshness 进入 CI；
- CI 不伪造宿主机 AI 内存预算：无调度主张的 mock fixture 使用 0 estimate；
  Runtime 单测以固定预算直接验证 LRU 逐出和 lease 保护；
- `load_model` 持有生命周期锁时，LRU 通过同一锁域内的私有卸载路径逐出，避免重入
  `unload_model` 等待自身锁；
- 需要模型权重、目标机引擎或网络 source build 的真实 smoke 标记为 Cargo ignored，
  仅以显式 `--ignored` 命令作为真实模型证据；
- ad-hoc Runner 绑定的路径刷新、rename 与 unregister 由 Runtime 组合回归测试固定；
- Provider 选择的 requested / selected / reason 持久化并出现在管理 API。

下方表格记录本决策的持续验证义务。

| Acceptance | Failure surface | Direct evidence | Result |
| --- | --- | --- | --- |
| 已验证 package 的 fake Runner 可在 isolated test 中完成发现、信任、加载、调用和卸载；启动失败不留下进程，源码 package 修改后不可执行 | manifest、trust、supervision、composition | `cargo test -p ai-daemon --test runner_plugin_composition` | passed at `cadfa31`（5 tests） |
| 新 Model Profile 可选择 Runner，且首次下载前 Runner 已装配 | profile resolution、registration | `builtin_runner_attaches_before_its_model_artifact_exists`；动态 provider 按 descriptor 能力判定 | passed（2026-09-03） |
| 已注册模型恢复注册时 Profile，catalog 更新不改写它 | persistence、restart | registry binding roundtrip + `registered_runner_model_restores_its_immutable_profile_snapshot` | passed（2026-09-03） |
| Worker 崩溃返回 `backend_crashed`、清空 resident，并可重新 load | process supervision、status、recovery | `crashed_worker_clears_residency_and_can_be_reloaded` | passed（2026-09-03） |
| lease、busy guard、LRU 和 keep-alive 对动态 Provider 走同一 Runtime owner | lifecycle composition | `active_model_lease_blocks_unload` + 固定预算的 Runtime LRU/lease tests + `runner_runtime_composition` | passed |
| ad-hoc Runner 模型刷新路径、改名或删除时绑定与注册表一致 | binding lifecycle | `adhoc_runner_binding_tracks_refresh_rename_and_unregister` | passed（2026-09-05） |
| Provider 缺省选择、显式选择和兼容别名可审计 | API + persistence | `provider_selection_records_default_alias_and_explicit_reasons` + registry roundtrip + `/v1/models`、`/api/runtime` 字段 + Swift decoding test | passed（2026-09-05） |
| status snapshot 不等待 Runner I/O 锁，停止后无 resident | observability concurrency | `status_reflects_environment_phase_without_resident_worker` + crash/reload composition | passed（2026-09-03） |
| Worker Instance 暴露实时 resident RSS，独立于 Runner I/O 锁并累加子进程树；停止后清空 | process supervision、observability | `process_memory` 单元测试 + `status_reflects_environment_phase_without_resident_worker` 驻留/卸载断言 | passed（2026-09-07） |
| 未信任 Runner 不执行；已信任 Runner 以 daemon 用户权限执行 | trust boundary | discovery/trust 单测 + digest-bound staging；OS 权限边界由文档明确，不宣称沙箱 | passed for declared v1 boundary |
| GUI 仅根据 daemon descriptor 展示 Runner 与环境状态 | real client composition | MacAIConsole `DaemonAPI` decoding/request tests + `swift test --enable-xctest` | passed |
| GUI 推荐目录与下载动作完全来自 daemon Profile，未知 Runner/Profile 无需 Swift 分支 | catalog contract、client composition、negative removal | daemon profile projection/pull tests + Swift unknown-Runner decoding/request tests + GUI 源码负向搜索 | passed（2026-09-05） |
| Kokoro 经新路径产生有效 WAV，并保持当前中文、英文、长文本、voice 和 speed 契约 | real model behavior | `MACAI_KOKORO_SMOKE_MODEL=… scripts/tests/kokoro_runner_smoke.sh` + `MACAI_KOKORO_WIRING_MODEL=… cargo test --test runner_kokoro_real_wiring` | passed（2026-09-02，见 Kokoro 验证参考） |
| 旧 Kokoro 路径在迁移完成后完全不可达 | source、registration、GUI、docs、tests | 负向搜索（零残留）+ 全套测试绿（legacy 删除 bounded change，2026-09-03） | passed |
| whisper.cpp 经统一安装、动态 Provider 与常驻 server 完成转写，旧静态路径不可达 | source build、STT composition、migration | [`whisper.cpp Runner 迁移决策`](2026-09-05-whisper-runner-migration.md) 的 install smoke、real wiring、负向搜索与全套测试 | passed（2026-09-05） |

完整变更还必须运行：

```bash
cargo fmt --all -- --check
cargo test --workspace
cd apps/MacAIConsole
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest
```

Python 与真实 Kokoro 命令由 uv 决策和
[`Kokoro Runner 验证参考`](../reference/kokoro-runner-verification.md) 拥有。

## Consequences

- 通用协议可能演变成透传任意 JSON 的弱类型接口。v1 只接受已版本化 capability
  contract，模型特有参数必须命名空间化且不能改变核心语义。
- 插件发现扩大本地代码执行面。v1 把显式信任定义为完整本地代码权限；digest、环境
  变量 allowlist、路径契约和进程监督不提供 OS 沙箱。开放第三方安装前 UI 必须明确
  展示这一授权边界；更弱权限模型需要独立安全设计。
- 已完成的迁移切片必须删除旧运行路径，避免状态和错误语义再次分叉。
- Kokoro 的成功只能证明 TTS/Python Runner 路径，不能证明 token streaming、视觉
  输入或实时双向音频。
- 插件协议一旦供第三方使用会形成兼容负担。v1 已由 fake Runner 和七个生产 Runner
  验证；在 Plugins 第三方安装路径落地前，对第三方作者的稳定性承诺保持克制。
