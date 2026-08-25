# MacAIConsole 任务记录页 Implementation Plan

> **For Hermes:** Implement this plan task-by-task. Do not commit unless the user explicitly asks.

**Goal:** 在 MacAIConsole 侧边栏新增「任务」页面，实时展示 aiworkd 当前执行中的推理任务与最近完成的任务，并可点开查看输入、输出、耗时和错误详情。

**Architecture:** aiworkd 新增一个内存型、有容量上限的任务注册表，作为任务状态和 `active_requests` 的唯一事实来源；管理 API 提供轻量列表与按 ID 获取详情两个端点。MacAIConsole 继续复用现有 2 秒轮询，任务详情打开后对运行中任务单独进行 1 秒轮询。V1 只记录 LLM 对话、STT 转写、TTS 合成三类推理请求，不记录 health、模型查询、加载/卸载和下载操作。

**Tech Stack:** Rust 2021、Axum、Serde、Swift 5.9、SwiftUI/macOS 14、Swift Observation。

---

## 1. 已确认的现状

- 导航只有「运行状态」「模型管理」，入口在 `apps/MacAIConsole/Sources/MacAIConsole/Views/RootView.swift:3-25`。
- GUI 每 2 秒通过 `DaemonController.refresh()` 拉取 `/api/runtime`、`/v1/models`、`/api/providers`，见 `apps/MacAIConsole/Sources/MacAIConsole/DaemonController.swift:102-109,234-241`。
- 运行状态页目前只显示 `active_requests` 数量，无法列出具体请求，见 `apps/MacAIConsole/Sources/MacAIConsole/Views/RuntimeStatusView.swift:58-71`。
- aiworkd 当前只有 `AtomicU64 active_requests` 和 RAII `RequestGuard`，没有任务元数据、结果和历史，见 `crates/ai-daemon/src/runtime.rs:37-68,624-629`。
- Chat 已生成 `req_*` ID 但只写日志；STT 的 ID 只用于临时文件名；TTS 没有任务 ID，见 `crates/ai-daemon/src/main.rs:565-607,610-740`。
- Chat 流式响应的 guard 会随流结束或客户端断开释放，这个生命周期语义必须保留。
- 当前 Swift Package 没有测试 target，见 `apps/MacAIConsole/Package.swift:4-14`。

## 2. 推荐的 V1 边界

### 2.1 纳入记录

1. `POST /v1/chat/completions`：LLM 对话生成。
2. `POST /v1/audio/transcriptions`：STT 语音识别。
3. `POST /v1/audio/speech`：TTS 语音合成。
4. 有效请求进入推理路径后，无论成功、失败或客户端中断，都生成一条任务记录。

### 2.2 暂不纳入

- `/health`、`/v1/models`、`/api/runtime` 等只读管理请求。
- 模型注册、加载、卸载、改名、下载。
- 排队位置、百分比进度和取消按钮；当前 Provider/worker 协议尚未提供可靠的队列和取消能力。
- 跨 daemon 重启的长期任务审计。

### 2.3 历史保留策略

- 所有运行中任务均保留，不参与淘汰。
- 仅保留最近 **100 条**终态任务，按完成时间倒序；新增第 101 条时淘汰最旧记录。
- 历史只存在 aiworkd 内存中，daemon 重启后清空。
- 每条任务的请求文本和结果文本分别最多保留 **64 KiB**；超限时截断并设置 `request_truncated` / `result_truncated`。
- 不保存上传的原始音频和 TTS 生成的音频字节，只保存文件名、大小、MIME/格式和结果字节数。

这样可以先满足“看当前任务和最近完成内容”，同时避免 SQLite 迁移、无限磁盘增长和敏感提示词长期落盘。若后续确认需要跨重启审计，再单独设计持久化、清理周期和隐私开关。

## 3. 页面信息架构

### 3.1 侧边栏

在「运行状态」与「模型管理」之间增加：

- 标题：`任务`
- SF Symbol：`list.bullet.rectangle.portrait`
- 页面标题：`任务记录`

### 3.2 正在运行

区块标题：`正在运行 · N`

每行展示：

- 类型图标与名称：对话生成 / 语音识别 / 语音合成。
- 输入摘要：Chat 取最后一条 user 消息，STT 取原始文件名，TTS 取输入文本；单行最多约 120 字符。
- 模型 ID；有值时同时显示 Provider。
- 旋转中的 `ProgressView` 与状态 `运行中`。
- 已运行时长，使用等宽数字并随轮询更新。
- 整行悬停高亮，点击打开详情。

