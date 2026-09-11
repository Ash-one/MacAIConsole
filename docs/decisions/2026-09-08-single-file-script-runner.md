# Decision: 单文件 Script Runner 创建

Status: implemented

Class: feature

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[uv Python 运行环境](2026-09-02-uv-python-environments.md)

## Problem

当前 Runner 作者必须同时维护 `runner.toml`、`pyproject.toml`、`uv.lock`、协议入口、
引擎适配代码和目录结构。即使只是把一个 Python 推理库接到现有 `chat.v1`、`stt.v1`
或 `tts.v1`，也需要理解完整 Runner Protocol 与打包约定，GUI 只能安装仓库随附的
built-in Runner。

用户需要一个能在普通文本编辑器或 MacAIConsole 内完成的最小作者入口，同时保持
daemon 对代码信任、依赖安装、进程监督和模型生命周期的唯一所有权。

## Decision

引入 `.macai.py` 单文件 Script Runner。文件使用 [PEP 723](https://peps.python.org/pep-0723/)
`script` metadata：标准字段声明 Python 版本和直接依赖，`[tool.macai]` 声明 Runner ID、
版本、单一 capability、adapter、模型形态、权限和 timeout。`load` 返回由 host 持有的模型
对象，对应 capability 的推理 hook 接收该对象；`unload(model)` 是可选清理 hook。

daemon 的 inspect API 只解析 UTF-8 源码并返回 digest、元数据、依赖和权限，不导入或
执行文件。用户确认后，create API 重新核对 digest，在 staging 生成标准 `runner.toml`、
`pyproject.toml`、`uv.lock`、用户 `runner.py` 与 MacAI-owned protocol host；随后执行临时
`uv sync` 和 import/hook probe，验证依赖、导入和 hook 后删除临时环境，再把 package 移入受管
Plugins 目录并记录 digest trust。创建后返回 `restart_required = true`；GUI 使用现有
daemon 重启状态机完成装配。

MacAIConsole 在「管理 → 引擎」提供“新建 Script Runner”，使用原生 AppKit 文本系统载入
daemon 返回的 Chat/STT/TTS 模板，并对 Python 与 PEP 723 metadata 提供轻量语法高亮。
“推理库”助手提供标准库、Transformers、MLX-LM 和 MLX-Audio 预设，也可安全解析用户粘贴
的 `pip install` / `uv add` 命令；GUI 只把归一化后的直接依赖写入 metadata，完整依赖树由
uv lock 拥有。首版每个文件只声明一种 capability；复杂包继续使用完整 Runner package。

## Alternatives considered

**在单文件内手写完整 Runner Protocol。** 不需要 host，却把 framing、correlation ID、
生命周期和错误语义暴露给每个作者，保留了当前主要门槛。

**新增独立 Script Provider。** 可以直接从 daemon 调 Python hook，也会形成第二套进程、
环境、lease 和错误 owner。编译为标准 Runner 包能复用已经验证的组合路径。

**在 GUI 中生成完整目录工程。** 适合复杂 Runner；简单适配仍需维护多个生成文件，无法
满足单文件可分享和普通编辑器可编辑的目标。

## Verification

- `script::tests` 固定 PEP 723 单块解析、标准 package 生成，以及 Chat/STT/TTS 三个模板的
  完整 initialize/load/infer/unload/shutdown 生命周期。
- daemon handler 测试固定 inspect 不执行 Python 顶层代码，并拒绝 inspection 后发生变化的
  source digest；真实 create 组合测试固定 lock、临时 sync/probe、清理和 Runner trust 持久化。
- `trusted_script_runner_attaches_from_plugins_after_restart` 固定已信任 package 在重启后从
  `Plugins/` 被发现并装配为 Provider。
- Swift API 请求测试固定源码和 expected digest 的传输；MacAIConsole 全套 Swift tests
  覆盖客户端编译与现有行为回归。
- dependency helper 测试固定预设合并、安装命令解析、去重和参数拒绝；Swift 测试固定 daemon
  请求以及只替换 active PEP 723 dependency 行。
- hook 存在性与 import 由 create 阶段的临时受管环境 probe 验证；inspect 不执行用户 Python。
  probe 只在用户点击“信任并添加”后执行，create 失败会清理 staging，不留下 trusted 半成品。
- `cargo fmt --all -- --check` 与 `cargo test --workspace` 通过：136 passed、3 ignored；
  `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --enable-xctest`
  通过：47 passed；`git diff --check` 与 Python host 编译检查通过（2026-09-11）。
- release App 实际路径验证：依赖助手将 `pip install sentencepiece` 归一化并写回
  `dependencies = ["sentencepiece"]`，编辑器高亮和安全提示同时可见（2026-09-10）。

## Consequences

- 新 Python 后端无需实现 framing、worker supervision、lease 或环境状态机，也不需要新增
  Swift Runner 枚举；生成包复用现有 Runtime 边界。
- 单文件源码仍是以 daemon 用户权限运行的本地代码；digest 固定身份但不提供作者认证或
  OS 沙箱。
- create 阶段需要 uv 解析、同步依赖并导入用户脚本进行 probe，可能联网并以 daemon 用户权限
  执行代码；GUI 在按钮前明确展示该行为。
- Python hook API 是新的作者契约，需要以 host composition test 固定，不能让其语义从
  当前 Python 实现细节隐式生长。
- 首版放弃多 capability、额外本地模块、原生源码资产、在线市场、热加载和 LSP；出现真实
  需求时迁移为完整 Runner package 或另立扩展决策。
