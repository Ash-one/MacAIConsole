"""MLX-LM Chat Runner engine：legacy `scripts/mlx_lm_worker.py` 语义迁移。

保持：MLX/Metal 加载、chat template（缺失/损坏时降级 plain join）、
`make_sampler(temp=…)`（temp=0 对应库内贪心解码）、流式 GenerationResponse
迭代、单次失败不杀 worker。stdout 保留给协议帧，库日志一律走 stderr。
"""

from __future__ import annotations

import contextlib
import sys
from dataclasses import dataclass
from typing import Any, Iterator

DEFAULT_MAX_TOKENS = 1024
DEFAULT_TEMPERATURE = 0.7
DEFAULT_TOP_P = 1.0
MAX_TEMPERATURE = 2.0


class ChatRequestError(ValueError):
    """The chat request frame violates the worker protocol."""


@dataclass(frozen=True)
class LoadedModel:
    model: Any
    tokenizer: Any
    device: str = "metal"


class ThinkingStreamParser:
    """Split thinking deltas without leaking partial boundary tags."""

    START = "<think>"
    END = "</think>"

    def __init__(self, enabled: bool) -> None:
        self.in_reasoning = enabled
        self.trim_content_prefix = False
        self.buffer = ""

    def feed(self, text: str) -> tuple[str, str]:
        if not self.in_reasoning:
            if self.trim_content_prefix:
                text = text.lstrip("\n")
                if text:
                    self.trim_content_prefix = False
            return "", text
        self.buffer += text
        if self.buffer.startswith(self.START):
            self.buffer = self.buffer[len(self.START) :].lstrip("\n")
        elif self.START.startswith(self.buffer):
            return "", ""

        end = self.buffer.find(self.END)
        if end >= 0:
            reasoning = self.buffer[:end]
            content = self.buffer[end + len(self.END) :].lstrip("\n")
            self.buffer = ""
            self.in_reasoning = False
            self.trim_content_prefix = not content
            return reasoning, content

        held = _tag_prefix_length(self.buffer, self.END)
        reasoning = self.buffer[:-held] if held else self.buffer
        self.buffer = self.buffer[-held:] if held else ""
        return reasoning, ""

    def finish(self) -> tuple[str, str]:
        pending = self.buffer
        self.buffer = ""
        return (pending, "") if self.in_reasoning else ("", pending)


def _tag_prefix_length(text: str, tag: str) -> int:
    return next(
        (size for size in range(min(len(text), len(tag) - 1), 0, -1) if text.endswith(tag[:size])),
        0,
    )


def tokenizer_supports_thinking(tokenizer: Any) -> bool:
    return "enable_thinking" in (getattr(tokenizer, "chat_template", "") or "")


def validate_chat_request(
    request: dict[str, Any],
) -> tuple[list[dict[str, str]], int, float, float]:
    """Return validated sampling inputs or raise ChatRequestError."""
    messages = request.get("messages")
    if not isinstance(messages, list) or not messages:
        raise ChatRequestError("messages must be a non-empty list")
    normalized: list[dict[str, str]] = []
    for message in messages:
        if not isinstance(message, dict):
            raise ChatRequestError("each message must be an object")
        role = message.get("role")
        content = message.get("content")
        if not isinstance(role, str) or not role:
            raise ChatRequestError("message role must be a non-empty string")
        if not isinstance(content, str):
            raise ChatRequestError("message content must be a string")
        normalized_message = {"role": role, "content": content}
        reasoning = message.get("reasoning_content")
        if reasoning is not None:
            if not isinstance(reasoning, str):
                raise ChatRequestError("message reasoning_content must be a string")
            normalized_message["reasoning_content"] = reasoning
        normalized.append(normalized_message)

    max_tokens = request.get("max_tokens", DEFAULT_MAX_TOKENS)
    if max_tokens is None:
        max_tokens = DEFAULT_MAX_TOKENS
    if not isinstance(max_tokens, int) or isinstance(max_tokens, bool) or max_tokens <= 0:
        raise ChatRequestError("max_tokens must be a positive integer")

    temperature = request.get("temperature", DEFAULT_TEMPERATURE)
    if temperature is None:
        temperature = DEFAULT_TEMPERATURE
    if not isinstance(temperature, (int, float)) or isinstance(temperature, bool):
        raise ChatRequestError("temperature must be a number")
    temperature = float(temperature)
    if temperature < 0 or temperature > MAX_TEMPERATURE:
        raise ChatRequestError(f"temperature must be within [0, {MAX_TEMPERATURE}]")

    top_p = request.get("top_p", DEFAULT_TOP_P)
    if top_p is None:
        top_p = DEFAULT_TOP_P
    if not isinstance(top_p, (int, float)) or isinstance(top_p, bool):
        raise ChatRequestError("top_p must be a number")
    top_p = float(top_p)
    if top_p < 0 or top_p > 1:
        raise ChatRequestError("top_p must be within [0, 1]")

    return normalized, max_tokens, temperature, top_p


