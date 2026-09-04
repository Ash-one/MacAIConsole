# Runner Manifest v1

Status: current（已实现并被产品装配消费）

Contract owner: this file

Decision owner: [`2026-09-02-runner-plugin-architecture.md`](../decisions/2026-09-02-runner-plugin-architecture.md)

本文件定义 `runner.toml` 的 schema。daemon 的 `ai-daemon::runners::manifest`
模块实现完整字段的解析与校验（含 `macai.runner.v1` schema、SemVer、协议版本、
路径/entrypoint 约束、未知字段拒绝与 `runtime.probe`——`python-uv` 必填、与
entrypoint 相同的 argv 安全校验和模板解析）。isolated fake Runner 测试覆盖
discovery、信任、digest-bound staging、Profile snapshot 和启动握手；真实 uv 的
环境行为由 `tests/runner_environment_manager.rs` 覆盖。Runtime/scheduler 组合
路径与生产装配（`main.rs` `bootstrap_runners`、HTTP 管理面、GUI）均已接入；
Kokoro 与 qwen3-asr 两个 Runner package 经此 schema 运行。

## Location

Runner package 根目录包含：

```text
<runner>/
├── runner.toml
├── pyproject.toml       # runtime.type = "python-uv" 时必需
├── uv.lock              # runtime.type = "python-uv" 时必需
├── src/ or bin/
└── profiles/
```

manifest 中的相对路径以 Runner package 根为基准。路径不得包含 `..`、逃逸 package
root 或通过 symlink 指向未授权位置。

## Example: Kokoro

```toml
schema = "macai.runner.v1"
id = "org.macai.kokoro"
version = "0.1.0"
protocols = ["macai.runner.v1"]
capabilities = ["tts.v1"]

[entrypoint]
command = ["{environment.python}", "-m", "macai_kokoro_runner"]
working_directory = "package"

[runtime]
type = "python-uv"
id = "org.macai.kokoro-python"
project = "."
lock = "uv.lock"
python = ">=3.12,<3.13"
# probe argv 与 entrypoint 同受 shell 元字符校验（禁 `;`），用纯 import 表达式
probe = ["{environment.python}", "-c", "import mlx_audio, misaki, phonemizer, espeakng_loader"]

[capacity]
max_instances = 1
max_concurrency_per_instance = 1

[timeouts]
boot_seconds = 15
load_seconds = 300
inference_seconds = 300
shutdown_seconds = 5

[security]
network_during_install = true
network_during_runtime = false
inherit_environment = ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"]

[[models]]
profile = "profiles/kokoro-82m-zh.toml"
adapter = "kokoro-mlx"
```

## Top-level fields

| Field | Required | Meaning |
| --- | --- | --- |
| `schema` | yes | 固定为 `macai.runner.v1` |
| `id` | yes | 反向域名风格稳定 ID；升级不得改变 |
| `version` | yes | Runner package 版本 |
| `protocols` | yes | 支持的 Runner Protocol 版本 |
| `capabilities` | yes | 支持的版本化 Capability Contract |
| `entrypoint` | yes | 无 shell 的 argv 模板 |
| `runtime` | yes | 环境类型和可复现输入 |
| `capacity` | yes | 实例及并发边界 |
| `timeouts` | yes | 可覆盖的默认 deadline |
| `security` | yes | 网络和环境变量权限 |
| `models` | no | 随 Runner 分发的 Model Profile 引用 |
| `default_adapter` | no | 无 bundled Model Profile 的引擎（如 llama.cpp）的 ad-hoc 绑定默认 adapter；注册路径据此为任意本地模型构造内存绑定 |
| `engine` | no | 引擎预编译产物声明（cpp 引擎 Runner 化）：`download_url` / `sha256`（强制校验）/ `binary`（压缩包内可执行文件相对路径）。install 流程在环境同步之后下载、校验、解压到 `<app-support>/Engines/<runner-id>/`，指纹写入 `.macai-engine.json`，已就绪时幂等跳过 |

未知顶层字段被拒绝，防止拼写被静默忽略。未来兼容策略在 v1 面向第三方发布前根据
真实扩展需求确定。

