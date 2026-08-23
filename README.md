# MacAI Workbench Runtime

这是 `handoff.md` 中 Local AI Runtime 架构的可运行实现。SwiftUI、CLI 与 OpenAI-compatible 客户端最终都连接同一个 Rust daemon：

```text
ai CLI ─┐
        ├─ HTTP ─> aiworkd ─┬─> MockProvider
OpenAI ─┘                   └─> llama.cpp worker ─> Metal / GGUF
```

## 当前实现

- Rust workspace：`ai-core`、`ai-daemon`、`ai-cli`
- `aiworkd` 默认只监听 `127.0.0.1:11435`
- Provider Registry 与统一 `ProviderDescriptor` / `ProviderStatus`
- MockProvider：内置回显测试路径
- LlamaCppProvider：管理持久 `llama-server` 子进程
- GGUF load / unload 与 Apple Silicon Metal offload
- 模型 lease / busy guard：推理期间 unload 返回 503，流结束或断开后自动释放
- stale handle 自恢复：worker 崩溃后下一次请求自动重建 `llama-server`
- 普通 Chat Completion 与逐 token SSE streaming
- OpenAI-compatible endpoints：
  - `GET /health`
  - `GET /v1/models`
  - `POST /v1/chat/completions`
- Runtime 管理 endpoints：
  - `GET /api/runtime`
  - `GET /api/providers`
  - `POST /api/models/load`
  - `POST /api/models/{id}/load`
  - `POST /api/models/{id}/unload`
- 统一 API / Provider 错误结构
- 活跃请求计数；流结束或客户端断开时自动释放
- CLI：`status`、`list`、`chat`、`load`、`unload`、`run`、`ps`、`serve`

## 构建与验证

需要：

- Apple Silicon Mac
- Rust stable toolchain
- CMake
- Apple Clang / Xcode Command Line Tools

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
```

## 构建 llama.cpp Worker

项目固定 llama.cpp commit：

```text
8086439a4cea94c71a5dfb8fe4ad1546aebd640f
```

运行：

```bash
./scripts/build-llama-server.sh
```

产物：

```text
.build/llama.cpp/bin/llama-server
```

该 server 只作为内部推理 worker，因此构建脚本关闭嵌入式 WebUI，并启用 Metal 与 Accelerate。`.build/` 不进入 Git。

也可使用已有二进制：

```bash
export AIWORK_LLAMA_SERVER=/absolute/path/to/llama-server
```

内部 worker 默认监听 `127.0.0.1:11436`，可通过 `AIWORK_LLAMA_PORT` 修改。端口被其他进程占用时，daemon 会明确报错，不会终止未知进程。

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
cargo run -p ai-cli --bin ai -- ps
```

构建后也可直接使用：

```bash
./target/debug/aiworkd
./target/debug/ai chat mock hello
```

## 运行 GGUF 模型

显式加载：

```bash
./target/debug/ai load /path/to/model.gguf \
  --id local-model \
  --context-length 4096

./target/debug/ai chat local-model "Hello"
./target/debug/ai ps
./target/debug/ai unload local-model
```

也可把 GGUF 路径直接交给 `run`。CLI 会自动注册并加载模型：

```bash
./target/debug/ai run /path/to/model.gguf
```

`ai models` 保留为 `ai list` 的兼容别名。

## HTTP API

非流式：

```bash
curl http://127.0.0.1:11435/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "local-model",
    "messages": [{"role": "user", "content": "hello"}],
    "stream": false
  }'
```

流式：

```bash
curl --no-buffer http://127.0.0.1:11435/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "local-model",
    "messages": [{"role": "user", "content": "hello"}],
    "stream": true
  }'
```

流式响应以 OpenAI-compatible `data: [DONE]` 结束。

## 当前边界

当前版本完成 Milestone 1 的真实 llama.cpp 端到端路径。SQLite model registry、Hugging Face 下载、MLX Provider、STT、TTS、内存预算/LRU/keep-alive 和 SwiftUI GUI 在后续里程碑实现。

完整产品与架构说明见 [`handoff.md`](handoff.md)。
