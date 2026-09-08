# MacAI 决策记录

本目录是 MacAI 非机械改动的设计理由 owner。当前实现状态仍由代码、测试、根
`README.md` 和相关子系统 README 共同描述；决策记录解释为什么选择某个方向。

## 何时需要记录

改变以下任一内容时，在同一 bounded change 中创建或更新一个 owning record：

- 用户、客户端或模型可观察行为；
- 模块所有权、依赖方向、进程模型或协议；
- API、持久化、配置、manifest 或 wire format；
- 构建、安装、发布、测试或文档流程；
- 安全、兼容性、资源管理或故障语义。

纯格式化、拼写修正和不改变行为或理由的局部机械修改不需要记录。

## 生命周期

每个文件使用以下语义之一：

- `Status: proposed`：工作提案。拥有问题、候选方向、真实备选、验收标准和风险；不得写成当前行为。
- `Status: implemented`：代码、直接证据和当前文档已经收敛。文件需改写为现在时的决策、后果和验证，不能只修改状态字段。
- `Status: rejected`：保留仍能防止重复错误的被拒方案和原因。

一个决策只有一个理由 owner。精确 wire/schema 契约可放在 `docs/specs/`，由决策记录链接；实施步骤和未执行的验证放在 `docs/plans/`。摘要只链接 owner，不复制易漂移细节。

## 文件与替换

文件名使用 `YYYY-MM-DD-topic.md`。工作提案在交付后原地改写为已落地决策。
已经交付的决策发生反转时创建新的交叉链接记录；只替换部分边界时，两份记录都明确各自继续拥有的条款。

## 当前决策记录

- [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)——`implemented`
  （built-in Runner、通用协议、Runtime/GUI 环境与 Model Profile catalog 管理面、选择审计已落地；第三方
  Plugins、并发/cancel 和 OS 沙箱仍是独立后续边界）
- [uv 管理全部 Python 环境](2026-09-02-uv-python-environments.md)——`implemented`
  （daemon-owned uv environment manager、七个独立 lock 与 GUI API 消费已落地）
- [whisper.cpp Runner 迁移](2026-09-05-whisper-runner-migration.md)——`implemented`
  （官方 source build、常驻 server、兼容别名和静态 Provider 删除已落地）
- [模型下载链路修复](2026-09-02-download-link-fixes.md)——`implemented`
- [GUI 公开远端模型下载](2026-09-08-gui-remote-model-download.md)——`implemented`
  （输入 Hugging Face/ModelScope 仓库 ID，预览并选择文件后下载到模型仓库；不自动注册或加载。）
- [本地模型目录检测与 Runner 路由](2026-09-06-local-directory-model-routing.md)——`implemented`
  （daemon-owned、无执行的目录签名检测与唯一匹配 routing token；GUI/CLI 消费同一管理 API。）
- [GUI 与 daemon 的联合重建启动流程](2026-09-06-gui-daemon-rebuild-workflow.md)——`implemented`
  （一个脚本停止旧进程、重建同一配置的前后端，并启动新 app。）
- [模型管理页子条目详细设置与 Runner 识别展示](2026-09-06-model-management-detail-settings.md)——`implemented`
  （模型管理页全条目左键点击开启详细设置，透明展示 Runner 及识别依据，提供上下文与文件管理。）
- [开源筹备阶段 1：环境解耦、脱敏与历史文档退役](2026-09-07-open-source-phase1-sanitization.md)——`implemented`
  （DaemonController 消除开发者绝对路径、规范组织命名空间、补全 .gitignore、退役 handoff.md。）
- [开源筹备阶段 2：社区治理规范、安全策略与第三方许可合规](2026-09-07-open-source-phase2-governance-and-compliance.md)——`implemented`
  （落地 CONTRIBUTING、CODE_OF_CONDUCT、SECURITY、Issue/PR 模板、THIRD_PARTY_LICENSES 与模型免责声明。）
- [开源筹备阶段 3：独立打包流水线与自动化发布 (DMG + CLI Release)](2026-09-07-open-source-phase3-packaging-and-release-pipeline.md)——`implemented`
  （实现自包含 MacAIConsole.app、支持 DMG 制作与 hdiutil 降级、配置 GitHub Actions 自动化 Release 流水线。）
- [Kokoro G2P 容灾韧性与 Runner 退出诊断增强](2026-09-07-kokoro-g2p-resilience-and-runner-diagnostics.md)——`implemented`
  （将 espeak-ng 数据迁移至 `~/.cache/macai/espeak-<hash>` 免于 `/tmp` 清理，提供纯 Python 拼读降级机制，增强 Supervisor 子进程 stderr 退出诊断。）
- [替换推荐 LLM 模型：以 MiniCPM5-2B-MLX 替代 Qwen3-8B-4bit](2026-09-08-replace-recommended-mlx-llm-with-minicpm5.md)——`implemented`
  （将内置推荐 LLM 替换为更轻量、端侧表现优异的 MiniCPM5-2B-MLX，维持 7 个推荐模型 Profile 总数与全量文件可达性。）
已完成的 Kokoro 证据保存在本地验证参考（`docs/reference/kokoro-runner-verification.md`）。
