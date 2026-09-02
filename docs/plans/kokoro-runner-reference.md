# Kokoro 首个完整 Runner 实施与验证计划

Status: planned

Architecture owner: [`Runner 插件架构`](../decisions/2026-09-02-runner-plugin-architecture.md)

Environment owner: [`uv Python 环境`](../decisions/2026-09-02-uv-python-environments.md)

本文件只拥有 Kokoro 切片的实施顺序、保留行为和证据矩阵。Runner Protocol、
manifest、Model Profile 与 uv 规则分别由链接的 decision/spec owner 定义。本计划
完成后应删除完成型 checklist，并把仍有价值的实际验证路径折回已落地决策。

## Outcome

Kokoro-82M-zh-MLX 通过动态发现的 `org.macai.kokoro` Runner、提交的 uv project/
lock 和数据型 Model Profile 完成下载、环境安装、注册、加载、TTS、观测、卸载和
故障恢复。该路径不要求在 daemon 或 GUI 中增加 Kokoro 专属注册和环境枚举。

## Explicit non-goals

- 本切片不迁移 MLX-LM、Qwen3-TTS、ASR、whisper.cpp 或 sherpa-onnx；
- 不把所有 `mlx-audio` 模型提前合并进同一 Runner；
- 不承诺 Runner Protocol v1 对第三方稳定，直到 fake Runner 和 Kokoro 均通过；
- 不实现插件市场、自动更新、签名基础设施或环境垃圾回收；
- 不增加新 TTS format、voice cloning、流式 PCM 或局域网访问；
- 不改变 `aiworkd` 作为唯一 Runtime Authority 的边界。

## Current behavior to retain

迁移前先为以下当前可观察行为建立或确认直接证据：

- Provider ID `kokoro-mlx` 当前提供 TTS；迁移需要明确持久化和 CLI/API 兼容策略；
- worker 加载一次模型并常驻，单次合成失败后继续服务；
- stdout 只承载协议，第三方库输出重定向到 stderr；
- 空文本拒绝；输入最多 5000 字符；
- `speed` 范围为 `0.25...4.0`；
- 首版只返回 WAV；
- voice 优先级为请求显式值、模型默认值、`zf_001`；
- 中文使用 v1.1 G2P workaround，英文提供 fallback；
- 中文和超长文本切分不丢字符，多段音频按顺序合并；
- 每个结果 WAV 被 daemon 读取后删除；
- inference timeout 当前为 300 秒；
- worker 崩溃映射为结构化 backend failure；
- resident/PID 快照独立于推理 I/O 锁，推理时 `/api/runtime` 仍可返回；
- unload 和 daemon shutdown 不留下孤儿 worker。

保留行为以实施开始时的代码、测试和真实运行结果为准。如果真实模型观察与本表冲突，
先更新工作提案和证据边界，再决定保留、修正或明确撤回。

## Planned repository shape

```text
runners/kokoro/
├── runner.toml
├── pyproject.toml
├── uv.lock
├── profiles/
│   └── kokoro-82m-zh.toml
├── src/
│   └── macai_kokoro_runner/
│       ├── __init__.py
│       ├── __main__.py
│       ├── adapter.py
│       └── protocol.py
└── tests/
    ├── test_adapter.py
    └── test_protocol.py

crates/ai-daemon/src/runners/
├── manifest.rs
├── profile.rs
├── protocol.rs
├── registry.rs
├── supervisor.rs
└── environment.rs
```

路径是计划的一部分，实施发现更清晰的本地模块边界时可在同一工作提案中调整。不要
同时保留新的 Runner owner 和旧 Provider owner。

## Current status and execution order

截至 2026-09-02 commit `cadfa31`：