## ID and version

- ID 只允许 ASCII 小写字母、数字、点和连字符；
- `version` 必须是合法 SemVer；
- 用户安装的 Runner 不得使用保留前缀 `org.macai.builtin.`；
- 同一 ID 可以安装多个版本，daemon 根据 Model Profile、信任和兼容策略选择一个；
- 选择结果必须记录 requested、selected 和 reason，不允许静默换 Runner。

## Entrypoint

`entrypoint.command` 是 argv array，不经过 shell。允许的模板变量仅包括：

- `{environment.python}`——展开为环境的运行解释器（`<fingerprint>/.venv/bin/python`）；
- `{package.root}`；
- `{runtime.temp_root}`。

每个变量占据完整 argv。首个 argv 只能是批准模板变量或 package 内的普通相对路径；
绝对可执行路径（包括 `/bin/sh`）被拒绝。manifest 不得嵌入 shell pipeline、重定向或
command substitution。

`working_directory` 可为 `package` 或 daemon 创建的 `runtime` 目录。Runner 不依赖
MacAI 仓库 cwd。

## Runtime

v1 首个实现必须支持：

```toml
[runtime]
type = "python-uv"
id = "org.macai.kokoro-python"
project = "."
lock = "uv.lock"
python = ">=3.12,<3.13"
# probe argv 与 entrypoint 同受 shell 元字符校验（禁 `;`），用纯 import 表达式
probe = ["{environment.python}", "-c", "import mlx_audio, misaki, phonemizer, espeakng_loader"]
```

`id` 标识可独立复用的 environment project；两个 Runner 只有 ID、project/lock
digest、Python ABI 和平台产生的完整 fingerprint 一致时才能共享。

`project = "."` 表示 package 根目录，必须在 canonicalize 后仍位于 package root 内
且解析为目录；`lock` 必须解析为 package 内普通文件。

### Environment probe（Phase 1B contract）

`python-uv` runtime 必须声明 `probe` argv。它使用与 `entrypoint.command` 相同的模板、
首 argv 和无 shell 规则，并在已锁定环境同步完成后由 daemon 执行：

```toml
probe = ["{environment.python}", "-c", "import mlx_audio, misaki, phonemizer, espeakng_loader"]
```

`{environment.python}` 展开为该环境的**运行解释器**：`uv sync`（`UV_PROJECT_ENVIRONMENT`）
创建的 `<fingerprint>/.venv/bin/python`，lock 依赖都已同步进该 venv。manifest `[runtime].python`
声明的版本约束只选择 uv `--python` 输入的基础解释器，它不含任何依赖，不能被 probe 或
entrypoint 直接执行。probe 在 staging venv 上以 `timeouts.boot_seconds` 为 deadline 运行，
成功后才原子提升为最终 fingerprint 目录；entrypoint 在提升后的同一 venv 解释器上启动。

probe 只验证受管解释器、Runner package import 和运行依赖，不加载模型 artifact，也不
创建常驻 worker。退出码 0 表示成功，非零、超时或 signal exit 均使环境进入 `failed`。
daemon 使用清空后的环境变量和 manifest allowlist 启动 probe。`network_during_runtime`
描述 Runner 声明的运行期网络需求；v1 没有 OS 级网络沙箱，显式信任的 Runner 仍拥有
daemon 用户权限。stdout/stderr 只作为有界诊断输出处理，不进入 Runner Protocol。

该字段已由本 spec 确定，`RunnerRuntime` 解析器已实现（Phase 1B）：`python-uv`
缺少 probe、probe 首个 argv 非法或包含 shell 元字符时 manifest 拒绝解析；probe
超时、非零退出或 spawn 失败时环境进入 `failed`，不产生 ready 目录。

`python-uv` 的安装、fingerprint、staging 和升级语义由
[`2026-09-02-uv-python-environments.md`](../decisions/2026-09-02-uv-python-environments.md)
拥有。

原生 binary runtime 留作后续类型。没有实现的 runtime type 必须将 Runner 报告为
unavailable，并给出机器可判定 code 和人类可读原因。

## Capacity

