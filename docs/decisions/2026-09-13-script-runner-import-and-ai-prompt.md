# Decision: Script Runner 编辑页导入 .py 与复制 AI 生成 Prompt

Status: implemented

Class: feature

Owner: this file

Related current decisions: [单文件 Script Runner 创建](2026-09-08-single-file-script-runner.md)

## Problem

`.macai.py` 的合规事实（PEP 723 元数据 schema、hook 签名、能力契约、模型识别规则）
分散在 daemon 源码、`docs/specs/` 与决策记录里。想把已有 Python 推理代码接入 MacAI
的用户，要么在编辑器里手工改写模板，要么把这些事实手工搬运给外部 AI——两者都容易
遗漏或过时，而 `local_detectors.required_files` 一旦凭空编造，模型目录将永远无法被
识别，且错误要到「检查」甚至创建之后才暴露。

## Decision

「新建单文件 Runner」编辑页（`ScriptRunnerEditorSheet`）新增两个入口：

**导入 .py 文件**：通过 SwiftUI `fileImporter`（`.py` 类型）读取本地文件，依次执行
空文件、1 MiB 上限（对齐 daemon `script.rs` 的 `MAX_SOURCE_BYTES`）、UTF-8 三道守卫，
失败走现有错误横幅。覆盖语义与切换能力一致：编辑器内容已被用户改动（非空且不等于
已加载模板；模板基线缺失时任何非空内容都按已改动处理）时先弹覆盖确认，否则直接
覆盖。导入内容缺少 `# /// script` 块时显示中性提示（非错误），引导使用 AI Prompt
闭环：复制 Prompt → AI 生成 → 存为 .py → 导入。

**复制 AI Prompt**：组装完整上下文 prompt 写入剪贴板，按钮短暂反馈「已复制」。
prompt 由 Swift 侧静态脚手架（`ScriptRunnerAIPrompt`）+ daemon 返回的当前能力官方
模板原文拼装；脚手架包含第 0 步确认环节——用户未提供模型（缺名称或网址）时 AI 必须
先询问模型网址，无法确认模型真实文件构成时禁止编造 `required_files`。模板按能力
缓存，复制时缓存未命中再拉取。

同时修复既有行为：切换能力时不再无条件覆盖用户编辑（原实现直接重置 `source`），
改用与导入相同的覆盖确认。否则「导入后误触能力切换」会静默丢掉导入内容。

## Alternatives considered

**daemon 新增 `/api/runner-scripts/ai-prompt` 端点生成全文。** 可让 daemon 成为
prompt 文本的唯一 owner，消除 Swift 脚手架与 v1 契约的漂移；但 prompt 是面向作者的
教学文本而非契约执行点——契约执行在 inspect/create，AI 产出偏差会被「检查」以精确
错误拦截并可贴回 AI 自愈。首版采用 GUI 侧拼装（模板仍来自 daemon，与依赖助手分工
一致）；协议升级 v2 时再评估收拢到 daemon。

**导入时智能合并元数据与代码。** 不可靠且无法预测合并结果，明确放弃，导入一律整体
覆盖。

**导入后自动触发「检查」。** 保持「检查」是显式动作；hint 引导用户自查。可作为后续
增强。

## Verification

- `RunnerScriptTextEditorTests` 固化：覆盖守卫判定（空内容/等于模板/已编辑/模板基线
  缺失四种情形）、PEP 723 块检测与 daemon 解析器开块规则一致、AI prompt 完整注入
  daemon 模板原文、包含 schema 固定值与第 0 步确认指令、三种能力各自声明正确的
  hook 签名。
- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest`
  全套通过：75 passed（2026-09-13）。
- GUI 实际点击路径（fileImporter 权限、剪贴板、确认对话框交互）未做人工验证，属
  明确的未验证边界；纯逻辑均已由测试覆盖。

## Consequences

- 编辑器内容有了统一的覆盖确认语义，导入与切换能力共用同一判定函数；后续任何新的
  覆盖类入口必须复用 `isEditorContentUserModified`。
- Swift 脚手架镜像 v1 元数据 schema 与 hook 签名；daemon 契约变更时需同步更新
  （「检查」环节的精确报错使漂移可被发现，但不会自动修复）。
- AI 产出的 Runner 仍需走既有 inspect → digest → create 信任流程，prompt 不改变
  任何信任边界；第 0 步确认仅约束生成行为，不构成安全机制。