| Slice | Current state | Direct evidence / gap |
| --- | --- | --- |
| Phase 1A：contract、registry、trust、supervisor | completed as isolated foundation | Runner lib 12 tests；composition 5 tests；启动失败清理、SemVer 选择和 digest-bound staging 已覆盖 |
| Phase 1B：daemon-owned uv environment | completed as isolated foundation | `tests/runner_environment_manager.rs` 8 tests（真实 uv：cold/exact sync、stale lock、probe failure/timeout、cancel、promotion、重启恢复）；manifest `runtime.probe` parser + 拒绝路径；CI 固定 uv 0.9.21。未接入 Runtime/scheduler/HTTP |
| Phase 1C：Runtime/scheduler Runner Instance | completed as isolated composition | `tests/runner_runtime_composition.rs` 6 tests：fake Runner 经 RunnerProvider bridge 完成 load/infer(tts.v1 WAV)/unload、shutdown 后无孤儿进程、输出目录清理、status 反映 environment phase；`runners/instance.rs` 拥有进程/协议组合与错误映射；Runtime 增加可选 Runner 通道与 shutdown 收口。Model Profile 持久 snapshot 与 HTTP 管理面接入未实现 |
| Phase 0：旧 Kokoro 基线 | partially checked | Rust Kokoro tests passed；Python pytest 环境和真实 WAV/性能基线未完成 |
| Phase 2–5：Kokoro package、adapter、product composition、cutover | not implemented | 没有 `runners/kokoro/`，产品仍使用 `KokoroMlxProvider` 与 `.build/kokoro-venv` |

从当前状态按以下 bounded changes 前进：

1. **Phase 1B — uv environment manager**：实现 uv 定位、安装状态、staging sync、
   manifest probe、原子 promotion、失败/取消恢复及真实 uv integration tests；
2. **Phase 1C — Runner Instance composition**：让 fake Runner 经模型注册、Runtime、
   scheduler 和 `tts.v1` capability bridge 完成 load/infer/unload，而不是测试直接调用
   `RunnerProcess`；
3. **Phase 0 completion**：在改动 Kokoro worker 前补齐 Python tests 和目标 Apple
   Silicon 的中文、混合中英、长文本、RSS、时延与并发 status 基线；
4. **Phase 2 + 3**：生成真实可冷安装的 Kokoro uv lock，再迁移 adapter；
5. **Phase 4 + 5**：接入 daemon/GUI 产品路径，真实验收后一次性切换并删除旧 owner。

Phase 1C 必须先定义并实现 Model Profile 持久 snapshot/digest 与 legacy `ModelSpec`
迁移边界。Runner Instance 需要拥有协议事件分发、并发容量、cancel、crash/error mapping、
PID/RSS/status snapshot，以及 graceful shutdown、异常退出和 drop 后的 worker、package
staging 与 request output 清理；lease、busy guard、LRU 和 keep-alive 继续由现有 Runtime/
scheduler 裁决。生产代码不得出现 fake 或 Kokoro 专属分支。

## Phase 0: freeze evidence

在改动 Kokoro 前：

1. 运行当前 Python worker tests；
2. 运行当前 Rust Kokoro Provider tests；
3. 确认推荐 Kokoro Model Profile 所需的完整文件集合和 immutable HF revision；
4. 在目标 Apple Silicon Mac 上保存一份短中文、混合中英、长中文的真实 WAV 基线；
5. 记录 model load time、首包/总耗时、RSS、sample rate、duration 和 worker PID；
6. 推理期间轮询 `/api/runtime`，确认现有观测行为基线。

这些结果是迁移比较输入，不成为黄金音频 snapshot。模型和依赖可能存在浮点或声学
差异，验收关注可听内容完整性、时长、格式、错误和生命周期。

Phase 0 只要求在首次修改 Kokoro worker/依赖前完成，因此不阻塞通用 Phase 1B/1C。
本轮 `cargo test -p ai-daemon --lib --tests` 已覆盖现有 Rust Kokoro Provider tests；
`python3.12 -m pytest scripts/tests -v` 因当前解释器未安装 pytest 而未运行，不能记录
为通过。

## Phase 1: generic foundation

### Phase 1A: isolated contract and process foundation（completed）

当前已完成：Runner Manifest / Model Profile parser、protocol frame codec、Runner registry、
trust snapshot、digest-bound package staging、process supervisor、fake Runner fixture，以及
启动失败 kill/wait。直接证据是 `cargo test -p ai-daemon --lib --tests` 中 12 个 Runner
lib tests 和 5 个 composition tests。

### Phase 1B: daemon-owned uv environment（completed as isolated foundation）

按 [`uv Python 环境`](../decisions/2026-09-02-uv-python-environments.md) 的 next slice 实现
环境 manager。Phase 1B 的 composition fixture 使用真实 uv 和无外部依赖的锁定
project，覆盖 cold/exact sync、stale lock、probe failure（非零退出与超时）、取消、
原子提升和重启恢复（`tests/runner_environment_manager.rs`，8 tests）。CI 固定
uv 0.9.21 + Python 3.12。环境 manager 尚未接入 Runtime/scheduler/HTTP 管理面。

