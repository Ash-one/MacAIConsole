# MacAIConsole

MacAIConsole 是 MacAI 的原生 macOS 控制台。应用只通过本地 HTTP API 与 `aiworkd` 通信，不直接运行模型推理。

## 当前页面

- 运行状态：版本、PID、内存预算、系统内存压力、活跃请求和已加载模型
- 任务：当前运行及最近完成的 Chat / STT / TTS 请求，支持查看输入、输出、耗时和错误
- 管理：页面顶端置顶展示 Runner 引擎环境状态与一键安装控制（按 LLM、STT、TTS 顺序分类排列）；推荐目录来自 daemon `/api/model-profiles`，本地目录经 `/api/models/inspect` 由 daemon 路由；GUI 只提交 Profile ID 或 routing token，按 LLM / STT / TTS 分组显示仓库和注册状态，支持加载、卸载、改名与删除注册；GGUF 会读取展示 metadata
- 日志：查看 GUI 与 daemon 最近日志，默认显示 Info，可启用 Debug，并按日志级别着色；"在访达中显示"可打开日志目录
- 设置：应用外观（跟随系统 / 明亮 / 暗黑）、自动拉起守护进程开关、内存预算、网络代理和模型下载源
  - 模型下载源可切换 Hugging Face 官方源、`hf-mirror.com` 或自定义 Hugging Face 兼容源；修改后重启 aiworkd 生效
  - 引擎：daemon `/api/runners` 动态提供 Runner 安装与状态，入口位于「管理」页顶端，GUI 不保留旧引擎环境管理器或本地脚本安装
- 推荐模型条目保持紧凑精简，在引擎不可用时显示提示，可直接在管理页顶端完成引擎安装后立即启动

运行状态中的模型条目可进入详细设置页，调整 keep-alive、LLM 上下文长度和 TTS 默认音色。

## 系统要求

- Apple Silicon Mac
- macOS 14 或更高版本
- 完整 Xcode 或兼容的 Swift 5.9 工具链
- 已构建的 `aiworkd`

## 构建与打包

```bash
cd apps/MacAIConsole

# 本地联调：停止旧 GUI 与 aiworkd，重建并启动新的 app
scripts/build-app.sh release

# 生成自包含 DMG 安装包（自动包含 aiworkd、macai、uv 与 runners）
scripts/build-app.sh dmg
```

产物：

```text
build/MacAIConsole.app
build/MacAIConsole.dmg
```

仅需单独启动已构建 app 时：

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
POST /api/models/inspect
POST /api/models/load
POST /api/models/{id}/load
POST /api/models/{id}/unload
GET  /api/runners
POST /api/runners/{id}/install
```

`/v1/models` 与 `/api/runtime` 会返回 daemon 记录的 Provider 选择结果：
`requested_provider`、实际 `provider`、`provider_selection_reason` 及已加载模型的
`effective_device`。

## 本地数据

```text
~/Library/Application Support/MacAIConsole/
├── Models/
│   ├── llm/
│   ├── stt/
│   └── tts/
├── model-settings.json
├── models.db
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
