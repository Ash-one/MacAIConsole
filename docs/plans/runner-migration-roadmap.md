# Runner 全面迁移路线（2026-09-02）

Status: accepted（用户拍板：目标是把所有合适 provider 切换到 Runner 实验版本）

## 目标与边界

- 目标：Python worker 类 provider 全部迁移到 daemon-owned uv Runner；
  日常模型注册/默认 provider 指向 Runner 版本；最终删除 legacy Provider 路径。
- 边界：Runner 的运行时形态是 python-uv 受管环境。因此 **llama.cpp / whisper.cpp
  这类外部原生二进制 provider 保留现有 Provider**（进程隔离已具备），不硬套 Runner；
  `mock` / `macos-say` 保留为测试与 CLI 兜底能力（2026-09-03 用户拍板：不删除，
  生产装配不含，GUI 无注册入口）。
- 迁移全集（按序）：kokoro（✓ 已完成）→ qwen3-asr-mlx → qwen3-tts（✓ 2026-09-03）
  → sherpa-onnx → mlx-lm LLM（✓ 2026-09-03 完成，含 chat.v1 能力通路）。

## 当前缺口核对（2026-09-02）

| 缺口 | 状态 |
| --- | --- |
| Runner 环境安装/状态管理面 | ✓（`GET /api/runners` + `POST /install`） |
| 动态 provider 注册 | ✓（6494f78：按 descriptor 能力判定） |
| 请求级 provider 日志 | ✓（e7b0a71） |
| GUI Provider 行识别 Runner | ✓（e7b0a71 RUNNER tag） |
| Runner 模型 voices 端点 | ✓（本就按 `spec.path/voices/*.safetensors` 扫描，116 voices 已实测；default_voice 缺省由引擎兜底 zf_001） |
| 注册加载失败回滚 | ✓（ac3d640） |
| 主动取消/多并发 | 不属于 current v1；需独立协议决策与进程 actor/HTTP abort 全链路 |
| Plugins 目录 + 显式信任 | 未做（内置 Runner 先行；第三方插件后续） |
| GUI 本地导入 provider 选择 | 暂缓（用户不选下拉；改用「默认注册目标集中由 daemon 决定」） |

## 迁移单元模板（每个引擎一次 bounded change）

1. `runners/<engine>/`：manifest + pyproject 锁定依赖 + bundled Model Profile；
2. adapter 迁移 legacy worker 语义；adapter 单测锁 text/voice/错误逻辑；
3. 真实接线证据（env-gated：real model → 真实输出，复用 runner_kokoro_real_wiring 模板）；
4. daemon 装配：新增 runner 目录即被 `bootstrap_runners` 自动发现/绑定（零 daemon 改动）；
5. **默认切换**：把该类型的「默认 provider」集中改到 daemon 装配/推荐模型映射一处，
   不再由 GUI 页写死 legacy（含 ModelRepository.directoryProvider 的 legacy 启发式迁移目标）；
6. **删除 legacy**：每迁一个删一个对应 Provider 实现与 GUI/PythonEnvironment 引用。

## 双轨并行的纪律

- 同一模型路径在两个 provider 同时 resident 会互相挤 MLX 内存 → 迁移切换用
  `/api/models/load` 显式覆盖注册（provider=org.macai.*），一次只激活一个。
- 请求来源以日志 `provider=` 与 `/api/providers`、`/v1/models.owned_by` 为准。

## Cutover 验收（每引擎）

- 真实验证通过（输出合法且与 legacy 语义一致，参考 Kokoro zh/mixed/长文本证据）；
- 默认 provider 已切（curl `GET /v1/models` owned_by 为 org.macai.*）；
- legacy Provider 删除后全套 Rust/Swift/测试绿；
- README / AGENTS / decisions 同步。

## 执行顺序

1. 迁移 qwen3-asr-mlx（复用 MLX uv 环境套路，最接近 Kokoro）✓（2026-09-02）
2. qwen3-tts ✓（2026-09-03，见下方 Phase 记录）
3. sherpa-onnx ✓（2026-09-03，见下方 Phase 记录）
4. mlx-lm LLM ✓（2026-09-03：chat.v1 能力通路 + `runners/mlx-lm` 包 + legacy 删除，
   见下方 Phase 记录）
