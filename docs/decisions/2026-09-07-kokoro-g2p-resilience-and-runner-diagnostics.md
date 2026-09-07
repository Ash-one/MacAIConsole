# Decision: Kokoro G2P 容灾韧性与 Runner 退出诊断增强

Status: implemented

Class: architecture, runner, diagnostics

Owner: this file

## Problem

在 MacAI 运行 Kokoro-82m-zh TTS 进行中英文混合或纯英文推理时，曾出现如下故障模式：

1. **`/tmp` 文件清理导致 espeak-ng 静默损坏与 C 级崩溃**：
   为了规避 `phonemizer` / `espeak-ng` 内部库路径长限制（超过 200 字符无法加载），Runner 会将 `espeak-ng-data` 复制到 `/tmp/macai-espeak-<hash>/espeak-ng-data`。但 macOS 系统会定期（通常按 3 天无访问时间）清理 `/tmp` 目录下的陈旧文件。清理导致目录结构虽然仍存在，但内部关键表文件（如 `phontab`, `phondata`, `phonindex`）被删空。
   此前 Runner 启动时仅做浅层目录判断 `if not os.path.isdir(target)`，误判数据依然完备。当处理中英文混排的非中文 token 时，`misaki` 调用 `phonemizer` / `espeak-ng` C 接口，C 库在找不到 `phontab` 时直接执行了 C 运行时 `exit(1)`。
   因为 `exit(1)` 发生在 C 层面，Python 代码完全无法通过 `try...except` 捕获，导致 Worker 进程瞬时退出。

2. **G2P 对 espeak 依赖脆弱，缺乏纯 Python 降级通道**：
   Kokoro 的中文前端 `misaki.zh.ZHG2P` 对英文及 OOD (out-of-distribution) 词汇硬依赖 espeak。一旦运行环境中的 espeak 二进制或数据损坏，整个 TTS 管道直接中断，缺少优雅降级机制。

3. **Supervisor 对 Runner 崩溃诊断盲区**：
   在 `crates/ai-daemon` 中，Runner Supervisor 在与子进程通信时，仅监听子进程的标准输出 stdout。当子进程因致命错误或 C `exit(1)` 退出时，Supervisor 仅捕获到 stdout 的 EOF，向上抛出泛化错误：
   `runner supervision: Runner protocol violation: runner protocol I/O error: early eof`。
   而子进程在 stderr 输出的具体错误日志（例如 `Can't read phoneme file: .../phontab`）被 Supervisor 忽略，导致难以从 API 或日志中直观排查崩溃原因。

## Decision Authority

本决策符合 MacAI 核心架构不变量：
- **故障隔离**：Worker 崩溃时 daemon 必须保持存活并返回结构化可诊断错误；
- **单一 Owner**：Runner 进程的生命周期与健康度由 Runner 自身（自愈）与 Supervisor（监控诊断）分层负责；
- **文档与测试要求**：所有架构与行为改动均需提供决策记录、单元测试与验证。

## Decision

### 1. Kokoro espeak-ng 迁移至用户持久缓存目录与自愈

在 `runners/kokoro/src/macai_kokoro_runner/engine.py` 中：
- **迁移目标至持久缓存 (`_get_espeak_target_root`)**：将重定向目标从极易被 macOS 清理的 `/tmp` 改为标准用户缓存目录 `~/.cache/macai/espeak-<digest>`（受 `XDG_CACHE_HOME` / `MACAI_CACHE_DIR` 控制）。路径长度仅约 60 字符（远低于 200 字符限制），且免受 macOS `/tmp` 定期任务清理，重启后无需重新复制。仅在极端超长 HOME 路径场景下才安全降级回退到 `/tmp`。
- **深度完整性检查 (`_is_espeak_data_intact`)**：不仅检查目标目录是否存在，更严格验证 `phontab`、`phondata`、`phonindex` 等核心数据文件均存在且文件大小大于 0。
- **自动防清理 (`_touch_espeak_data`)**：在每次初始化和校验时，主动更新关键文件的 access/modification 时间戳。
- **自愈式原子重构 (`_ensure_short_espeak_data`)**：若发现目标目录缺失或内部数据损坏，立即将其原子重置，并从当前受管 Python 环境（`site-packages/espeakng_loader`）重新复制完整数据；若复制或环境异常，明确返回 `False` 标志而非抛出未捕获异常。

### 2. 纯 Python G2P 兜底与 misaki 补丁

- **Pure-Python 字母拼读音标映射 (`_pure_python_spelling_fallback`)**：
  实现纯 Python 的 English G2P 转换逻辑，将英文字母/单词映射到 Kokoro 标准的 IPA 音素集合（如 `A` -> `eɪ`, `B` -> `bi`, `C` -> `si` 等），同时支持基础英文数词与常见符号转换。
- **Misaki 中文前端容灾补丁 (`_patch_misaki_zh_version`)**：
  在 Runner 引擎启动时动态修补 `misaki.zh.ZHG2P` 的英文/未知词转换方法。如果检测到 `espeak` 数据未就绪或无法使用，自动降级至纯 Python 兜底实现，保障即便完全缺少底层 C 依赖，中英文混排语音合成仍可稳定发音，绝不引发进程崩溃。

### 3. Supervisor 子进程 stderr 捕获与崩溃诊断上报

在 `crates/ai-daemon/src/runners/supervisor.rs` 中：
- 在启动 Runner 子进程时，以非阻塞/异步方式监听子进程的 `stderr`，并缓冲最近的 stderr 输出行（`captured_stderr`）。
- 当子进程在握手阶段（Hello 交换）或后续推理流交互中意外退出触发 `early eof` 时，Supervisor 自动抓取已记录的 stderr 内容，将其拼接至结构化错误信息中输出，如：
  `runner protocol I/O error: early eof; captured stderr: ...`。
- 彻底消除“黑盒崩溃”，使系统及用户可第一时间在日志中看到真实崩溃堆栈或致命错误信息。

## Consequences

### Positive
- **健壮性大幅提升**：彻底解决了长期运行或休眠后 `/tmp` 清理导致 Kokoro TTS 必然崩溃的问题，并在极端损坏场景下具备纯 Python 兜底发音能力。
- **排障效率显著改善**：任何 Runner 子进程的启动或运行时意外退出，均能携带精准的 stderr 诊断上下文，无需附加调试脚本即可定位。
- **完全兼容现有协议**：不修改 Runner Protocol 的现有消息格式，对外部客户端透明。

### Negative / Trade-offs
- 当降级到纯 Python 兜底时，生僻英文单词将按字母逐字发音，音质及连读略逊于完整 espeak-ng 词典，但相比进程崩溃或 500 错误，保障了系统的可用性与韧性。

## Verification Evidence

1. **Python 单测**：
   - 编写 `test_adapter.py`，覆盖了纯 Python 字母音标映射、空/非字母过滤、espeak-ng 深度完整性检测、损坏数据自动重建与自愈，以及在无 espeak 支持下 ZHG2P 的正常发音流程。所有 20 项测试全部通过。
2. **Supervisor 错误信息丰富化单测与 Cargo 校验**：
   - `cargo test --workspace` 全部通过。
   - `cargo fmt --all -- --check` 检查通过。
3. **真实端到端合成验证**：
   - 运行 Hermes 包含英文缩写与中文混合的文本（如 "MacAI", "TTS", "API"），合成成功并输出有效音频帧。
