# 开源筹备阶段 1：环境解耦、脱敏与历史文档退役

- Status: `implemented`
- Date: 2026-09-07
- Deciders: MacAI 核心维护团队

## 背景与问题

MacAI 计划正式面向开源社区发布。在初步开源就绪度评估中，发现以下阻碍外部用户与开发者直接克隆使用的环境耦合与安全隐患：

1. **绝对路径硬编码**：`apps/MacAIConsole/Sources/MacAIConsole/DaemonController.swift` 在探测 `aiworkd` 二进制时，硬编码了开发者个人的本地路径（`LocalDocuments/GitLocalStore/MacAI`），导致外部克隆或构建缺少 `AIWORKD_PATH` 时无法启动 daemon；
2. **命名空间未标准化**：GUI 应用的 `CFBundleIdentifier` 及日志异步队列使用了个人域前缀（`com.guanxuzeng.MacAIConsole`），需统一为项目组织前缀；
3. **`.gitignore` 不完备**：根目录仅有 9 行规则，缺少 Xcode/SwiftPM 派生数据、Python 虚拟环境（`.venv/`）、macOS 隐藏文件及本地环境文件的忽略，存在意外提交本地产物的风险；
4. **冗余历史材料**：根目录保留了 6.5 万字的历史交接材料 `handoff.md`。根据架构规范，该文件已不作为当前事实或决策依据，保留在根目录会导致外部贡献者理解混淆。

## 决策内容

### 1. 二进制多级自适应探测（DaemonController.resolveBinary）

废除写死的主机绝对路径，建立健壮、可移植的 4 级探测链：

1. **显式环境变量**：`AIWORKD_PATH` 且可执行（最高优先级）；
2. **App Bundle 内置产物**：检查 `Bundle.main.resourceURL/aiworkd` 及 `Contents/MacOS/aiworkd`（支持未来独立 DMG 分发）；
3. **工作目录向上遍历**：从当前工作目录（`FileManager.default.currentDirectoryPath`）向上逐层探测 `target/release/aiworkd` 和 `target/debug/aiworkd`（支持命令行开发与 Xcode 调试运行）；
4. **系统与用户标准路径**：探测 `/opt/homebrew/bin/aiworkd`、`/usr/local/bin/aiworkd`、`~/.cargo/bin/aiworkd` 及 `~/.local/bin/aiworkd`（支持已全局安装或 cargo 安装的场景）。

### 2. 统一组织命名空间

- `CFBundleIdentifier` 统一为 `org.macai.MacAIConsole`；
- AppLog 的内部文件日志队列标签统一为 `org.macai.MacAIConsole.file-log`。

### 3. 补全 `.gitignore`

系统性补齐 Rust、Swift/Xcode（`.swiftpm/`, `DerivedData/`）、Python（`.venv/`, `__pycache__/`, `*.egg-info/`）、macOS（`._*`, `.DS_Store`）、IDE（`.vscode/`, `.idea/`, `.zed/`）及环境变量文件的忽略规则。

### 4. 退役并删除 `handoff.md`

彻底从仓库移除 `handoff.md`，同步清理 `AGENTS.md`、`README.md`、`docs/decisions/README.md` 与 `docs/index.html` 中对其的指引与提及。当前所有设计与实现事实唯一归属于代码、测试、当前 `README.md` 与 `docs/decisions/`。

## 后果与验证

- 外部开发者克隆仓库后，无论在根目录还是子目录，只要执行过 `cargo build`，MacAIConsole 均可自动感知并拉起 `aiworkd`；
- 消除任何个人开发机特定路径泄漏；
- 运行 `cargo test --workspace` 与 `swift test` 保持全绿。
