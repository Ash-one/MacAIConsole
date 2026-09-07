# 使用案例：为 Hermes Agent 提供本地 TTS 与 STT

[Hermes Agent](https://hermes-agent.nousresearch.com/docs/) 支持命令型语音 Provider。MacAI 通过本地 OpenAI-compatible API 给 Hermes 提供：

| Hermes 能力 | MacAI endpoint | MacAI Provider |
| --- | --- | --- |
| TTS（文字转语音） | `POST /v1/audio/speech` | Kokoro（org.macai.kokoro Runner） |
| STT（语音转文字） | `POST /v1/audio/transcriptions` | org.macai.whisper.cpp Runner |

接入后的数据流：

```text
Hermes text_to_speech
  -> samples/hermes_macai_tts.py
  -> MacAI /v1/audio/speech
  -> WAV audio

Hermes voice message
  -> samples/hermes_macai_stt.sh
  -> ffmpeg: input audio -> 16 kHz mono PCM WAV
  -> MacAI /v1/audio/transcriptions
  -> UTF-8 transcript
```

### 前置条件

1. `aiworkd` 正在 `http://127.0.0.1:11435` 运行。
2. MacAI 已注册至少一个 `tts` 模型和一个 `stt` 模型。
3. 本机已安装 `hermes`、`python3`、`curl` 和 `ffmpeg`。
4. Hermes 版本支持 `tts.providers.<name>` 与 `stt.providers.<name>` 命令型 Provider。

先确认 MacAI 这边就绪：

```bash
curl --fail http://127.0.0.1:11435/health
curl --fail http://127.0.0.1:11435/v1/models
```

`/v1/models` 的 `data` 数组里应该有 `type: "tts"` 和 `type: "stt"` 的模型。

### 一键配置

在 MacAI 仓库根目录运行：

```bash
./samples/configure-hermes-audio.sh
```

脚本会：

1. 检查 MacAI、Hermes、Python、curl 与 ffmpeg。
2. 从 `/v1/models` 自动选择第一个 TTS 和 STT 模型。
3. 向当前 Hermes profile 写入名为 `macai` 的 TTS/STT Provider。
4. 启用 Hermes 的 `tts` toolset。
5. 读回 `tts.provider` 与 `stt.provider`，确认配置已落盘。

自动发现的结果可以用环境变量覆盖：

```bash
MACAI_BASE_URL=http://127.0.0.1:11435 \
MACAI_TTS_MODEL=Kokoro-82M-zh \
MACAI_TTS_VOICE=zf_001 \
MACAI_STT_MODEL=whisper-large-v3-turbo \
MACAI_STT_LANGUAGE=zh \
./samples/configure-hermes-audio.sh
```

可用变量：

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `HERMES_BIN` | `hermes` | 指定 Hermes 可执行文件 |
| `MACAI_BASE_URL` | `http://127.0.0.1:11435` | MacAI 根地址或 `/v1` API 地址 |
| `MACAI_TTS_MODEL` | 自动发现 | MacAI TTS 模型 ID |
| `MACAI_TTS_VOICE` | 空 | 可选的 Kokoro 音色 ID；为空时使用模型默认音色 |
| `MACAI_STT_MODEL` | 自动发现 | MacAI STT 模型 ID |
| `MACAI_STT_LANGUAGE` | `zh` | STT 语言提示 |

> [!NOTE]
> 配置里保存了 `samples` 脚本的绝对路径。移动 MacAI 仓库后，重新跑一遍配置脚本。

### 生成的 Hermes 配置

脚本通过 `hermes config set` 写入当前 profile。核心结构如下，脚本路径与模型 ID 会替换为本机真实值：

```yaml
tts:
  provider: macai
  providers:
    macai:
      type: command
      command: >-
        python3 "/absolute/path/to/MacAI/samples/hermes_macai_tts.py"
        --input "{input_path}"
        --output "{output_path}"
        --base-url "http://127.0.0.1:11435/v1"
        --model "{model}"
        --voice "{voice}"
        --format "{format}"
        --speed "{speed}"
      model: Kokoro-82M-zh
      voice: ""
      output_format: wav
      timeout: 150
      max_text_length: 3000
      voice_compatible: true

stt:
  enabled: true
  echo_transcripts: true
  provider: macai
  language: zh
  providers:
    macai:
      type: command
      command: >-
        bash "/absolute/path/to/MacAI/samples/hermes_macai_stt.sh"
        "{input_path}"
        "{output_path}"
        "http://127.0.0.1:11435/v1"
        "{model}"
        "{language}"
      model: whisper-large-v3-turbo
      language: zh
      format: txt
      timeout: 300
```

两个适配脚本的分工：TTS 脚本使用 Python 标准库读取 Hermes 生成的 UTF-8 文本文件，请求 MacAI `/v1/audio/speech` 并将音频流写入 `{output_path}`；STT 脚本则将 Hermes 接收到的多格式音频（WAV、OGG、Opus 等）通过 ffmpeg 归一化为 PCM WAV，交由 MacAI 转写并输出纯文本。

STT Provider 显式命名为 `macai` 亦可规避模型名称改写：Hermes 原生 `openai` STT 逻辑可能将 `whisper-large-v3-turbo` 映射为特定云端厂商的 `whisper-1`。采用独立 command Provider 可确保本地模型 ID 被原样透传。

### 验证适配脚本

直接验证 TTS：

```bash
printf '你好，这是 MacAI 提供给 Hermes 的语音合成测试。' > /tmp/macai-tts.txt

python3 samples/hermes_macai_tts.py \
  --input /tmp/macai-tts.txt \
  --output /tmp/macai-tts.wav \
  --base-url http://127.0.0.1:11435 \
  --model Kokoro-82M-zh \
  --format wav

file /tmp/macai-tts.wav
```

直接验证 STT：

```bash
bash samples/hermes_macai_stt.sh \
  /path/to/input.ogg \
  /tmp/macai-transcript.txt \
  http://127.0.0.1:11435 \
  whisper-large-v3-turbo \
  zh

cat /tmp/macai-transcript.txt
```

检查 Hermes 的最终选择：

```bash
hermes config get tts.provider
hermes config get stt.provider
hermes config get tts.providers.macai
hermes config get stt.providers.macai
```

配置完成后重启 Hermes Desktop；Gateway 用户执行 `/restart`。之后用 Hermes 的 `text_to_speech` 工具、CLI `/voice tts` 模式或消息平台的语音消息验证整条链路。

### 常见排错

- **`model_not_found`**：请求 `curl http://127.0.0.1:11435/v1/models` 确认已注册模型的准确 `id`，并将其填入脚本配置，或重新执行一键配置。
- **`Connection refused`**：守护进程未启动。请在 MacAIConsole 中开启服务，或在终端运行 `./target/release/aiworkd`。
- **`Required command is missing: ffmpeg`**：系统缺少 `ffmpeg` 依赖。请安装后重新执行配置脚本（STT 适配器依赖其归一化音频格式）。
- **Hermes 仍沿用旧 Provider**：重启 Hermes Desktop 或在 Gateway 中执行 `/restart`；同时确认配置脚本写入的 profile 与运行中的 Hermes profile 一致。
- **首次推理延迟偏高**：whisper.cpp 的 Core ML encoder 在首次调用时需完成 Apple Neural Engine (ANE) 架构特化，完成编译后将复用缓存。

Hermes 命令型语音 Provider 的完整说明见 [Voice & TTS](https://hermes-agent.nousresearch.com/docs/user-guide/features/tts)。