空状态：`当前没有正在运行的任务`。

### 3.3 最近完成

区块标题：`最近完成 · N`

每行展示：

- 与运行中相同的类型、摘要、模型信息。
- 状态徽标：成功（绿）、失败（红）、已中断（灰/橙）。
- 总耗时。
- 完成时间使用相对时间，例如“2 分钟前”。
- 最新完成的任务排在最上方。

空状态：`当前 aiworkd 会话中还没有已完成任务`。

### 3.4 任务详情 Sheet

沿用 Running Models 的交互方式：任务行点击后以 sheet 打开，避免引入第三列导航。

**固定头部**

- 类型名称、状态徽标、运行中进度。
- 任务 ID（等宽字体）与复制按钮。

**基本信息**

- 模型 ID。
- Provider（若任务在 Provider 解析前失败则显示 `—`）。
- 开始时间。
- 结束时间或 `仍在运行`。
- 已运行/总耗时。

**输入内容**

- Chat：按 role 展示完整消息列表；附 stream、temperature、max_tokens。
- STT：原始文件名、文件大小、请求语言、响应格式。
- TTS：完整输入文本、音色、格式、语速。
- 文本均允许选择复制；被截断时明确显示 `内容过长，仅保留前 64 KiB`。

**输出内容**

- Chat：当前已生成/最终 assistant 文本、finish reason、token usage（可用时）。
- STT：转写文本、识别语言。
- TTS：content type、生成字节数；不在历史中保留音频本体。
- 流式 Chat 运行时显示已收集的部分输出，每 1 秒刷新；进入终态后停止详情轮询。

**错误信息**

- 失败任务显示标准化错误消息。
- 流式连接提前断开显示 `客户端连接已中断`，保留此前已生成的部分输出。

## 4. 管理 API 契约

### 4.1 列表

`GET /api/tasks?completed_limit=100`

列表只返回摘要，避免 GUI 每 2 秒重复拉取完整提示词和结果。

```json
{
  "running": [
    {
      "id": "req_1787670000000_3",
      "kind": "chat",
      "status": "running",
      "model": "qwen3",
      "provider": "llama.cpp",
      "input_preview": "请总结这段内容……",
      "started_at_ms": 1787670000000,
      "completed_at_ms": null,
      "duration_ms": null,
      "error": null
    }
  ],
  "completed": []
}
```

- `completed_limit` 服务端限制在 `1...100`；缺省 100。
- `running` 返回全部运行中任务。
- 未知 `kind`/`status` 必须能被 Swift 客户端作为普通字符串保留，不能造成整组解码失败。

### 4.2 详情

`GET /api/tasks/{id}`

```json
{
  "id": "req_1787670000000_3",
  "kind": "chat",
  "status": "succeeded",
  "model": "qwen3",
  "provider": "llama.cpp",
  "started_at_ms": 1787670000000,
  "completed_at_ms": 1787670002418,
  "duration_ms": 2418,
  "request": {
    "messages": [{"role": "user", "content": "请总结这段内容……"}],
    "input_text": null,
    "file_name": null,
    "file_size_bytes": null,
    "language": null,
    "voice": null,
    "format": null,
    "speed": null,
    "stream": true,
    "temperature": 0.7,
    "max_tokens": 512
  },
  "result": {
    "output_text": "……",
    "language": null,
    "finish_reason": "stop",
    "prompt_tokens": null,
    "completion_tokens": null,
    "total_tokens": null,
    "content_type": null,
    "byte_count": null
  },
  "request_truncated": false,
  "result_truncated": false,
  "error": null
}
```

- 不存在或已被历史容量淘汰时返回标准 404 `ApiErrorBody`。
- 时间戳统一使用 Unix 毫秒；列表与详情字段一致。

## 5. 实现步骤

### Task 1: 定义任务 DTO 与内存注册表

**Objective:** 建立单一任务状态源，替代只有计数的 `RequestGuard`。

**Files:**
- Create: `crates/ai-daemon/src/tasks.rs`
- Modify: `crates/ai-daemon/src/main.rs:6-11`
- Modify: `crates/ai-daemon/src/runtime.rs:37-68,160-174,586-629,757-802`
- Modify: `crates/ai-core/src/response.rs`
- Modify: `crates/ai-core/src/lib.rs`

**Steps:**

