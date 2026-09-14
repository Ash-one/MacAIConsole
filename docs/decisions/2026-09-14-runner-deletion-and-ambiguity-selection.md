# Runner 引擎删除与歧义候选显式选择机制

- **Status**: implemented
- **Date**: 2026-09-14
- **Author**: Antigravity
- **Related Records**:
  - `docs/decisions/2026-09-14-local-detector-directory-matching.md`（本地模型目录检测器关键词匹配）
  - `docs/decisions/2026-09-13-runner-provider-availability-unbound.md`（Runner Provider 可用性解耦）
  - `docs/decisions/2026-09-02-uv-python-environments.md`（uv Python 环境与受管生命周期）

## 1. 背景与问题

1. **扩展引擎缺少删除生命周期**：
   在 MacAIConsole 管理页（`ModelsView`）的引擎卡片上，用户仅能进行“卸载引擎”（清除受管 Python 运行环境）与“忽略引擎”。对于用户自行编写或导入至 `Plugins/` 目录的 Script Runner / 扩展 Runner，缺乏彻底从系统中移除的路径。
2. **歧义状态下的 GUI 交互断点**：
   当本地模型目录同时满足多个 Runner 的探测规则时（例如通用权重结构同时被内置与自定义模型探测器捕获），daemon 会将检验状态标记为 `ambiguous`。在此之前，GUI 仅能展示错误提示并建议用户改用 CLI 传入 `--provider`，无法在界面内闭环完成确定性绑定与注册加载。

## 2. 决策与架构设计

### 2.1 引擎删除机制（`DELETE /api/runners/{runner}`）

1. **内置 Runner 保护**：
   - 内置 Runner（`llama.cpp`、`whisper.cpp`、`mlx-lm` 等，根目录在应用 bundle 或源码 `runners/` 中）受核心保护，**不可删除**，仅支持卸载运行环境。调用删除接口时返回 400 Bad Request。
   - 仅位于用户数据目录 `Plugins/`（通过 `app_support/Plugins/` 判定）的扩展/脚本 Runner 允许彻底删除。
2. **Busy Guard 与状态清理顺序**：
   - **Busy Guard**：检查当前是否有模型正在该 Runner 上驻留运行。若有，返回 409 Conflict，提示必须先卸载模型。
   - **停止实例**：通过 `instance_manager.remove_runner(&runner)` 关闭并回收该 Runner 的运行实例（如常驻 Python 进程）。
   - **卸载环境**：调用 `environment_manager.uninstall(&runner)` 清理其专属虚拟环境及依赖。
   - **磁盘清理**：在异步任务中安全移除 `Plugins/<runner>` 目录所有文件。
   - **撤销信任与注册表注销**：
     - 在 SQLite 注册表执行 `untrust_runner_package`，删除其信任记录。
     - 从 `RunnerRegistry`、`Runtime` 的 Provider 列表及 `Discovery` 缓存中完全注销。
3. **GUI 客户端适配**：
   - `RunnerEntry` 新增 `isPlugin: Bool` 计算属性。
   - 仅对 `isPlugin == true` 的引擎行在右键菜单中显示标红的「删除引擎」按钮。
   - 点击后触发二次确认弹窗（`.confirmationDialog`），警示操作将彻底清除脚本与环境且不可撤销。

### 2.2 歧义候选匹配显式选择机制

1. **Routing Token 扩展与候选下发**：
   - 探测匹配项 `DetectorMatch` 增加 `routing_token: Option<String>`。
   - 当 daemon 执行 `/api/models/inspect` 时，不论单一匹配还是歧义匹配，均按 `(path, runner)` 生成单次有效的 `routing_token` 并注入到每个候选匹配中。
2. **模型详细设置（`ModelDetailSheet`）交互闭环**：
   - 在模型详细设置中，针对 `ambiguous` 状态或匹配数 > 1 的情况，展示交互式候选 Runner 卡片列表（包含名称、能力类型、探测器 ID、匹配依据）。
   - 用户可点击单选项或「选择」按钮确定具体的承载 Runner。
   - 选定后：
     - 动态刷新「承载 Runner」展示及其就绪状态（若该 Runner 尚未安装环境，立即展示「去安装引擎」引导）。
     - 赋予「注册并加载」按钮可用状态。
     - 用户点击注册加载时，携带选定候选的 `routing_token` 向 daemon 发起请求。
   - daemon 校验通过后，将模型与用户指定的 Runner 进行唯一持久化绑定并加载，完全避免静默 fallback。

### 2.3 自定义/插件 Runner 模型重命名与注销绑定同步修复

此前 `Runtime::rename_model` 与 `Runtime::unregister_model` 中存在历史遗留限制，仅在 `spec.provider.starts_with("org.macai.")` 时才通知 `provider.rename_bound_model` / `unbind_model`。对于第三方或用户自定义编写的 Script Runner（其 ID 为自定义命名如 `org.hojoai.*` 或 `custom.*`），重命名模型 ID 会导致 Runner 内存中的活跃绑定未能更新，进而引起加载时报 `HTTP 404: model '...' is not bound to this Runner`。
现已移除 `starts_with("org.macai.")` 的硬编码前缀假设，统一根据 `self.runner_providers` 进行动态查询与同步。

## 3. 影响范围与不变量核对

- **客户端不拥有模型与状态**：所有 Runner 发现、删除与路由决策均在 daemon 内裁决，GUI 只展示 daemon 下发的候选与结果。
- **禁止静默 fallback**：歧义场景必须由用户在 UI 中显式选定或通过 CLI 显式传递 `--provider`，不存在任何隐式或随机挑取。
- **故障隔离与单一 Owner**：Runner 卸载与删除遵循相同的 busy guard；路由 token 校验由 daemon 统一拥有。

## 4. 验证与测试

1. **Rust 单元测试**：
   - `crates/ai-daemon/src/registry.rs`: `runner_package_trust_roundtrips` 测试 untrust 的完整持久化往返。
   - `crates/ai-daemon/src/main.rs`: `delete_runner_rejects_builtin_runner` 验证内置引擎拦截。
   - `crates/ai-daemon/src/main.rs`: `inspect_issues_routing_token_for_each_match` 验证歧义时为各个匹配项签发独立 token。
   - `crates/ai-daemon/src/runtime.rs`: `custom_runner_binding_tracks_rename_and_unregister_without_org_macai_prefix` 验证非 `org.macai.*` 自定义 Runner 的重命名与注销绑定同步。
2. **Swift 单元测试**：
   - `DaemonAPIRequestTests.testDeleteRunnerSendsDeleteToRunnerEndpoint`：验证 DELETE `/api/runners/{id}` 的调用与报文。
   - `DaemonAPIRequestTests.testInspectModelsDecodesMatchesWithRoutingToken`：验证多匹配与候选 token 的正确解码。
3. **工作区测试**：
   - `cargo fmt --all -- --check`：格式完全合规。
   - `cargo test --workspace`：全部 77+31+12+6+12+11 = 149 个测试用例全部通过。
   - `swift test --enable-xctest`：全部 77 个测试用例通过。
