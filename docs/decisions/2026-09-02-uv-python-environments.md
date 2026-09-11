# Decision: uv 管理全部 Python 环境

Status: implemented

Class: architecture

Owner: this file

## Problem

本决策引入前，Python worker 环境由 SwiftUI、Provider、README 和 CI 分别
定义，且可能由多个 Provider 以隐式约定复用。

由此产生的问题包括：

- 环境内容没有统一 lock owner，安装结果随时间漂移；
- GUI、daemon、文档和 CI 可能安装不同依赖；
- Provider ID 被用作环境 ID，无法准确表达共享与依赖冲突；
- 安装依赖仓库 `.build/` 和 shell 中已有 Python，不适合正式 app 分发；
- `pip install -U` 会原地改变正在被 worker 使用的环境；
- 失败安装、升级和回滚没有统一的原子性及可观测状态。

问题保持成立，无论最终选择 `uv`、Conda、venv+pip 或打包解释器。

## Decision authority

用户已明确授权采用 `uv` 管理所有 Python 环境。该授权覆盖新的工具依赖、由 `uv`
解析和同步 Python 包，以及在用户触发安装时下载受管 Python。它不授权后台静默
联网、执行模型仓库代码或删除仍被 worker 使用的环境。

当前七个 built-in Runner 全部运行在 daemon-owned uv 环境上；llama.cpp 与
whisper.cpp Runner 还由同一 install 流程管理受校验的原生引擎产物。

## Decision

`uv` 成为 MacAI Python Runtime Environment 的唯一创建、解析、同步、执行和探测
工具。产品路径不再调用：

- `python -m venv`；
- `pip`、`python -m pip` 或 venv 内的 `pip`；
- GUI 自己维护的包数组；
- 对共享环境执行原地 `pip install -U`。

### Environment project

每个 Python Runtime Environment 是一个标准 `uv` project。首版由 Runner package
内嵌；未来多个 Runner 需要共享时，可以引用同一个独立版本化 environment project：

```text
runners/<runner-id>/
├── runner.toml
├── pyproject.toml
├── uv.lock
├── src/
└── tests/
```

`pyproject.toml` 拥有直接依赖和 `requires-python`；`uv.lock` 是完整解析结果。两者
必须提交并一起评审。Runner 安装不得临时拼接包名，也不得在首次运行时重新解析
未锁定版本。

单文件 Script Runner 是本地作者入口的受控例外：用户点击“信任并添加”后，daemon 在
staging 中从 PEP 723 依赖生成 `pyproject.toml` 与 `uv.lock`，校验 package 并记录 digest；
后续环境安装仍只消费该 lock，不在首次推理时解析依赖。该边界由
[`单文件 Script Runner`](2026-09-08-single-file-script-runner.md) 拥有。

`uv sync` 默认执行 exact sync，会移除 lock 外包。MacAI 使用：

```bash
UV_PROJECT_ENVIRONMENT=<staging-env> \
  uv sync --project <runner-project> --locked --no-dev
```

`--locked` 要求 `uv.lock` 与 project metadata 一致。已安装环境的运行不触发 sync；
worker 直接使用受管环境中的 Python entrypoint。开发和测试通过 `uv run --locked`
进入同一 project。

### Python ownership

Runner manifest 声明 Python implementation 和版本约束，例如 CPython 3.12。`uv`
负责发现或下载满足约束的 Python。首个实现允许使用 `uv` 管理的 CPython，也允许
已有系统 CPython 满足约束时被选择；实际 interpreter 来源必须进入环境状态和日志。

如果后续验证发现系统 Python 与 managed Python 的二进制兼容性不同，再通过新的
证据修订为 `--managed-python` 强制策略。本决策不预设这一限制。

### Environment identity

环境身份由稳定 environment ID 和不可变 fingerprint 组成。fingerprint 至少覆盖：

- environment project ID 和 version；
- `pyproject.toml` digest；
- `uv.lock` digest；
- Python implementation、major/minor 与 ABI；
- OS、architecture；
- 影响解析或原生 wheel 的 uv source/config digest。

环境路径：

```text
~/Library/Application Support/MacAIConsole/
└── Runtimes/
    └── python/
        └── <environment-id>/
            └── <environment-fingerprint>/
                └── .venv/
```

