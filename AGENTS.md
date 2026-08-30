## Scope and Decisions

- Treat reviews, audits, explanations, and reports as read-only; plans and proposals do not authorize implementation. Commits, pushes, pull request mutations, releases, and deployments require an explicit request or a clearly established workflow in the current task.
- Ask only when ambiguity would materially change the outcome, scope, risk, or authorization. Otherwise state the assumption and proceed. When viable paths have meaningful tradeoffs, recommend one.
- For maintenance work, prefer targeted changes and established conventions. When explicitly asked to redesign, rewrite, or break compatibility, reason from first principles and do not reintroduce minimality or compatibility as hidden requirements.
- Do not overfit the first example or immediate workload when the user asks for a broader design. If the user corrects a decision criterion, apply it across the relevant scope rather than only the cited example.

## Evidence, Review, and Design

- Base repository-specific claims on inspected code, tests, configuration, current state, and useful history; cite exact evidence when it matters. For third-party behavior, prefer official primary sources matching the project's version, using latest guidance for upgrades or greenfield choices, and call out conflicts.
- Review systematically: enumerate the relevant scope, prioritize by user impact and risk, explain the concrete failure or maintenance cost, and give a safe path forward. Omit generic or cosmetic findings that tools already cover.
- Make unexplained complexity justify itself. Ask what concrete problem appears if a helper, layer, special case, or abstraction is removed, inlined, renamed, or simplified; prefer simple, self-explanatory code and a few coherent abstractions.
- Evaluate public APIs from the caller's perspective, including discoverability, misuse resistance, error semantics, configuration, and evolution. Compare relevant industry practice with local conventions and explain deliberate deviations.

## Execution and Git

- For long tasks, maintain the global plan and end goal. Report only material progress. Final handoffs should state the result, validation, remaining risks or work, and any required user input.
- Never force-push unless explicitly asked to rewrite the published history of the specific branch. If a normal push is rejected as non-fast-forward, report it instead of forcing.
- Never merge a pull request or enable auto-merge unless explicitly asked to merge that specific pull request. Green CI, approval, or a request to continue is not merge authorization.
- When commits are requested, keep each commit coherent and reviewable, exclude unrelated changes, and report the commit hash and validation performed.

## Tests and Documentation

- Add tests for realistic observable regressions, non-trivial invariants or boundaries, and concrete bugs. Code changing or coverage increasing is not sufficient justification by itself.
- Prefer existing coverage at the behavior boundary. Avoid tests that mirror literals, mappings, obvious control flow, implementation details, or removed features unless absence is itself a contract. For concurrency, prefer deterministic coordination or controlled scheduling over sleeps when practical.
- Comments should explain non-obvious rationale, invariants, safety constraints, or external quirks rather than restating code. Public API documentation should describe observable contracts, not incidental implementation details.

---

# MacAI 项目指南

本节是本仓库的项目级事实与约定索引，供编码 Agent 使用；上方各节的通用规则继续适用。事实核对于 2026-08 的当前代码。文档分工：`README.md` 记录当前实现状态（与 `handoff.md` 冲突时以 README 和代码为准）；`handoff.md` 是长期目标架构与决策记录，§70 起为已落地的决策记录（问题 / 决策 / 被放弃方案 / 后果 / 验证）；`apps/MacAIConsole/README.md` 专述 GUI。

## 项目一句话

MacAI 是面向 Apple Silicon 的本地 AI Runtime：Rust daemon `aiworkd`（默认 `127.0.0.1:11435`）是唯一的 Runtime Authority，统一管理模型注册、推理 worker、内存调度和 HTTP API；`macai` CLI、SwiftUI 应用 MacAIConsole 与 OpenAI-compatible SDK 都只是它的客户端。

## 架构不变量（改动前先对照）

来自 `handoff.md` §59 与已落地决策记录，违反即架构回归：

1. **客户端不拥有模型**：GUI / CLI / API 只通过 `aiworkd` 的 HTTP API 工作。给 GUI 加功能时不得引入本地推理、本地模型状态或第二套 Provider 推断；UI 展示的状态必须来自 daemon（`/api/providers`、`/api/runtime` 等）。
2. **故障隔离**：Provider 以独立进程承载（llama.cpp / whisper.cpp 二进制；Kokoro、Qwen3-ASR Python worker）。worker 崩溃时 `aiworkd` 必须保持存活并返回结构化错误（如 `backend_crashed`）。
3. **Python worker 常驻**：按 Provider 复用 persistent worker 与 venv，禁止一请求一进程的 spawn → load → infer → exit。
4. **卸载经过 lease / busy guard**：手动 unload、LRU 逐出和 keep-alive reaper 都不能中断 lease count > 0 的进行中请求。
5. **禁止静默 fallback**：Provider 选择结果必须显式记录（requested / selected / effective device / reason）；显式指定 `--provider` 是硬选择，不可用时失败而非切换。UI 不得自行推断设备或可用性。
6. **单一 owner**：keep_alive 字符串解析只有 `ai-daemon::scheduler::parse_keep_alive`；STT 上传音频格式裁决只有 daemon 入口 `crates/ai-daemon/src/audio.rs`（decode-on-ingest 归一化为 PCM WAV，Provider 契约始终是"只接收 PCM WAV"）；GUI 设置只有主窗口侧边栏一页（经 `AppRouter` 导航），没有独立 Settings scene。修改这些行为时改 owner，不要新增平行实现。
7. **安全边界**：daemon 只绑 `127.0.0.1` 且无认证，不得默认暴露到局域网；日志不记录 API token 或模型内容，原始音频不写入任务历史。
8. **Provider 必须诚实**：`available` / `ready` / `resident` 是不同状态；单个 Provider 探测失败时报告 unavailable + reason，不得让 `/api/providers` 整体 500。

