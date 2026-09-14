# Decision: 本地模型目录检测器支持目录名关键词匹配

Status: implemented

Class: feature

Owner: this file

Related current decisions: [本地模型目录检测与 Runner 路由](2026-09-06-local-directory-model-routing.md)、[单文件 Script Runner 创建](2026-09-08-single-file-script-runner.md)、[runner-manifest-v1](../specs/runner-manifest-v1.md)

## Problem

远端模型（HF / ModelScope）下载后落盘目录为 `<owner>--<model_id>`（如 `HojoAI--Hojo-TTS-Light-40M`）；
而单文件 Script Runner 与许多轻量开源模型的文件构成极其通用（仅包含 `config.json` 与 `model.safetensors`
或 `model.onnx`，内部无独占标识）。既有 `local_detectors` 仅检测目录内部相对文件与 JSON 内容，完全不感知
根目录自身的文件夹名。若单文件 Runner 仅依赖通用文件特征，会导致所有同格式模型目录均被匹配并以 `ambiguous`
拒绝；若企图利用模型名称区分，又因下载时的 `<owner>--` 前缀而无法在不知道确切 owner 的情况下进行全等匹配。

## Decision

在 `runner-manifest-v1` 的 `[[local_detectors]]`（以及单文件 Runner 的 `[[tool.macai.local_detectors]]`）中
新增可选的 `directory_contains` 字段：

1. **复合 AND 门禁**：`directory_contains` 声明一个或多个子串。仅当候选目录本身的文件夹名
   （`root.file_name()`）包含其中所有子串，**且**原有的 `required_files`、`required_directories`、
   `required_globs`、`json_predicates` 与 `required_absent` 全部满足时，探测器才返回命中。禁止
   单凭目录名做弱模式路由推断，避免同名空目录或无关文件被误识别。
2. **前缀解耦**：基于子串匹配（Case-Sensitive Substring），天然兼容 GUI/API 远端下载生成的
   `<owner>--<model_id>` 目录名与用户手动下载/解压的纯 `<model_id>` 目录名。
3. **输入校验与安全边界**：`directory_contains` 中的子串非空，禁止包含路径分隔符（`/` 或 `\`）或 `..`，
   仅作为叶子文件夹名的字面量约束；未知字段依然严格拒绝。
4. **单点权威**：继续由 daemon `aiworkd` 的 `inspect_local_directory` 拥有检测逻辑，GUI / CLI 无平行规则。

## Alternatives considered

**直接采用全局目录名模糊匹配。** 放弃文件签名而仅根据目录名关键词路由。该方案会导致假阳性（如包含 "tts"、"chat"
的普通目录被误认）与多 Runner 冲突（触发 `ambiguous`），违反本地模型检测的排他性确定原则；否定。

**在下载端去除 `<owner>--` 命名。** 修改 `pull.rs` 仅使用 repo name。会破坏多组织同名模型的隔离机制，
且未解决通用文件在单文件 Runner 下无法通过模型名特征建立门禁的根本问题；否定。

**仅支持完整 Glob（如 `*Hojo-TTS*`）。** 单纯的子串包含比 Glob 意图更明确、实现无正则/通配歧义、
对用户更不易出错；否定。

## Consequences

- 单文件 Script Runner 作者与外部 AI 生成器可以通过 `directory_contains = ["<ModelID>"]` +
  `required_files = [...]` 轻松适配通用 safetensors/onnx 模型，不再受 HF 下载前缀影响。
- 现有 Runner manifest 缺省 `directory_contains` 为空数组，行为完全保持原有逻辑，向后兼容。
- 目录名大小写敏感，用户自定义修改目录名时需保留核心模型标识子串。

## Verification

- `manifest.rs` 测试：验证 `directory_contains` 的 TOML 反序列化、合法子串通过、空子串 / 含斜杠 / 含 `..` 被拒绝。
- `script.rs` 测试：验证单文件 Runner 的 metadata 块正确解析 `directory_contains` 并透传至生成的 `runner.toml`。
- `inspection.rs` 测试：
  - 目录名包含指定子串且文件完整时匹配成功（覆盖 `<owner>--<model_id>` 与 `<model_id>` 两种场景）；
  - 目录名不包含指定子串时匹配失败（避免假阳性）；
  - 多子串全匹配要求；
  - 空 `directory_contains` 保持原有行为。
- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` 全套通过。