### Phase 1C: Runtime and scheduler composition（completed as isolated composition）

已落地的组合能力：

- `runners/instance.rs`：RunnerInstanceManager 拥有 resolve → ensure_environment →
  spawn（digest 复核 + staging 副本）→ initialize → load → infer（事件分发
  accepted/progress/delta/metrics/result/error/cancelled）→ unload → shutdown
  的完整生命周期，及 SupervisorError/Runner error 到结构化错误的映射；
- `runners/provider.rs`：RunnerProvider 把 instance manager 桥接为 ai-core
  `Provider` + `TTSProvider`，Runtime 以普通 Provider 身份调度 Runner-backed
  模型（lease、busy guard、LRU、keep-alive 继续由既有 Runtime/scheduler 裁决）；
- per-request 输出目录由 daemon 创建、校验路径越界并读取后清理；
- Runtime 增加可选 `runner_instances` 通道，`shutdown_all` 收口 Runner instance；
- 直接证据：`tests/runner_runtime_composition.rs` 6 tests。

尚未实现（转入 Kokoro 迁移基线/Phase 2）：Model Profile 持久 snapshot/digest 的
SQLite 注册路径、`main.rs` HTTP 管理面接入、真实 GUI 状态路径。生产代码无 fake
或 Kokoro 专属分支。

Phase 1 完成条件是在 isolated foundation 已通过的基础上，fake Runner 从已注册 Model
Profile 经 environment manager、Runtime 和 scheduler 完成 infer/unload；lease/busy guard、
LRU、keep-alive、取消、crash、status 并发与所有临时目录清理都有直接组合证据。仅有
parser unit tests 或直接调用 `RunnerProcess` 的 isolated test 不足以进入下一阶段。

## Phase 2: uv-lock Kokoro

当前状态（2026-09-02，Phase 2 环境与 adapter 基线完成）：

- `runners/kokoro/` 已建立：`runner.toml`（python-uv、probe、capacity 1/1）、
  `pyproject.toml`（mlx-audio 0.5.1、misaki[zh]/[en] 0.9.4、numpy>=2<3、
  phonemizer-fork 3.3.2、espeakng-loader 0.2.4；dev group: pytest）、
  `uv.lock`（124 packages，misaki 中文依赖链 jieba/spacy 显式锁定）；
- `src/macai_kokoro_runner/`：Runner Protocol v1 adapter（frame codec 与
  daemon 对称、engine 迁移 G2P v1.1 patch / 长文本切分 / 多段拼接）、
  `--probe` 离线探针；
- `tests/test_adapter.py` 10 tests（切分不丢字符、wire format 对齐、
  EOF 语义）；
- 解释器接线修复：`status.python` 只描述 uv `--python` 输入的基础解释器；
  probe 与 entrypoint 的 `{environment.python}` 解析为运行解释器
  `status.runtime_python()`（`<env_root>/.venv/bin/python`）。

直接证据：

| Claim | Evidence | Result |
| --- | --- | --- |
| 提交的 lock 可在无旧 venv 的隔离位置冷安装 | `uv sync --project runners/kokoro --locked --no-dev` → `/tmp/kokoro-cold-venv`（124 packages） | passed |
| 冷安装环境 import 全部运行依赖 | `import mlx_audio, misaki, phonemizer, espeakng_loader, jieba` | passed |
| manifest probe 在冷安装环境离线通过 | `python -m macai_kokoro_runner --probe` | passed |
| 冷安装环境真实合成有效 WAV | kokoro_worker.py 驱动：`ok=true`，4.15s @ 24000Hz | passed |
| 旧 Python worker tests 基线（Phase 0） | `uv run --with pytest python -m pytest scripts/tests -v` | passed（34 tests） |
| Rust Kokoro Provider tests 基线（Phase 0） | `cargo test -p ai-daemon kokoro` | passed（4 tests） |
| 旧环境真实合成基线（Phase 0） | `.build/kokoro-venv`：`ok=true`，3.83s @ 24000Hz | passed |
| adapter 逻辑单测 | `uv run --with pytest --with-editable . python -m pytest tests/test_adapter.py` | passed（10 tests） |
| immutable HF revision 核对 | HF commit `4bd6c964…8ba7e8` 的 `model.safetensors` LFS sha256 与本地文件一致（config.json size 3380 一致）；revision 已填入 `profiles/kokoro-82m-zh.toml` | passed |
| 第二次 sync exact/no-op 与离线 sync | `uv sync --locked --no-dev` 二次运行为纯 audit；`--offline` sync 与离线 `--probe` 通过 | passed |
| 真实模型经 Runner Protocol 端到端 | `MACAI_KOKORO_SMOKE_MODEL=… scripts/tests/kokoro_runner_smoke.sh`（自带独立 frame codec）：短中文 4.45s、混合中英 5.0s、长中文 116 字 22.9s 无截断 @ 24000Hz，unload/shutdown 无残留 | passed |
| probe/entrypoint 解析到 synced venv 解释器（接线缺陷回归） | `cargo test -p ai-daemon --test runner_environment_manager probe_runs_under_the_synced_venv_interpreter` | passed（runner_environment_manager 9 tests 全绿） |
| daemon 侧 RunnerProvider 真实接线 | `MACAI_KOKORO_WIRING_MODEL=… cargo test -p ai-daemon --test runner_kokoro_real_wiring`（env-gated） | 修复后复跑结果见下节 |