5. 收尾：mock/macos-say 保留（见决策记录）；GUI Python 环境管理器退役
   ✓（2026-09-03：`PythonEnvironmentManager`/`PythonEnvironmentSpec` 改名
   `EngineEnvironmentManager`/`EngineEnvironmentSpec`，venv/pip 安装路径删除，
   只留 llama.cpp 脚本安装；Runner 环境统一走 daemon `/api/runners`）；
   清理 scripts/legacy worker 与 `.build` 引用、更新 README/AGENTS。
6. 注册路径统一（2026-09-03，`1c206cc`）：main.rs provider 白名单只剩
   llm/stt 缺省两行，显式 provider（静态装配或 org.macai.*）统一走 descriptor
   能力判定；目录型校验合并为通用分支。加新引擎不再改 main.rs。

## Phase：sherpa-onnx legacy 删除（2026-09-03 落地）

迁移内容（`3a8e4b0`）：

- `runners/sherpa-onnx/` 包：manifest `org.macai.sherpa-onnx`（capabilities=["stt.v1"]）、
  uv 受管环境（sherpa-onnx 1.13.7）、Model Profile
  `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`（HF 镜像
  `csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30` @ `ad658fa0…`
  immutable revision——GitHub release tar.bz2 无 commit SHA，无法满足 Profile
  source 契约，选用维护者本人上传的同布局 HF 镜像）。
- 依赖修复：sherpa-onnx 1.13.6 的 macOS arm64 wheel 缺 `sherpa-onnx-core`
  （`libonnxruntime.dylib` 随 core 包分发），import 即 dlopen 失败；1.13.7
  拆包修复。core 依赖 uv 0.9.21 未从 wheel METADATA 继承（真实 probe 暴露，
  fake probe 会掩盖），pyproject 显式声明 `sherpa-onnx-core==1.13.7`。
- adapter `macai_sherpa_onnx_runner`：legacy worker 全部语义迁移（PCM16 校验、
  多声道下混、0.032s 分块流式解码、0.66s 右上下文 flush、仅中文）；
  engine 校验单测 3 passed。
- 真实接线证据：模型自带中文 test wav → 流式转写通顺文本 → unload →
  shutdown_complete 全链路（`tests/real_wiring_manual.py`）。
- legacy 完整消费面删除：`providers/sherpa_onnx.rs`、`scripts/sherpa_onnx_worker.py`
  及 pytest、`scripts/setup-sherpa-onnx.sh`、runtime 静态装配
  （providers/stt_providers 双表）、main.rs 的目录深校验分支与 format 分支、
  GUI `PythonEnvironmentSpec.sherpaOnnx`。`AIWORK_SHERPA_ONNX_*` 零消费者。
- GUI 目录识别启发式：sherpa 四文件目录改指 `org.macai.sherpa-onnx`。
- mock / macos-say 处置（2026-09-03 用户拍板）：保留为测试与 CLI 兜底能力，
  不删除；生产装配不含（`Runtime::new()` 测试构造注入），GUI 无注册入口。

至此 Python worker 类 provider 全部迁移完成：`scripts/` 无 Python worker，
生产 provider 只剩 llama.cpp / whisper.cpp 原生二进制 + `org.macai.*` Runner
（kokoro / qwen3-asr / qwen3-tts / sherpa-onnx / mlx-lm）。

## Phase：qwen3-tts legacy 删除（2026-09-03 落地）

迁移内容（`dcef0fc`）：

- `runners/qwen3-tts/` 包：manifest `org.macai.qwen3-tts`（capabilities=["tts.v1"]）、
  uv 受管环境（mlx-audio==0.5.1，40 包 uv.lock）、Model Profile
  `Qwen3-TTS-0.6B-CustomVoice-4bit`（HF `mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-4bit`
  @ `08c72cad…` immutable revision）。
