# Runner Protocol v1

Status: draft

Contract owner: this file

Decision owner: [`2026-09-02-runner-plugin-architecture.md`](../decisions/2026-09-02-runner-plugin-architecture.md)

本文件定义 aiworkd 与受管 Runner 子进程之间的 wire contract。daemon 的
`ai-daemon::runners::protocol` 已实现 frame codec（长度前缀、上限与校验），
`supervisor` 已实现受监督子进程与 fake Runner 的全链路集成测试；既有
Provider/worker 不实现该协议，真实 Runner 迁移通过后才能把它改写为已实现协议。

## Goals

- 跨 Rust、Python、C++ 等语言实现；
- daemon 可监督进程、关联请求、取消、卸载和清理；
- 支持非流式和流式 capability；
- stdout 不被第三方库日志破坏；
- 错误可稳定映射到 MacAI 公共错误模型；
- status/RSS 查询不依赖推理 I/O 锁。

## Transport

首版 transport 是子进程 stdio：

- stdin：daemon → Runner frame；
- stdout：Runner → daemon frame；
- stderr：Runner 日志，UTF-8 文本，不属于协议；
- EOF：对端终止；
- daemon 是唯一进程 supervisor。

每个 frame 使用：

```text
4-byte unsigned big-endian payload length
N-byte UTF-8 JSON object
```

规则：

- 最大 frame 长度由 daemon 配置，v1 默认上限为 8 MiB
  （`DEFAULT_MAX_FRAME_BYTES`），超限即 protocol violation；
- JSON 顶层必须是 object；
- 未知必需字段、非法 UTF-8、越界长度和截断 payload 都是 protocol violation；
- stdout 中任何非 frame 字节都是 protocol violation；
- 大型音频、图像和其他二进制通过 daemon 授权的文件或后续 binary frame 扩展传递，
  v1 不使用 base64 承载大对象。

## Envelope

所有 frame 共享：

```json
{
  "protocol": "macai.runner.v1",
  "type": "infer",
  "id": "req-01J...",
  "instance_id": "inst-01J...",
  "payload": {}
}
```

- `protocol`：固定为 `macai.runner.v1`；
- `type`：frame 类型；
- `id`：命令和其事件的 correlation ID；
- `instance_id`：`load` 成功后由 daemon 指定，pre-load frame 可省略；
- `payload`：类型化内容；
- `extensions`：可选 object，key 必须使用 Runner ID 命名空间。

模型特有 extension 不得改变 capability 的成功、错误、取消和 cleanup 语义。

## Startup handshake

Runner 启动后必须在 boot deadline 内先发送 `hello`：

```json
{
  "protocol": "macai.runner.v1",
  "type": "hello",
  "id": "startup",
  "payload": {
    "runner_id": "org.macai.kokoro",
    "runner_version": "0.1.0",
    "protocol_versions": ["macai.runner.v1"],
    "capabilities": ["tts.v1"],
    "pid": 12345
  }
}
```

daemon 验证 manifest、实际 Runner identity 和协议交集后发送 `initialize`：

```json
{
  "protocol": "macai.runner.v1",
  "type": "initialize",
  "id": "startup",
  "payload": {
    "runner_id": "org.macai.kokoro",
    "log_level": "info",
    "temp_root": "/private/.../macai-runner-...",
    "network": false
  }
}
```

Runner 回 `initialized`。identity 不一致、无协议交集、超时或提前退出时，daemon
终止进程并将 Runner 标记 unavailable。

## Lifecycle commands

### load

daemon 发送规范化 Model Profile、只读 model root、adapter、capability 和允许的
配置。Runner 完成真实模型加载后发送 `loaded`；进度可发送 `progress`。

`loaded.payload` 至少包含：

- `model_id`；
- `effective_device`；
- `resident_bytes`（可得时）；
- `capabilities`；
- `limits`，如 `max_concurrency`。

收到 `loaded` 前，daemon 不得把模型报告为 ready。

### unload

Runner 停止接收新请求，等待或取消由 daemon 指定的请求，释放模型资源并返回
`unloaded`。daemon 只有在 lease/busy guard 允许时发送该命令。