def build_prompt_tokens(tokenizer: Any, messages: list[dict[str, str]]) -> list[int]:
    """Apply the model's chat template; fall back to a plain join for base models."""
    apply_template = getattr(tokenizer, "apply_chat_template", None)
    if apply_template is not None:
        try:
            kwargs = {"enable_thinking": True} if tokenizer_supports_thinking(tokenizer) else {}
            tokens = apply_template(messages, add_generation_prompt=True, **kwargs)
            if tokens:
                return list(tokens)
        except Exception as error:  # template missing/broken: degrade instead of dying
            log(f"[mlx-lm-runner] chat template failed ({error}); using plain prompt")
    text = "".join(f"{m['role']}: {m['content']}\n" for m in messages)
    encoded = getattr(tokenizer, "encode", None)
    if encoded is not None:
        return list(encoded(text))
    raise ChatRequestError("tokenizer supports neither chat template nor encode")


def _common_prefix_len(a: list[int], b: list[int]) -> int:
    limit = min(len(a), len(b))
    index = 0
    while index < limit and a[index] == b[index]:
        index += 1
    return index


# mlx-lm 依赖只在真正推理时可用（单测不装 mlx），统一经这些 seam 惰性导入，
# 测试通过 monkeypatch 模块属性注入假实现。
def _make_prompt_cache(model: Any) -> Any:
    from mlx_lm.models.cache import make_prompt_cache

    return make_prompt_cache(model)


def _trim_prompt_cache(cache: Any, num_tokens: int) -> int:
    from mlx_lm.models.cache import trim_prompt_cache

    return trim_prompt_cache(cache, num_tokens)


def _make_sampler(temperature: float, top_p: float) -> Any:
    from mlx_lm.sample_utils import make_sampler

    return make_sampler(temp=temperature, top_p=top_p)


def _stream_generate(model: Any, tokenizer: Any, prompt: list[int], **kwargs: Any) -> Any:
    from mlx_lm import stream_generate

    return stream_generate(model, tokenizer, prompt, **kwargs)


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


