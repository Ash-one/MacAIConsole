# Model Profile v1

Status: current（已实现并被产品装配消费；v1 对第三方作者的稳定性承诺在 Plugins
安装路径落地前保持克制）

Contract owner: this file

Decision owner: [`2026-09-02-runner-plugin-architecture.md`](../decisions/2026-09-02-runner-plugin-architecture.md)

Model Profile 是纯数据模型说明。它允许兼容现有 Runner 的新权重在不修改 daemon、
CLI 或 GUI 源码的情况下被识别、下载、注册和加载。daemon 的
`ai-daemon::runners::profile` 模块实现该格式的解析与校验（`macai.model.v1`、
immutable commit revision、typed defaults/resources 与 SemVer compatibility）；
当前 catalog 快照持久化在 `model_profiles`；模型首次注册时再复制到
`registered_model_profiles`，以 model ID 绑定 canonical JSON + sha256 digest。
`bootstrap_runners` 在 daemon 启动时优先恢复 per-model snapshot，bundled catalog
更新只影响未来注册。

GUI 通过 `GET /api/model-profiles` 读取 catalog 投影，并通过
`POST /api/model-profiles/{id}/pull` 提交 `auto_load` 意图，并可携带仅用于活动下载查询的
`progress_id`；`GET /api/downloads/{progress_id}` 返回当前文件序号、字节数与百分比。
source、artifact 清单、
model type 与 Runner 选择均由 daemon 从可信 Profile 构造；客户端不得复制或重组
Profile 下载请求。

## Example: Kokoro-82M-zh-MLX

```toml
schema = "macai.model.v1"
id = "kokoro-82m-zh"
name = "Kokoro 82M 中文"
capabilities = ["tts.v1"]
runner = "org.macai.kokoro"
adapter = "kokoro-mlx"
format = "mlx-directory"

[source]
type = "huggingface"
repo = "1038lab/Kokoro-82M-zh-MLX"
revision = "<immutable commit>"

[artifacts]
directory = "kokoro-82m-zh"
files = [
  "config.json",
  "model.safetensors",
  "voices/zf_001.safetensors"
]

[defaults]
keep_alive = "always"
voice = "zf_001"
format = "wav"

[resources]
memory_estimate_bytes = 400000000

[compatibility]
runner = ">=0.1,<0.2"
```

示例中的 revision、完整 voice files 与资源值已由 Kokoro 实施切片生成
（`runners/kokoro/profiles/kokoro-82m-zh.toml`）。

## Ownership

Model Profile 拥有：

- 模型身份、展示名称和 capability；
- artifact 来源、immutable revision、路径和完整文件集合；
- Runner 和 adapter 选择；
- 模型级默认值；
- 模型资源估算；
- Runner 兼容范围。

Runner Manifest 拥有：

- adapter 是否实现；
- environment、entrypoint、capacity、timeouts 和权限；
- 引擎级 device 与进程行为。

daemon 拥有：

- 本地安装路径、注册状态、用户 override、最终 Runner 选择；
- lease、内存裁决、load/unload 和任务；
- artifact 下载、路径安全、完整性和持久化。

## Required fields

| Field | Meaning |
| --- | --- |
| `schema` | 固定为 `macai.model.v1` |
| `id` | 稳定模型 ID |
| `name` | 用户可见名称 |
| `capabilities` | 版本化 capability contracts |
| `runner` | 明确 Runner ID |
| `adapter` | Runner 内模型家族 adapter |
| `format` | artifact 布局 |
| `source` | 来源和 immutable revision |
| `artifacts` | 完整下载与校验集合 |
| `defaults` | 可选的模型级默认值，字段见下表 |
| `resources` | 可选的模型级资源估算，字段见下表 |
| `compatibility.runner` | 可接受 Runner version range |

`format = "directory"` 时 `artifacts.directory` 是 `Models/<type>/` 下的运行时根目录；
单文件格式必须只声明一个 `artifacts.files` 项，运行时路径为
`Models/<type>/<files[0]>`。`artifacts.directory` 仍作为 Profile schema 的稳定下载分组
字段存在，不参与单文件路径拼接。

`source.type` 当前接受两种值：catalog Profile 使用 `huggingface`，并提供 repo 与
immutable revision；daemon 为显式本地路径构造的 ad-hoc 绑定使用 `local`，其 repo 与
revision 必须为空。`local` 是内部注册快照形态，当前没有通用的用户 Profile 导入 API。

## Defaults and resources schema

`[defaults]` 与 `[resources]` 都是可选 section；出现的字段必须使用下面的类型，空字符串
和 `memory_estimate_bytes = 0` 被拒绝。未出现的字段表示 Profile 没有该建议值，而不是
daemon 自行猜测默认值。

| Section / field | Type | Meaning |
| --- | --- | --- |
| `defaults.keep_alive` | string | 首次注册的 keep-alive 建议 |
| `defaults.voice` | string | capability contract 定义的默认音色 |
| `defaults.format` | string | capability contract 定义的首选输出格式 |
| `resources.memory_estimate_bytes` | positive integer | scheduler 输入的模型内存估算 |

