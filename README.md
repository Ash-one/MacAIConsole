# MacAI Workbench Runtime Prototype

这是 `handoff.md` 第 62 节首个开发任务的可运行原型。它验证同一个本地 Runtime 同时服务 CLI 与 OpenAI-compatible HTTP 客户端的核心链路：

```text
ai CLI ─┐
        ├─ HTTP ─> aiworkd ─> MockProvider
OpenAI ─┘
```

## 当前实现

- Rust workspace：`ai-core`、`ai-daemon`、`ai-cli`
- `aiworkd` daemon，默认只监听 `127.0.0.1:11435`
- Provider / ChatProvider / STTProvider / TTSProvider 抽象
- MockProvider：回显最后一条 user 消息
- OpenAI-compatible endpoints：
  - `GET /health`
  - `GET /v1/models`
  - `POST /v1/chat/completions`
  - `GET /api/runtime`
- SSE 流式输出与 `[DONE]` 终止事件
- 统一 API 错误结构
- 活跃请求计数；流结束或客户端断开时自动释放
- CLI：`status`、`list`、`chat`、`run`、`ps`、`serve`

## 构建与验证

需要 Rust stable toolchain。

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
```

## 启动

终端 1：

```bash
cargo run -p ai-daemon --bin aiworkd
```

终端 2：

```bash
cargo run -p ai-cli --bin ai -- status
cargo run -p ai-cli --bin ai -- list
cargo run -p ai-cli --bin ai -- chat mock hello
cargo run -p ai-cli --bin ai -- run mock
cargo run -p ai-cli --bin ai -- ps
```

构建后也可直接使用：

```bash
./target/debug/aiworkd
./target/debug/ai chat mock hello
```

预期输出：

```text
hello
```

`ai models` 保留为 `ai list` 的兼容别名。

## HTTP API

非流式：

```bash
curl http://127.0.0.1:11435/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "mock",
    "messages": [{"role": "user", "content": "hello"}]
  }'
```

流式：

```bash
curl --no-buffer http://127.0.0.1:11435/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "mock",
    "messages": [{"role": "user", "content": "hello"}],
    "stream": true
  }'
```

## 原型边界

当前版本以 MockProvider 验证 Runtime、API、CLI 与 streaming 架构。真实 llama.cpp / MLX provider、SQLite model registry、下载、STT、TTS、内存调度和 SwiftUI GUI 属于后续里程碑。

完整产品与架构说明见 [`handoff.md`](handoff.md)。