1. 在 `ai-core` 定义可序列化的 `TaskSummary`、`TaskDetail`、`TaskRequestDetail`、`TaskResultDetail`、`TaskListResponse`；`kind` 和 `status` 使用稳定 snake_case 字符串值。
2. 在 `tasks.rs` 实现 `TaskRegistry`：运行中使用 `HashMap`，终态使用容量 100 的 `VecDeque`。
3. 实现 `TaskHandle` 生命周期：`succeed`、`fail`、追加流式输出；仍为 running 时 Drop 自动转为 `cancelled`。
4. 实现文本容量限制、摘要生成和终态幂等保护，防止同一任务重复结束。
5. 将 `RuntimeInfo.active_requests` 改为 `TaskRegistry.running_count()`，删除 `AtomicU64 active_requests` 与旧 `RequestGuard`，保留 `ModelLease`。

**Tests:**

- 新任务进入 running，成功后移入 completed。
- 失败记录错误。
- 未显式结束的 handle Drop 后变为 cancelled。
- 第 101 条终态记录淘汰最旧一条，运行中任务不淘汰。
- 文本超过 64 KiB 时被截断且标记正确。
- `active_requests` 与 running 数量一致。

### Task 2: 接入 Chat / STT / TTS 生命周期

**Objective:** 三条真实推理路径都产生准确任务记录。

**Files:**
- Modify: `crates/ai-daemon/src/main.rs:565-740`
- Modify: `crates/ai-daemon/src/runtime.rs:457-515`

**Steps:**

1. Chat 在 JSON 成功解码后创建 task，记录 messages、stream、temperature、max_tokens。
2. Provider/model 查找失败时，把同一 task 标为 failed 后返回现有 API 错误。
3. 非流式 Chat 成功时记录 assistant 输出、finish reason 和 usage。
4. 流式 Chat 使用现有 `async-stream` 依赖逐 chunk 累积文本：正常结束标为 succeeded；中途 Provider 错误标为 failed；客户端断开依靠 handle Drop 标为 cancelled。
5. STT 在 multipart 字段全部校验通过后创建 task，只记录原始文件名和大小，不暴露临时文件路径或音频字节。
6. TTS 在默认 model 规范化后创建 task，成功只记录输出元数据，不复制音频到任务历史。
7. 移除 `transcribe` / `synthesize` 内旧 request guard，避免双计数。

**Tests:**

- Mock 非流式 Chat 产生 succeeded 任务和 usage。
- Mock 流式 Chat 正常完成后任务为 succeeded，输出可重建。
- 主动提前 drop 流后任务为 cancelled。
- 不存在模型的有效请求产生 failed 任务。

### Task 3: 暴露任务管理 API

**Objective:** GUI 能轻量拉列表并按需读取详情。

**Files:**
- Modify: `crates/ai-daemon/src/main.rs:17-23,77-95,161-173`

**Steps:**

1. 增加 `GET /api/tasks` 路由和 `completed_limit` 查询参数。
2. 增加 `GET /api/tasks/{id}` 路由。
3. 对 limit 做 `1...100` 收敛；running 永远返回全部。
4. 详情不存在时复用 `api_error(AIError::ModelNotFound, ...)` 不合适，应增加/选用通用 not-found 错误语义并保持标准 `ApiErrorBody`。
5. 为 JSON 排序、字段名、404 和上限行为增加 handler/registry 测试。

### Task 4: 增加 Swift DTO、客户端与解码测试

**Objective:** 固定 Rust/Swift API 契约，防止字段名不一致导致列表静默为空。

**Files:**
- Modify: `apps/MacAIConsole/Sources/MacAIConsole/DaemonAPI.swift`
- Modify: `apps/MacAIConsole/Package.swift`
- Create: `apps/MacAIConsole/Tests/MacAIConsoleTests/TaskDecodingTests.swift`

**Steps:**

1. 定义 `InferenceTaskSummary`、`InferenceTaskDetail`、request/result DTO；`kind`、`status` 保持 `String` 并通过 computed properties 映射显示文案。
2. 增加 `tasks(completedLimit:)` 与 `task(id:)`。
3. 测试三种 kind、四种状态、nil 可选字段、毫秒时间戳、错误详情和未知 kind/status。
4. 解码错误必须抛出给调用方，不使用会吞掉根因的 `try?` 作为客户端 API 实现。

### Task 5: 将任务数据接入全局轮询

**Objective:** 任务列表随现有 daemon 状态机刷新，离线时正确清空。

**Files:**
- Modify: `apps/MacAIConsole/Sources/MacAIConsole/DaemonController.swift:20-33,102-109,199-254`

**Steps:**

