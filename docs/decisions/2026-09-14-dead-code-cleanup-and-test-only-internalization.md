# Decision: 死代码清理与测试专用 Provider 收敛进 cfg(test)

Status: implemented

Class: simplification

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)（生产后端全部由 Runner 进程承载）、[whisper.cpp Runner 迁移](2026-09-05-whisper-runner-migration.md)（静态 Provider 删除后仅保留测试能力）

## Problem

2026-09-14 全仓库死代码扫描（编译告警 + pyflakes/vulture + 逐项引用核查）发现四类无消费者表面，以及一类"文档声称测试专用、编译图却仍进入生产二进制"的代码：

1. GUI 的 `HealthResponse`（`DaemonAPI.swift`）：`/api/health` 数据模型，但 GUI 侧零引用（DaemonController 的存活探测不经过该类型）。
2. manifest 未用依赖：ai-cli 的 `serde`、`tokio`（CLI 是阻塞 TcpStream 实现，JSON 走 serde_json）；ai-daemon 的 `clap`（aiworkd 用环境变量配置，`src/bin` 两个 fake-runner 夹具也不用它）。
3. kokoro Runner 的模块级 `import json`（`engine.py`、`__main__.py`，pyflakes 确认零使用）。
4. 编译告警噪声：`runners/provider.rs` 的 `let mut tx`；AppUpdater 两处 `try?` 未用结果。
5. `providers/`（mock + macos_say）与 `Runtime::new`/`inject_test_providers` 只被单元测试组合消费，却作为普通模块编进 release `aiworkd`，每次非测试编译产生 12 条 dead_code 告警——与 AGENTS.md「providers/ 只保留 macos_say、mock 测试能力」的描述只在文档层面成立。

## Decision

- 删除 1–3 的全部无消费者表面；4 按机械修复处理（`_ = try?`、去 `mut`）。
- `providers` 模块、`Runtime::new`、`inject_test_providers` 显式 `#[cfg(test)]`：测试能力保留，且只在测试编译中存在；release 二进制不再包含它们，bin 目标死代码告警清零。
- 有意保留的表面与理由见 Retained；它们不是本决策的删除对象。

## Alternatives

**连 mock / macos_say 一起删除。** 它们是 `Runtime::new` 注入的既有测试 fixture，被 runtime 与 HTTP handler 的数十个单元测试消费；删除需要把全部测试组合改写为 Runner fixture，并与 AGENTS.md「保留测试能力」的约定冲突；否定。

**原样保留 + `#[allow(dead_code)]`。** 这是 `registry.rs` 的既有先例，改动最小，但死表面仍会编进 release 二进制，且 12 条告警以 allow 逐个压制；cfg(test) 让「测试专用」从文档约定变成结构事实，语义等价于文档描述；否定 allow 方案。

**删除 `scripts/build-llama-server.sh`。** 文内无直接引用，但 README 目录表与 AGENTS.md 把它描述为「llama.cpp 可选本地构建」，2026-09-12 prompt-cache 决策的引擎版本验证（build 8086439）依赖该构建产物；开发者工作流仍有效；保留。

**删除 runner `__main__.py` 的 run_probe 导入块。** pyflakes 报 unused，但那是 probe 对运行依赖的可用性检查（`# noqa: F401`），是功能而非死代码；保留。

## Retained（有意保留的非删除项）

- `crates/ai-daemon/src/providers/`（mock、macos_say）：测试 fixture，本决策起 cfg(test) 化。
- `registry.rs` 的 `get_profile` / `load_profiles`：`#[allow(dead_code)]` 注释声明的 Profile 持久化查询 API，被持久化 round-trip 测试消费；durable format 的可执行证据依赖它们。
- `speech.wav`：`runner_whisper_real_wiring` 集成测试的音频 fixture。
- `scripts/build-llama-server.sh`、`scripts/download-whisper-model.sh`：文档化的开发者工作流。
- runner probe 导入块与 kokoro G2P fallback 内的 `*args, **kwargs` 签名：功能代码，静态工具的误报。

## Verification

- 负向检索：`grep -rw HealthResponse`（Sources+Tests）仅剩声明本身（删除前）；ai-cli 内 `serde`/`tokio` 零引用；ai-daemon 全部三个 bin 内 `clap` 零引用；kokoro 两文件 `json.` 零命中。
- `cargo fmt --all -- --check` 与 `cargo test --workspace` 通过（2026-09-14）；bin `aiworkd` 的 dead_code 告警 12 → 0。
- `swift test --enable-xctest`（Xcode DEVELOPER_DIR）：75 tests, 0 failures（2026-09-14）；AppUpdater `try?` 告警清零。
- `uv lock --check --project runners/kokoro` 通过；`pytest runners/kokoro/tests` 21 passed（2026-09-14）。
- pyflakes / vulture 对 runners 的残余输出均为 probe 导入与 misaki 补丁签名，经逐条阅读确认非死代码。

## Consequences

- release `aiworkd` 不再包含 mock/macos_say 代码路径；若未来需要 daemon 内置兜底 Provider，需在本记录上开新决策（该方向同时触碰「禁止静默 fallback」不变量）。
- `macai speak` 未指定 TTS 模型时没有兜底 Provider——`inject_test_providers` 上「macos-say 兜底」的陈旧注释已更正为生产语义；该行为自 whisper 迁移删除静态 Provider 起即如此，非本次改变。
- 本决策确立「manifest 依赖必须对应源码引用」的维护约定；后续清理可复用同一方法（编译告警 + pyflakes/vulture + 逐项引用核查），新增的测试专用代码应直接写成 `#[cfg(test)]` 而不是靠 allow 压告警。
