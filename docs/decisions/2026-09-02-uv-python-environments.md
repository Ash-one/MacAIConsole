# Proposal: uv 管理全部 Python 环境

Status: proposed

Class: architecture

Owner: this file

## Problem

MacAI 的 Python worker 环境目前由多个位置共同定义：SwiftUI 保存包名和 venv
路径，Provider 保存解释器探测路径和安装提示，README 保存手工 `venv`/`pip`
命令，CI 使用另一套直接 `pip install`。同一个环境还可能被多个 Provider 以隐式
约定复用。

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

本提案只描述未来行为。当前 Provider 和 GUI 仍使用已有 venv 路径，直到 Kokoro
迁移切片通过全部验收。

## Proposal

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
证据修订为 `--managed-python` 强制策略。当前提案不预先制造这一限制。

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

daemon 负责定位并报告 `uv`，GUI 只消费状态。解析顺序提案为：

1. 明确配置的 `MACAI_UV_PATH`；
2. 应用随附或 MacAI 受管 tools 目录中的固定版本；
3. 开发态 `PATH` fallback。

实施切片必须在代码或受管 tool manifest 中记录经过测试的 uv version/range，并在
状态接口中返回实际版本。本机当前观测到 `uv 0.9.21`，这只是设计期环境证据，
不构成未来版本承诺。

### Network and cache

- 安装只能由用户显式触发，并展示下载、解析、同步和 probe 状态；
- daemon 统一传递已经脱敏的代理与 index 配置；
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
- Python implementation、version 和 source（managed/system）；
- environment path；
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
- GUI 请求 daemon 安装、取消、重试或检查环境；
- GUI 展示 daemon 返回的 phase、实际 Python/uv 版本和错误；
- daemon 重启后可从磁盘和环境注册信息恢复状态；
- CLI 后续提供等价环境管理命令，具体命令名在实现切片决定。

## Kokoro first slice

Kokoro 的 uv project 是首个完整实例。它必须锁定当前 worker 真正需要的依赖，包括
`mlx-audio`、`numpy`、`misaki` 中文/英文依赖、phonemizer 和 espeak 相关包。

依赖版本以真实冷安装、import、模型加载、中英文与长文本合成结果为准。当前 README
中的未固定包名只能作为调查输入，不能直接抄成最终 lock。解析或模型行为不正确时，
在 Kokoro 工作提案内记录观察，再调整 project 和 lock。

## Migration

1. 引入 daemon-owned uv probe、环境 identity 和状态模型；
2. 在 `runners/kokoro/` 创建 `pyproject.toml` 与 `uv.lock`；
3. GUI 的 Kokoro 安装动作改为请求 daemon 安装该 environment；
4. Kokoro Runner 从环境 registry 获取 entrypoint；
5. 冷安装和真实模型验证通过后，删除 Kokoro 的 `.build/kokoro-venv` 默认路径与
   GUI 包数组；
6. 逐个迁移其他 Python Runner；
7. 全部迁移完成后，从产品代码、README 和 CI 中删除 `venv`/`pip` 环境管理路径。

迁移期间现有 `AIWORK_*_PYTHON` override 仍是当前行为。是否长期保留自定义解释器
override 需要根据 Kokoro 切片的调试和第三方使用价值决定；本提案不提前承诺删除。

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

## Acceptance criteria and evidence

| Acceptance | Failure surface | Direct evidence | Result |
| --- | --- | --- | --- |
| 全新机器状态可只通过 uv project+lock 创建 Kokoro 环境 | tool discovery、Python acquisition、resolution | 清空专用测试 runtime 后执行 Kokoro cold install smoke | not run |
| 相同 project+lock 在重复同步后产生同一 fingerprint 且无多余包 | identity、exact sync | 环境管理单测；`uv sync --locked` 后 `uv tree --frozen` 记录 | not run |
| lock 与 pyproject 不一致时安装在修改现有 ready 环境前失败 | lock enforcement、atomicity | 使用过期 lock fixture 的集成测试 | not run |
| 安装失败或取消不破坏当前可用环境 | staging、cancellation、promotion | 受控失败 Runner environment integration test | not run |
| 推理期间环境升级不替换进程正在使用的目录 | lease、upgrade lifecycle | 确定性并发测试 | not run |
| GUI 不再创建 venv 或执行 pip/uv，只调用 daemon | ownership boundary | Swift request tests和产品代码负向搜索 | not run |
| 已迁移 Runner 的产品路径不存在 `python -m venv` 或 pip 安装 | complete removal | `rg -n "python.*-m venv|python.*-m pip|/pip(3)? install"` 定向检查 | not run |
| CI 使用提交的 lock 运行 Kokoro worker tests | reproducibility | `uv sync --project runners/kokoro --locked --group dev`；`uv run --project runners/kokoro --frozen pytest -v` | not run |
| 离线启动 ready 环境和真实 Kokoro 合成不访问 package index | runtime/network boundary | `UV_OFFLINE=1` 的 Runner smoke 与网络观察 | not run |

## Risks

- `uv` 成为产品工具依赖，需要独立处理版本、下载完整性、代理和更新。
- macOS native wheel 与 Python patch 版本可能影响 MLX 依赖；fingerprint 和真实模型
  smoke 必须覆盖实际 interpreter。
- exact sync 会删除环境中的手工包。受管环境明确不可由用户手工修改，自定义实验
  使用独立 environment/override。
- 自动 Python 下载来自 Astral 使用的 `python-build-standalone` 分发，需要在正式
  发布前完成许可、来源和校验审查。
- 多个 immutable 环境会增加磁盘占用。垃圾回收必须尊重 worker 和 lease，且不属于
  Kokoro 首个切片的完成条件。
