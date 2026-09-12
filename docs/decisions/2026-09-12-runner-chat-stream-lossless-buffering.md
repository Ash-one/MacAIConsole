# Runner chat 流式 delta 无丢失缓冲

Status: implemented

Class: defect

Owner: this file

Related decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[结构化推理输出契约](2026-09-12-structured-reasoning-output.md)

## Problem

`RunnerProvider::chat_stream` 用容量 8 的有界 mpsc channel 把 Runner delta 事件转发给 SSE 响应流，转发用 `try_send`：消费端（HTTP 客户端）读取慢于模型产出时 channel 满，chunk 被静默丢弃。后果是流式输出文本丢失且无任何错误或日志——调用方拿到的是残缺的最终回答或推理，违反"delta 顺序和文本均不丢失"的 chat.v1 桥接语义。

## Decision

转发缓冲改为 `tokio::sync::mpsc::unbounded_channel`，事件回调用 `send`（仅在接收端被丢弃时失败，此时丢弃正是客户端断开的正确语义）。

有界 channel 在这个位置不提供任何价值：Runner Protocol v1 没有流控，Runner adapter 无论 daemon 消费多快都按模型产出速度写 stdout，daemon 无法用缓冲上限"暂停"模型。有界缓冲的唯一实际效果就是把滞后转化为丢字。缓冲上界因此是单次请求的完整生成文本量，它由 manifest `timeouts.inference_seconds` 与注册规格 `max_tokens` 共同约束，且调度层的 lease / 单 flight 并发限制了同时缓冲的请求数。

回调运行在 `infer` 的异步事件循环内（`RunnerInstanceManager::infer` 逐帧 `receive` 后同步调用），不是独立线程，因此 `blocking_send`（在 async 上下文 panic）与阻塞等待（卡死 tokio worker）都不可行；无界缓冲是不改变 `infer` 回调签名前提下唯一无丢失方案。

## Alternatives considered

### 有界 channel + 满时阻塞

向 Runner 施加真实背压需要 `infer` 支持异步回调或独立的 stdout 读取线程，等于为当前没有流控需求的协议重构进程 I/O 模型；且阻塞运行时线程在单线程 runtime 下会与消费端死锁。

### 丢弃时计入指标 / 记日志

保留丢字、只让它可观测。对 chat 输出而言残缺文本与错误同样不可接受，还把恢复责任推给调用方；不采用。

### 改造 `infer` 为 async 回调或事件流

可以让转发 `send().await` 享有背压，但 `infer` 同时服务 TTS/STT 的同步回调路径，为 chat 一个消费点扩大共享协议面；在协议本身没有流控的前提下收益为零。

## Consequences

- 滞后的 SSE 客户端不再丢失 delta；单请求缓冲上界为该请求的生成文本总量（推理 + 最终回答，含 `reasoning_text` 通道）。
- 客户端断开时响应流被丢弃，后续 `send` 失败被忽略——这是唯一的预期失败路径，不产生错误或日志。
- 若未来 v1 协议引入流控（如 credit 窗口），应回到本记录用真实背压替换无界缓冲。

## Verification

- `cargo fmt --all -- --check` 通过；`cargo test -p ai-daemon` 通过，其中 `runner_runtime_composition.rs` 的 `chat_stream_maps_delta_events_and_result_usage` 覆盖该转发的流式顺序、文本聚合与 usage 终帧。
- 无丢失由结构保证：`send` 仅在接收端 dropped 时失败，不存在容量拒绝路径；不再有静默丢弃分支可供回归。
