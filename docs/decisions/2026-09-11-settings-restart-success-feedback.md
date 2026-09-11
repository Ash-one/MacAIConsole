# 设置应用并重启成功反馈

Status: implemented

Class: feature

Owner: this file

## Problem

设置页的“应用设置并重启 aiworkd”只发起生命周期操作。用户无法在当前页面确认 daemon 已经完成重启并重新在线，容易重复点击或误判设置没有生效。

## Decision

`SettingsView` 在用户点击按钮后记录本次请求，并观察共享 `DaemonController.phase`。请求进行期间，按钮显示进度指示和“正在重启 aiworkd…”文案，同时进入禁用灰色状态，防止重复提交。当状态到达 `.online`，按钮恢复可用，下方显示“设置已应用，aiworkd 已重启”成功提示，并在两秒后自动淡出。

提示只由当前设置页发起的请求触发。普通探活连接、应用启动时自动拉起和其他页面发起的重启不会产生该提示；启动失败继续使用 `DaemonController.lastError` 的既有错误通道。

## Alternatives considered

- **点击后立即提示成功**：只能证明操作已提交，无法证明 daemon 已重新在线。
- **新增全局 toast 系统**：当前只有一个局部、短时反馈需求，新增全局展示基础设施会扩大状态和视图组合面。

## Consequences

- 进行中反馈与本次请求绑定，按钮在 daemon 重新在线前不可重复点击；成功反馈与实际在线状态绑定，持续两秒后自动消失。
- 用户离开设置页会销毁局部提示状态；重新进入时不会重放旧提示。

## Verification

- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` 验证状态观察、两秒异步任务与设置页组合可编译，并运行 MacAIConsole 全套单元测试。
- 运行 MacAIConsole，在 daemon 在线时点击“应用设置并重启 aiworkd”，验证按钮立即变灰并显示转圈、重启期间无法重复点击，提示只在重新在线后出现，并在约两秒后消失。此项属于交付后的人工界面验证边界。
