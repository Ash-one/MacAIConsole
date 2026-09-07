# Decision: whisper.cpp Runner 迁移

Status: implemented

Class: architecture

Owner: this file

Architecture owner: [`Runner 插件架构`](2026-09-02-runner-plugin-architecture.md)

## Problem

whisper.cpp 曾由 `Runtime::with_store` 静态装配 `WhisperCppProvider`，二进制由仓库
脚本或 `AIWORK_WHISPER_CLI` 提供。它绕过 `/api/runners` 的统一安装、状态、进程
监督和环境契约，使 GUI「引擎」区块无法完整管理所有生产 Provider。

## Requirement delta

| Item | Previous accepted state | New accepted state | Disposition |
| --- | --- | --- | --- |
| Provider owner | daemon 内静态 `WhisperCppProvider` | `runners/whisper.cpp` + 通用 `RunnerProvider` | replace |
| 引擎安装 | 本地构建脚本或 `AIWORK_WHISPER_CLI` | `/api/runners/org.macai.whisper.cpp/install` 校验官方 source archive、CMake 构建并原子提升 | replace |
| 进程生命周期 | 每次请求执行 `whisper-cli` | load 时启动常驻 `whisper-server` 并加载一次模型，infer 复用 | replace |
| 模型选择 | 静态 `whisper.cpp` provider | 缺省 `org.macai.whisper.cpp`；旧别名规范化且保存 requested / selected / reason | replace |
| STT 输入 owner | daemon decode-on-ingest 为 PCM WAV | 保持；Runner 只接受 PCM WAV | keep |
| 设备语义 | Provider 解析 Core ML / Metal | Runner 从 server 日志报告 effective device，daemon 只消费 | replace |

## Decision

仓库随附 `org.macai.whisper.cpp` built-in Runner，实现 `stt.v1`。Python worker 只承担
Runner Protocol 与 loopback HTTP adapter；`whisper-server` 子进程承担真实模型驻留和
推理。daemon 把每个 Runner 启动为独立进程组，Runner 启动的引擎子进程继承该进程组；
正常 unload/shutdown 由 adapter 优雅终止 server，Runner 崩溃、协议超时或被强杀时由
daemon 终止整个进程组。一个 Runner instance 同时只承载一个已加载模型和一个活动
inference，继续受 daemon lease、busy guard、LRU、keep-alive 与 shutdown owner 约束。

官方 b4938 未发布 macOS CLI 二进制，因此 manifest 固定 commit
`371b5a7561823ab2bb32142d2751e35e7534727b` 的 source archive 和 SHA-256。Runner install
在 daemon-owned staging 中调用 CMake，关闭 shared libraries，构建 `whisper-server`，
校验目标二进制后原子提升到 `Engines/org.macai.whisper.cpp/`。静态链接保证 staging
目录提升后不会留下指向旧 build tree 的 dylib 路径。

`no_context` 属于 `/inference` 的逐请求字段；b4938 server 不提供同名启动参数，Runner
不会把请求级选项写入 server 启动 argv。

`MACAI_WHISPER_SERVER` 是受 manifest allowlist 约束的开发/诊断 override。生产默认路径
来自 `.macai-engine.json` 指纹，`AIWORK_WHISPER_CLI` 和旧构建脚本不再属于产品契约。

注册本地 STT `.bin` 时，daemon 为它构造 ad-hoc Model Profile；重启 bootstrap 会从
注册表恢复绑定。历史 provider 值 `whisper.cpp` 在读取注册表时迁移为
`org.macai.whisper.cpp` 并保存迁移原因。

Core ML encoder 仍使用 whisper.cpp 上游约定的模型同目录命名；加载日志确认 Core ML
时报告 `coreml`，否则确认 Metal 时报告 `metal`，其余报告 `cpu`。单个 Runner 探测或
加载失败只影响该 Provider，返回结构化错误，不使 `/api/providers` 整体失败。

## Alternatives considered

**保留静态 Provider。** 语音路径成熟，但会长期保留第二套安装、状态和生命周期
owner，也无法从 GUI 统一安装。

**把旧构建脚本包装为 GUI 操作。** 它只能补安装入口，Provider 装配、status 和进程
监督仍然分叉。

**采用第三方预编译 macOS 资产。** 构建速度更快，但供应来源和目标选项无法由
whisper.cpp 官方 release 直接证明。本决策选择固定官方 source archive。

## Verification obligations

| Acceptance | Failure surface | Direct evidence |
| --- | --- | --- |
| 全新环境可安装引擎 | download、checksum、CMake、atomic promote | `whisper_source_engine_installs_when_smoke_is_enabled` |
| `.bin` 缺省、显式与旧别名选择可审计 | API、registry、restart | provider selection + registry migration + ad-hoc binding tests |
| 常驻 server 完成真实转写 | protocol、environment、load/infer/unload | `runner_whisper_real_wiring`（显式目标机环境） |
| Runner 异常退出不遗留引擎子进程 | crash、timeout、forced drop | `dropping_exited_runner_kills_its_descendant_process_group` |
| 上传容器统一为 PCM WAV | wav/mp3/flac/ogg/m4a decode-on-ingest | `audio` 模块 fixture tests；Runner adapter 拒绝非 PCM WAV |
| effective device 来自 worker | Core ML / Metal / CPU 日志解析与 status | adapter tests + real wiring instance snapshot |
| 旧实现不可达 | source、config、docs、tests | 负向搜索 + Rust/Swift/Runner 全套测试 |

真实安装与组合测试需要网络、模型和目标机硬件，因此保持显式 opt-in；CI 固化 manifest、
协议、adapter、选择、迁移与音频归一化边界。

### 2026-09-05 target-machine evidence

- 官方 source archive SHA-256：`89051d8fca516a3ad1f5c2f8f9d2fccb089afbaec338fca3f8731999babc6f81`；
- `MACAI_WHISPER_ENGINE_INSTALL_SMOKE=1 ... cargo test -p ai-daemon whisper_source_engine_installs_when_smoke_is_enabled -- --ignored --nocapture`：下载、构建、提升及 `--help` smoke passed；
- base 模型 SHA-256：`60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe`；
- `cargo test -p ai-daemon --test runner_whisper_real_wiring -- --ignored --nocapture`：load → infer → unload → shutdown passed，输入 `speech.wav` 的实际结果为“你好,这是一段视听文本。”，language=`chinese`；
- `cargo test --workspace`、MacAIConsole 36 tests、七个 Runner 的 42 个 adapter tests、
  `cargo fmt --all -- --check` 与 `git diff --check` passed。
- 异常清理修正后，`dropping_exited_runner_kills_its_descendant_process_group` 验证 daemon
  丢弃 Runner 时同步终止其进程组内的引擎子进程。

## Consequences

- 所有生产 Provider 现在都由 Runner 动态装配，`providers/` 只保留测试/CLI 兜底能力；
- GUI 和 API 使用同一 Runner 安装及观测面；
- whisper.cpp source build 使首次安装耗时增加，并要求目标机存在 CMake 与 Xcode toolchain；
- 升级 whisper.cpp 必须同时更新 immutable source revision、checksum、构建选项并重跑真实
  引擎安装和转写验证。