- adapter `macai_qwen3_tts_runner`：legacy `scripts/qwen3_tts_worker.py` 语义完整迁移
  （speaker/instruct 拆分、`generate_custom_voice`、5000 字限制、speed
  0.25..=4.0 校验——mlx-audio CustomVoice 无 speed 参数，校验通过不改变合成速度）；
  engine 校验单测 5 passed；manifest probe 真实通过。
- 真实接线证据：Vivian speaker metal 合成 → 241964 字节合法 WAV（24kHz）→
  unload → shutdown_complete 全链路（`tests/real_wiring_manual.py`）。
- legacy 完整消费面删除：`providers/qwen3_tts.rs`、`scripts/qwen3_tts_worker.py` 及
  pytest、runtime 静态装配（providers/tts_providers 双表）、main.rs 的
  format/keep-alive 分支、GUI `PythonEnvironmentSpec.qwen3Tts`。
  `AIWORK_QWEN3_TTS_PYTHON` / `AIWORK_QWEN3_TTS_SCRIPT` 零消费者。
- voices 端点：Qwen3-TTS 内置 speaker 清单改按 `org.macai.qwen3-tts` 识别，
  清单常量归端点所有（展示契约）；通用路径继续扫 `spec.path/voices/`。
- GUI 推荐 Qwen3-TTS 条目改指 Runner（id 同步为 Profile id）。
- 兼容性代价：显式 `--provider qwen3-tts` 注册入口消失（MLX TTS 目录本地导入
  暂绑 `org.macai.kokoro`——目录形状无法区分，双轨纪律本就要求显式 provider）。

## Phase：mlx-lm legacy 删除（2026-09-03 落地）

前置：Runner chat.v1 能力通路落地（`9ad777e`）——RunnerProvider 实现 ChatProvider，
delta 事件映射 OpenAI SSE chunk（首 chunk role、末 chunk finish_reason+usage），
attach_runner 注册 chat_providers 表，fake-runner chat 行为 + 组合测试覆盖
流式/非流式/快速失败。

迁移内容（`f4572ae`）：

- `runners/mlx-lm/` 包：manifest `org.macai.mlx-lm`（capabilities=["chat.v1"]）、
  uv 受管环境（mlx-lm==0.31.3，39 包 uv.lock）、Model Profile
  `SmolLM2-135M-Instruct-8bit`（HF `mlx-community/SmolLM2-135M-Instruct-8bit` @
  `0f0d9b82…` immutable revision）。
- adapter `macai_mlx_lm_runner`：legacy `scripts/mlx_lm_worker.py` 语义完整迁移
  （chat template 失败降级 plain join、`make_sampler(temp=…)` temp=0 贪心、
  流式 GenerationResponse 迭代、stdout 重定向隔离协议帧）；engine 校验单测
  4 passed；manifest probe（`import mlx_lm, mlx.core`）真实通过。
- 真实接线证据：SmolLM2（143MB）metal 加载 → 逐 token delta（52 段）→
  result 带 usage（prompt 46 + completion 64，finish=length）→ unload →
  shutdown_complete 全链路（`tests/real_wiring_manual.py`）。
- legacy 完整消费面删除：`providers/mlx_lm.rs`、`scripts/mlx_lm_worker.py` 及
  pytest、runtime 静态装配（providers/chat_providers 双表）、main.rs 的
  `("llm", Some("mlx-lm"))` 映射、目录校验分支与 format/keep-alive 分支、
  GUI `PythonEnvironmentSpec.mlxLm`、`ModelRepository` LLM 目录启发式改指
  `org.macai.mlx-lm`。`AIWORK_MLX_LM_PYTHON` 覆盖变量零消费者。
- GUI 推荐模型：新增 SmolLM2（Runner 轻量 LLM），Qwen3-8B provider 改指
  `org.macai.mlx-lm`；`DaemonAPIRequestTests` pull payload 断言同步。
- 兼容性代价：显式 `--provider mlx-lm` 注册入口消失（`llm` 缺省仍是 llama.cpp，
  MLX LLM 目录本地导入自动绑 `org.macai.mlx-lm`）。

