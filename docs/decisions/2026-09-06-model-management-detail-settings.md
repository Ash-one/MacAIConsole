# 模型管理页子条目详细设置与 Runner 识别展示

Status: implemented

Class: feature

Owner: this file

Related current decisions: [本地模型目录检测与 Runner 路由](2026-09-06-local-directory-model-routing.md)、[Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[Model Profile v1](../specs/model-profile-v1.md)

## Problem

在 MacAIConsole 的模型管理页中，用户可以看到「模型注册 ID」、「模型仓库」和「推荐模型」三大列表，但各条目默认仅展示紧凑的几行标签（如 ID、owned_by、请求 provider）。用户无法直接查看：
1. 模型具体由哪个 Runner 承载，该 Runner 的隔离级别与环境可用/就绪状态；
2. 该 Runner 是如何被识别与裁决的（如本地探测器的匹配规则、GGUF/Whisper 文件签名检测、抑或显式绑定）；
3. 模型的完整本地文件路径、文件规格元数据（如 GGUF 架构、原生上下文长度、量化级别）；
4. 无法在统一的界面中对模型进行详细设置（如调整上下文长度与默认生成参数、修改模型注册 ID、在访达中定位文件、触发重载等）。

## Decision

在 MacAIConsole 模型管理页（`ModelsView`）中，为每一个子条目实现左键点击交互，统一打开结构化的原生 macOS 详细设置弹窗（`ModelDetailSheet`）：

1. **统一覆盖三大列表**：
   - 已注册模型（`ModelEntry`）：展示注册 ID、Runner 标识、进程隔离级别、daemon 裁决原因（`provider_selection_reason`）、请求来源（`requested_provider`）、来源路径、实时运行指标（生效设备、内存占用等），并支持在弹窗内直接更改 ID、调节 LLM 上下文长度与默认生成参数、启动/停止和删除注销。
   - 仓库模型（`RepoModel`）：展示文件名、路径、文件/目录大小。对于目录模型，展示 daemon 本地探测器匹配详情（探测器 ID、适配器、能力、判定 reason，或未匹配诊断信息）；对于单文件 GGUF/Whisper 模型，展示文件签名检测与对应 Runner 自动绑定依据；若包含 GGUF 元数据，展示其架构与原生上下文上限；支持调节上下文、快速注册/重新注册、TTS 试听等。
   - 推荐模型（`ModelProfile`）：展示模型 Profile 规格、显式绑定的 Runner、预估内存、官方仓库来源与下载启动。

2. **Runner 与识别方式透明化**：
   - 明确突出「承载 Runner」与「Runner 可用性/就绪状态」，若环境未就绪提供跳转「设置 → 引擎」的一键引导；
   - 明确展示「Runner 识别方式」，将底层探测器（Local Detector）、静态文件签名（GGUF Magic Header / Whisper .bin）以及 daemon 裁决依据（`provider_selection_reason`）向用户清晰解构。

3. **模型 ID 重命名一致性与全覆盖**：
   - 修复了此前在 `Runtime::rename_model` 中针对 Profile-backed 模型的阻断限制（此前返回 400 `Runner-backed Model Profile IDs are stable and cannot be renamed`）；
   - 支持已注册模型无论来源于本地 ad-hoc 还是 Model Profile，均可在 GUI（右键菜单或详细设置弹窗）中自由重命名为简短 ID（如将 `Qwen3-TTS-0.6B-CustomVoice-4bit` 重命名为自定义 ID，或一键恢复原始 ID）；
   - 重命名时同步更新 `models` 注册表、`registered_model_profiles` 快照映射以及当前 Provider 的 `RunnerModelBinding`；重启时依注册 ID 自动装配对应 Runner 实例。

4. **原生交互与文件契约**：
   - 遵循 macOS 交互规范：整行左键点击激活、悬停背景与提示（`hoverableRow` + `help`）、行内独立按钮截断事件避免冲突；
   - 提供「在访达中显示」（`NSWorkspace.activateFileViewerSelecting`）与安全写入剪贴板（`NSPasteboard` 清空先验）。

5. **LLM 默认生成参数归 daemon 所有**：
   - 仅在已注册 LLM 的详细设置中展示 `temperature`、`top_p` 与 `max_tokens`，初始默认值分别为 1.0、0.95 与 1024；合法范围分别为 0...2、0...1 与 1...1048576；
   - GUI 通过 `POST /api/models/{id}/generation` 保存，daemon 将设置写入模型注册表，无需重载即可供后续请求使用；
   - Chat 请求显式传入参数时保持调用值，省略时由 daemon 注入模型默认值；llama.cpp 与 MLX-LM Runner 都接收三个参数；运行状态页 LLM 子条目的右键使用示例包含该模型当前 `temperature`、`top_p` 与 `max_tokens`。

## Alternatives considered

- **纯右键菜单展示**：通过更多菜单项塞入信息，但菜单无法容纳富文本、状态点、长路径和交互式输入框，体验局促且难以阅读详细判定理由。
- **右侧展开 Inspector 侧边栏**：在 `NavigationSplitView` 中使用三栏或 `.inspector`，但在小窗口或低分辨率屏幕下会严重挤压中央内容区域，且模型管理页主要为列表卡片流，Sheet 模态弹窗更聚焦且与「任务详情」等保持一致的设计语言。

## Consequences

- 用户获得清晰透明的模型运行后端及识别依据可视度，避免排查「为什么使用了这个 Runner」时的黑盒感；
- 提供一站式的上下文与 ID 管理，免去命令行或多处跳转；
- 每模型生成默认值对 GUI、CLI 与 OpenAI-compatible SDK 一致生效，SQLite 旧库启动时自动补列并为既有 LLM 补齐默认语义；`max_tokens` 省略时沿用每模型配置；
- 遵循客户端不拥有模型原则，所有状态、识别理由与环境信息均来自 daemon 提供的公开 API。

## Verification

- 静态类型与语法校验：`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swiftc -parse Sources/MacAIConsole/Views/ModelDetailSheet.swift Sources/MacAIConsole/Views/ModelsView.swift` 验证通过；
- 全局 Swift 源文件校验：`swiftc -parse Sources/MacAIConsole/*.swift Sources/MacAIConsole/Views/*.swift` 验证通过；
- 架构一致性校验：`cargo check --workspace` 确保 Rust 数据契约无回归；
- 完整产物构建与启动：`./scripts/build-app.sh release` 成功编译后端 release `aiworkd` 与前端 `MacAIConsole.app`，组装签名并验证通过。
- `tests::generation_settings_validate_and_update_llm` 验证 daemon 的范围校验与内存更新；注册表 roundtrip/partial-update 测试验证 `temperature` 与 `top_p` 的 SQLite 持久化。
- llama.cpp 与 MLX-LM Runner 聚焦单测验证 `top_p` 传递和范围校验；MacAIConsole 51 项 Swift 测试通过，其中请求契约测试固定 `/api/models/{id}/generation` 的字段。
