# LLM 多轮对话 Prompt Cache（KV 前缀复用）

Status: implemented

Class: feature

Owner: this file

Related current decisions: [Runner 插件架构](2026-09-02-runner-plugin-architecture.md)、[Runner Protocol v1](../specs/runner-protocol-v1.md)、[结构化推理输出契约](2026-09-12-structured-reasoning-output.md)

## Problem

多轮对话的每一轮请求都携带完整 `messages` 历史，但两个 LLM Runner 对历史 KV 的处理不对称：

- MLX-LM Runner 每轮用 `build_prompt_tokens` 全量重 tokenize 并完整 prefill，上一轮生成的 KV 全部丢弃。TTFT 随历史长度线性增长，长对话下每轮都在为已确认的前缀重复付费。
- llama.cpp Runner 的 llama-server 是常驻进程，同一 slot 上公共前缀的 KV 复用事实上已经发生——但这依赖"server 恰好常驻 + 请求恰好顺序到达"的隐式行为，启动参数没有显式声明，slot 丢失或上下文偏移后的复用不受保障，且没有任何观测手段。
- 公共契约层面没有会话标识。`ChatRequest` 与 Runner infer 载荷都不携带"这个请求延续哪个对话"的信息，无法表达缓存粘性，后续任何 cache 感知调度都要先补这个契约。

问题不依赖具体引擎：任何常驻 worker 承载的自回归生成，都应让已确认的对话前缀只计算一次。模型格式的解释留在 Runner（与推理输出归属同一边界），daemon 只负责契约与调度。

## Decision

三个改动互相独立、已分别交付。

### 1. MLX-LM Runner：显式 prompt cache 与增量 prefill

`MlxLmEngine` 持有一个与 worker 同生命周期的 prompt cache（`load` 创建、`unload` 随引擎丢弃），并维护它当前表示的 token 序列：

- `stream_chat` 每轮计算新 prompt 与已缓存序列的公共前缀，复用上限取 `min(公共前缀, len(prompt)-1)`——至少留 1 个 token 交给 `generate_step`（空 prompt 会 `ValueError`），与官方 `mlx_lm.server` 的 `fetch_nearest_cache` 同款规则；以 `prompt[keep:]` 调 `stream_generate(..., prompt_cache=cache)`，只增量 prefill 分歧尾部；
- 裁剪经 `trim_prompt_cache` 原地完成，并校验返回的实际裁剪数：公共前缀为 0、裁剪异常或裁剪数与请求不符时重建 cache 全量 prefill；
- 生成迭代由引擎内 `_recording` 包装器透传，正常耗尽后把 cache 登记为 `prompt + 全部已产出 token`（含末帧携带的终止 token）：`generate_step` 在每个 yield 点前已把当前 token 回喂进 cache，该序列与真实 KV 状态一致，与 `mlx_lm.server` 的 cache_key 登记规则相同；迭代异常时丢弃登记，下一轮退化为全量 prefill。

任何 cache 路径的失败都退化为既有行为，worker 不进入新错误语义。不设独立 cache 配额，内存经进程 RSS 由 daemon 现有 resident 统计观测；单 cache 的正确性由 `max_concurrency_per_instance = 1`（`runners/mlx-lm/runner.toml`）的串行保证，未来提升并发前必须先引入会话隔离。不使用库内 `LRUPromptCache`/`PromptTrie` 多条目管理：单并发单会话场景下单 cache + 公共前缀裁剪即可覆盖。

mlx-lm 依赖经 `engine.py` 模块级 seam（`_make_prompt_cache` / `_trim_prompt_cache` / `_stream_generate` / `_make_sampler`）惰性导入，单测以假实现注入，不需要安装 mlx。

### 2. llama.cpp Runner：显式声明 cache-reuse

`_server_command` 构造的启动参数包含 `--cache-reuse 256`（本地 `.build` 与受管 b10785 引擎均支持；值取 llama.cpp 文档建议的最小复用块长度）。既有隐式 slot 前缀复用被显式声明，不被引擎默认值变化静默改变；上下文偏移和 system prompt 之后的前缀变化场景做分块 KV 复用而不是丢弃重来。参数是 adapter 内常量，不新增 runner.toml 或 load payload 配置面。

### 3. daemon：可选 `session_id` 契约与透传

- `ChatRequest` 携带可选 `session_id: Option<String>`（`skip_serializing_if`），缺省时请求语义与序列化字节与字段引入前一致；
- Runner bridge 的 `chat.v1` 载荷仅在字段存在时写入 `session_id`（字符串），缺席时不写该 key——null 与缺席不等价，第三方 Runner 可据此区分；
- Runner 协议 v1 的"未知可选字段必须忽略"规则覆盖不支持它的 Runner；两个 LLM adapter 对该字段不赋予语义。

`session_id` 的即时可观察效果是契约本身：调用方可以声明对话延续性，wire 契约从此稳定。本期没有任何客户端被要求发送它，daemon 也不做调度消费。

## Alternatives considered

### 引入 vLLM 类引擎

PagedAttention + continuous batching 面向多用户高并发服务；本项目单机单用户、`max_concurrency=1`。vLLM 不原生支持 Apple Silicon GPU（官方仅实验性 CPU backend，`vllm-metal` 插件内部仍以 MLX 为计算后端），引入它同时违反"低依赖优先"与 Runner 边界约定，收益为负。

### 在 daemon 统一管理 KV cache

