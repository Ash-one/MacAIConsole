# 本地模型目录检测与 Runner 路由

Status: implemented

Class: feature

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[Model Profile v1](../specs/model-profile-v1.md)

## Problem

目录型 MLX、ASR 与 TTS 模型没有可靠的本地注册路径：仅由 GUI 或文件名推断会复制
Runner 知识，并可能把同一目录在不同机器路由到不同后端。

## Decision

`aiworkd` 通过 `POST /api/models/inspect` 对候选目录执行只读、受限检测。可信 Runner 的
`runner.toml` 可声明 `[[local_detectors]]`；规则只包含安全相对路径、受限 glob、JSON
Pointer 标量比较和 reason。检测遍历拒绝 symlink，限制深度、条目数与 metadata 大小，且
从不执行模型文件、Python、shell 或 Runner hook。

唯一匹配返回五分钟 routing token。`POST /api/models/load` 只有携带该 token 才自动选择
Runner；它再次 canonicalize、检测目录 fingerprint，并验证 detector 与 trusted manifest
digest。token 与显式 `provider` / `model_type` 互斥。无匹配返回 `unsupported`，多匹配返回
`ambiguous`，二者都不会自动注册或启动 worker。

自动注册将 selected Runner、adapter、capability、detector ID、manifest digest 与 reason
冻结在 `source.type = "local"` 的 Model Profile snapshot。`ModelSpec` 继续保存
`requested_provider = "auto"`、selected provider 与选择理由；重启从 snapshot 恢复，不受
后续 catalog 或 detector 修改影响。

MacAIConsole 扫描模型仓库的顶层目录并调用同一 API；CLI 的 `macai inspect <path...>`
输出原始诊断，`macai load <path> --routing-token <token>` 完成注册。Swift 与 CLI 不包含
目录到 Runner 的规则。

首批规则覆盖 MLX Llama text、Qwen3-ASR、Qwen3-TTS CustomVoice、Kokoro 与 sherpa-onnx
transducer。`.gguf` / `.bin` 保留现有单文件路径。

## Alternatives considered

- Swift `directoryProvider`：会造成 GUI 与 daemon 的双重路由 owner。
- 目录名、`config.json` 或扩展名猜测：无法区分同为 safetensors 的 LLM、ASR 与 TTS。
- Runner 探测脚本：把浏览目录变成执行本地代码的路径。
- 多个匹配取第一个可用 Runner：环境状态会改变模型语义。

## Consequences

静态签名只说明受支持的声明布局；实际 load、chat template、内存和质量仍由 Runner load
验证。唯一匹配会保守地拒绝部分通用 Hugging Face 目录。fingerprint 是有界目录元数据，
不替代本地文件系统完整性保护。

## Verification

`cargo check -p ai-daemon --bin aiworkd`、`cargo check -p ai-cli` 和
`cargo test -p ai-daemon --lib` 覆盖 manifest/schema、检测器 glob、Runtime snapshot 和
daemon 编译。Swift API/视图测试使用 `swift test --enable-xctest`；若运行环境拒绝 SwiftPM
manifest sandbox，该限制必须在交付记录中明确。