Phase 2 收尾（2026-09-02 第二轮 bounded change）已完成 revision 核对、exact/no-op
与离线 sync、真实模型端到端 smoke（上表）。该轮暴露并处理：

- manifest probe 的 `-c` 脚本含 `;`，被 entrypoint 同源的 shell 元字符校验拒绝；
  probe 已改为纯 import 表达式（`runner.toml`）。
- profile `artifacts.directory = "."` 被 `safe_relative` 拒绝；已按
  [`model-profile-v1`](../specs/model-profile-v1.md) 示例改为 `kokoro-82m-zh`。

**解释器接线修复（2026-09-02 第三轮 bounded change，owner：
`ai-daemon::runners::environment`）**：真实接线曾暴露 `run_sync` 经
`UV_PROJECT_ENVIRONMENT` 把依赖装进 staging `.venv`，而 `run_probe` 与
`instance.rs` entrypoint 的 `{environment.python}` 都解析为受管**基础解释器**，
真实 probe 报 `ModuleNotFoundError: mlx_audio`。fake fixture 的 no-op probe
掩盖了该缺陷。修复后：probe 在 staging `.venv/bin/python` 上运行（sync 产物缺失时
结构化失败），entrypoint 用 `status.runtime_python()`（`<env_root>/.venv/bin/python`）
启动，基础解释器只作为 uv `--python` 输入；`status.python` 语义已在 uv 提案与
runner-manifest-v1 spec 中澄清为「uv 输入基础解释器的 provenance」。回归测试见下表
`probe_runs_under_the_synced_venv_interpreter`。

Phase 2 完成条件在接线测试通过后判定：中英文混合、长文本已由 smoke 覆盖；
daemon 侧真实接线修复后复跑结果见下表。

## Phase 3: Runner adapter

迁移现有 worker 的模型语义到 `macai_kokoro_runner`：

- 保留 G2P、英文 fallback、长文本切分和音频拼接；
- 实现 Runner Protocol v1 handshake/load/infer/cancel/unload/shutdown；
- stdout 改为 framed protocol，所有库日志留在 stderr；
- 输出写入 daemon 授权的 per-request directory；
- 返回 WAV metadata，daemon 验证路径、RIFF/WAVE 和清理；
- health/status 不获取活动 inference I/O 锁；
- 单请求失败不杀死 worker；protocol violation 和进程退出由 supervisor 处理。

Kokoro adapter unit tests 只覆盖可独立判定的文本、voice 和错误逻辑；worker lifecycle
和协议通过真实子进程集成测试覆盖。

## Phase 4: real composition

真实用户路径：

```text
MacAIConsole / macai
  → aiworkd management API
  → Model Profile resolution
  → Runner registry
  → uv environment
  → supervised Kokoro worker
  → tts.v1
  → validated WAV
  → client output
```

需要验证：

- 环境缺失时下载模型不假装 Runner ready；
- GUI 安装动作只请求 daemon，进度和错误来自 daemon；
- 环境 ready 后无需重编译 daemon/GUI 即可发现 Runner；
- Model Profile 注册并显式选择 Runner，无静默 fallback；
- 首次 load、重复请求、默认 voice 更新和 unload；
- 合成期间 `/api/runtime`、Runner/environment status 和 RSS 查询；
- 取消、timeout、worker crash、daemon restart；
- 输出文件和 worker process cleanup。

