# Decision: Runner Provider 可用性与模型绑定解耦

Status: implemented

Class: fix

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[单文件 Script Runner 创建](2026-09-08-single-file-script-runner.md)、[Runner 环境 probe 工作目录对齐 package root](2026-09-13-runner-probe-package-cwd-fix.md)

## Problem

单文件 Script Runner 的第一个模型无法从 GUI 注册。用户视角：仓库里的模型目录被
detector 正确识别（`/api/models/inspect` 返回 `recognized`），但管理页的「注册并加载」
按钮始终禁用，提示"匹配的 Runner 环境尚未就绪"——而引擎环境实际早已 Ready。

根因是一个跨层的死锁：

- `RunnerProvider::status` 用 `bindings.first()` 的 environment 查询环境状态；bindings
  只在模型注册（`bind_model`/`bind_adhoc_model`）后存在。
- 脚本 Runner 的生成 manifest 没有 `[[models]]` 目录（模型由用户目录注册，而非
  manifest 预声明），因此装配后到首个模型注册前 `bindings` 恒为空，
  `available` 恒为 `false`，reason 为 "no Runner-backed model is bound"。
- GUI（`ModelsView.canRouteDirectory`）要求 `recognized && runnerAvailable` 才启用
  注册按钮。

于是：没有模型 → provider unavailable → 按钮禁用 → 无法注册模型。built-in Runner
不受影响，因为其 manifest 预声明了 `[[models]]`，bootstrap 即产生绑定。GUI 侧的
`inspect` 响应里 `runner_available=false` 与"环境 Ready"并存，正是该死锁的直接观测
（2026-09-13 日志与 API 证据）。

## Decision

`RunnerProvider::status` 的环境状态按 manifest `runtime.id` 查询（bindings 为空时经
既有的 `default_environment_id()`），不再依赖 bindings 首项。语义修正为：

- `available` = 该 Runner 的引擎环境已 Ready（与是否有绑定模型无关）；
- `ready` = available 且 worker 存活并驻留模型（不变）；
- 无绑定时 `reason` 仍为 "no Runner-backed model is bound"——它现在是 available
  provider 上对"为什么还不 ready"的诚实解释，而不是不可用原因。

这符合架构不变量 8（available / ready / resident 是不同状态）：环境就绪的 Runner
具备承载能力，是否已绑定模型是注册状态而非可用性。GUI 侧无需改动——死锁解除后
「注册并加载」按钮在"目录识别成功 + 环境就绪"时自然启用，即用户请求的"手动触发
识别注册"入口本就存在，只是被此缺陷禁用。

## Alternatives considered

**GUI 侧放开 runnerAvailable 门槛。** 按钮可用但 `/api/providers` 继续谎报
unavailable，违反不变量 8，且其他消费方（CLI、任务面）仍看到错误状态；否定。

**为脚本 Runner 生成带占位 `[[models]]` 的 manifest。** 在 manifest 层伪造绑定来
绕过症状，Profile 快照指向不存在的 artifact；否定。

**GUI 增加独立的"手动识别"扫描按钮。** 仓库行的识别与注册机制已存在且自动刷新；
新增入口不解决死锁本身，只绕开它；否定（本修复后原按钮即为目标入口）。

## Verification

- `tests/runner_runtime_composition.rs::unbound_provider_reports_available_once_environment_is_ready`：
  未绑定 provider 在环境安装前 `available=false`（reason 为绑定缺失），真实 uv 安装
  环境后 `available=true && ready=false`（reason 不变）——钉住死锁回归。
- 既有 `status_reflects_environment_phase_without_resident_worker` 等组合测试不变
  通过（fixture 路径均带绑定，语义不受影响）。
- `cargo fmt --all -- --check` 与 `cargo test --workspace` 通过（2026-09-13）。
- 用户路径：修复后重建 app，对任意"已识别 + 引擎就绪"的未注册目录，GUI 注册按钮
  应为启用状态。

## Consequences

- 装配即就绪的 Runner provider（即使零模型）现在在 `/api/providers`、`/api/runners`
  与 GUI 中显示为可用；"无绑定模型"作为 reason 呈现，不再伪装成不可用。
- GUI 管理页对目录模型的标准注册路径恢复为设计意图：仓库行自动识别 →
  「注册并加载」→ routing token 注册。
- 排查同类问题时注意：`runner_available=false` 现在只表示环境未就绪，模型缺失
  一律以 "no Runner-backed model is bound" reason 表达。
