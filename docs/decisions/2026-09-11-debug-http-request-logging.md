# Debug 模式记录 daemon HTTP 请求结果

Status: implemented

Class: diagnostics

Owner: this file

## Problem

daemon 只在部分业务入口和内部故障处写日志。无效 JSON、路由失败及多数由 `api_error` 返回的 4xx/5xx 响应缺少统一记录，Debug 日志无法还原请求是否到达 daemon 以及最终 HTTP 结果。

## Decision

在 daemon Router 的统一入口安装 HTTP 日志 middleware。Debug 级别为每个请求生成进程内递增的 `request_id`，分别记录请求开始和响应完成；两条记录都包含方法与路径，完成记录同时包含 HTTP 状态码和毫秒耗时。

日志不包含查询参数、请求体、响应体、Header、Prompt、模型输出或 API Token。Info 级别不输出这些逐请求记录。已进入推理任务的 Chat、STT 和 TTS 请求继续由任务历史保存其既有详情和错误。

## Alternatives considered

- **在每个 handler 分别记录**：容易遗漏 Axum extractor、404 和 405 等 handler 外失败，并产生重复代码。
- **记录请求及响应正文**：诊断信息更完整，但会把模型上下文、输出或凭据带入持久日志，违反数据最小化边界。
- **引入 HTTP tracing 依赖**：当前需求只需要固定的开始/结果字段，Axum 自带 middleware 已足够。

## Consequences

- Debug 日志覆盖成功响应和可形成 HTTP 响应的失败请求，并可通过 `request_id` 关联开始与结果。
- 请求若在响应形成前被进程终止，日志只保留开始记录。
- 日志可用于 HTTP 级诊断，不构成请求正文审计记录。

## Verification

- `cargo fmt --all -- --check` 通过。
- 以 `RUST_LOG=debug` 在临时目录和端口运行 daemon：`GET /health` 返回 200、畸形 JSON 的 `POST /v1/chat/completions` 返回 400、`GET /missing` 返回 404；三者均生成使用同一 `request_id` 的请求开始与响应完成日志，完成日志包含状态码和耗时。
- `cargo test -p ai-daemon` 编译通过并执行 100 项测试，其中 99 项通过，既有 `tests::script_runner_create_locks_probes_and_persists_trust` 因当前环境返回 503 而失败；单独复跑结果相同。该失败发生在 Script Runner 环境准备路径，与 HTTP 日志 middleware 无关。
