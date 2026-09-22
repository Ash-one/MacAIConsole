# MacAI

<p align="center">
  <img src="docs/logo.png" alt="MacAI logo" width="280">
</p>

MacAI 是面向 Apple Silicon 的开源本地 AI 运行管理系统，你可以导入任意包括大语言模型（LLM）、语音转文字模型（STT）、文字转语音模型（TTS）在内的等多种 AI 模型，在一个 Mac 原生应用中利用 Mac Silicon 的 CoreML 与 Metal 加速，并以 OpenAI 兼容方式提供所有的本地服务，将所有语音隐私数据在本地完成转写和生成。
> ⚠️项目处于开发阶段，适合本地开发、实验与个人工作流；目前无法承诺 API 稳定性和跨版本兼容。

## 功能

- **一站式配置 Apple GPU 加速的 AI 模型**： 包括大语言模型（LLM）、语音转文字模型（STT）、文字转语音模型（TTS），开箱下载即用所有推荐的开源 AI 模型，支持本地文件导入、Huggingface 下载、ModelScope 下载
- **所有数据都在本地处理**：文字对话、语音转写、语音生成所有数据都在 Mac 上处理
- **提供本地 AI 接口**：使用 OpenAI 兼容的格式调用，`/v1/chat/completions` `/v1/audio/transcriptions` `/v1/audio/speech`，无缝接入所有 AI 工具



## 重要更新路线图

- [x] LLM 现支持缓存保持，在守护进程退出前可以始终保留 prefill KV 缓存
- [x] Runner 现在支持外部导入，可以通过 Prompt 指示 AI 生成 Runner 文件以获得任意模型的环境支持
- [x] 为音色克隆模型提供自定义音色注册，当前支持命令行注册音色
- [x] 为自定义音色注册提供 GUI
- [ ] Homebrew 分发、正式签名、公证与安装包

## 系统要求

- Apple Silicon Mac
- macOS 14 或更高版本
- 使用`.DMG`安装，无需提前配置开发环境
- 源码构建额外需要：Rust stable toolchain、Xcode Command Line Tools、CMake、Python 3.12

仓库源码与 DMG 均不包含模型权重文件，所有推理产物均在本地受管目录按需生成或存储。

## 快速开始
![](docs/imgs/MacAIConsole.png)

> [!TIP]
> 更系统的网页版使用说明见 **[docs/guide/](docs/guide/index.html)**（安装、控制台导览、模型实操与故障排查、开发者指南）。

### 当前支持与推荐模型

MacAI 原生支持大语言模型（LLM）、语音转文字（STT）和文字转语音（TTS）三大能力。在控制台「管理」页中预置了精选**开箱即用推荐模型**（支持一键下载与自动注册），同时也支持直接导入运行本地任意兼容格式的自定义模型：

| 类型 | 模型名称 / ID | 驱动引擎  | 预估内存 | 特点与说明 |
| :--- | :--- | :--- | :--- |  :--- |
| LLM | **MiniCPM5 2B MLX 4-bit** | `org.macai.mlx-lm` | ~2.5 GB | **[开箱推荐]** 端侧旗舰通用大模型，Metal 原生加速，长上下文与工具调用优化 |
| LLM | **通用 GGUF 模型** | `org.macai.llama.cpp` | 视权重而定 | 支持本地直接导入并运行任意兼容的 GGUF 格式模型 |
| STT | **Whisper Large v3 Turbo Q5** | `org.macai.whisper.cpp` | ~570 MB | **[开箱推荐]** 高速多语言转写，支持可选 Core ML / Metal 加速 |
| STT | **Qwen3-ASR 0.6B MLX 4-bit** | `org.macai.qwen3-asr` | ~800 MB | **[开箱推荐]** 新一代高效语音识别，中文与混合语言识别精度优异 |
| STT | **sherpa-onnx Zipformer zh-int8** | `org.macai.sherpa-onnx`  | ~400 MB | 专注中文流式识别与长音频切片转写，CPU 轻量低功耗运行 |
| TTS | **Kokoro 82M zh** | `org.macai.kokoro` | ~400 MB | **[开箱推荐]** 高品质多音色语音合成，内置 20+ 款中英文音色 |
| TTS | **Qwen3-TTS 0.6B CustomVoice 4-bit** | `org.macai.qwen3-tts` |  ~1.7 GB | **[开箱推荐]** 表现力丰富，支持 9 款角色音色与情感指令控制 |
| TTS | **macOS 内置语音** (`macos_say`) | 内置系统 Provider | 系统级（极小） | 零权重下载开销，即刻发声 |