## Phase：kokoro / qwen3-asr legacy 删除（2026-09-03 落地）

Cutover 验收满足后（真实 STT/TTS 闭环、推荐模型已指向 Runner、默认注册目标
集中），删除了两个 legacy 引擎的完整消费面：

- daemon：`providers/kokoro_mlx.rs`、`providers/qwen3_asr.rs` 与全部静态装配；
  `/api/models/load` 的 `stt→qwen3-asr-mlx`、`tts→kokoro-mlx` 显式映射与
  目录校验分支；`/v1/audio/speech` 空模型默认从「写死 kokoro-mlx」改为
  「取第一个已注册 TTS 模型，无则 404」。`/api/models/pull` 对 `org.macai.*`
  provider 的注册走 descriptor 能力判定（此前已有）。
- GUI：`PythonEnvironmentSpec` 移除 `qwen3ASRMlx` / `kokoroMlx`；
  `ModelRepository.directoryProvider` 的 TTS 目录与 Qwen3-ASR 目录检测改为
  指向 `org.macai.kokoro` / `org.macai.qwen3-asr`（后者按 config.json
  `model_type == "qwen3_asr"` 判别，4bit/8bit 兼容）；
  `RuntimeStatusView` 的 provider→类型 fallback 更新。
- scripts：删除 `kokoro_worker.py`、`qwen3_asr_mlx_worker.py` 及其 worker 测试
  （Runner smoke 与 engine 单测已覆盖同语义）。
- 兼容性代价：模型目录手动重注册同一目录的旧入口消失（双轨纪律本就要求
  显式 `/api/models/load` provider 覆盖，无需兼容读取）；`AIWORK_KOKORO_PYTHON` /
  `AIWORK_QWEN3_ASR_MLX_PYTHON` 覆盖变量不再有消费者。

## qwen3-asr-mlx 迁移子任务（2026-09-02 recon）

Legacy 语义已核对：
- worker：`scripts/qwen3_asr_mlx_worker.py`（JSONL stdin/stdout；ready 帧后逐行
  `{id,audio,language}` → `{id,ok,text,language,device}`；load 用
  `mlx_audio.stt.utils.load_model(model_dir, lazy=False)`，generate
  `max_tokens=8192,batch_size=1,temperature=0.0,language=…`；PCM WAV 校验前置；
  `--model/--device(auto|metal)`；设备 env `AIWORK_QWEN3_ASR_MLX_DEVICE`）。
- provider：`providers/qwen3_asr.rs`（python `.build/qwen3-asr-mlx-venv/bin/python`；
  安装提示 mlx-audio==0.5.0——**需与 kokoro 已锁 0.5.1 对齐并真实验证**；
  validate_mlx_8bit_model_dir 校验模型目录）。

子任务与依赖：
1. ✅ **daemon 侧 RunnerProvider 支持 SpeechToText**（144b3c2）。
2. ✅ runner 包：`runners/qwen3-asr/`（e0de16c：manifest org.macai.qwen3-asr +
   profile Qwen3-ASR-0.6B-MLX-4bit、pyproject+uv.lock 40 包、probe 导入通过）。
3. ✅ adapter：`macai_qwen3_asr_runner`（协议入口 + engine 语义迁移；engine
   校验单测 4 passed）。
4. ✅ 接线证据（真实）：Kokoro 生成 WAV → Qwen3-ASR Runner 转写逐字还原
   （"你好，这是Runner接线后的声音。"）。模型源补 preprocessor_config.json
   （mlx-audio feature extractor 必需，ModelScope 仓库未带；取原 8bit HF 同款）。
5. ✅ 默认切换 + 删 legacy（2026-09-03，见上方 Phase 记录）。

阻塞（已解除）：模型源与 immutable revision 已核（ModelScope
`aufklarer/Qwen3-ASR-0.6B-MLX-4bit` @ 3478f17…），下载链路缺陷（TLS/UA/502
映射）修复见 `2026-09-02-download-link-fixes.md`。真实验证待模型落盘后执行。
