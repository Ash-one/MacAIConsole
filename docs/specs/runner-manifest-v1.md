# Runner Manifest v1

Status: draft

Contract owner: this file

Decision owner: [`2026-09-02-runner-plugin-architecture.md`](../decisions/2026-09-02-runner-plugin-architecture.md)

本文件定义 `runner.toml` 的 schema。daemon 的 `ai-daemon::runners::manifest`
模块已实现该格式的解析与校验（含 `macai.runner.v1` schema、协议版本、路径约束
与字段拒绝），但插件目录发现、信任记录与真实 Runner 启动仍待后续切片接入；
当前 Provider/worker 不经 manifest 运行。

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

未知顶层字段在 draft 阶段拒绝，防止拼写被静默忽略。未来兼容策略在 v1 发布前根据
真实扩展需求确定。

## ID and version

- ID 只允许 ASCII 小写字母、数字、点和连字符；
- 用户安装的 Runner 不得使用保留前缀 `org.macai.builtin.`；
- 同一 ID 可以安装多个版本，daemon 根据 Model Profile、信任和兼容策略选择一个；
- 选择结果必须记录 requested、selected 和 reason，不允许静默换 Runner。

## Entrypoint

`entrypoint.command` 是 argv array，不经过 shell。允许的模板变量仅包括：

- `{environment.python}`；
- `{package.root}`；
- `{runtime.temp_root}`。

每个变量占据完整 argv 或在 v1 发布前定义明确转义；首版实现建议只允许完整 argv
替换。manifest 不得嵌入 shell pipeline、重定向或 command substitution。

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
```

`id` 标识可独立复用的 environment project；两个 Runner 只有 ID、project/lock
digest、Python ABI 和平台产生的完整 fingerprint 一致时才能共享。

`python-uv` 的安装、fingerprint、staging 和升级语义由
[`2026-09-02-uv-python-environments.md`](../decisions/2026-09-02-uv-python-environments.md)
拥有。

原生 binary runtime 留作后续类型。没有实现的 runtime type 必须将 Runner 报告为
unavailable，并给出机器可判定 code 和人类可读原因。

## Capacity

- `max_instances`：该 Runner 同时允许的 worker process 数；
- `max_concurrency_per_instance`：单实例同时处理的 inference 数；
- 值必须为正整数；
- daemon scheduler 是容量裁决 owner；Runner 仍需防御超限命令。

Kokoro 首版使用 1/1，与当前模型和 worker 行为一致。

## Timeouts

manifest timeout 是 Runner 作者建议值，daemon 可以施加更严格的系统上限。最终有效
值必须进入 status 和任务详情。环境变量不再是第三方 Runner timeout 的唯一配置面。

## Security

- `network_during_install` 只控制显式环境安装；
- `network_during_runtime = false` 是 Kokoro 首版要求；
- `inherit_environment` 是 allowlist，daemon 默认不传递 token、credential 或完整父
  进程环境；
- Runner 只能读取授权 model root、package root 和 environment；
- 输出只能写 daemon 分配的 request directory；
- 安装来源、Runner package 和 lockfile digest 进入信任记录。

manifest 声明是权限请求，daemon policy 可以收紧，不能由 Runner 自行扩大。

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
- capability 未知；
- 路径逃逸、symlink 越界或 entrypoint 需要 shell；
- Python project/lock 缺失或 digest 不匹配；
- 未实现 runtime type；
- trust record 与 package digest 不一致；
- Model Profile capability/adapter 与 Runner 不一致。

单个 Runner 无效只影响自身状态，Runner 列举继续 best-effort。

## Required evidence before implementation status

- 完整和最小 manifest parse；
- 每个必填字段缺失；
- path traversal、symlink escape 和 shell entrypoint 拒绝；
- 未知 capability/runtime 拒绝；
- 同 ID 多版本选择及 no-silent-fallback；
- 单个坏 manifest 不影响其他 Runner；
- descriptor 经 daemon API 被 Swift 客户端正确解码。
