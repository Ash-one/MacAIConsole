# Decision: TTS 音色清单覆盖参考音频型 Runner

Status: implemented

Class: fix

Owner: this file

Related current decisions: [单文件 Script Runner 创建](2026-09-08-single-file-script-runner.md)、[Runner Provider 可用性与模型绑定解耦](2026-09-13-runner-provider-availability-unbound.md)、[TTS 音色滚轮快速切换与即时试听](2026-09-13-tts-voice-wheel-selection.md)

## Problem

参考音频克隆型 Runner（Hojo-TTS-Light-80M：音色 = 模型目录内的相对 WAV 路径）在
GUI 中没有任何音色可选。`/api/models/{id}/voices` 的清单逻辑只认两种约定：Qwen3-TTS
的内置 speaker 常量表，与 Kokoro MLX 式的 `voices/*.safetensors` 文件名。Hojo 模型
`voices/` 目录里放的是 `.wav` 文件，扫描结果为空，GUI 音色选择器因此不渲染；而
speech 端点不会注入模型 default_voice，客户端也就无法发出带音色的请求——注册了
default_voice 也没用。

## Decision

`list_model_voices` 的非 Qwen3 分支抽取为 `collect_model_voices`，音色枚举扩展为：

- `voices/*.safetensors` → 以文件名（去扩展名）为音色（Kokoro 约定，不变）；
- `voices/*.wav`（大小写不敏感，仅普通文件）→ 以 `voices/<文件名>` 相对路径为音色
  （参考音频型 Runner 的原生 voice 语义，脚本按相对路径解析并校验目录边界）；
- 已注册的 `default_voice` 始终并入清单（去重），路径型音色即使被改名或目录暂缺
  也在 GUI 可见可选。

两种约定的值都是 Runner 能直接消费的 voice 标识符，wire 语义不变：GUI 选择什么就
原样作为 `voice` 发送。GUI 侧无需改动——清单非空后选择器自然渲染并预选 default。

## Alternatives considered

**Runner 改用裸音色名（`voices/<name>.wav` 由脚本自行拼接）。** 需要用户重走
Script Runner 创建流程换脚本，且相对路径语义本身合法（脚本已有目录边界校验），
为迁就清单端点改脚本方向反了；否定。

**GUI 对空清单显示自由文本输入。** 绕开 daemon 的事实来源，音色值不受清单约束、
与滚轮控件的交互模型（settle 后持久化 default）冲突；否定。

**只把 default_voice 显示为静态文本。** 换参考音频仍要 API/CLI，GUI 无法在多个
参考音色间切换；扩展枚举才是完整闭合。

## Verification

- `main.rs` 单元测试
  `collect_model_voices_lists_safetensors_stems_wav_paths_and_keeps_default`：两种
  扩展名分别产出文件名与相对路径、`.txt` 与 `.wav` 目录项被排除、default_voice
  去重并入、无路径时返回空。
- `cargo fmt --all -- --check` 与 `cargo test --workspace` 通过（2026-09-14）。
- 真实路径：`GET /api/models/HojoAI--Hojo-TTS-Light/voices` 返回
  `voices/reference.wav`，GUI 音色选择器可见并预选；试听出声。

## Consequences

- `voices/` 目录成为两种音色约定共存的命名空间：`.safetensors` 与 `.wav` 各按
  自己的 Runner 语义解释；同一模型目录混放两者时清单同时呈现两类值。
- `default_voice` 可能指向已被删除的 WAV（用户清理后）——清单仍会列出它，合成时
  由 Runner 报错提示文件缺失，GUI 不做存在性过滤（诚实呈现注册状态）。
- Qwen3-TTS 分支的常量表行为不变。