## 代码地图

| 路径 | 内容 |
| --- | --- |
| `crates/ai-core` | 共享类型：model / provider / request / response / errors |
| `crates/ai-daemon` | `aiworkd` 本体。`main.rs` HTTP endpoints；`runtime.rs` 模型与 Provider 生命周期（lease）；`scheduler.rs` 内存预算（`min(ram*0.75, ram-8GB)`，`AIWORKD_MEMORY_BUDGET` 可覆盖）、LRU 与 keep-alive reaper；`registry.rs` SQLite 注册表（内存 HashMap 为唯一读路径）；`tasks.rs` 有界任务历史（终态保留最近 100 条）；`pull.rs` Hugging Face 下载（断点续传）；`audio.rs` STT 音频归一化；`providers/` 含 llama_cpp、whisper_cpp、kokoro_mlx、qwen3_asr（同一文件内定义 `qwen3-asr` 与 `qwen3-asr-mlx` 两个 Provider）、macos_say、mock |
| `crates/ai-cli` | `macai` CLI（clap，单文件 `main.rs`），只调 daemon |
| `apps/MacAIConsole` | SwiftUI 控制台。`DaemonAPI.swift`（HTTP 客户端）、`DaemonController.swift`（daemon 探测与启停：`AIWORKD_PATH` → `target/release` → `target/debug`）、`PythonEnvironment.swift`（一键装 venv 到 `.build/`）、`AppSettings.swift`、`AppRouter.swift`；视图在 `Views/` |
| `scripts/` | `build-llama-server.sh` / `build-whisper-cli.sh` / `download-whisper-model.sh`（固定经过验证的 revision）；Python worker：`kokoro_worker.py`、`qwen3_asr_worker.py`、`qwen3_asr_mlx_worker.py`；`tests/` 为 pytest |
| `samples/` | Hermes TTS/STT 命令型 Provider 适配器与一键配置脚本 |

## 构建与验证

改动后至少跑通对应语言的全套；CI（`.github/workflows/ci.yml`）在 macOS runner 上跑前两项、ubuntu runner 上跑第三项：

```bash
# Rust（fmt 是硬门槛）
cargo fmt --all -- --check
cargo test --workspace

# MacAIConsole（必须设置完整 Xcode 的 DEVELOPER_DIR，仅 Command Line Tools 时 XCTest 定位不到 SDK）
cd apps/MacAIConsole
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest

# Python workers（依赖仅 pytest + numpy，无需 venv）
python3.12 -m pytest scripts/tests -v
```

构建 GUI app：先 `cargo build --release -p ai-daemon`，再在 `apps/MacAIConsole` 下 `scripts/build-app.sh release`，产物为 `build/MacAIConsole.app`。

## 约定与陷阱

- **提交信息**：conventional commits + 中文描述（如 `feat: xxx`、`test: xxx`）。
- **低依赖优先**：引入新依赖或外部二进制需要充分理由（先例：STT 格式归一化放弃 ffmpeg，选 symphonia 纯 Rust 解码）。
- **测试哲学**：不写镜像字面量、clap derive 声明或系统框架往返的零防护力测试；同一契约只在单一位置固化（如 Kokoro 音色清单只在 DaemonAPIRequestTests 的 pull payload 测试固化）。发现死代码连同死测试时倾向删除，而不是补测试。
- **本地产物不入库**：`target/`（Rust）、`.build/`（第三方引擎、Python venv）与模型权重都不在仓库；运行时数据在 `~/Library/Application Support/MacAIConsole/`（models.db、模型、日志）。
- **`.wt-qa/` 是 QA 用的 git worktree 副本**，已 gitignore，不属于本仓库：不要在其中工作，不要把它的变更算进本仓库。
- **常用环境变量**：`AIWORKD_PATH`（daemon 二进制）、`AIWORK_LLAMA_SERVER`、`AIWORK_WHISPER_CLI`、`AIWORK_KOKORO_PYTHON`、`AIWORK_QWEN3_ASR_MLX_PYTHON`、`AIWORK_QWEN3_ASR_DEVICE=mps|cpu|auto`、`AIWORKD_MEMORY_BUDGET`（字节）。
- **文档同步**：改动对外行为（endpoint、CLI 命令、环境变量、目录布局）时同步 `README.md`；落地架构级决策时在 `handoff.md` 追加决策记录（参照 §70–§72 的结构）。