两个 Runner 只有 environment ID 和 fingerprint 都完全一致时才复用环境。共享是环境身份的结果，
不通过 Swift 数组、Provider ID 或共同 venv 名称推断。

### Atomic install and upgrade

daemon 是环境操作的唯一 owner：

1. 为目标 fingerprint 获取跨任务安装锁；
2. 在同一父目录创建 staging environment；
3. 执行 `uv sync --locked --no-dev`；
4. 运行 Runner 声明的只读 probe；
5. probe 成功后原子提升为最终目录；
6. 更新环境注册状态；
7. 失败时保留当前可用环境，清理或标记 staging 供诊断。

升级创建新 fingerprint。仍有 resident worker 或 lease 的旧环境保持可用；没有
消费者后才可由明确的垃圾回收策略删除。首个 Kokoro 切片不自动删除旧环境。

### uv executable

daemon 负责定位并报告 `uv`，GUI 只消费状态。解析顺序为：

1. 明确配置的 `MACAI_UV_PATH`；
2. macOS App Bundle 内置产物（`Contents/MacOS/uv` 或 `Contents/Resources/uv`）；
3. `PATH` 中的可执行文件 fallback；
4. 常见系统与用户安装路径 fallback（`/opt/homebrew/bin/uv`、`/usr/local/bin/uv`、`~/.local/bin/uv`、`~/.cargo/bin/uv`）。

代码以 `TESTED_UV_VERSION = "0.9.21"` 和 CI 配置记录经过测试的版本，并在状态接口
返回实际版本；当前不会拒绝其他 uv 版本。独立打包（`build-app.sh`）自动提取固定版本
`uv` 静态二进制放入 App Bundle，实现完全自包含运行。

### Network and cache

- 安装只能由用户显式触发，并展示下载、解析、同步和 probe 状态；
- daemon 统一传递已经脱敏的代理配置（HTTP_PROXY/HTTPS_PROXY/NO_PROXY）；
- 安装使用 **uv 默认共享 cache**（不强制私有 `UV_CACHE_DIR`）：热 cache 时
  `--locked` 安装离线即可完成，失败重试复用已下载 artifact；
- 受管 Python（python-build-standalone，GitHub）下载镜像经
  `MACAI_UV_PYTHON_INSTALL_MIRROR` 透传（默认关闭；2026-09-02 实测 TUNA/
  SJTU/华为等公共 github-release 镜像均未托管该仓库资产，需运维提供可用镜像）；
- PyPI 镜像加速边界（2026-09-02 实测记录）：`uv.lock` v1 逐包写死
  `registry = "https://pypi.org/simple"` 与 files.pythonhosted.org 直链，
  把 default index 换成 TUNA 会让 `uv sync --locked` 报「lockfile needs to be
  updated」而拒绝安装。已提交 lock 保持官方源身份，仓库可移植；镜像/加速只经
  共享 cache 预热或代理实现，不得替换 lock 的 index 身份；
- API token 不写入 manifest、lock、任务历史或日志；
- 默认使用 uv 自身 cache，同 fingerprint 的失败重试复用已下载 artifact；
- 离线 probe 和 worker 启动不得访问网络；
- `uv sync --offline --locked` 只用于验证 cache 完整性，不作为首次安装保证。

uv 官方文档说明其 project 使用 `pyproject.toml`、`uv.lock` 和专属环境，`uv sync`
默认 exact sync：<https://docs.astral.sh/uv/concepts/projects/sync/>。自定义环境路径由
`UV_PROJECT_ENVIRONMENT` 支持：<https://docs.astral.sh/uv/concepts/projects/config/>。
uv 也可以按需管理 Python：<https://docs.astral.sh/uv/guides/install-python/>。

## Runtime status contract

daemon 对每个 Python 环境至少报告：

- environment ID 和 fingerprint；
- runner consumers；
- phase：`missing / resolving / syncing / probing / ready / failed`；
- uv version；
- Python implementation、version 和 source（managed/system）——`status.python`
  描述 uv `--python` 输入的基础解释器（不含 lock 依赖）；环境的**运行解释器**
  固定为 `<environment path>/.venv/bin/python`，由 `status.path` 派生
  （`EnvironmentStatus::runtime_python`），probe 与 Runner entrypoint 的
  `{environment.python}` 模板都解析到它；
