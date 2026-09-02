# Model Profile v1

Status: draft

Contract owner: this file

Decision owner: [`2026-09-02-runner-plugin-architecture.md`](../decisions/2026-09-02-runner-plugin-architecture.md)

Model Profile 是纯数据模型说明。它允许兼容现有 Runner 的新权重在不修改 daemon、
CLI 或 GUI 源码的情况下被识别、下载、注册和加载。daemon 的
`ai-daemon::runners::profile` 模块已实现该格式的解析与校验（`macai.model.v1`，
含 immutable commit revision 约束），但模型注册与 `/api/models/load` 尚未接入
该格式，当前 `ModelSpec` 与推荐模型清单仍按旧路径工作。

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

示例中的 revision、完整 voice files 和资源值必须由 Kokoro 实施切片生成；占位内容
不能作为最终推荐清单。

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
| `compatibility.runner` | 可接受 Runner version range |

## Resolution

1. daemon 解析并验证 Profile；
2. 定位满足 ID、version、trust 和 capability 的已安装 Runner；
3. 选择结果记录 requested/selected/reason；
4. 没有匹配 Runner 时模型保持已注册但不可加载，并返回明确安装提示；
5. daemon 不选择 Profile 未允许的 fallback Runner；
6. 用户显式选择 Runner 时，它必须满足 Profile compatibility，否则失败。

`runner = "auto"` 不进入 v1。未来如需自动选择，要有独立的可解释路由决策和稳定
优先级，不能通过扫描顺序实现。

## Artifact integrity

- Hugging Face source 必须固定 immutable commit revision；
- 文件列表必须完整，不能只列代表性 shard；
- 每个文件最终需要 size 和 digest；
- 下载使用临时文件、断点续传和完成后的原子 rename；
- artifact path 必须位于 daemon 管理的 Models root；
- Profile 不得包含 executable、Python module 或自动运行 hook。

大型 voice collection 可引用经过版本化和 digest 固定的 artifact group；该机制在
真实 Kokoro profile 过大时再设计，v1 draft 不预设第二套清单 owner。

## Defaults and user overrides

- Profile default 是首次注册建议值；
- 用户持久化设置由 daemon registry 拥有；
- Profile 升级不得静默覆盖用户 keep-alive、voice 或其他明确 override；
- Runner 返回的 runtime limit 可以拒绝超出能力的 override；
- capability contract 拥有字段语义，Profile 不创建私有同义字段。

## Local profiles

用户可以导入本地 Model Profile 或由 daemon 为已存在 artifact 生成草稿。生成只基于
文件和可安全解析的 metadata，不执行模型代码。用户确认 Runner/adapter 后才注册。

## Required evidence before implementation status

- parse、required fields、version range 和 path safety 单测；
- 完整 artifact list 与 digest 校验；
- Runner 缺失、版本不匹配和未信任错误；
- no-silent-fallback；
- 用户 override 在 Profile 升级后保持；
- 一个未编入 daemon/GUI 的测试 Profile 完成注册和 fake inference；
- Kokoro profile 经推荐下载、注册、真实 TTS 路径验证。
