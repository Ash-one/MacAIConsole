# LLM 多轮对话 Prompt Cache（KV 前缀复用）

Status: proposed

Class: feature

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[Runner Protocol v1](../specs/runner-protocol-v1.md)、[结构化推理输出契约](2026-09-12-structured-reasoning-output.md)

## Problem

多轮对话的每一轮请求都携带完整 `messages` 历史，但当前两个 LLM Runner 对历史 KV 的处理不对称：

- MLX-LM Runner 每轮用 `build_prompt_tokens` 全量重 tokenize 并完整 prefill（`runners/mlx-lm/src/macai_mlx_lm_runner/engine.py:115-143`），上一轮生成的 KV 全部丢弃。TTFT 随历史长度线性增长，长对话下每轮都在为已确认的前缀重复付费。
- llama.cpp Runner 的 llama-server 是常驻进程（`runners/llama.cpp/src/macai_llama_cpp_runner/engine.py:134-168`），同一 slot 上公共前缀的 KV 复用事实上已经发生——但这依赖"server 恰好常驻 + 请求恰好顺序到达"的隐式行为，启动参数没有显式声明（仅 `--host/--port/--model`，engine.py:139-147），slot 丢失或上下文偏移后的复用不受保障，且没有任何观测手段。
- 公共契约层面没有会话标识。`ChatRequest`（`crates/ai-core/src/request.rs:18-26`）与 Runner infer 载荷（`crates/ai-daemon/src/runners/provider.rs:643-648`）都不携带"这个请求延续哪个对话"的信息，无法表达缓存粘性，后续任何 cache 感知调度都要先补这个契约。

问题不依赖具体引擎：任何常驻 worker 承载的自回归生成，都应让已确认的对话前缀只计算一次。模型格式的解释留在 Runner（与推理输出归属同一边界），daemon 只负责契约与调度。

## Proposed direction

三个改动互相独立、可分别交付，按下列顺序实施。

### 1. MLX-LM Runner：显式 prompt cache 与增量 prefill

引擎持有的不再只是模型和 tokenizer：

- `MlxLmEngine` 在 `_load` 时调用 `mlx_lm.models.cache.make_prompt_cache(model)` 建一个 cache，并维护它当前表示的 token 序列（上一轮的 prompt tokens + 生成的 tokens）；
- `stream_chat` 每轮计算新 prompt tokens 与已存序列的公共前缀长度：用 `can_trim_prompt_cache` / `trim_prompt_cache` 裁掉分歧尾部，然后以 `prompt[公共前缀:]` 调 `stream_generate(..., prompt_cache=cache)`，只增量 prefill 剩余部分（mlx-lm 0.31.3 的 `generate_step` 接受 `prompt_cache` 并原地更新，但不自动做前缀检测，前缀管理由 Runner 负责）；
- 生成结束后把 prompt + 生成 tokens 记为 cache 的新表示；
- 公共前缀为 0、cache 不可裁剪或裁剪异常时，重建 cache 全量 prefill——任何 cache 路径的失败都退化为现有行为，worker 不因此进入新错误语义。

cache 生命周期与 worker 一致（`load`/`unload` 帧创建/销毁），随对话历史与模型上下文自然有界；不引入独立的 cache 配额，内存占用经进程 RSS 由 daemon 现有 resident 统计观测。单 cache 的正确性由 `[capacity] max_concurrency_per_instance = 1`（`runners/mlx-lm/runner.toml`）的串行保证；未来提升并发前必须先引入会话隔离。

不使用库内 `LRUPromptCache`/`PromptTrie` 多条目缓存管理：单并发单会话场景下单 cache + 公共前缀裁剪即可覆盖，多条目管理的复杂度等出现真实多会话需求时再评估。

### 2. llama.cpp Runner：显式声明 cache-reuse

`LlamaCppEngine.start()` 的启动参数追加 `--cache-reuse 256`（受管引擎 b10785 支持；值取 llama.cpp 文档建议的最小复用块长度）。效果：

- 保障既有隐式 slot 前缀复用的语义被显式声明，不被引擎默认值变化静默改变；
- 在上下文偏移（context shift）和 system prompt 之后的前缀变化场景下做分块 KV 复用，而不是丢弃重来。

参数作为 adapter 内常量，不新增 runner.toml 或 load payload 的配置面：当前没有需要按模型调节的实证，先建立确定性默认。

### 3. daemon：可选 `session_id` 契约与透传

- `ChatRequest` 增加可选 `session_id: Option<String>`（serde default + `skip_serializing_if`），缺省时请求语义与序列化字节与现状一致；
- Runner bridge 的 `chat_request` 载荷在字段存在时透传 `session_id`，缺席时不写该 key；
- Runner 协议 v1 的"未知可选字段必须忽略"规则（`docs/specs/runner-protocol-v1.md`）覆盖不支持它的第三方 Runner；llama.cpp / mlx-lm adapter 本期只接受不赋予语义。

`session_id` 的即时可观察效果是契约本身：调用方（未来 GUI 会话、`step 4` 调度）可以声明对话延续性，wire 契约从此稳定，后续扩展不需要二次 break。本期不要求任何客户端开始发送它。