- environment path（受管 fingerprint 目录：`<environment-id>/<fingerprint>/`）；
- lock digest；
- install/update time；
- structured failure kind、message 和可重试性；
- 当前 worker/lease consumers。

`available`、`ready` 和 `resident` 继续分离：

- environment `ready` 表示 lock 已同步且 probe 成功；
- Runner `available` 表示其 manifest、环境和 executable 均可用；
- Worker `resident` 表示已有模型进程。

## GUI and CLI

- GUI 不直接执行 `uv` 或 Python；
- GUI 请求 daemon 安装或重试，并通过状态接口检查环境；
- GUI 展示 daemon 返回的 phase、实际 Python/uv 版本和错误；
- daemon 重启后可从磁盘和环境注册信息恢复状态；
- CLI 当前没有独立环境管理命令；自动化调用 daemon `/api/runners` 接口。

## Kokoro first slice

Kokoro 的 uv project 是首个完整实例。它必须锁定当前 worker 真正需要的依赖，包括
`mlx-audio`、`numpy`、`misaki` 中文/英文依赖、phonemizer 和 espeak 相关包。

依赖版本以真实冷安装、import、模型加载、中英文与长文本合成结果为准。当前 README
中的未固定包名只能作为调查输入，不能直接抄成最终 lock。解析或模型行为不正确时，在迁移路线内记录观察，再调整 project 和 lock。

## Verification history

截至 2026-09-02 commit `cadfa31`，`ai-daemon::runners::environment` 只实现 fingerprint。
Phase 1B（daemon-owned `python-uv` environment manager）随后在同日 bounded change
实现，完成面与本节第 1–8 项一一对应：

1. `EnvironmentManager::resolve_uv`（`MACAI_UV_PATH` → PATH 绝对路径 fallback，
   实测版本缓存）；Python 来源经 `uv python install` + `--offline find` 解析；
2. 以 environment ID 为 key 的单飞异步安装锁 + 磁盘持久状态（`ready.json`）；
3. staging `uv sync --locked --no-dev`（`UV_PROJECT_ENVIRONMENT` 指向与最终
   fingerprint 目录同父级的 `installing/`）；
4. manifest `runtime.probe` 契约执行（离线、`boot_seconds` deadline、清空环境
   变量 + allowlist）；
5. probe 成功后同目录 rename 原子提升；失败/取消保留已有 ready 环境；
6. `missing/resolving/syncing/probing/ready/failed` descriptor，状态查询只读
   内存快照、不等待安装锁；
7. daemon 重启恢复 ready/failed，遗留 `installing/` 标记为 failed；
8. `tests/runner_environment_manager.rs` 8 个真实 uv isolated integration tests
   覆盖 cold sync、重复 exact sync、stale lock、probe failure（非零退出与超时）、
   取消、promotion 与重启恢复；CI 通过 `astral-sh/setup-uv` 固定 uv 0.9.21。

它不创建 Kokoro Runner，也不修改现有 Provider 或 GUI 安装路径（boundary 保持）。
该 slice 的直接证据和后续 Runtime/scheduler 接线顺序由
[`kokoro-runner-verification.md`](../reference/kokoro-runner-verification.md) 记录。

**解释器语义修正（2026-09-02，真实 Kokoro Runner 接线暴露并修复）**：
真实接线曾暴露 `run_probe` 与 entrypoint 的 `{environment.python}` 解析为受管
基础解释器，而 `uv sync` 经 `UV_PROJECT_ENVIRONMENT` 把依赖写入 staging
`.venv`——真实 probe 因此报 `ModuleNotFoundError`。fake fixture 的 no-op probe
掩盖了该语义分裂。修复后：probe 与 entrypoint 解析为 `<fingerprint>/.venv/bin/python`
（运行解释器，见上方 status contract 与 `EnvironmentStatus::runtime_python`），
基础解释器只作为 uv `--python` 输入；probe 在 staging venv 上运行、entrypoint 在
提升后的同一 venv 上启动。回归证据：`runner_environment_manager.rs` 新增
`probe_runs_under_the_synced_venv_interpreter`（probe 写入 sys.path，断言含
`installing/.venv/`），并复跑真实 Kokoro Runner 接线（结果见
[`kokoro-runner-verification.md`](../reference/kokoro-runner-verification.md) 证据路径）。

## Current scope