class MlxLmEngine:
    """单实例 chat 引擎：load 一次，常驻服务所有 infer（流式迭代）。

    持有一个随对话延续的 prompt cache（KV 前缀复用）：每轮只对与上一轮
    分歧的 token 增量 prefill。单 cache 的正确性由 runner.toml 的
    max_concurrency_per_instance = 1 串行保证。
    """

    def __init__(self, model_root: str) -> None:
        self.model_root = model_root
        self._loaded: LoadedModel | None = None
        self._prompt_cache: Any = None
        self._cached_tokens: list[int] = []
        self._load()

    def _load(self) -> None:
        with contextlib.redirect_stdout(sys.stderr):
            from mlx_lm import load

            log(f"[mlx-lm-runner] loading model from {self.model_root}")
            model, tokenizer = load(self.model_root)
        self._loaded = LoadedModel(model=model, tokenizer=tokenizer)
        self._prompt_cache = _make_prompt_cache(model)
        self._cached_tokens = []

    # -- prompt cache -----------------------------------------------------

    def _fresh_cache(self) -> Any:
        cache = _make_prompt_cache(self._loaded.model)
        self._prompt_cache = cache
        self._cached_tokens = []
        return cache

    def _reset_cache(self) -> None:
        # 生成中途失败时 KV 状态不可信：丢弃登记，下一轮全量 prefill。
        self._prompt_cache = None
        self._cached_tokens = []

    def _prepare_cache(self, prompt: list[int]) -> tuple[Any, list[int]]:
        """返回 (prompt_cache, 需增量 prefill 的 prompt 尾部)。

        复用上限取 min(公共前缀, len(prompt)-1)：至少留 1 个 token 交给
        generate_step（空 prompt 会 ValueError），这也是 mlx_lm.server
        fetch_nearest_cache 的同款规则。公共前缀为 0、裁剪异常或实际裁剪数
        与请求不符时重建 cache 全量 prefill——cache 路径失败一律退化为既有
        行为。
        """
        cache = self._prompt_cache
        cached = self._cached_tokens
        if cache is None:
            return self._fresh_cache(), prompt
        if not cached:
            return cache, prompt  # _load 刚建的空 cache，直接服务首轮
        keep = min(_common_prefix_len(cached, prompt), len(prompt) - 1)
        if keep == 0:
            return self._fresh_cache(), prompt
        drop = len(cached) - keep
        try:
            trimmed = _trim_prompt_cache(cache, drop)
        except Exception as error:
            log(f"[mlx-lm-runner] prompt cache trim failed ({error}); rebuilding")
            return self._fresh_cache(), prompt
        if trimmed != drop:
            log("[mlx-lm-runner] prompt cache trim incomplete; rebuilding")
            return self._fresh_cache(), prompt
        self._cached_tokens = cached[:keep]
        return cache, prompt[keep:]

    def _recording(self, responses: Any, prompt: list[int], cache: Any) -> Any:
        """透传生成事件；正常耗尽后把 cache 登记为 prompt + 全部生成 token。

        generate_step 在每个 yield 点之前已把当前 token 回喂进 cache（含
        最后终止帧的 token），因此该序列与真实 KV 状态一致，与 mlx_lm.server
        的 cache_key 登记规则相同。迭代异常时不信任 cache 状态。
        """
        generated: list[int] = []
        complete = True
        try:
            for response in responses:
                token = getattr(response, "token", None)
                if isinstance(token, int):
                    generated.append(token)
                else:
                    complete = False
                yield response
        except BaseException:
            self._reset_cache()
            raise
        if complete:
            self._prompt_cache = cache
            self._cached_tokens = prompt + generated
        else:
            self._reset_cache()

    # -- 推理 --------------------------------------------------------------

    def stream_chat(
        self,
        request: dict[str, Any],
    ) -> tuple[Iterator[Any], list[int], int, float, float, bool]:
        """Validate the request and return the raw generation iterator.

        返回 (responses, prompt_tokens, max_tokens, temperature)；调用方逐
        GenerationResponse 取 text/generation_tokens/finish_reason。生成循环
        可能向 stdout 打日志，next() 必须在调用方的 redirect 上下文内执行。
        """
        loaded = self._loaded
        if loaded is None:
            raise RuntimeError("model is not loaded")
        messages, max_tokens, temperature, top_p = validate_chat_request(request)
        prompt = build_prompt_tokens(loaded.tokenizer, messages)
        with contextlib.redirect_stdout(sys.stderr):
            # mlx-lm 0.31+ 的采样参数是 sampler 对象；temp=0 时 make_sampler
            # 返回 None，对应库内的贪心解码路径。
            cache, prefill = self._prepare_cache(prompt)
            responses = _stream_generate(
                loaded.model,
                loaded.tokenizer,
                prefill,
                max_tokens=max_tokens,
                sampler=_make_sampler(temperature, top_p),
                prompt_cache=cache,
            )
        return (
            self._recording(responses, prompt, cache),
            prompt,
            max_tokens,
            temperature,
            top_p,
            tokenizer_supports_thinking(loaded.tokenizer),
        )


def next_response(responses: Iterator[Any]) -> Any | None:
    """Step the generation iterator with stdout redirected; None when exhausted.

    mlx 的生成循环可能向 stdout 打日志，因此重定向必须覆盖每次 next() 调用；
    协议帧由调用方在重定向之外写出。绝不能把 yield 放进 redirect 上下文里，
    否则生成器挂起期间 serve 的 delta 帧会被吞进 stderr。
    """
    with contextlib.redirect_stdout(sys.stderr):
        try:
            return next(responses)
        except StopIteration:
            return None
