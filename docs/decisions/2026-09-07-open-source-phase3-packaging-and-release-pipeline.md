# 开源筹备阶段 3：独立打包流水线与自动化发布 (DMG + CLI Release)

- Status: `implemented`
- Date: 2026-09-07
- Deciders: MacAI 核心维护团队

## 背景与问题

此前 MacAI 的构建与分发存在以下限制：

1. **GUI 应用依赖本地源码与开发环境**：`MacAIConsole.app` 依赖外部环境变量 `AIWORKD_PATH` 或本地 `target/` 目录中的产物，自身未内置 `aiworkd` 二进制与 `runners/` 声明。普通用户下载 `.app` 后无法开箱即用；
2. **缺少标准安装分发格式**：仅有本地编译测试脚本，缺少生成 macOS 标准安装镜像（`.dmg`）及挂载快捷方式的能力；
3. **缺少自动化发布流水线**：没有针对 GitHub Releases 的自动编译与产物上传流水线，发布新版本需人工手动打包。

## 决策内容

### 1. 自包含 App Bundle 架构 (Standalone App)

对 `MacAIConsole.app` 的结构进行自包含组装：
- `Contents/MacOS/`：放置 `MacAIConsole`（主程序）、`aiworkd`（守护进程）与 `macai`（CLI 工具）；
- `Contents/Resources/`：放置 `AppIcon.icns` 及内置 `runners/` 结构（自动排除本地 `.venv`、`__pycache__` 等开发缓存）；
- 守护进程自适应：`crates/ai-daemon/src/main.rs` 的 `builtin_runners_root()` 增加检查 App Bundle 的 `Contents/Resources/runners`；
- GUI 控制器自适应：`DaemonController.swift` 在启动时若检测到 Bundle 内置 `runners`，自动注入 `MACAI_RUNNERS_DIR` 并设置安全的工作目录。

### 2. 双模构建脚本 (build-app.sh)

升级 `apps/MacAIConsole/scripts/build-app.sh`：
- `scripts/build-app.sh [release|debug]`：保持原有本地开发联调体验（停止旧进程、编译、组装并自动拉起应用）；
- `scripts/build-app.sh package`：编译全量组件，组装自包含的 `.app`；
- `scripts/build-app.sh dmg`：组装自包含应用并创建 `MacAIConsole.dmg`。DMG 生成逻辑优先使用 `create-dmg` 提供图形化窗口布局；若未安装则自动无缝降级为 macOS 原生 `hdiutil`（UDZO 压缩，带 `/Applications` 快捷链接），确保在任何机器或 CI runner 上均能 100% 成功生成。

### 3. GitHub Actions Release 自动化流水线 (.github/workflows/release.yml)

配置在推送 tag（`v*`）或手动 `workflow_dispatch` 时触发：
1. 构建自包含的 `MacAIConsole.dmg`；
2. 构建命令行分发归档 `macai-vX.Y.Z-darwin-arm64.tar.gz`（包含 `macai`、`aiworkd`、干净的 `runners/`、README 与 License）；
3. 自动生成 `checksums.txt`（包含各文件的 SHA-256 校验和）；
4. 通过 `softprops/action-gh-release@v2` 自动发布至 GitHub Releases。

## 后果与验证

- 产物自包含：普通用户下载 `MacAIConsole.dmg` 拖入 `/Applications` 后即可直接运行，无需本机配置 Rust 或克隆代码仓库；
- 打包验证：本地执行 `build-app.sh dmg` 成功生成约 8.1MB 的压缩镜像，挂载验证文件结构完整无误；
- CI 具备端到端发布能力。
