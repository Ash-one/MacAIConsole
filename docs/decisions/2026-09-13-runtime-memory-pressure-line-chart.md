# 运行状态页系统内存压力实时折线图

Status: implemented

Class: feature

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)（内存调度与 Worker RSS 采样归 daemon runtime 所有）、[管理页引擎状态置顶与推荐条目精简](2026-09-07-management-view-engine-status-and-compact-recommendations.md)（GUI 信息密度精简先例）

## Problem

运行状态页对"系统内存压力"的旧展示是一个瞬时进度条：每次轮询用最新读数整体覆盖上一帧。这回答不了运行状态页最核心的问题——

- 加载或卸载模型、跑长任务时，系统内存压力朝哪个方向走？现值 62% 是正在回落还是正在爬升？
- 距离 MacAI 内存预算红线还有多远、以什么速度逼近？预算值孤立地摆在 `MemoryBudgetCard` 里，与压力读数没有任何空间上的对照。
- 压力冲顶的时刻发生在用户看屏幕之前还是正在发生？

daemon 与 GUI 原本都没有任何时间序列设施：`memory_used / memory_total` 在每次 `/api/runtime` 请求时由 sysinfo 现算（`crates/ai-daemon/src/runtime.rs` 的 `runtime_info`、`crates/ai-daemon/src/scheduler.rs`），GUI 侧 `refresh()` 每轮直接覆盖 `info`，无历史样本。任何"把瞬时读数变成可判断趋势"的展示，都需要某个持有者积累样本序列。

## Decision

范围：仅 GUI 展示层（MacAIConsole），daemon 与 wire 契约零改动。样本直接来自现有 `/api/runtime` 响应中的 `memory_budget / memory_total / memory_used`（`crates/ai-core/src/response.rs` 的 `RuntimeInfo`），不新增端点、不改字段、不加 daemon 周期任务。

### 1. 指标与口径

沿用旧压力条的口径：占用率 = `memory_used / memory_total`（sysinfo 系统物理内存 used/total），即 README「系统内存压力」一直指向的事实。图表由两部分构成：

- **主折线 + 面积填充**：占用率随时间变化的折线，折线下方至 0 基线的面积以纵向渐变着色（线色 opacity 0.32 → 0.04，延续旧压力条的渐变表现）；线与面积的颜色随最新样本所在阈值区间变化（<0.6 success、<0.85 warning、≥0.85 danger，沿用旧压力条规则）。
- **当前值 accessory**：标题行右侧保留 `used / total · 百分比` 读数，在线时与折线末端同源（同一次刷新）；daemon 离线后回退显示最近样本百分比。

图内不叠加内存预算参考线（首版曾提供，评审裁撤，见 Consequences）；预算读数仍由页面 `MemoryBudgetCard` 统计卡承载。

### 2. 采样归属与缓冲

`apps/MacAIConsole/Sources/MacAIConsole/MemoryPressureRecorder.swift` 持有纯逻辑，`DaemonController` 持有缓冲：

- 采样挂在 `DaemonController.refresh()` 成功路径（`recordMemoryPressure()`）：`runtimeInfo()` 成功后 `record` 一个样本。`DaemonController` 是全应用唯一轮询者（在线 2.0s），采样随之获得唯一节拍源——无第二个定时器、无平行刷新路径。`runtimeInfo` 失败的轮询整轮 throw，自然不出样本。
- `MemoryPressureSample { timestamp, usedFraction }`；`used` 缺席或 `total <= 0` 不采样，缺口不插值、不回填。
- 环形缓冲容量 300（2 倍余量容忍节拍退化），显示窗口取相对最新样本回看的 300 秒。窗口以最新样本而非墙钟为基准：daemon 离线后曲线停在末样本处，而不是被逐渐清空。
- 缓冲归 `DaemonController`（App 生命周期内持久），切页返回历史保留；`finishStop()` 不清空——占用率是宿主机事实，跨 daemon 重启依旧真实，离线断档由时间戳缺口呈现。不落盘。
- `MemoryPressureRecorder` 为 `@ObservationIgnored`，镜像到可观察属性 `memorySamples` 触发视图刷新。

### 3. 渲染

`RuntimeStatusView.swift` 中的 `MemoryPressureCard` 用 Swift Charts（系统框架，`import Charts`，`Package.swift` 零改动）绘制：`AreaMark` 面积 + `LineMark` 折线，X 轴按时间戳自动刻度（分钟步进），Y 轴固定 0–100%。替换 `MemoryPressureBar` 成为"系统内存压力"卡片的唯一展示，同一事实不做条形/折线双渲染；样本为空时按 phase 显示"正在积累内存样本…/守护进程离线，暂无数据"空态，单样本时以 `PointMark` 呈现首点。不做逐点滚动动画——每 2s 平移动画会持续闪烁，静态重绘更符合监控读数场景。

### 4. 测试与文档