1. 增加只读状态 `runningTasks`、`completedTasks`。
2. `refresh()` 使用 `async let` 并行获取 runtime、models、providers、tasks，避免新增端点让 2 秒轮询串行变慢。
3. runtime 获取失败仍作为连接失败；tasks 获取失败保留现有列表并设置可见错误，不影响模型数据。
4. daemon 离线/停止时清空任务数组，因为 V1 历史只属于当前 daemon 会话。
5. 保证运行状态页 `active_requests` 与任务页 running 数量来自同一后端状态源。

### Task 6: 构建任务列表页与详情页

**Objective:** 完成用户可见页面和点开详情交互。

**Files:**
- Create: `apps/MacAIConsole/Sources/MacAIConsole/Views/TasksView.swift`
- Modify: `apps/MacAIConsole/Sources/MacAIConsole/Views/RootView.swift:3-25`
- Modify: `apps/MacAIConsole/Sources/MacAIConsole/Format.swift`

**Steps:**

1. 给 `Page` 增加 `.tasks`，侧边栏加入「任务」。
2. `TasksView` 用两个 GroupBox 展示「正在运行」和「最近完成」，复用现有 GroupBox、悬停高亮、克制状态色和行高。
3. 实现 task kind 图标、状态 badge、输入摘要、模型/Provider、耗时和相对时间。
4. 行点击通过 `.sheet(item:)` 打开详情，保留整行 `contentShape` 与 hover 动画。
5. 详情页按类型渲染输入/输出；所有长文本开启 `.textSelection(.enabled)`，不把任意模型输出当 Markdown/HTML 执行。
6. 运行中详情每 1 秒读取 `/api/tasks/{id}`；进入 succeeded/failed/cancelled 后停止轮询。
7. `Format` 增加毫秒耗时、Unix 毫秒绝对时间与相对时间格式化。
8. 离线、空列表、详情被淘汰/404 都给出明确状态，不显示无限 loading。

### Task 7: 文档与完整验证

**Objective:** 用真实构建和请求证明任务状态、详情和 UI 契约工作。

**Files:**
- Modify: `README.md:38-48`

**Verification commands:**

1. `cargo fmt --check`
2. `cargo test --workspace`
3. `cargo build --release -p ai-daemon`
4. `cd apps/MacAIConsole && swift test`
5. `cd apps/MacAIConsole && swift build -c debug`
6. `cd apps/MacAIConsole && scripts/build-app.sh release`
7. `plutil -lint apps/MacAIConsole/build/MacAIConsole.app/Contents/Info.plist`
8. `codesign --verify --deep --strict apps/MacAIConsole/build/MacAIConsole.app`

**Manual API checks:**

1. `GET /api/tasks` 初始返回两个数组。
2. 发起一个成功的 Chat/STT/TTS 请求后，列表出现 succeeded，详情输入/输出正确。
3. 对不存在模型发起有效请求，列表出现 failed，详情保留错误消息。
4. 发起流式 Chat，在生成中读取列表能看到 running；正常完成后转 succeeded。
5. 流生成中断开客户端，任务转 cancelled，并保留部分输出。
6. `/api/runtime.active_requests == /api/tasks.running.count`。
7. 连续产生超过 100 条终态记录，completed 保持 100 条且最旧记录被淘汰。

**Visual checks:**

- 任务页面在常用窗口宽度下不出现状态文字换行。
- 行悬停、点击和 sheet 动画与 Running Models 一致。
- 长提示词/转写结果不会撑破 sheet，文本可滚动、可复制。
- 浅色/深色模式均使用系统背景，不引入 `underPageBackgroundColor`。

## 6. 验收标准

- 侧边栏出现可进入的「任务」页面。
- 页面同时区分运行中和最近完成任务，并正确处理空状态、离线状态。
- Chat、STT、TTS 的成功、失败和中断均有记录。
- 点击任意任务能查看该类型对应的完整可用信息。
- 流式 Chat 详情在运行时能看到部分输出，结束后状态可靠进入终态。
- 不保存音频二进制，不无限保留文本，不把模型输出作为可执行富文本渲染。
- 运行状态页的活跃请求数字与任务页数量一致。
- Rust workspace 测试、Swift 解码测试、Debug/Release 构建、应用签名验证全部通过。

## 7. 后续可选增强（不进入本次实现）

- 将模型下载、加载、卸载纳入 `kind=model_pull/model_load/model_unload`。
- 增加取消按钮和 queued 状态；前提是 worker 协议提供真实 cancel acknowledgement。
- SQLite 持久化、按天清理、清空历史和隐私开关。
- 按类型/状态筛选、搜索、导出 JSON。
- Provider 暴露可靠进度后显示 STT/TTS/下载百分比。
