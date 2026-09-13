# Runner 推理被放弃时的实例回收

Status: implemented

Class: defect

Owner: this file

Related decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[Runner chat 流式 delta 无丢失缓冲](2026-09-12-runner-chat-stream-lossless-buffering.md)、[TTS 音色滚轮快速切换与即时试听](2026-09-13-tts-voice-wheel-selection.md)

Related specs: [`docs/specs/runner-protocol-v1.md`](../specs/runner-protocol-v1.md) §cancellation boundary

## Problem

Runner Protocol v1 没有 daemon→Runner cancel（见 spec §cancellation boundary）。当
daemon 侧的 infer 事件循环在消费到终态帧之前被放弃——HTTP 客户端断开会让 axum
直接 drop 处理器 future，`RunnerInstanceManager::infer` 的接收循环随之消失——Runner
子进程并不知情，继续完成该推理并把终态帧写进 stdout 管道。这些残留帧无人消费，
同实例的下一次推理会先读到它们，被 id 校验拒绝并整体失败：

```text
runner protocol violation: event id 'infer-3' does not match inference 'infer-4'
```

实测触发路径（2026-09-13 音色试听）：kokoro 首次合成约 39 秒，用户在合成期间
把滚轮转到下一个音色，GUI 取消上一路 in-flight HTTP 请求，下一个试听在 30.7 秒后
以上述错误失败，实例被整体重启。除错误本身，`active_requests` 计数也随 future
drop 泄漏（收尾递减不再执行），且报错文案把责任指向 Runner，而 Runner 完全按协议
行事。

## Decision

`RunnerInstanceManager::infer` 引入放弃看门狗（`InferAbandonGuard`）：事件循环消费到
任一终态帧（`result` / `error` / `cancelled`）时解除；在提前返回或整个 future 被
drop 时，把实例标记为 `alive = false`、清空 `loaded_model`、递减
`active_requests` 并写入真实原因（`inference abandoned before terminal frame …`），
同时输出 WARN 日志。

回收本身不复用新机制：实例 `alive = false` 后，既有的恢复路径接管——
`load_model` 发现快照不存活即 `shutdown_instance`（graceful shutdown 超时后
kill 进程组，杀掉仍在合成的孤儿计算），再重新 spawn + load。这与 deadline、
进程崩溃、protocol violation 走的是同一条回收路径，实例恢复行为保持单一。
`last_error` 经 `RunnerProvider::status()` 的 `reason` 暴露到 `/api/providers`
与 runtime 的 stale-handle WARN，运维可见的真实原因是"推理被放弃"，而不是
误导性的 protocol violation。

## Alternatives considered

### 后台排空（放弃后继续消费到终态帧）

保住实例与已完成的计算，但下一次请求必须排在被放弃的合成之后（实测 kokoro
约 30 秒），为用户已明确取消的工作排队；且放弃的推理继续占用 GPU。不采用。

### 跳过不匹配 id 的帧（下一次推理时排空残留）

读帧时丢弃 id 不匹配的事件直到匹配自己的请求。无法区分"孤儿帧"与 Runner
真实错乱，掩盖真实协议错误；等待孤儿完成的时间上限同样不可控。不采用。

### 实现 daemon→Runner cancel 帧

规范为 v2 预留的方向，需要独立 stdin writer、stdout dispatcher、worker 并发
接收与 HTTP/task abort 消费路径（spec §cancellation boundary 已列明），改动面
远超本缺陷。保留为未来工作。

## Consequences

- 客户端断开不再产生误导性的 protocol violation；下一次请求付出的代价是实例
  回收（graceful 尝试 + kill 上界约 2×`shutdown_seconds`，再加 spawn + load），
  而不是一次必失败的推理加同等回收。
- 被放弃的底层计算被杀死而不是完成——这修正了 spec 原先"不宣称底层计算已中止"
  语义下的隐性浪费：孤儿推理在 daemon 侧已无人等待，继续算完只会毒化管道。
- deadline 与 protocol violation 路径现在同样经看门狗标记（结果与原收尾一致，
  幂等），不再有"循环提前退出但实例仍标记存活"的缝隙。
- 已知边界一：`load` / `unload` 请求中途被放弃时，残留帧仍靠既有的
  UnexpectedMessage 错误路径在再下一次调用自愈（多花一次失败调用），未纳入
  看门狗；观察到真实故障再扩。
- 已知边界二：被 drop 请求的任务记录停留非终态直至 daemon 重启（有界任务
  历史），属展示层遗留，不在本记录范围。

## Verification

- `crates/ai-daemon/tests/runner_runtime_composition.rs` 新增
  `abandoned_inference_marks_instance_and_recovers_on_next_load`：在 accepted 帧
  消费后 abort 推理 future（等价客户端断开），断言实例立即不可用、原因含
  abandoned、status 不再 ready，随后 load 成功回收并恢复正常推理。
- 可证伪性已验证：stash 掉看门狗实现后该测试在
  `abandoned inference must mark the instance not alive` 处失败。
- `cargo fmt --all -- --check` 与 `cargo test --workspace` 通过。