- v1 当前只接受 `max_instances = 1` 与 `max_concurrency_per_instance = 1`；
- daemon 以单 Runner 单进程、单活动 inference 执行；
- 多实例或多路复用需要 stdout dispatcher、按 request ID 路由和独立容量裁决，必须通过
  后续协议/架构决策引入；manifest 不能提前声明实现无法兑现的容量。

## Timeouts

manifest timeout 是 Runner 作者建议值，daemon 可以施加更严格的系统上限。最终有效
值必须进入 status 和任务详情。环境变量不再是第三方 Runner timeout 的唯一配置面。

## Security

- `network_during_install` 声明显式环境安装是否需要网络；
- `network_during_runtime` 声明 Runner 的运行期网络需求；Kokoro/qwen3-asr 声明 false；
- `inherit_environment` 是 allowlist，daemon 默认不传递 token、credential 或完整父
  进程环境。v1 允许的完整白名单：`HOME`（受管引擎产物定位 `Engines/`）、
  `HTTP_PROXY`、`HTTPS_PROXY`、`NO_PROXY`；
- daemon 只把规范化 model root 和 request directory 写入协议，并拒绝消费越界输出；
- 安装来源、Runner package 和 lockfile digest 进入信任记录。

v1 的信任边界是本地代码信任：用户显式信任 Runner 即授予与 aiworkd 相同的 OS 用户
权限。`env_clear`、digest-bound staging 与返回路径校验提供身份、秘密最小化和数据流
约束，不构成文件系统或网络沙箱。真正的 OS 级隔离需要独立安全决策和直接逃逸测试。

## Model references

`[[models]]` 引用 package 内的 Model Profile，并指定 Runner 内 adapter。Model
Profile 拥有 artifact 和用户可配默认值；Runner manifest 拥有 adapter 可用性和
运行边界。

第三方 Model Profile 也可以引用已安装 Runner，不要求被写入 manifest。daemon
必须验证 profile 的 capability 和 adapter 被 Runner 明确支持。

## Discovery result

manifest 解析后 daemon 产生 descriptor，至少包括：

- ID、version、origin 和 trust state；
- protocol/capability；
- runtime environment identity/state；
- capacity/timeouts；
- availability reason；
- installed Model Profiles；
- artifact and lock digest。

descriptor 是 GUI/CLI 的状态 owner；客户端不自行重新解析 `runner.toml`。

## Validation failures

以下情况拒绝 Runner，且不启动任何进程：

- schema、ID、version 或 protocol 无效；
- capability 未知、空 version（例如 `tts.v`）或 entrypoint 绝对路径；
- 路径逃逸、symlink 越界或 entrypoint 需要 shell；
- Python project/lock 缺失或 digest 不匹配；
- 未实现 runtime type；
- trust record 与 package digest 不一致；
- Model Profile capability/adapter 与 Runner 不一致。

单个 Runner 无效只影响自身状态，Runner 列举继续 best-effort。

## Evidence

以下证据均已落地（实现时的验收清单收敛为当前验证义务）：

- 完整和最小 manifest parse；每个必填字段缺失的拒绝路径；
- path traversal、symlink escape 和 shell entrypoint 拒绝；
- 未知 capability/runtime 拒绝；
- v1 拒绝 1/1 之外的 capacity；
- 同 ID 多版本 SemVer 选择及 no-silent-fallback（ambiguity 结构化失败）；
- `project = "."`、Profile defaults/resources、SemVer compatibility 解析；
- `python-uv` 缺少 probe、probe 绝对 executable、probe 超时/非零退出时拒绝 ready；
- discovery 后 package 被修改时，从 digest 校验的 daemon-owned staging 副本执行
  （`PACKAGE_EXCLUDED_DIRS` 排除 `.venv` 等本地产物）；
- discovery 后修改 bundled Profile 不改变已解析的选择结果；
- boot timeout、identity 或 protocol error 后无遗留 Runner PID；
- 单个坏 manifest 只影响自身，Runner 列举 best-effort；
- descriptor 经 `/api/runners` 被 Swift 客户端解码（`DaemonAPI` tests）。
