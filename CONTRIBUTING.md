# MacAI 贡献指南 (Contributing to MacAI)

感谢你对 MacAI 的关注与支持！我们欢迎各种形式的贡献：无论是修复缺陷、改进文档、新增模型 Runner，还是优化系统调度性能与 GUI 交互体验。

在提交 Pull Request 之前，请仔细阅读本指南以确保协作顺畅。

---

## 开发前置环境

由于 MacAI 充分利用了 Apple Silicon 架构特性（如 MLX、Metal 与 Accelerate 框架），建议在原生 Apple Silicon 硬件上进行开发：

- **操作系统**：macOS 14.0 (Sonoma) 或更高版本；
- **硬件**：Apple Silicon Mac（M1 / M2 / M3 / M4 系列）；
- **Rust**：Stable 工具链（推荐 1.75+，需带 `rustfmt`）；
- **Xcode**：Xcode 15+（包含命令行工具，提供 Swift 5.9+ 工具链）；
- **Python / uv**：Python 3.12+，以及 [uv](https://docs.astral.sh/uv/)（高性能 Python 包管理器，daemon 管理 Runner 环境的核心依赖）。

---

## 代码地图与架构不变量

在编写代码前，请对照 MacAI 的核心架构边界与约定（详见 [AGENTS.md](AGENTS.md) 与 [docs/decisions/](docs/decisions/README.md)）：

1. **客户端不拥有模型状态**：GUI（`MacAIConsole`）、CLI（`macai`）和第三方 SDK 只是客户端。所有的模型注册、生命周期维护、内存调度和推理状态，**必须由 `aiworkd` daemon 统一管理**。客户端展示的数据必须来自 daemon 的 HTTP API，不得在客户端重复实现推理逻辑或状态缓存。
2. **进程级故障隔离**：生产推理引擎均由独立的 Runner 进程承载（例如 `llama.cpp`、`whisper.cpp`、`mlx-lm` 等）。Worker 进程异常退出或 OOM 时，`aiworkd` 必须保持稳定存活并返回结构化错误。
3. **Python worker 常驻**：基于持久化进程维持推理实例，禁止一请求一启动的低效模式。Python 环境由 daemon 统一通过 `uv` 进行同步与安装。
4. **禁止静默回退（No Silent Fallback）**：显式指定 `--provider` 是硬选择；若该 Provider 不可用应直接报错并说明原因，绝不悄悄切换为其他 Provider。

```text
MacAI 代码地图：
├── crates/
│   ├── ai-core        # 共享核心类型定义（模型、Provider、请求、响应、错误）
│   ├── ai-daemon      # aiworkd 守护进程本体（HTTP 服务、生命周期调度、内存管理、SQLite 注册表）
│   └── ai-cli         # macai 命令行工具（单文件 clap 实现，纯 HTTP 客户端）
├── runners/           # 独立 Runner 包（llama.cpp, whisper.cpp, mlx-lm, kokoro 等）
├── apps/
│   └── MacAIConsole   # macOS 原生 SwiftUI 控制台应用
├── docs/
│   ├── decisions/     # 架构设计决策记录（所有非机械改动的理由 owner）
│   └── specs/         # 精确协议与 Wire Format 契约
└── scripts/           # 辅助脚本
```

---

## 本地构建与验证

任何 PR 提交前，请务必在本地跑通对应模块的验证套件：

### 1. Rust 代码校验与单元测试
```bash
cargo fmt --all -- --check
cargo test --workspace
```

### 2. MacAIConsole Swift 单元测试
```bash
cd apps/MacAIConsole
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest
cd ../..
```

### 3. Runner 包依赖与适配器测试
```bash
for runner in runners/*; do
  test -f "$runner/pyproject.toml" || continue
  echo "Checking $runner..."
  uv lock --check --project "$runner"
  uv run --project "$runner" pytest "$runner/tests" -q
done
```

### 4. 构建并启动 GUI 联调
```bash
# 自动停止旧进程、编译 release daemon 与 release app 并启动
apps/MacAIConsole/scripts/build-app.sh release
```

---

## 如何贡献新的 Runner 扩展

若要接入新的开源模型推理引擎，建议按照 [Runner 插件架构决策](docs/decisions/2026-09-02-runner-plugin-architecture.md) 进行：

1. 在 `runners/<engine-name>/` 下创建 Runner 包；
2. 提供 `pyproject.toml`（或编译说明）与受管的 `uv.lock`；
3. 提供 `runner.toml` 声明 Runner 的名称、版本、支持协议类型（`chat`、`stt`、`tts`）及入口点；
4. 实现标准 JSONL 双向协议：`hello` 握手、`load`、`infer`（支持 chunk 流式返回）、`unload`；
5. 编写单元测试覆盖适配器解析与协议序列化；
6. 在 `crates/ai-daemon` 注册对应的模型识别或内置 Runner 装配规则；
7. 同步编写/更新相关的 decision record。

---

## 提交与 Pull Request 规范

1. **Commit 规范**：
   - 遵循 [Conventional Commits](https://www.conventionalcommits.org/) 格式：`<type>(<scope>): <subject>`；
   - 常用类型：`feat`（新特性）、`fix`（缺陷修复）、`docs`（文档）、`test`（测试）、`refactor`（重构）、`perf`（性能优化）；
   - 示例：`feat: 支持新的 MLX 语音识别 Runner`、`fix: 修复客户端断开连接时的任务状态泄漏`。

2. **Decision Record 同步**：
   - 任何涉及行为、架构、协议格式或工具链变更的非机械修改，请在 `docs/decisions/` 中新增或更新对应的决策记录。

3. **创建 Pull Request**：
   - 保持 PR 职责单一，聚焦于具体问题或特性；
   - 关联对应的 GitHub Issue；
   - 确保 CI 流水线状态为全绿。
