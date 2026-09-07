# 第三方开源组件与许可证说明 (Third-Party Notices)

MacAI 本身基于 [MIT 许可证](LICENSE) 发布。在构建与运行过程中，本项目集成了若干第三方开源库、工具与推理引擎。各组件的权利归其各自原作者所有，并在其各自许可证条款下使用。

---

## 关键第三方组件与引擎

| 组件 / 项目 | 主要用途 | 许可证 | 官方主页 / 仓库 |
| :--- | :--- | :--- | :--- |
| **llama.cpp** | GGUF 大语言模型原生推理引擎 | MIT | [ggerganov/llama.cpp](https://github.com/ggerganov/llama.cpp) |
| **whisper.cpp** | 语音识别（STT）C++ 高性能引擎 | MIT | [ggerganov/whisper.cpp](https://github.com/ggerganov/whisper.cpp) |
| **MLX / mlx-lm** | Apple Silicon 原生机器学习框架与 LLM 库 | MIT | [ml-explore/mlx](https://github.com/ml-explore/mlx) |
| **Kokoro (mlx-audio)** | 轻量级本地语音合成（TTS）Runner | Apache-2.0 | [hexgrad/kokoro](https://huggingface.co/hexgrad/Kokoro-82M) |
| **Qwen3-ASR / Qwen3-TTS** | 阿里通义开源端到端语音识别与合成 | Apache-2.0 / Qwen 许可协议 | [QwenLM/Qwen](https://github.com/QwenLM) |
| **sherpa-onnx** | 离线轻量级语音识别（STT）引擎 | Apache-2.0 | [k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) |
| **Symphonia** | 纯 Rust 音频解码库（入口音频格式归一化） | **MPL-2.0** | [pdeljanov/Symphonia](https://github.com/pdeljanov/Symphonia) |
| **rusqlite / SQLite** | 本地模型注册表存储 | MIT / Public Domain | [rusqlite/rusqlite](https://github.com/rusqlite/rusqlite) |
| **tokio / axum** | 异步网络与 HTTP API 基础设施 | MIT | [tokio-rs](https://github.com/tokio-rs) |
| **uv** | 高性能受管 Python 环境管理器 | Apache-2.0 / MIT | [astral-sh/uv](https://github.com/astral-sh/uv) |

---

## 特别说明：Symphonia (MPL-2.0)

MacAI 的 `ai-daemon` 使用了纯 Rust 音频解码库 `symphonia`。
- `symphonia` 依据 **Mozilla Public License, v. 2.0 (MPL-2.0)** 获得许可。
- MacAI 直接引用 `symphonia` 发布的官方 crates 依赖，**未对 symphonia 源代码进行任何修改**。
- 依据 MPL-2.0 条款，本项目的其余独立模块仍遵循其原本的 MIT 许可证。如您需要获取 Symphonia 的原始源代码，可访问其官方代码库：https://github.com/pdeljanov/Symphonia。

---

## AI 模型版权与使用免责声明 (Model Disclaimer)

1. **运行时与模型权重分离**：
   - MacAI 是一个本地 AI Runtime 调度系统，**不拥有、不托管、亦不直接分发任何 AI 模型的权重文件**；
   - 项目中预设的推荐配置（Model Profiles）仅提供了向 Hugging Face 或 ModelScope 等公开托管平台的元数据索引与下载引导。

2. **模型许可合规责任**：
   - 通过 MacAI 下载、加载并执行的具体模型（如 Qwen2.5、Whisper、Kokoro、Qwen3-ASR 等），其知识产权及衍生权利完全归各模型权利人所有；
   - 不同的模型可能附带独特的开源许可、使用政策或商用限制（例如，通义千问模型许可协议、Llama 3 社区许可协议等）；
   - **用户在使用具体模型前，有责任自行查阅、理解并遵守各模型发布方制定的专属许可条款与法律要求**。MacAI 维护团队对用户使用特定模型产生的任何法律争议概不承担责任。
