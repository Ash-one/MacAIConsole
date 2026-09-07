# 推荐模型 Hugging Face 下载链路修复与全量可达性保障

Status: implemented

Class: defect

Owner: this file

Related current decisions: [模型下载链路缺陷修复（TLS/UA/错误映射/Profile revision）](2026-09-02-download-link-fixes.md)、[Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[模型清单与 Model Profile v1 契约](../specs/model-profile-v1.md)

## Problem

在 MacAIConsole 管理页点击下载推荐模型 `Qwen3-ASR 0.6B MLX 4bit` 时操作失败，daemon 返回 502 错误：
```
HTTP 502: HF returned 404 Not Found for https://hf-mirror.com/aufklarer/Qwen3-ASR-0.6B-MLX-4bit/resolve/main/configuration.json
```

## 根因与全量核查

### 1. Qwen3-ASR 推荐模型仓库与文件差异
- `runners/qwen3-asr/profiles/Qwen3-ASR-0.6B-MLX-4bit.toml` 最初创建时参考了 ModelScope 仓库 `aufklarer/Qwen3-ASR-0.6B-MLX-4bit`；
- ModelScope 仓库含有 ModelScope 专有的 `configuration.json` 文件；但在 Hugging Face（`huggingface.co` 及镜像 `hf-mirror.com`）上，`aufklarer/Qwen3-ASR-0.6B-MLX-4bit` 既没有 `configuration.json`，也没有 `preprocessor_config.json`（导致拉取 `configuration.json` 时返回 404 Not Found）；
- Hugging Face 上官方维护的 MLX 社区标准仓库为 `mlx-community/Qwen3-ASR-0.6B-4bit`（最新 immutable SHA: `313d850181767edf09f00a9c289becca70e58cd0`），其原生完整包含了 `mlx-audio` 运行所需的所有 9 个模型及分词/预处理文件（含 `preprocessor_config.json`），完全兼容 `runners/qwen3-asr` 的 `qwen3-asr-mlx` adapter。

### 2. 全量 7 个推荐模型 Hugging Face 可达性探测结果
对仓库内所有 7 个内置 Runner 的推荐模型 Profile 进行全文件 HEAD 探测（`resolve/main`）：
1. `kokoro-82m-zh`（`1038lab/Kokoro-82M-zh-MLX`，105 个文件）：105/105 可达（200/302）；
2. `SmolLM2-135M-Instruct-8bit`（`mlx-community/SmolLM2-135M-Instruct-8bit`，8 个文件）：8/8 可达（200/302）；
3. `qwen3-8b-mlx-4bit`（`mlx-community/Qwen3-8B-4bit`，9 个文件）：9/9 可达（200/302）；
4. `Qwen3-TTS-0.6B-CustomVoice-4bit`（`mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-4bit`，12 个文件）：12/12 可达（200/302）；
5. `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`（`csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`，4 个文件）：4/4 可达（200/302）；
6. `whisper-large-v3-turbo-q5`（`ggerganov/whisper.cpp`，1 个文件）：1/1 可达（200/302）；
7. `Qwen3-ASR-0.6B-MLX-4bit`：原 `aufklarer` 仓库在 Hugging Face 缺失 `configuration.json` 与 `preprocessor_config.json`；切换为 `mlx-community/Qwen3-ASR-0.6B-4bit` 后全部 9 个文件可达（200/302）。

## Decision

1. **修正 `Qwen3-ASR-0.6B-MLX-4bit.toml`**：
   - 将 `source.repo` 更新为 `mlx-community/Qwen3-ASR-0.6B-4bit`；
   - 更新 immutable commit revision 为 `313d850181767edf09f00a9c289becca70e58cd0`；
   - 更新 `artifacts.files` 清单为该仓库中的 9 个必要文件：
     - `chat_template.json`
     - `config.json`
     - `generation_config.json`
     - `merges.txt`
     - `model.safetensors`
     - `model.safetensors.index.json`
     - `preprocessor_config.json`
     - `tokenizer_config.json`
     - `vocab.json`
2. **同步文档与测试**：
   - 更新 `README.md` 中 Qwen3-ASR 推荐模型来源说明；
   - 在 `crates/ai-daemon/src/main.rs` 增加对所有内置 Runner 随附 Profile 的完整性回归测试（校验解析合法性、HF 源配置与 pull 目标构建）。

## Verification

1. **自动化网络可达性验证**：脚本通过 Python 遍历请求各模型在 `https://hf-mirror.com` 的 `resolve/main` 端点，全部 7 个推荐模型的所有文件均返回 200/302 成功响应；
2. **Rust 测试套件**：`cargo test --workspace` 全绿；
3. **Swift 测试套件**：`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest` 全绿（39 passed）；
4. **格式检查**：`cargo fmt --all -- --check` 通过。
