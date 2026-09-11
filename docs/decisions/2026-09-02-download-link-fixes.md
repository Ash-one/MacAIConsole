# 模型下载链路与进度可观测性

Status: implemented

Class: behavior

Owner: this file

## Problem

修复前，从 GUI 推荐页下载 ModelScope 的 Qwen3-ASR 模型连续失败：截图报
HTTP 500 / “error sending request”，日志显示 ModelScope LFS CDN 403。
问题在移除任何“直连绕过/关代理”建议后依然存在，说明是客户端能力缺陷
而非网络策略。

## 根因（三层，均可复现）

1. **reqwest 无 TLS backend**：`Cargo.toml` 用 `default-features=false` 且只开
   `json,stream`，依赖树无 `default-tls/native-tls/rustls-tls`；GUI
   `proxyMode=disabled` 删除代理后，daemon 直连 HTTPS 在 `GET.send()` 立即失败。
   curl 自带 TLS，不能作为对照证据。
2. **ModelScope LFS CDN 按 User-Agent ACL 拒空 UA**：空 UA → `403
   X-Tengine-Error: denied by UA ACL`；带 `MacAI/0.1 aiworkd` → 206 正常。
3. **下载错误伪装成 500**：`pull_model` 把所有下载失败映射 `AIError::Internal`，
   已有 `DownloadFailed/502` 枚举未使用。
4. **Profile 缺 immutable revision**：parser 要求 40/64-hex commit；qwen3-asr
   profile 无 revision → daemon 启动拒绝该 Runner。
5. **下载过程不可见**：pull HTTP 请求只在全部文件完成后返回；GUI 只能显示无限转圈，
   daemon 日志只记录字节数，用户无法判断当前文件的完成比例。

## Decision

- reqwest 启用明确 TLS backend：`rustls-tls`（macOS 无 openssl 依赖）。
- 下载 client 设置稳定 UA `MacAI/0.1 aiworkd`；保留 120s 总超时（空转快速失败）。
- 下载失败映射 `AIError::DownloadFailed`（HTTP 502，code `download_failed`），
  保留底层 connect/TLS/UA 错误链供诊断。
- ModelScope profile 补 immutable revision
  `3478f178e267548f04a6b616ff10beeb1e644e54`（git ls-remote master HEAD）与
  完整七文件 artifact 清单。
- pull 请求可携带短生命周期 `progress_id`；daemon 用它拥有活动下载状态，
  `GET /api/downloads/{id}` 返回当前文件序号、字节数和百分比，结束或失败后立即清理。
  GUI 对推荐模型与在线仓库下载轮询该只读状态，显示当前文件/总文件及百分比。
- daemon 进度日志使用同一百分比计算，并按 5% 台阶记录；未知响应长度时保留不定进度。
- 不再建议“关代理/直连绕过”作为修复（curl 直连成功不能证明 reqwest 可直连）。

## Verification

- `cargo test` 相关（lib + bin discovery 回归）通过；
- `progress_percent` 覆盖正常值、上限钳制和未知总大小；Swift API 请求测试覆盖
  `progress_id` 的两条 GUI 下载路径；
- 2026-09-12：`cargo test --workspace` 的本改动相关下载测试及其余已执行项通过；
  1 项既有 Script Runner 环境测试因返回 503 失败，单独重跑结果相同；
  `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest`
  通过（51/51）。
- 真实链：ModelScope 无代理拉取 693M 全目录成功 → Runner
  `org.macai.qwen3-asr` ready 且模型绑定 → 注册加载（capability 由 manifest
  推导 stt.v1）→ **Kokoro 合成 WAV 经 Runner 转写逐字还原**（
  「你好，这是Runner接线后的声音。」）；
- 链路发现补充：ModelScope 仓库未带 `preprocessor_config.json`（mlx-audio
  feature extractor 必需），已从原 8bit HF 同款补入 artifact 清单。

## 关联

- 下载行为理由由本记录拥有；Model Profile 精确契约由
  [`model-profile-v1.md`](../specs/model-profile-v1.md) 拥有。
