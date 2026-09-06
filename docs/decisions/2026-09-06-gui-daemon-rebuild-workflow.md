# GUI 与 daemon 的联合重建启动流程

Status: implemented

Class: process

Owner: this file

## Problem

仅重建 `MacAIConsole.app` 会替换 bundle 中的文件，却不会替换已运行的 GUI 进程；该进程
还可能连接旧 `aiworkd`。开发者因此看见已修改的后端或 UI 行为未生效。

## Decision

`apps/MacAIConsole/scripts/build-app.sh [release]` 是本地 GUI 联调的唯一重建启动入口。
它先停止同名 `MacAIConsole` 与 `aiworkd`，构建 release `ai-daemon` 和 SwiftUI app，重新
组装并签名 app bundle，再用 `open -n` 启动新的 GUI。GUI 的既有解析顺序首先使用刚构建的
`target/release/aiworkd`。

脚本默认使用完整 Xcode 的 `DEVELOPER_DIR`（若调用者未覆盖且 Xcode 存在），避免 Command
Line Tools 的 SwiftPM manifest sandbox 限制。

## Alternatives considered

- 只构建 app：已运行进程继续使用旧代码。
- 只重启 GUI：它可能复用旧 daemon。
- 让 GUI 自行决定是否重启 daemon：无法保证手动启动的旧 daemon 被替换。

## Consequences

脚本会停止当前用户的所有同名 MacAIConsole / aiworkd 开发进程；它不删除模型、注册表或
环境。需要保留现有 daemon 进行独立实验时，应直接使用底层构建命令，不运行该脚本。

## Verification

`bash -n apps/MacAIConsole/scripts/build-app.sh` 验证脚本语法；实际执行后应看到新的 GUI
PID、daemon PID 与当前编译产物，并在模型管理页确认目录检测结果。
