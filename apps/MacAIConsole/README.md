# MacAIConsole

MacAIConsole 是 MacAI 的原生 macOS 控制台。应用只通过本地 HTTP API 与 `aiworkd` 通信，不直接运行模型推理。

## 当前页面

- 运行状态：版本、PID、内存预算、系统内存压力、活跃请求和已加载模型
- 任务：当前运行及最近完成的 Chat / STT / TTS 请求，支持查看输入、输出、耗时和错误
- 模型管理：按 LLM / STT / TTS 分组显示模型仓库和注册状态，支持加载、卸载、改名与删除注册
- 日志：查看 GUI 与 daemon 最近日志，默认显示 Info，可启用 Debug，并按日志级别着色
- 设置：daemon 路径、自动启动、内存预算和日志目录

运行状态中的模型条目可进入详细设置页，调整 keep-alive、LLM 上下文长度和 TTS 默认音色。

## 系统要求

- Apple Silicon Mac
- macOS 14 或更高版本
- 完整 Xcode 或兼容的 Swift 5.9 工具链
- 已构建的 `aiworkd`

## 构建

先在仓库根目录构建 daemon：

```bash
cargo build --release -p ai-daemon
```

再构建应用：

```bash
cd apps/MacAIConsole
scripts/build-app.sh release
```

产物：

```text
build/MacAIConsole.app
```

启动：

```bash
open build/MacAIConsole.app
```

也可以直接开发运行：

```bash
swift run
```

## 连接 aiworkd

默认 API 地址：

```text
http://127.0.0.1:11435
```

GUI 可以连接已经运行的 daemon。需要由 GUI 自动启动时，请在设置中选择 `target/release/aiworkd` 的实际路径。

主要管理 endpoints：

```text
GET  /health
GET  /api/runtime
GET  /api/providers
GET  /api/tasks
GET  /api/tasks/{id}
GET  /api/logging
POST /api/logging
GET  /v1/models
POST /api/models/load
POST /api/models/{id}/load
POST /api/models/{id}/unload
```

## 本地数据

```text
~/Library/Application Support/MacAIConsole/
├── Models/
│   ├── llm/
│   ├── stt/
│   └── tts/
├── models.db
├── model-settings.json
└── logs/
    ├── aiworkd.log
    └── gui.log
```

日志页面每两秒读取最近 500 行，单次最多读取 512 KiB。日志达到 5 MB 时轮换，并保留一份 `.1` 文件。

## 测试

```bash
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
  swift test --enable-xctest
```

仅选中 Command Line Tools 时，XCTest 可能无法定位 macOS SDK；显式设置完整 Xcode 的 `DEVELOPER_DIR` 可以避免该问题。