- 七个 built-in Runner 各自提交 `pyproject.toml` 和 `uv.lock`；
- daemon 拥有 uv/Python 定位、fingerprint、单飞安装、probe、原子提升和重启恢复；
- GUI 只调用 `POST`/`DELETE /api/runners/{id}/install` 并展示 daemon descriptor；卸载由
  daemon 串行删除受管环境，客户端不接触环境目录；
- 产品代码中不存在 legacy Python Provider、`python -m venv`、`pip install`
  或 GUI 包数组 owner；
- CI 对七个 Runner 分别执行 `uv lock --check` 和纯 adapter pytest。真实
  MLX/模型接线仍属于目标 macOS smoke，Linux CI 不宣称覆盖该边界。

## Alternatives considered

**继续使用 `venv` + pip。** 标准库可用、额外工具少。它需要自行实现 lock、Python
下载、exact sync、cache 和多环境复现，并延续当前多 owner 问题。

**一个仓库级共享 uv workspace/environment。** 依赖安装和磁盘占用更少。所有
Runner 被迫共享一次解析，任一新模型的冲突会升级为全局冲突，也无法独立发布。

**每个模型一个环境。** 隔离最强。大量兼容权重会重复安装相同依赖，模型 ID 也不
是运行环境的真实兼容边界。

**Conda/Mamba。** 能管理更多原生依赖，分发体积、solver 复杂度和 Apple Silicon
包源约束高于当前 Python worker 所需范围。原生 Runner 依赖继续由独立 artifact
机制拥有。

## Verification

下方表格区分环境管理组合、纯 adapter 单测与真实模型 smoke。

| Acceptance | Failure surface | Direct evidence | Result |
| --- | --- | --- | --- |
| 全新机器状态可只通过 uv project+lock 创建 Kokoro 环境 | tool discovery、Python acquisition、resolution | `tests/runner_environment_manager.rs` cold sync（真实 uv，隔离 runtime） | passed（Phase 1B，8 tests） |
| 相同 project+lock 在重复同步后产生同一 fingerprint 且无多余包 | identity、exact sync | `runner_environment_manager.rs` exact/no-op sync test；Kokoro 二次 sync 纯 audit（Kokoro 验证参考） | passed（2026-09-02） |
| lock 与 pyproject 不一致时安装在修改现有 ready 环境前失败 | lock enforcement、atomicity | `runner_environment_manager.rs` stale lock test | passed（Phase 1B） |
| 安装失败或取消不破坏当前可用环境 | staging、cancellation、promotion | `runner_environment_manager.rs` probe failure/timeout/cancel/promotion tests | passed（Phase 1B） |
| 推理期间环境升级不替换进程正在使用的目录 | lease、upgrade lifecycle | staging→promotion 原子 rename 设计 + `ready.json` 持久状态；运行中环境目录不可变 | passed by design（staging 提升） |
| GUI 不创建 venv 或执行 pip/uv，只调用 daemon | ownership boundary | `EngineEnvironment` / `PythonEnvironment` 负向搜索 + Swift 测试 | passed |
| 产品路径不存在 `python -m venv` 或 pip 安装 | complete removal | legacy Python Provider/worker/环境 manager 负向搜索 | passed |
| CI 检查七个提交 lock 并运行每个 Runner adapter 单测 | reproducibility | `.github/workflows/ci.yml` matrix；`uv lock --check --project runners/<pkg>` + `PYTHONPATH=... pytest` | passed locally（2026-09-05，42 tests） |
| ready 环境启动不再访问 package index | runtime/package-resolution boundary | `uv sync --offline` + 离线 `--probe`（Kokoro 验证参考）；worker 直接使用 ready venv | passed（2026-09-02；不等同 OS 网络沙箱） |

## Consequences

- `uv` 成为产品工具依赖，需要独立处理版本、下载完整性、代理和更新。
- macOS native wheel 与 Python patch 版本可能影响 MLX 依赖；fingerprint 和真实模型
  smoke 必须覆盖实际 interpreter。
- exact sync 会删除环境中的手工包。受管环境明确不可由用户手工修改，自定义实验
  使用独立 environment/override。
- 自动 Python 下载来自 Astral 使用的 `python-build-standalone` 分发，需要在正式
  发布前完成许可、来源和校验审查。
- 多个 immutable 环境会增加磁盘占用。垃圾回收必须尊重 worker 和 lease，且不属于
  Kokoro 首个切片的完成条件。
