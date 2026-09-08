# 替换推荐 LLM 模型：以 MiniCPM5-2B-MLX 替代 Qwen3-8B-4bit

Status: implemented

Class: enhancement

Owner: this file

Related current decisions: [推荐模型 Hugging Face 下载链路修复与全量可达性保障](2026-09-07-fix-recommended-models-huggingface-download.md)、[Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[模型清单与 Model Profile v1 契约](../specs/model-profile-v1.md)

## Problem

此前 MacAI 内置推荐的 LLM 模型为 `qwen3-8b-mlx-4bit`（8B 参数，约 4.6 GB 显存占用）。
在 Apple Silicon Mac（特别是 8GB/16GB 统一内存设备）的日常使用场景下：
1. 8B 模型在加载后占用统一内存较高，与多任务共存时内存压力明显；
2. OpenBMB 最新开源的端侧模型 `MiniCPM5-2B-MLX` 采用 4-bit 权重量化，模型仅 ~1.42 GB 下载大小，运行时预估显存 ~2.5 GB；
3. `MiniCPM5-2B-MLX` 在长文本、工具调用及端侧推理速度上具备优异性能，架构基于标准 `LlamaForCausalLM`，原生支持 Metal 加速，更适合作为开箱推荐的轻量高效端侧大模型。

## Decision

1. **移除旧 Profile**：
   - 移除 `runners/mlx-lm/profiles/qwen3-8b-mlx-4bit.toml`；
   - 维持本地用户通过目录直接加载导入已有 `qwen3-8b-mlx-4bit` 权重的兼容性。
2. **新增 `MiniCPM5-2B-MLX` Profile**：
   - 在 `runners/mlx-lm/profiles/MiniCPM5-2B-MLX.toml` 建立模型契约：
     - `id`: `MiniCPM5-2B-MLX`
     - `name`: `MiniCPM5 2B MLX 4-bit`
     - `capabilities`: `["chat.v1"]`
     - `runner`: `org.macai.mlx-lm`
     - `adapter`: `mlx-lm-chat`
     - `format`: `directory`
     - `source`: Hugging Face 官方仓库 `openbmb/MiniCPM5-2B-MLX`，固定 commit revision 为 `32f8dd5df1188512a20413f1297083238306634c`；
     - `artifacts`: `directory = "MiniCPM5-2B-MLX"`，包含推断与分词所需的 7 个文件（`chat_template.jinja`、`config.json`、`generation_config.json`、`model.safetensors`、`model.safetensors.index.json`、`tokenizer.json`、`tokenizer_config.json`）；
     - `defaults.keep_alive`: `"5m"`；
     - `resources.memory_estimate_bytes`: `2500000000` (~2.5 GB)。
3. **更新 Runner 配置与推荐清单**：
   - 更新 `runners/mlx-lm/runner.toml`，将 `[[models]]` 引用更新为 `profiles/MiniCPM5-2B-MLX.toml`；
   - 内置推荐模型 Profile 总数保持为 7 个不变，满足回归断言。
4. **同步文档与主页**：
   - 同步更新 `README.md` 与 `docs/index.html` 中的模型名称、模型 ID、仓库地址及内存预估。

## Verification

1. **网络可达性验证**：
   - 使用 Python 对 Hugging Face（`huggingface.co`）及国内镜像（`hf-mirror.com`）全部 7 个文件进行 HEAD 请求探测，所有文件均返回 HTTP 200 OK。
2. **Runner 推荐模型完整性测试**：
   - `cargo test --package ai-daemon --bin aiworkd builtin_runner_bundled_profiles_are_valid_and_construct_pull_targets` 通过（7/7 Profile 全部通过校验与 pull targets 构建）。
3. **Rust 测试套件与代码格式**：
   - `cargo fmt --all -- --check` 检查通过；
   - `cargo test --workspace` 全量测试通过。
4. **Swift 测试套件**：
   - `swift test --enable-xctest` 全量通过。