daemon 看不到模型内部结构，cache 语义（slot、前缀裁剪、模板渲染）都是引擎事实；集中管理会重演"Runner 拥有模型特有适配"边界被侵蚀的问题。

### 使用 mlx-lm 的 `LRUPromptCache` / trie 前缀检索

库内为多 prompt 多会话服务设计（`mlx_lm.server` 使用），单并发场景下单 cache + 公共前缀裁剪已覆盖，多条目管理是未被证明需要的复杂度。

### 现在不加 `session_id`，等 cache 感知调度需要时再加

省一次字段落盘，但届时若调用方已在构造请求，消费者验证与文档同步都要重做；可选字段提前加入的成本近似为零。

## Consequences

- **temp=0 数值分岔已实测出现（仅 MLX 侧）**：Metal 上增量 prefill 与全量 prefill 的浮点求和路径不同（共享前缀的 K/V 在不同 batch 形状下计算），贪心解码在概率接近处可能分岔。MiniCPM5-2B-MLX 两轮对比中，第二轮输出在 94 字符公共前缀后出现措辞级分岔，语义连贯、无重复/乱码等状态损坏特征；MiniCPM5-1B 的短问答与长文档两场景均复现分岔。同底座的 llama.cpp（Q4_K_M gguf）在相同场景下缓存命中与全量 prefill 输出完全一致。该现象是前缀复用系统的固有精度行为，被接受为已记录的风险；对输出确定性有硬要求的调用方目前没有开关可关闭复用。
- 思考型对话的前缀复用上限受模板对齐约束：历史 assistant 消息的渲染（模板会剥离 think 部分）与生成流（含 think 标签或预填）在 assistant 边界分歧，公共前缀到该边界为止（MiniCPM5 实测：短问答首轮追问仅复用指令前缀 ~25 token；长文档场景复用完整用户轮 ~508 token）。这是正确的降性而非错误——KV 依赖绝对位置，分歧后必须重算。
- 常驻 KV 使进程 RSS 超出"模型权重 + 固定开销"的调度假设。当前先观测不设限；若实测触碰内存预算，再评估 `make_prompt_cache` 的 `max_kv_size` 上限并回到本记录修订。
- `--cache-reuse` 的行为绑定受管引擎版本；升级引擎产物时需在验证清单中复核该参数（本地 `.build` llama-server build 8086439 已确认支持）。
- cache 收益依赖请求串行到达同一 worker/slot；`max_concurrency_per_instance = 1` 是该假设的现行保证，未来放开并发必须连带重审。
- `session_id` 在 cache 感知调度落地前没有 daemon 侧消费者，存在"死契约"窗口；这是有意的提前量，字段缺席即零影响。

仍然有意不做：cache 感知调度（keep-alive reaper 到期先"清 cache 不卸载"、LRU 驱逐代价纳入热 cache、`session_id` → slot/prompt-cache 粘性绑定——都需要协议扩展与并发模型放开，属本决策的 Deferred 方向）；KV cache 磁盘持久化（`save_prompt_cache` / `--slot-save-path`）——重启免 prefill 的收益未被证明且引入磁盘格式契约；多 slot / 并发提升与会话隔离；Runner 向 daemon 上报 cache 命中统计及 GUI 展示；GUI 会话与 CLI 发送 `session_id`。

## Verification

- `cargo fmt --all -- --check` 通过；`cargo test --workspace` 全部通过（含 ai-daemon 各测试组，2 例环境门禁的 real-wiring ignored）。
- `cargo test -p ai-core` 固化 wire 契约：缺省 `session_id` 解码为 None 且再序列化无该 key，携带时 canonical 名进出。
- `runner_runtime_composition.rs` 的 `chat_stream_forwards_session_id_to_runner_payload`：fake Runner 服务端校验——携带 `session_id` 时 infer 载荷必须含匹配的 string key；缺席时载荷不得出现该 key（daemon 若写 null 即失败）。不支持该字段的 v1 Runner（mock、Script Runner）现有测试保持通过。
- Runner 包 `uv lock --check` 与 pytest 通过：mlx-lm 16 例（含 `test_engine_cache.py` 8 例——假 `stream_generate` 使每轮实际 prefill token 数可观测，断言增量 prefill、prompt 被完全覆盖时仅 prefill 末 token、公共前缀为 0/裁剪异常/裁剪数不符的退化重建、生成失败后 cache 重置）；llama.cpp 8 例（含 `_server_command` 断言 `--cache-reuse 256`）。
- 真实模型（本机 MiniCPM5-2B-MLX，两轮对话第二轮复用第一轮真实输出）：第二轮实际 prefill 304 → 21 tokens（94% 复用），首 token 延迟 0.65s → 0.20s；temp=0 分岔已按 Consequences 记录。
- 真实模型 llama.cpp 侧（本机导入的 MiniCPM5-1B-Q4_K_M，llama-server SSE 末帧原生 `timings`）：长文档场景第二轮完整历史 536 tokens，引擎路径实际 prompt 处理 28 tokens（复用 508），TTFT 0.45s → 0.03s；短问答场景受模板对齐约束仅复用 25 tokens；缓存命中与全新 server 全量 prefill 的 temp=0 输出完全一致。与 MiniCPM5-1B-MLX 同场景对比：两引擎复用同一公共前缀（508 tokens），MLX 第二轮 TTFT 0.58s → 0.29s。
- 根 `README.md` 与 `docs/specs/runner-protocol-v1.md` 描述当前契约。
