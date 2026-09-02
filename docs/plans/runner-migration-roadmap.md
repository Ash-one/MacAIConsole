# Runner 全面迁移路线（2026-09-02）

Status: accepted（用户拍板：目标是把所有合适 provider 切换到 Runner 实验版本）

## 目标与边界

- 目标：Python worker 类 provider 全部迁移到 daemon-owned uv Runner；
  日常模型注册/默认 provider 指向 Runner 版本；最终删除 legacy Provider 路径。
- 边界：Runner 的运行时形态是 python-uv 受管环境。因此 **llama.cpp / whisper.cpp
  这类外部原生二进制 provider 保留现有 Provider**（进程隔离已具备），不硬套 Runner；
  `mock` 直接删除；`macos-say` 若无需要随清理。
- 迁移全集（按序）：kokoro（✓ 已完成）→ qwen3-asr-mlx → qwen3-tts → sherpa-onnx
  →（可选）mlx-lm LLM。

## 当前缺口核对（2026-09-02）

| 缺口 | 状态 |
| --- | --- |
| Runner 环境安装/状态管理面 | ✓（`GET /api/runners` + `POST /install`） |
| 动态 provider 注册 | ✓（6494f78：按 descriptor 能力判定） |
| 请求级 provider 日志 | ✓（e7b0a71） |
| GUI Provider 行识别 Runner | ✓（e7b0a71 RUNNER tag） |
| Runner 模型 voices 端点 | ✓（本就按 `spec.path/voices/*.safetensors` 扫描，116 voices 已实测；default_voice 缺省由引擎兜底 zf_001） |
| 注册加载失败回滚 | ✓（ac3d640） |
| cancel 双向通路（worker 线程化 + HTTP abort） | 未做（每引擎迁移时可后置；长请求目前不可中途取消，与 legacy 同级） |
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

1. 迁移 qwen3-asr-mlx（复用 MLX uv 环境套路，最接近 Kokoro）
2. qwen3-tts
3. sherpa-onnx
4. 收尾：删 mock/macos-say、删 GUI PythonEnvironmentManager（.build 一键安装 legacy 路径）、
   清理 scripts/legacy worker 与 `.build` 引用、更新 README/AGENTS。
