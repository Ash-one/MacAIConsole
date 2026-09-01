# MacAIConsole

MacAIConsole 是 MacAI 的原生 macOS 控制台。应用只通过本地 HTTP API 与 `aiworkd` 通信，不直接运行模型推理。

## 当前页面

- 运行状态：版本、PID、内存预算、系统内存压力、活跃请求和已加载模型
- 任务：当前运行及最近完成的 Chat / STT / TTS 请求，支持查看输入、输出、耗时和错误
- 模型管理：按 LLM / STT / TTS 分组显示模型仓库和注册状态，支持加载、卸载、改名与删除注册
- 日志：查看 GUI 与 daemon 最近日志，默认显示 Info，可启用 Debug，并按日志级别着色；"在访达中显示"可打开日志目录
- 设置：应用外观（跟随系统 / 明亮 / 暗黑）、自动拉起守护进程开关、内存预算、Python 运行环境、网络代理和模型下载源
  - 模型下载源可切换 Hugging Face 官方源、`hf-mirror.com` 或自定义 Hugging Face 兼容源；修改后重启 aiworkd 生效
  - Python 运行环境：一键安装 Qwen3-ASR（MLX / PyTorch）与 Kokoro TTS worker 所需的 Python 3.12 venv 及固定版本依赖（仓库 `.build/` 下，与 daemon 探测路径一致），支持取消与失败重试
- 模型管理中的推荐模型在 Provider 环境未就绪时，行内会显示 daemon 上报的具体原因，并提供「安装运行环境」入口

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

GUI 可以连接已经运行的 daemon。GUI 自动启动时按顺序探测 `AIWORKD_PATH` 环境变量、仓库 `target/release/aiworkd` 与 `target/debug/aiworkd`。

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
POST /api/models/pull
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