## Resolution

1. daemon 解析并验证 Profile；
2. 定位满足 ID、version、trust 和 capability 的已安装 Runner；
3. 选择结果记录 requested/selected/reason；缺省请求以 `requested = "auto"` 记录；
4. 没有匹配 Runner 时模型保持已注册但不可加载，并返回明确安装提示；
5. daemon 不选择 Profile 未允许的 fallback Runner；
6. 用户显式选择 Runner 时，它必须满足 Profile compatibility，否则失败。

`runner = "auto"` 不进入 v1。未来如需自动选择，要有独立的可解释路由决策和稳定
优先级，不能通过扫描顺序实现。

`compatibility.runner` 使用 SemVer range。解析失败、没有可信匹配版本或多个可信版本
同时匹配均为结构化失败；range 不能授权按目录或发现顺序选择版本。

## Artifact integrity

- Hugging Face source 必须固定 immutable commit revision；
- 文件列表必须完整，不能只列代表性 shard；
- Profile 的 canonical JSON 使用 sha256 digest 固定注册快照；v1 artifact schema
  只声明文件路径，不包含逐文件 size/digest；
- 下载使用临时文件、断点续传和完成后的原子 rename；
- artifact path 必须位于 daemon 管理的 Models root；
- Profile 不得包含 executable、Python module 或自动运行 hook。

大型 voice collection 的分组或逐文件完整性元数据需要扩展 schema 和唯一 owner；
v1 不预设这套尚未实现的机制。

## Defaults and user overrides

- Profile default 是首次注册建议值；
- 用户持久化设置由 daemon registry 拥有；
- Profile 升级不得静默覆盖用户 keep-alive、voice 或其他明确 override；
- Runner 返回的 runtime limit 可以拒绝超出能力的 override；
- capability contract 拥有字段语义，Profile 不创建私有同义字段。

## Registration snapshot（Phase 1C contract）

模型注册时，daemon 持久化经过校验的完整 Profile snapshot 及其 digest，并把用户
override 作为独立状态保存。后续 Runner package 更新、移除或重新发现不能静默改写
已注册模型的 artifact、默认值或 Runner compatibility；load 时以已注册 snapshot
重新解析当前可信 Runner，并记录 requested、selected 和 reason。

catalog Runner 模型通过通用注册路径引用 Profile identity/snapshot；显式本地 GGUF
和 Whisper `.bin` 注册由 daemon 分别为 llama.cpp / whisper.cpp Runner 构造 `local`
ad-hoc Profile。两种路径都把
requested provider、最终 selected provider 与 selection reason 存入模型注册表；
`/api/models/load` 对 `org.macai.*` provider 按 descriptor 能力判定。

早于选择审计字段的数据库行在 schema 迁移时写入 `requested = "unknown"` 和明确的
历史原因；daemon 不从最终 Provider 反推当时的调用者意图。

SQLite 使用两个 owner：`model_profiles` 保存当前可发现 catalog；
`registered_model_profiles` 按 model ID 保存注册时的 snapshot/digest。删除模型同步删除
绑定；daemon 重启从 per-model 表恢复，Profile catalog 升级不会改写既有模型。

## Local profiles

当前公共入口不接受用户提供的 Model Profile，也不生成可编辑草稿。本地文件注册仅支持
daemon 已知的 ad-hoc 形态：当前为 llama.cpp GGUF；daemon 从受控注册参数构造
`source.type = "local"` 的 Profile snapshot，不执行模型文件中的代码。新增本地模型
家族需要先扩展显式注册契约，不能把任意 Profile 当作可执行配置导入。

自动识别的本地目录 Profile 可带 `routing` snapshot：`detector_id`、64 位
`manifest_digest` 与 human-readable `reason`。该段仅允许 `source.type = "local"`，随模型
注册冻结，用于重启恢复与选择审计；bundled catalog Profile 不携带它。

## Required evidence

以下证据均已落地（实现时的验收清单收敛为当前验证义务）：

- parse、required fields、defaults/resources、version range 和 path safety 单测
  （`cargo test -p ai-daemon profile::`）；
- catalog artifact list 路径校验与 Profile snapshot digest（`ModelProfile::digest`
  确定性 + registry roundtrip）；
- Runner 缺失、版本不匹配和未信任错误（discovery/trust 拒绝路径）；
- no-silent-fallback（SemVer ambiguity 结构化失败）；
- daemon 重启后恢复同一 Profile snapshot/digest（registry 重开恢复测试）；
- ad-hoc Runner 绑定在同 ID 路径刷新、rename 与 unregister 后保持一致
  （`adhoc_runner_binding_tracks_refresh_rename_and_unregister`）；
- requested / selected / reason 经 registry roundtrip，并由 `/v1/models` 与
  `/api/runtime` 暴露；
- fake Runner 经注册 Profile 完成 load/infer（`runner_plugin_composition`）；
- Kokoro profile 经推荐下载、注册、真实 TTS 路径验证（见
  [`Kokoro Runner 验证参考`](../reference/kokoro-runner-verification.md)）。
