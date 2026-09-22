# MacAIConsole

MacAIConsole 是 MacAI 的原生 macOS 控制台。应用只通过本地 HTTP API 与 `aiworkd` 通信，不直接运行模型推理。

## 当前页面

- 运行状态：版本、PID、内存预算、系统内存压力、活跃请求和已加载模型
- 任务：当前运行及最近完成的 Chat / STT / TTS 请求，支持分别查看推理模型的思考过程与最终输出，以及耗时和错误
- 管理：页面顶端置顶展示 Runner 引擎环境状态与安装控制（按 LLM、STT、TTS 顺序分类排列）；已就绪引擎可从右键菜单卸载受管环境和引擎文件，回到未安装状态后可重新安装，使用中的模型需先卸载；并可在带 Python/PEP 723 语法高亮和推理库依赖助手的内置编辑器中从 Chat / STT / TTS 模板创建单文件 Script Runner；编辑器支持导入本地 .py 文件（覆盖前确认，含空文件 / 1 MiB / UTF-8 校验）与一键复制让外部 AI 生成 .macai.py 的完整 Prompt（要求 AI 在缺少模型网址时先询问），切换能力与导入均受覆盖确认保护；推荐目录来自 daemon `/api/model-profiles`，本地目录经 `/api/models/inspect` 由 daemon 路由；「添加模型」支持从本地文件导入，或输入公开 Hugging Face/ModelScope 仓库 ID/URL、预览文件与大小后下载；远端下载只写入仓库，不自动注册或加载；模型按 LLM / STT / TTS 分组显示，支持加载、卸载、改名与删除注册；GGUF 会读取展示 metadata
- 日志：查看 GUI 与 daemon 最近日志，默认显示 Info，可启用 Debug，并按日志级别着色；"在访达中显示"可打开日志目录
- 设置：应用外观（跟随系统 / 明亮 / 暗黑）、启动时自动拉起守护进程与退出时自动退出守护进程开关、内存预算、网络代理、模型下载源，以及恢复管理页中已忽略的引擎和推荐模型；退出软件时根据守护进程自动退出开关动态弹窗确认（开启时提供「关闭所有」与「取消」，未开启时提供「关闭所有」、「关闭GUI」与「取消」）；应用设置并重启成功后显示两秒完成提示
  - 模型下载源可切换 Hugging Face 官方源、`hf-mirror.com` 或自定义 Hugging Face 兼容源；修改后重启 aiworkd 生效
  - 引擎：daemon `/api/runners` 动态提供 Runner 安装与状态，入口位于「管理」页顶端，GUI 不保留旧引擎环境管理器或本地脚本安装
- 菜单栏常驻小窗（MenuBarExtra）：系统状态栏原生集成，支持卡片化统一内存（Unified Memory）水位条与 AI 调度预算监控、活跃请求与运行时长指标、API 端点（`http://127.0.0.1:11435`）一键复制、常驻模型直观查看与一键卸载（支持右键拷贝 curl 调用示例）、未驻留已注册模型快速拉起菜单、守护进程一键启停/重启，以及直达主窗口各页面（运行状态/任务/管理/日志/设置）的快捷导航。状态栏图标根据在线空闲、活跃推理、启停中与离线状态动态切换
- 引擎和推荐模型子条目均可通过右键菜单忽略，后续启动不再显示；设置页可分别恢复全部引擎或推荐模型，其中已下载的推荐模型仍保持隐藏

运行状态中的模型条目可进入详细设置页，调整 keep-alive、LLM 上下文长度、默认 `temperature` / `top_p` / `max_tokens` 和 TTS 默认音色；LLM 条目的右键使用示例会带上这三个当前生成参数。TTS 的音色选择与试听由共享控件提供（运行状态页设置与「管理」页模型详情共用）：音色下拉框支持滚轮快速上下步进，滚轮停止后自动保存默认音色并试听当前音色（可关）；试听可随时被下一次选择打断，播放不落盘临时文件。

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
POST /api/models/remote/inspect
GET  /api/downloads/{progress_id}
POST /api/models/load
POST /api/models/{id}/load
POST /api/models/{id}/unload
GET  /api/runners
POST /api/runners/{id}/install
DELETE /api/runners/{id}/install
GET  /api/runner-scripts/template/{chat|stt|tts}
GET  /api/runner-scripts/dependency-presets
POST /api/runner-scripts/dependencies
POST /api/runner-scripts/inspect
POST /api/runner-scripts
```

在线目录模型保存为 `Models/<type>/<owner>--<repo>/` 并保留仓库相对路径；单个
LLM `.gguf` 或 Whisper `ggml-*.bin` 直接保存到对应类型根目录。远端下载只支持公开
仓库，且文件进入仓库并不代表当前 Runner 一定兼容。
推荐模型和在线仓库下载会显示当前文件/总文件及百分比；进度由 daemon 的活动下载状态提供。

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
├── Plugins/              # 已信任的单文件 Script Runner 生成包
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
