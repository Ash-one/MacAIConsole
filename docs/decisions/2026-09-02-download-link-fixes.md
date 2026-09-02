# 模型下载链路缺陷修复（TLS/UA/错误映射/Profile revision）

Status: landed（2026-09-02；真实 STT 链路证据见 roadmap，待模型落盘后补）

## 问题（独立于任何实现）

从 GUI 推荐页下载 ModelScope 的 Qwen3-ASR 模型连续失败：截图报
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

## 决策

- reqwest 启用明确 TLS backend：`rustls-tls`（macOS 无 openssl 依赖）。
- 下载 client 设置稳定 UA `MacAI/0.1 aiworkd`；保留 120s 总超时（空转快速失败）。
- 下载失败映射 `AIError::DownloadFailed`（HTTP 502，code `download_failed`），
  保留底层 connect/TLS/UA 错误链供诊断。
- ModelScope profile 补 immutable revision
  `3478f178e267548f04a6b616ff10beeb1e644e54`（git ls-remote master HEAD）与
  完整七文件 artifact 清单。
- 不再建议“关代理/直连绕过”作为修复（curl 直连成功不能证明 reqwest 可直连）。

## 验证

- `cargo test` 相关（lib + bin discovery 回归）通过；
- 真实链：ModelScope 无代理拉取 693M 全目录成功 → Runner
  `org.macai.qwen3-asr` ready 且模型绑定 → 注册加载（capability 由 manifest
  推导 stt.v1）→ **Kokoro 合成 WAV 经 Runner 转写逐字还原**（
  「你好，这是Runner接线后的声音。」）；
- 链路发现补充：ModelScope 仓库未带 `preprocessor_config.json`（mlx-audio
  feature extractor 必需），已从原 8bit HF 同款补入 artifact 清单。

## 关联

- 决策 owner 更新：本记录；roadmap `runner-migration-roadmap.md` 阻塞行解除。