## Phase 5: cutover and removal

所有真实验收通过后，在同一 bounded change 中：

- 迁移已注册 `provider = "kokoro-mlx"` 记录，或提供显式兼容读取；
- 删除 Runtime 中 `KokoroMlxProvider` 静态装配；
- 删除 `main.rs` Kokoro provider/format/default 分支；
- 删除 GUI `PythonEnvironmentSpec.kokoroMlx` 和 Kokoro 专属安装逻辑；
- 删除 `.build/kokoro-venv` 默认探测和 `AIWORK_KOKORO_PYTHON`，除非实施证据支持
  把 override 保留为正式契约；
- 移走或删除旧 `scripts/kokoro_worker.py` 和重复测试；
- 更新 README、GUI README、API 文档和 CI；
- 对旧路径做负向搜索；
- 将两个工作提案改写为已落地决策，并把 draft specs 改写为当前契约。

不要在真实 Runner 尚未通过时提前删除旧路径，也不要在切换完成后无限期保留两套
可达实现。

## Evidence matrix

### Documentation-only stage

| Claim | Evidence | Current result |
| --- | --- | --- |
| 当前代码仍使用旧 Provider 和手工 venv | source/README inspection | inspected |
| 文档明确区分 proposal、draft contract 和 current behavior | local Markdown link check and terminology review | passed / inspected |
| 没有把提案描述成已实现 | `rg` review of status fields and README wording | inspected |
| Phase 1A 启动、版本与 trust 修订 | `cargo fmt --all -- --check`；`cargo test -p ai-daemon --lib --tests`；`cargo test --workspace` | passed at `cadfa31`（12 Runner lib + 88 daemon + 5 composition；workspace passed） |
| 旧 Python worker tests | `python3.12 -m pytest scripts/tests -v` | not run：当前 Python 3.12 环境缺少 pytest |

### Planned automated evidence

```bash
# Runner/core composition
cargo test -p ai-daemon runner_
cargo test -p ai-daemon kokoro_
cargo fmt --all -- --check
cargo test --workspace

# Kokoro locked environment and worker
uv sync --project runners/kokoro --locked --group dev
uv run --project runners/kokoro --frozen pytest -v

# GUI ownership and decoding
cd apps/MacAIConsole
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
  swift test --enable-xctest
```

测试名在实现时可以按 Rust 模块组织调整，proposal 中的 acceptance 与 failure
surface 必须保持可追踪。

### Planned real-model smoke

实现应提供仓库内 smoke script，并从环境变量读取模型目录：

```bash
MACAI_KOKORO_SMOKE_MODEL=/absolute/path/to/kokoro-82m-zh \
  scripts/tests/kokoro_runner_smoke.sh
```

脚本至少执行：

1. 使用隔离的 daemon data/runtime 目录；
2. 发现 Runner 和 environment；
3. 注册/加载指定 Model Profile；
4. 合成短中文、混合中英、长中文；
5. 验证 WAV header、sample rate、非零 duration 和长文本完整时长；
6. 合成期间并发轮询 runtime/status；
7. 取消一条长请求；
8. 终止 worker，验证 `backend_crashed` 与 daemon 存活；
9. 重新加载并再次合成；
10. unload/shutdown 后检查无 worker 和临时 WAV 残留。

真实 smoke 只在目标 Apple Silicon Mac、完整模型和已授权依赖下载存在时可判定。
Linux CI 的 fake Runner 与 Python unit tests不能替代这条证据。

## Stop conditions

出现以下任一情况时暂停 cutover，保留旧 Kokoro 路径并更新工作提案：

- uv lock 无法在目标 macOS/Apple Silicon 环境重建；
- framed protocol 引入无法解释的吞吐或长音频稳定性回归；
- 新路径无法保持推理期间 status 非阻塞；
- Runner trust/路径边界允许模型目录执行代码或越界输出；
- 环境安装失败可能破坏现有 ready 环境；
- 真实中文、英文或长文本结果未达到当前基线；
- migration 会丢失已注册模型或默认 voice。

达到 stop condition 后，以观察结果修订同一工作提案；不要通过新增专用旁路绕过通用
Runner、uv environment 或安全 owner。
