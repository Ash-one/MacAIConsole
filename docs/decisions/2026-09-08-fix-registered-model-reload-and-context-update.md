# 已注册模型重载与上下文更新 Provider 继承缺陷修复

Status: implemented

Class: defect

Owner: this file

Related current decisions: [本地模型目录检测与 Runner 路由](2026-09-06-local-directory-model-routing.md)、[模型管理页子条目详细设置与 Runner 识别展示](2026-09-06-model-management-detail-settings.md)、[Runner 插件架构](2026-09-02-runner-plugin-architecture.md)

## Problem

在 MacAIConsole 管理页中，用户成功注册并通过本地探测器加载了一个目录型 MLX 模型（例如基于 `mlx-text-directory` 探测器绑定的 `MiniCPM5-2B-MLX`，承载 Runner 为 `org.macai.mlx-lm`）。随后用户在「详细设置」弹窗（`ModelDetailSheet`）或状态栏浮窗（`RuntimeStatusView`）中调整上下文长度并点击「应用并重载」时，系统抛出错误：
```
重载模型失败: HTTP 400: expected a .gguf model file for provider 'org.macai.llama.cpp', got '/Users/guanxuzeng/Library/Application Support/MacAIConsole/Models/llm/MiniCPM5-2B-MLX'
```

### 根因剖析

1. **守护进程端（Authority 根因）**：
   - 模型动态重载统一由 HTTP 端点 `POST /api/models/load` 处理（对应 `register_and_load_model`）；
   - 初次注册完成后，一次性 5 分钟有效期的 `routing_token` 已被消费或已过期，因此重载请求通常不带 `routing_token`；
   - 当请求未指定 `provider` 时，原有逻辑无条件调用了 `select_provider(model_type, None)`，对于 `"llm"` 类型硬编码回退至 `"org.macai.llama.cpp"`；
   - 随后 `org.macai.llama.cpp` 的单文件校验 `!path.is_file()` 失败，导致已注册的目录型 MLX 模型无法再次调用重载；
   - 实际上底层的 `Runtime::register_and_load` 原本就具备保存与复用 `RegistryEntry.profile` 快照的机制，但 HTTP 处理层过早截断并覆盖了 Provider 决策。

2. **GUI 客户端端（MacAIConsole）**：
   - `ModelDetailSheet.swift` 的 `applyContextLength()` 在调用 `controller.registerAndLoad` 时未传递 `provider` 与 `routingToken`（均为 `nil`）；
   - `RuntimeStatusView.swift` 的 `applyContextLength()` 同样未传递已加载模型的 `provider`。

## Decision

1. **守护进程端：继承已有注册事实（Single Runtime Authority）**：
   - 在 `register_and_load_model` 入口提早根据 `request.id` 或 `default_model_id(&path)` 解析模型 ID 并查询已有注册规约 `existing_spec = state.runtime.get_model(&id).await`；
   - 当没有传递 `routing_token` 且请求中未显式指定 `provider` 时：
     - 若该模型已存在于注册表中，**继承其已注册的 `provider`、`format` 与 `model_type`**，并将裁决原因记录为继承已有注册；
     - 仅当模型尚未在注册表中存在时，才调用 `select_provider` 回退到系统默认 Provider；
   - 在构造 `spec` 时，优先保留已持久化的元数据（`source`、`format`、`memory_estimate` 等）。

2. **客户端端：显式传递已知 Runner 与探测凭据**：
   - 在 `ModelDetailSheet.swift` 的 `applyContextLength()` 中：
     - 判断模型是否已注册（`isRegistered`）；对于已注册模型或已知 `runnerID` 的模型，显式传入 `provider: runnerID`；
     - 对于未注册的仓库目录模型，若持有 `inspection?.routingToken`，则传入 `routingToken` 并将 `provider` 置空；
   - 在 `RuntimeStatusView.swift` 的 `applyContextLength()` 中：
     - 显式传入当前运行中模型的 `provider: model.provider`。

## Alternatives considered

- **为上下文长度新增独立 HTTP 端点 `/api/models/{id}/context-length`**：这会破坏 `POST /api/models/load` 作为模型规格配置与生命周期原子入口的统一契约；且 `register_and_load` 已天然包含 LRU 逐出、进程卸载与带参拉起。
- **仅在 Swift 客户端补传 `provider`**：虽然能解决特定界面的点击问题，但违反了 daemon 作为唯一 Runtime Authority 的原则——CLI 或直接调用 API 的客户端在重载已注册模型时依然会踩坑崩掉。守护进程必须具备自愈与状态一致性保证。

## Consequences

- 已注册的目录型模型（MLX、ASR、TTS）可以在 GUI、CLI 或 API 中任意次调用 `POST /api/models/load` 调整参数并重载生效，不会发生误降级至 `llama.cpp`；
- 保留完整的审计链条（`requested_provider` 与 `provider_selection_reason` 会忠实记录继承自已有注册）；
- 客户端在已知 `runnerID` 时显式声明意图，减少不必要的协议模糊。

## Verification

1. **Rust 自动化回归测试**：在 `crates/ai-daemon/src/main.rs` 增加 `registered_directory_model_reloads_context_length_without_defaulting_to_llama_cpp`，模拟注册目录型模型后调用 `/api/models/load` 更新上下文长度，验证 Provider 与目录形态被正确保留，HTTP 200 成功响应；
2. **全局 Rust 测试套件**：`cargo test --workspace` 确保所有契约无回归；
3. **Swift 测试套件**：`swift test --enable-xctest` 验证 GUI 客户端网络请求与模型映射无回归。