- `MemoryPressureRecorderTests` 以受控时钟覆盖：字节换算、缺失读数不采样、占用率钳制、容量淘汰、显示窗口切片、空/单样本。
- `README.md`「运行状态」条目同步为折线图口径。

## Alternatives considered

### daemon 侧采样循环 + 历史端点

daemon 内新增后台采样任务与历史端点，GUI 拉取整段历史。优点是历史独立于 GUI 进程、可做更长窗口；输在折线图是单页展示需求，为此在 daemon 引入第二个周期任务（当时仅 30s idle reaper）、新增管理面端点与窗口配置面，违反最小 owner 与低依赖倾向；历史要持久化才有跨 GUI 重启的价值，而该价值对 5 分钟窗口不成立。重新引入条件：出现 CLI 或其他客户端消费内存历史的真实第二消费者。

### SSE / WebSocket 推流

为 2s 一拍的展示数据引入长连接通道，重做 GUI 传输层。现有轮询是全部页面共享的成熟机制，推流收益为零。

### 引入第三方图表库

直接违反低依赖优先约定；macOS 14 自带 Swift Charts 覆盖需求。

### Canvas 自绘

可完全控制渲染，但需自建坐标轴、刻度、参考线，重写 Swift Charts 已提供的能力，增加无对应收益的维护面。

### 保留压力条与折线图并存

同一事实两处渲染，页面信息密度膨胀；精简先例支持单渲染。

### 折线叠加 MacAI 常驻内存第二序列

数据已在 wire 上（`loadedModels[].memory_usage_bytes` 求和），能回答"压力是 MacAI 造成的还是系统其他进程造成的"，成本低。作为后续扩展点保留而非默认内容：扩展只需给 `MemoryPressureSample` 加一个字段。

## Consequences

- **分辨率上限为轮询节拍**：采样与 2s 在线轮询耦合，亚秒级变化不可见。GUI 空闲 CPU 是既有约束（任务接口因此降到 10s），不为此加快轮询。
- **挂起后窗口渐进重建**：系统休眠/App Nap 长时间暂停后，恢复初期窗口内可能暂无样本，图表现为空态后逐步填充；时间戳保证横轴诚实，不回填伪数据。
- **口径易误读**：sysinfo 的 used/total 与活动监视器"内存压力"标签不同（不含 compressor 语义），且小模型的加载/卸载信号（亚 GB）常被系统级噪声淹没。重新引入条件：出现"used/total 高但系统未实际承压"误导用户的具体案例时，将真实内核 memory pressure（`vm_statistics64`）升格为独立的 daemon 采样决策。
- **图内无预算对照**：折线图不绘制内存预算参考线——首版提供过，评审裁撤；预算与压力的同图对照是有意放弃的能力，预算读数仍由 `MemoryBudgetCard` 承载。重新引入条件：用户需要趋势与预算红线的同图对照时，恢复 `MemoryPressureSample` 的预算字段并加回 `RuleMark`。
- **重绘成本**：约 150 点 `LineMark` 每 2s 重绘成本可忽略；显著加长窗口前需先评估降采样。
- 有意放弃：亚秒分辨率、跨会话持久历史、真实内核 pressure 指标、双序列（见备选与扩展点）。

## Verification

| 验收 | 直接证据 | 结果 |
|---|---|---|
| 缓冲逻辑确定性（换算/缺失/钳制/淘汰/窗口） | `cd apps/MacAIConsole && DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` | Passed：Executed 63 tests, with 0 failures（其中 `MemoryPressureRecorderTests` 6 项） |
| 旧压力条完整移除（负向） | `grep -rn MemoryPressureBar apps/MacAIConsole/Sources apps/MacAIConsole/Tests` | Passed：无残留引用 |
| 真实组合路径（daemon → 轮询 → 采样） | 实例日志 `~/Library/Application Support/MacAIConsole/logs/gui.log`：「已连接 aiworkd，PID 19583」仅在 `refresh()` 成功后打点，采样挂同一成功路径 | Inspected：新构建实例 17:46 连接成功，采样路径已执行 |
| 图表可见行为（面积填充/无预算线/配色/空态回退） | `ImageRenderer` 渲染真实 `MemoryPressureCard`（合成数据 62%→88% 爬升）为 PNG 人工检查：5 分钟窗口、折线下方渐变面积填充、无预算参考线、双轴刻度、≥85% danger 配色与 accessory「最近 88%」一致（渲染用临时测试跑完即删） | Passed |
| 数据管道实时性 | `GET /api/runtime` 实测 `memory_budget=8GiB / memory_total=16GiB / memory_used` 随系统浮动（78–81%）；`POST api/models/MiniCPM5-1B-MLX/load` → `state=ready` + worker RSS 0.74 GB → chat 200 → unload 200 | Passed（字段即图表数据源） |
| 带新 UI 的 .app 实例实时屏幕观察 | 宿主未授予 ZCode 屏幕录制/辅助功能权限，无法截图或读 AX 树 | Not run：需要用户在 GUI 中目视确认折线随轮询滚动（本机构造已具备全部数据条件） |
