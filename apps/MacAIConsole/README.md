# MacAIConsole

MacAI 的原生 macOS 控制台：Runtime 状态 + 模型管理，全部通过本地 HTTP API 驱动 `aiworkd`。

- 主窗口：侧栏「运行状态」「模型管理」两页
- 运行状态页：版本/PID/内存预算统计卡、系统内存压力条、Running Models 列表（类型图标、加速策略 tag（CoreML/Metal）、真实驻留内存、停止按钮、详细设置页）
- 模型详细设置页：keep_alive 策略、LLM 上下文长度（K 单位热调并重载）、TTS 默认音色与试听
- 模型管理页：llm/tts/stt 分组、仓库扫描（放入文件即出现）、模型改名与恢复原始 ID、右键拷贝 curl 使用示例、STT 附加 `.mlmodelc` 目录导入（CoreML 加速）
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
- 模型仓库 `~/Library/Application Support/MacAIConsole/Models/{llm,tts,stt}/`：文件放入即出现；模型设置（上下文长度等）同步 `model-settings.json`；daemon 侧注册表持久化在 SQLite（`models.db`），重启自动恢复