### shutdown

Runner 释放全部实例和临时资源后发送 `shutdown_complete` 并退出。超出 grace period
时 daemon 终止进程并记录强制清理。

### health

Runner 返回 `healthy`，内容可含引擎内部状态。daemon 的基础 PID/RSS/status 快照
不得等待 `health` 或活动推理 I/O 锁。

## Inference

### infer

`infer.payload.capability` 选择已在 manifest 和 load 结果中声明的 capability；
`request` 遵守对应 Capability Contract。

Runner 首先发送 `accepted`，随后可发送任意数量 `progress`/`delta`/`metrics`，最后
恰好发送一个 terminal frame：

- `result`；
- `error`；
- `cancelled`。

terminal frame 后同一 ID 的其他输出是 protocol violation。

### cancel

daemon 发送 `cancel`，payload 指向 inference ID。Runner 应尽快停止计算；底层引擎
无法中止时，Runner 立即停止发布外部结果，并在计算退出后完成内部清理。

取消属于正常 terminal state，不映射为 backend crash。

## tts.v1

Kokoro 首版使用以下 request：

```json
{
  "capability": "tts.v1",
  "request": {
    "text": "你好",
    "voice": "zf_001",
    "speed": 1.0,
    "format": "wav",
    "language": "auto"
  },
  "output": {
    "directory": "/private/.../request-id",
    "allowed_extensions": ["wav"]
  }
}
```

成功结果：

```json
{
  "content_type": "audio/wav",
  "path": "/private/.../request-id/output.wav",
  "bytes": 123456,
  "duration_ms": 2500,
  "sample_rate": 24000
}
```

安全要求：

- daemon 为每个请求创建独立 output directory；
- Runner 只能在该目录创建输出；
- daemon canonicalize 返回路径并验证它仍位于授权目录内；
- daemon 读取或发送完成后清理输出；
- Runner 返回其他路径按 protocol/security violation 处理。

Kokoro adapter 的文本切分、G2P 和 voice 选择属于 Runner 行为；公共输入限制和 WAV
结果语义属于 `tts.v1`。

## Error frame

```json
{
  "protocol": "macai.runner.v1",
  "type": "error",
  "id": "req-01J...",
  "instance_id": "inst-01J...",
  "payload": {
    "code": "invalid_request",
    "message": "speech input must not be empty",
    "retryable": false,
    "details": {}
  }
}
```

v1 允许的稳定 code：

- `invalid_request`；
- `model_not_found`；
- `model_load_failed`；
- `provider_unavailable`；
- `timeout`；
- `cancelled`；
- `backend_crashed`；
- `internal`。

Runner 返回的未知 code 映射为 `internal`，原值只进入脱敏诊断 details。Runner 进程
退出、stdout 破帧和 EOF-before-terminal 由 daemon 生成 `backend_crashed`。

## Concurrency and ordering

- 不同 `id` 的事件可以交错；同一 `id` 的事件保持发送顺序；
- manifest 声明实例数和每实例最大并发；
- Runner 超出声明并发时返回明确 busy/unavailable 错误，daemon scheduler 应在正常
  路径避免触发；
- request ID 由 daemon 生成，Runner 不复用；
- daemon 在 terminal frame 后释放 request lease 和 output directory。

## Compatibility

- v1 对未知可选字段必须忽略；
- 新的必需语义通过新 capability 或 protocol major version 引入；
- Runner manifest 声明完整支持的 protocol/capability versions；
- handshake 只选择双方明确支持的版本；
- v1 在 fake Runner 与真实 Kokoro Runner 验证完成前不承诺第三方稳定性。

## Required evidence before implementation status

- codec 对 partial read、coalesced frames、oversize、truncated 和 invalid JSON 的单测；
- fake Runner 的 handshake/load/infer/cancel/unload/crash composition test；
- stderr 噪声不影响 stdout 协议；
- Kokoro 真实 WAV 结果、非法 output path 和临时目录清理测试；
- 推理活动期间 status 快照无 I/O 锁等待；
- daemon shutdown 后无孤儿 Runner 进程。