> [!TIP]
> - **一键下载**：在 `MacAIConsole` 侧边栏进入**「管理」**页，先安装对应引擎，再在“推荐模型”卡片点击**「下载」**，daemon 将自动在后台断点续传下载并完成注册。
> - **本地模型导入**：若本地已有 GGUF 或 MLX 格式模型，可点击「管理」页顶部的**「+ 导入模型」**或使用 CLI 命令 `macai load` 直接注册运行。

### 方式一：DMG 安装包快速使用（推荐普通用户）

1. **安装应用**：下载并打开 `MacAIConsole.dmg`，将 `MacAIConsole.app` 拖入 `Applications`（应用程序）文件夹。
2. **首次打开绕过安全拦截（Gatekeeper）**：
   - 从网络接收下载后打开若提示“已损坏，无法打开。你应该将它移到废纸篓”或“未知的开发者”，请在终端执行：
     ```bash
     xattr -cr /Applications/MacAIConsole.app
     ```
   - 或前往 macOS「系统设置 → 隐私与安全性」，滑至最下方“安全性”区域，点击「仍要打开」。
3. **首次配置与模型体验**：
   - 启动 `MacAIConsole`；
   - 在左侧进入**「管理」**页，顶端「引擎」区点击**「安装」**所需引擎（如 `llama.cpp` 或 `mlx-lm`；Python 依赖由内置 `uv` 自动隔离部署，无需系统预装 Python）；
   - 在推荐模型列表点击**「下载」**（如 MiniCPM5 2B）；若网络较慢，可先在**「设置」**页将下载源切换为 `https://hf-mirror.com` 或配置代理；
   - 模型下载完成后即可直接在控制台加载启动，或通过本地 OpenAI 兼容 API（默认 `http://127.0.0.1:11435/v1`）接入任意客户端。

> [!TIP]
> **whisper.cpp 引擎编译依赖提示**：`llama.cpp` 会自动下载验证过的预编译二进制，Python 系列引擎使用内置 `uv` 安装预构建 wheel。仅 `whisper.cpp` 引擎在点击安装时需下载官方源码归档并在本机通过 `cmake` 编译，需预先安装 Xcode Command Line Tools 和 `cmake`（如 `brew install cmake`）；普通用户建议优先体验预编译完善的 `llama.cpp`、`mlx-lm`、`qwen3-asr` 或 `kokoro`。


## 使用案例 1：为 Hermes Agent 提供本地 TTS 与 STT
使用本地配置和脚本接入 Hermes Desktop 应用，无需修改 Hermes Agent 代码，可实现自定义语音对话模型。
详情见链接：[案例](samples/Hermes_TTS_STT_sample.md)

## 使用案例 2：为 CherryStudio 接入本地 LLM
![](docs/imgs/cherry_studio_sample.png)
添加本地 API 地址： 

```
http：//127.0.0.1:11435
```

点击`获取模型列表`按钮会自动获取所有可提供的模型 ID

## 使用案例 3：为 MacAI 增加新的模型和 Runner 支持

对于一个新的模型如果没有现有的 Runner 支持，可以自定义一个新的 Runner 支持。

详情见链接：[案例](samples/Hojo_TTS_Light_40M_sample.md)

## 社区与贡献

我们欢迎社区贡献！无论是新模型 Runner 插件、功能建议还是缺陷修复：
- 贡献指南请参见 [CONTRIBUTING.md](CONTRIBUTING.md)；
- 行为准则请参见 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。

## 安全政策

MacAI 专注于本机私有推理：`aiworkd` 默认仅监听本地 `127.0.0.1:11435` 且无鉴权机制，切勿直接公网暴露。完整威胁模型与漏洞报告指引请参见 [SECURITY.md](SECURITY.md)。

## 开源许可证与模型版权

- **项目许可证**：MacAI 依据 [MIT License](LICENSE) 开源；
- **第三方组件通知**：使用的第三方推理引擎与依赖库协议声明参见 [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)；
- **AI 模型免责声明**：MacAI 仅提供本地运行时与任务调度能力，不拥有亦不分发模型权重；下载与使用各开源模型需严格遵守其原始权利人的授权许可协议。

## 致谢与 AI 支持

感谢在本项目研发、设计与文档整理过程中提供支持的 AI 模型：

1. GPT 5.6
2. DeepSeek V4 flash
3. GLM 5.3 flash
4. Gemini 3.8 flash
