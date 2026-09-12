# 应用内更新（In-App Updater）

- Status: `implemented`
- Date: 2026-09-12
- Deciders: MacAI 核心维护团队

## 问题

通过 DMG 安装 `MacAIConsole.app` 的用户没有获知或安装新版本的途径：必须手动访问 GitHub Releases、下载新 DMG、重新拖入 `/Applications`。`.github/workflows/release.yml` 已经在推送 `v*` tag 时产出 `MacAIConsole.dmg` + `checksums.txt`（SHA-256），但这一产物与已安装应用之间没有任何更新通道。

## 决策

在 MacAIConsole 内实现轻量应用内更新，不复用第三方更新框架：

1. **更新源**：GitHub Releases API（`https://api.github.com/repos/Ash-one/MacAIConsole/releases/latest`，仓库常量 `AppUpdater.repository`），不新增 appcast 资产或 CI 改动；`/releases/latest` 天然排除 draft 与 prerelease。
2. **实现位置**：`apps/MacAIConsole/Sources/MacAIConsole/AppUpdater.swift`（纯逻辑 + 网络检查 + 安装替换）与 `Views/UpdateSection.swift`（UI）。更新只针对 GUI 自身的 `.app` bundle，属于 GUI 生命周期，不进入 daemon、不新增 `/api` 端点；daemon 在安装前由 `AppUpdateState.setInstallPreparation` 注入的 `DaemonController.stopDaemon()` 停止，新实例按设置重新拉起。
3. **更新 UI**：收敛到「设置」页「关于」Section（设置页唯一 owner 不变）：显示当前版本（`CFBundleShortVersionString`）、手动检查按钮、状态反馈（检查中 / 已最新 / 发现新版本 / 下载中 / 安装中 / 失败原因）与"启动时自动检查更新"开关（`AppSettings.autoUpdateCheckKey`，默认开启，启动 3 秒后静默检查一次，仅发现新版本才提示）。
4. **安装流程**（`AppUpdateState.downloadAndInstall`）：`URLSession.download` 将 DMG 与 `checksums.txt` 落盘下载（避免整包驻留内存）→ `FileHandle` 流式 SHA-256 校验 → 停止 aiworkd 并轮询等待其退出（最长 8 秒，避免新实例与垂死进程争抢 11435 端口）→ `hdiutil attach -mountpoint` 至临时目录 → 将当前 `.app` 移到临时备份、拷贝新 `.app` 回原路径（拷贝失败回滚）→ 卸载镜像并清理临时目录 → `open` 新 bundle 并 `exit(0)`。由于 `exit` 不展开 `defer`，现场清理显式发生在退出之前；子进程调用失败抛 `installFailed`（携带退出码）并触发恢复回调重新拉起 daemon。
5. **版本号单一来源**：`build-app.sh` 从 workspace `Cargo.toml` 读取 `version` 注入 `CFBundleShortVersionString`（此前硬编码 0.1.0）；约定 release tag 与 workspace 版本一致（`v<version>`）。比较逻辑为数值化 semver 段比较，容忍 `v` 前缀与缺段；无 bundle 版本（开发构建）不视为可更新目标。
6. **守护边界**：`AppUpdater.canInstall` 要求运行于 `.app` bundle 内，否则设置页不渲染更新区块且静默检查跳过；启动静默检查在 DEBUG 构建下整体跳过（手动检查仍可用，便于本地调试），避免开发构建被发布版本覆盖。

## 已考虑的替代方案

- **Sparkle 框架**：macOS 事实标准，自带 EdDSA 签名 appcast 与安装器。落选原因：引入本仓库首个 SwiftPM 第三方依赖，需要生成并长期保管 EdDSA 私钥与签名工作流；且应用当前 ad-hoc 签名、无公证，Sparkle 的"信任首个签名"安全收益无法兑现。重新引入条件：应用引入真实 codesign 身份与公证后可重评。
- **Homebrew Cask**：依赖用户安装 Homebrew，不覆盖 DMG 直接安装人群，且与现有发布流水线平行。不采纳。
- **daemon 承担更新**：更新对象是 GUI bundle，daemon 不应拥有 GUI 安装目录的写路径，也不应新增下载之外的 API 面。拒绝。

## 后果

- SHA-256 校验只保证下载完整性（校验和与 DMG 同为 GitHub HTTPS 资产、同源下载），不构成密码学签名链；在引入 codesign 身份与公证前，这是有意接受的弱点。
- 替换为"先移出再拷入"的非原子操作：拷贝失败会回滚原 bundle，但极端情况下（回滚也失败）会留下临时备份目录，用户需从 DMG 重装。
- 自动检查仅启动时一次、未认证 GitHub API（60 次/小时/IP）单请求，限流风险可忽略。
- 发布流程新增约束：打 `v*` tag 前必须同步 workspace `Cargo.toml` 版本，否则已安装应用报告的版本与 tag 脱节，更新比较失真。当前 `Cargo.toml` 为 0.1.0 而线上最新 tag 为 v0.1.3，下一次发布需先对齐。
- 预发布 tag（如 `v0.2.0-beta`）含非数字段，版本比较返回 nil，已安装用户不会收到提示；如需分发预发布版本，需另行扩展比较规则或使用独立通道。

## 验证

| 验收 | 直接证据 | 结果 |
| --- | --- | --- |
| 版本比较容忍 `v` 前缀、缺段、数值进位、非法输入 | `AppUpdaterTests.testVersionComparisonToleratesVPrefixAndMissingSegments` | Passed |
| 新版本判定在可解析与不可解析版本下的行为 | `AppUpdaterTests.testReleaseIsNewerOnlyWhenParseableAndGreater` | Passed |
| Release JSON 解析出 DMG 与 checksums 资产 URL；缺资产时为 nil | `AppUpdaterTests.testParseReleaseExtractsDMGAndChecksumAssets` / `testParseReleaseRejectsPayloadWithoutDMG` | Passed |
| `checksums.txt` 提取目标文件摘要并拒绝非 64 位十六进制行 | `AppUpdaterTests.testChecksumManifestParsing` | Passed |
| 摘要不匹配时拒绝安装 | `AppUpdaterTests.testVerifyRejectsDigestMismatch` | Passed |
| 全量 Swift 回归 | `swift test --enable-xctest`（57 tests, 0 failures） | Passed |
| 版本号注入 | `build-app.sh release package` 构建产物 `Info.plist` 的 `CFBundleShortVersionString` 与 `Cargo.toml` 一致（0.1.0） | Passed |
| 更新源与资产名匹配 | `GET /repos/Ash-one/MacAIConsole/releases/latest` 返回 v0.1.3，资产含 `MacAIConsole.dmg` 与 `checksums.txt` | Passed（真实 API） |
| 端到端"检查→下载→安装→重启" | 需已安装的 DMG 版应用与更新的发布版本同时存在，CI 无法自动化 GUI 自替换 | Not run：待下一次真实发布后手动验证 |
