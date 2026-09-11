# 管理页置顶 Runner 引擎状态与安装控制、精简推荐模型条目

Status: implemented

Class: architecture

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[模型管理页子条目详细设置与 Runner 识别展示](2026-09-06-model-management-detail-settings.md)、[uv Python 运行环境与依赖锁定](2026-09-02-uv-python-environments.md)

## Problem

此前 MacAIConsole 将 Runner 引擎的管理和安装入口置于「设置」页的次要位置（`SettingsView` 中的「引擎」区块），同时在「运行状态」页底部以只读形式列出各 Runner 的运行设备与就绪状态；而在「模型管理」页（`ModelsView`）中：
1. 用户在尝试下载或使用推荐模型时，若对应的 Runner 引擎尚未就绪，只能在条目内看到冗长的警告引导并被迫跳转到「设置」页去安装引擎；
2. 「模型管理」页并未提供一站式的运行环境视界，用户无法在同一个管理工作区直接了解当前支持的全部底层推理引擎（llama.cpp, whisper.cpp, mlx-lm, kokoro, qwen3-asr, qwen3-tts, sherpa-onnx）的就绪状态；
3. 「推荐模型」列表的每个子条目（`ProfileModelRow`）包含多行重复冗余文案（如每行固定的「由 daemon Model Profile 提供」、多行分散的内存估算与外链、以及两行宽的未就绪警告框），导致单行纵向高度过大，视觉层次臃肿；
4. 侧边栏「模型管理」命名偏窄，未能准确涵盖「引擎环境管理 + 模型生命周期管理」的完整职责。

## Decision

对 MacAIConsole 控制台进行页面职责收敛与信息架构重构：

1. **页面重命名为「管理」**：
   - 将侧边栏项目与页面导航标题由「模型管理」更名为「管理」（路由维持 `Page.models`），涵盖引擎与模型的综合运维能力。

2. **引擎功能迁移并置顶管理页，运行状态页移除冗余 Runner**：
   - 将原设置页中的「引擎」安装能力移出 `SettingsView`，设置页专注于应用外观、守护进程拉起、内存预算、代理配置与下载源；
   - 移除了「运行状态」页（`RuntimeStatusView`）底部的只读「Runner」区块，由管理页作为 Runner 引擎状态与安装动作的唯一展示收敛点；
   - 在「管理」页最顶端新增「引擎」区块（`engineSection`），直观呈现由 daemon `/api/runners` 与 `/api/providers` 报告的每一个 Runner 引擎，并严格按 **LLM → STT → TTS** 顺序分类排列；
   - 每一行引擎条目（`EngineRow`）展示短名、能力标签（LLM / STT / TTS）与就绪（已就绪 · 设备）/未安装/环境失败的状态展示，去除了冗余的 `WORKER` 标签保持界面轻量；
   - 未安装（missing）或失败（failed）的引擎直接在行内提供「安装」/「重试」操作按钮；已就绪引擎可从右键菜单确认卸载，操作完成后回到未安装状态并可重新安装。安装与卸载均展示进行中状态。

3. **`DaemonController` 集中维护 Runner 状态**：
   - 在 `DaemonController` 状态机中新增 `runners: [RunnerEntry]` 与 `busyRunnerIDs: Set<String>`；
   - 在后台轮询 `refresh()` 中并发拉取 `/api/runners`；
   - 提供 `installRunner(_ id: String) async` 方法，统一样式与错误反馈，安装成功后立即触发全局状态刷新。

4. **精简「推荐模型」条目（`ProfileModelRow`）**：
   - 移除冗余重复的「由 daemon Model Profile 提供」行文；
   - 将 Runner 标识优化为更紧凑短名（如 `llama.cpp`、`kokoro`）；
   - 将预估内存与 Hugging Face 来源链接合并为单行辅助信息；
   - 移除占行的两行黄色引导框，若引擎未就绪仅在条目右侧或标签旁提供微型警告标记与提示，引导用户参考本页顶部的引擎区域；
   - 压缩行高与垂直 padding，使其与已注册模型及仓库模型视觉风格保持协调一致。

5. **关联路由调整**：
   - `AppRouter` 新增 `goToManagement()`，将 `ModelDetailSheet` 等各处的引擎引导统一指向「管理」页。

## Alternatives considered

- **在设置页与管理页同时保留引擎安装**：违反单一 Owner 原则，导致两套 UI 状态与事件维护，增加代码发散与认知负担。
- **将引擎状态做成二级标签页（Segmented Picker）**：在管理页内做「引擎 / 模型」二级切换会增加一次点击，用户无法一眼看清「为什么这个推荐模型跑不了」是因为上面某个引擎未安装；单页流式置顶更符合一目了然的操作心理模型。

## Consequences

- 用户进入「管理」页后无需跳转即可一站式完成从底层引擎安装到模型下载、加载与配置的全流程；
- 推荐模型列表更加紧凑清爽，屏效大幅提升；
- 设置页恢复轻量，职责边界更加清晰。

## Verification

- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` 全量 39 项单元测试通过（包含新增的 Runner 解码与安装请求测试）；
- `cargo fmt --all -- --check` 格式检查无警告退出码 0；
- `cargo test --workspace` 全量通过（24 项 cli 测试、61 项 daemon 测试、10 项环境管理器测试、6 项插件组合测试、9 项运行时组合测试、2 项真实 wiring 测试）；
- 完整打包构建：`scripts/build-app.sh package` 编译 release 二进制、编译 MacAIConsole、打包自包含 App Bundle 并完成代码签名。
