# MacAIConsole

MacAI 的原生 macOS 控制台（第一版）：Runtime 状态 + 模型管理。

- 主窗口：侧栏两页「运行状态」「模型管理」
- 菜单栏常驻小窗：状态摘要 + 快捷启停
- GUI 掌管 aiworkd 生命周期：可在界面内启动 / 停止；离线时自动收编终端里手动拉起的守护进程

## 构建与运行

```bash
scripts/build-app.sh           # 默认 release，产物 build/MacAIConsole.app
open build/MacAIConsole.app
# 或直接开发运行
swift run
```

构建产物要求 macOS 14+，本机为 macOS 15。

## 与 aiworkd 的对接

- 全部通过本地 HTTP API（`http://127.0.0.1:11435`）交互：
  `GET /health`、`GET /api/runtime`、`GET /api/providers`、`GET /v1/models`、
  `POST /api/models/load`、`POST /api/models/{id}/load`、`POST /api/models/{id}/unload`
- 守护进程二进制探测顺序：设置中指定路径 → `AIWORKD_PATH` 环境变量 → 仓库 `target/{release,debug}/aiworkd`
- 启动的子进程日志写入 `~/Library/Application Support/MacAIConsole/logs/aiworkd.log`
- 添加的 GGUF 会记入本地模型库 `local-models.json`（守护进程注册表仅在运行期内存）

## 第二期规划

Chat 对话页、STT / TTS（whisper-base / macos-say 已注册，API 已具备）。