两个 LLM Runner 的 adapter 都需要容忍 assistant 历史携带 `reasoning_content`（结构化推理提案）导致的模板输出前缀分歧——公共前缀裁剪天然处理，无需特殊分支。

## Alternatives considered

### 引入 vLLM 类引擎

PagedAttention + continuous batching 面向多用户高并发服务；本项目单机单用户、`max_concurrency=1`。vLLM 不原生支持 Apple Silicon GPU（官方仅实验性 CPU backend，`vllm-metal` 插件内部仍以 MLX 为计算后端），引入它同时违反"低依赖优先"与 Runner 边界约定，收益为负。

### 在 daemon 统一管理 KV cache

daemon 看不到模型内部结构，cache 语义（slot、前缀裁剪、模板渲染）都是引擎事实；集中管理会重演"Runner 拥有模型特有适配"边界被侵蚀的问题。

### 使用 mlx-lm 的 `LRUPromptCache` / trie 前缀检索

库内为多 prompt 多会话服务设计（`mlx_lm.server` 使用），单并发场景下单 cache + 公共前缀裁剪已覆盖，多条目管理是未被证明需要的复杂度。

### 现在不加 `session_id`，等 step 4 需要时再加

省一次字段落盘，但届时若调用方已在构造请求，字段新增虽向后兼容，消费者验证与文档同步都要重做；可选字段现在加入的成本近似为零。

## Acceptance criteria

| 可观察验收 | 失败层 | 直接证据 |
| --- | --- | --- |
| 同一对话第二轮的实际 prefill token 数显著小于完整历史长度（长历史下 TTFT 相应下降） | mlx-lm adapter | adapter 单测用可观测 prefill 步数的假模型断言增量行为；`scripts/build-app.sh release` 后真实 MiniCPM 两轮请求对比 TTFT |
| temp=0 时，启用 cache 的多轮输出与每轮重建 cache 的输出一致 | mlx-lm adapter | 真实模型本地对比脚本（非 CI 门禁），分歧即数值精度问题需记录 |
| 前缀裁剪/前缀计算的任何异常都退化为全量 prefill，worker 存活且错误语义不变 | mlx-lm adapter | 注入异常的单测 |
| llama-server 启动命令包含 `--cache-reuse 256`；真实第二轮请求的 `timings.prompt_eval_count` 远小于完整历史 token 数 | llama.cpp adapter | command 构造单测；真实请求观察 SSE 末帧 timings |
| 缺省 `session_id` 的请求行为与现状一致；携带 `session_id` 时透传进 `chat.v1` infer 载荷 | ai-core 序列化、Runner bridge | serde 兼容单测 + fake Runner composition test |
| 不识别 `session_id` 的 v1 Runner（mock、Script Runner）请求不受影响 | Runner protocol 兼容 | 现有 mock tests 保持通过 |
| README 与本记录描述已实现契约，proposal 不再声称未来行为 | authority convergence | 实现交付时按生命周期改写本文并同步 README |

## Risks and trade-offs

- 常驻 KV 使进程 RSS 超出"模型权重 + 固定开销"的调度假设。本提案先观测不设限；若实测触碰内存预算，再在实现中评估 `make_prompt_cache` 的 `max_kv_size` 上限并补记录。
- Metal 上增量 prefill 与全量 prefill 的数值精度存在理论差异，temp=0 也可能偶发分岔；接受该风险，对比验证若出现即记录在案。
- `--cache-reuse` 的行为绑定受管引擎版本（当前 b10785）；升级引擎产物时需在验证清单中复核该参数。
- `session_id` 在 step 4 落地前没有 daemon 侧消费者，存在"死契约"窗口；这是有意的提前量，且字段缺席即零影响。
- llama.cpp 侧的隐式复用收益依赖请求串行到达同一 slot；`max_concurrency_per_instance = 1` 是该假设的现行保证，未来放开并发必须连带重审。

## Intentionally deferred

- **Cache 感知调度（本提案第 4 步，按要求记录、暂不实现）**：keep-alive reaper 到期与 LRU 驱逐目前以模型权重为粒度，`unload` 即杀 worker、cache 一并销毁（`crates/ai-daemon/src/runtime.rs:1118-1216`）。后续方向：新增"清 cache 不卸载"协议帧让 reaper 到期先释放 KV；驱逐代价把"worker 仍有热 cache"纳入考量（`last_used` 语义向 cache 命中延伸）；`session_id` → slot/prompt-cache 的粘性绑定（依赖并发模型放开）。这些都需要本提案的 `session_id` 契约与 Runner 协议扩展先行落地。
- KV cache 磁盘持久化（`save_prompt_cache` / llama.cpp `--slot-save-path`）：重启后免 prefill 的收益对当前单机对话场景未被证明，且引入磁盘格式契约。
- 多 slot / 并发提升与会话隔离。
- Runner 向 daemon 上报 cache 命中统计及 GUI 展示。
- GUI 会话与 CLI 开始发送 `session_id`。
