# 结构化推理输出契约

Status: implemented

Class: feature

Owner: this file

Related decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[Runner Protocol v1](../specs/runner-protocol-v1.md)

## Problem

推理模型会生成内部推理与最终回答两类文本。MacAI 的 `chat.v1`、OpenAI-compatible Chat Completions 和任务历史原本只有一个 `content` / `output_text` 通道，Runner 只能把原始生成文本全部塞进最终回答。

这会让调用方无法可靠地区分推理与答案。对 generation prompt 预填 `<think>` 的模型，起始标签属于输入而不会出现在新生成 token 中，原始输出可能从推理正文开始、只包含 `</think>`，形成不可独立解析的残缺文本。MiniCPM5-2B-MLX 的运行历史复现了该行为。

问题不依赖 `<think>` 这一种表示：不同引擎可能直接提供结构化 reasoning delta，模型也可能使用其他边界标记。公共契约表达"推理"和"最终回答"两个通道，模型格式解释留在对应 Runner。

## Decision

### Public Chat Completions contract

`/v1/chat/completions` 保持 endpoint 与 `chat.v1` capability 版本不变：

- 非流式 `choices[].message` 的可选 `reasoning_content` 承载推理，`content` 只包含最终回答；
- 流式 `choices[].delta` 的可选 `reasoning_content` 与 `content` 分别按原生成顺序流出；
- 请求消息接受可选 `reasoning_content`，并兼容读取 `reasoning` 别名；daemon 对外序列化只使用 `reasoning_content`，避免双重 owner；
- 非推理模型在 wire 上省略 `reasoning_content`（含空值），现有纯 `content` 行为保持；
- `usage.completion_tokens` 继续统计模型生成的全部 token，包括推理和最终回答，不另造无法由所有后端可靠提供的 token 口径。

`reasoning_content` 是 OpenAI-compatible 扩展字段。现有客户端读取 `content` 时仍可工作，并会得到干净的最终回答；依赖推理文本内联于 `content` 的调用方需要改为读取新字段。

### Ownership and Runner protocol

模型输出格式由 Runner 解释：

- Runner Protocol v1 的 `delta.payload` 提供互不混合的可选 `reasoning_text` 与 `text` 通道；`result.payload` 允许聚合后的 `reasoning_text` 与 `text`。daemon 的 Runner bridge 只将两个通道映射为 `ChatChunkDelta`，不识别、补写或删除任何模型标签；
- llama.cpp Runner 向 llama-server 请求 `reasoning_format: "auto"`，优先消费其已解析的 `delta.reasoning_content`、兼容读取 `delta.reasoning`，`delta.content` 映射为 `text`；
- MLX-LM Runner 在 adapter 内持有无模型依赖的增量 `ThinkingStreamParser`。模板声明 `enable_thinking` 时显式启用并以 reasoning 状态作为初始状态——该状态同时覆盖 generation prompt 预填起始标签与模型自行生成 `<think>` 两种情况；parser 跨 delta 缓冲边界标记，闭合标记被拆成多段时不向任一公共文本字段泄漏，生成结束仍未闭合时已进入 reasoning 状态的内容全部归入 `reasoning_content`，最终 `content` 可为空；
- 不支持推理的 built-in Runner、Script Runner 和第三方 v1 Runner 继续只发送 `text`；v1 对未知可选字段的兼容规则承载该扩展。

### Task history and GUI

- `TaskResultDetail` 的可选 `reasoning_text` 与 `output_text` 独立保存；流式任务分别追加两个通道，非流式完成时一次保存两者；两个文本字段分别执行 UTF-8 安全的 64 KiB 截断，任务历史保持最近 100 条且不写入日志；
- 任务列表 preview 只使用最终回答，思考内容不进入摘要；
- MacAIConsole 任务详情在存在 reasoning 时单独显示"思考过程"，"输出内容"只显示最终回答；两块文本均可选择复制；
- 本决策不引入聊天界面、思考开关、reasoning effort、模型注册设置或 CLI 展示开关。

## Alternatives considered

### 保留原始 `<think>...</think>` 于 `content`

实现文件最少，但 generation prompt 预填会导致起始标签不属于生成结果，流式调用方还要自行处理跨 chunk 边界；非 `<think>` 模型也无法共用。该方案把模型语法泄漏为公共契约。

### 在 daemon 统一解析标签

可让 Runner 保持单文本通道，但违反 Runner 拥有模型特有适配的现有边界。daemon 会积累模型家族分支，并重复 llama-server 已有的 reasoning parser。

### 新增 `chat.v2`

结构化 reasoning 是可选字段扩展；不支持它的 v1 Runner 和客户端仍保持既有成功、错误、取消与纯文本语义。升级 capability major version 只会复制现有协议和注册路径。

### 同时输出 `reasoning` 与 `reasoning_content`

可覆盖更多当前框架字段名，但会形成两个等价输出 owner，并让流式客户端承担冲突合并。输入兼容别名已经覆盖迁移需要，输出固定 `reasoning_content`。

## Consequences

- 依赖内联推理文本的调用方会观察到 `content` 变短；发布说明需指出该行为变化。
- MLX parser 的初始状态只由模板是否声明 `enable_thinking` 决定，不检查渲染后 generation prompt 是否预填起始标记。预填场景由初始 reasoning 状态覆盖（MiniCPM5-2B-MLX 已实测）；若某模板声明 `enable_thinking`、渲染不预填且模型也不生成起始标签，全部输出会归入 `reasoning_content` 且 `content` 为空。该组合是声明的未验证边界——出现这样的真实模型时回到本记录修订，而不是加入猜测规则。
- 单任务最坏文本内存上限由约 64 KiB 增至约 128 KiB；最近 100 条任务的有界策略不变。若实际内存压力成为问题，再评估共享总预算，不提前引入复杂配额。
- `reasoning_content` 可能包含敏感上下文。它沿用现有任务输出的本地内存可见范围，不进入日志或磁盘；本决策不扩大网络监听和持久化边界。

仍然有意不做：Responses API reasoning items；`reasoning_effort`、启停 thinking 与每模型默认值；reasoning token 独立计数；CLI 或独立聊天 UI 的思考折叠交互；为未知模型自动猜测任意 reasoning 标签。

## Verification

- `cargo fmt --all -- --check` 通过；`cargo test --workspace` 通过（`runner_plugin_composition` 两例在全量并行下偶发失败为环境性 flake，单独重跑 6/6 通过）。
- `cargo test -p ai-core` 固化 wire 契约：请求 `reasoning` 别名只入不出、响应 `reasoning_content` 缺席时 wire 无该键、携带时固定 canonical 名。
- `runner_runtime_composition.rs` 的 fake Runner 用例固化流式顺序、非流式聚合与 usage 终帧；Runner 包 `uv lock --check` 与 pytest（llama.cpp 7 例、mlx-lm 8 例）通过。
- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` 57/57 通过，Swift 解码固化 `reasoningText` 分区。
- 真实路径（本机运行中 daemon + MiniCPM5-2B-MLX 流式请求）：max_tokens 截断未闭合时全部输出归入 `reasoning_content`、`content` 为空、无残留标签；正常完成时 reasoning（2131 字符）与最终回答（247 字符）分别从两个 delta 通道流出，两通道均无 think 标签，`/api/tasks/{id}` 两个文本字段与 SSE 聚合一致。打包 GUI（`scripts/build-app.sh release`）内的任务详情分区属于交付后的人工界面验证边界，其数据来源 `/api/tasks/{id}` 已实测。
- 根 `README.md`、`apps/MacAIConsole/README.md` 与 `docs/specs/runner-protocol-v1.md` 描述当前契约。
