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

历史 `handoff.md` 可供追溯，不参与当前权威解析，也不接收新的工作提案或决策。

## 当前决策记录

- [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)——`implemented`
  （Phase 1–3 与 Kokoro / qwen3-asr 已落地；剩余迁移与 cancel/Plugins 见其 Migration）
- [uv 管理全部 Python 环境](2026-09-02-uv-python-environments.md)——`implemented`
  （daemon-owned uv environment manager 已落地；legacy venv 逐引擎退役中）
- [模型下载链路修复](2026-09-02-download-link-fixes.md)——已落地修复记录

当前实施顺序与迁移状态由
[`docs/plans/runner-migration-roadmap.md`](../plans/runner-migration-roadmap.md) 追踪。
