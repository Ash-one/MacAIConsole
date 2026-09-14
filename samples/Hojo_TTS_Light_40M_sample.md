# 使用案例：Hojo-TTS-Light-40M 单文件 Script Runner

[Hojo-TTS-Light-40M](https://huggingface.co/HojoAI/Hojo-TTS-Light-40M)（Apache-2.0）是
约 40M 参数的 ONNX 中英双语 TTS 模型，纯 CPU 推理、无 PyTorch 依赖。本示例演示用
[单文件 Script Runner](../docs/decisions/2026-09-08-single-file-script-runner.md)
把它接入 MacAI，全程不需要修改 daemon 代码：

| 项目 | 值 |
| --- | --- |
| 示例脚本 | `samples/hojo_tts_light_40m.macai.py` |
| capability | `tts.v1` |
| 依赖 | `onnx` / `onnxruntime` / `numpy` / `soundfile` / `tokenizers` |
| 音频输出 | 24 kHz WAV，15 个内置音色（`hojo_zh_f_01`、`hojo_en_m_02` 等） |

在 Apple Silicon（M 系列）CPU 上的实测：模型加载约 3.5s（含 BF16→FP32 提升），
合成速度约为实时的 2~4 倍（中文 RTF ≈ 0.24，英文 RTF ≈ 0.47），常驻内存约 300~400MB。

### 安装步骤

1. **模型入库**：把 HF 仓库 `HojoAI/Hojo-TTS-Light-40M` 的 7 个文件
   （3 个 `.onnx`、`voice.npz`、`config.json`、`tokenizer.json`、`tokenizer_config.json`）
   放入一个模型目录——可用 GUI「管理」页的模型下载（拉取后目录名通常为 `HojoAI--Hojo-TTS-Light-40M`），
   或手动下载（`hf download HojoAI/Hojo-TTS-Light-40M --local-dir <目录>`）后注册本地目录。
   脚本内的 `local_detectors` 通过 `directory_contains = ["Hojo-TTS-Light-40M"]` 与文件清单共同约束，无论目录名为 `HojoAI--Hojo-TTS-Light-40M` 还是直接为 `Hojo-TTS-Light-40M` 均可自动识别并路由到此 Runner。
2. **创建 Runner**：GUI「管理 → 引擎」→「新建 Script Runner」，粘贴
   `samples/hojo_tts_light_40m.macai.py` 全部内容，确认依赖解析后点击
   「信任并添加」，按提示重启 daemon 完成装配。
3. **确认就绪**：`/v1/models` 中出现对应 `tts` 模型（模型 ID 即模型目录名）。

### 请求示例

```bash
curl --fail http://127.0.0.1:11435/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Hojo-TTS-Light-40M",
    "input": "你好，这是一段中文语音合成测试。",
    "voice": "hojo_zh_f_01"
  }' --output out.wav
```

### 已知限制

- 模型不支持语速调节，`speed` 字段被忽略；`language` 字段被忽略（中英自动区分）。
- 单段合成上限约 40 秒音频（2048 speech token @ 50Hz）；长文本由脚本按句切分、
  逐段合成并拼接，相应放宽了 `inference_seconds` 超时（600s）。
- 随机种子固定（42），同文本同音色输出可复现；采样参数不透出为请求字段。
