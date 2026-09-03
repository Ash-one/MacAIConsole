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
MAX_TEMPERATURE = 2.0


class ChatRequestError(ValueError):
    """The chat request frame violates the worker protocol."""


@dataclass(frozen=True)
class LoadedModel:
    model: Any
    tokenizer: Any
    device: str = "metal"


def validate_chat_request(request: dict[str, Any]) -> tuple[list[dict[str, str]], int, float]:
    """Return validated (messages, max_tokens, temperature) or raise ChatRequestError."""
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
        normalized.append({"role": role, "content": content})

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

    return normalized, max_tokens, temperature


def build_prompt_tokens(tokenizer: Any, messages: list[dict[str, str]]) -> list[int]:
    """Apply the model's chat template; fall back to a plain join for base models."""
    apply_template = getattr(tokenizer, "apply_chat_template", None)
    if apply_template is not None:
        try:
            tokens = apply_template(messages, add_generation_prompt=True)
            if tokens:
                return list(tokens)
        except Exception as error:  # template missing/broken: degrade instead of dying
            log(f"[mlx-lm-runner] chat template failed ({error}); using plain prompt")
    text = "".join(f"{m['role']}: {m['content']}\n" for m in messages)
    encoded = getattr(tokenizer, "encode", None)
    if encoded is not None:
        return list(encoded(text))
    raise ChatRequestError("tokenizer supports neither chat template nor encode")


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


class MlxLmEngine:
    """单实例 chat 引擎：load 一次，常驻服务所有 infer（流式迭代）。"""

    def __init__(self, model_root: str) -> None:
        self.model_root = model_root
        self._loaded: LoadedModel | None = None
        self._load()

    def _load(self) -> None:
        with contextlib.redirect_stdout(sys.stderr):
            from mlx_lm import load

            log(f"[mlx-lm-runner] loading model from {self.model_root}")
            model, tokenizer = load(self.model_root)
        self._loaded = LoadedModel(model=model, tokenizer=tokenizer)

    def stream_chat(
        self,
        request: dict[str, Any],
    ) -> tuple[Iterator[Any], list[int], int, float]:
        """Validate the request and return the raw generation iterator.

        返回 (responses, prompt_tokens, max_tokens, temperature)；调用方逐
        GenerationResponse 取 text/generation_tokens/finish_reason。生成循环
        可能向 stdout 打日志，next() 必须在调用方的 redirect 上下文内执行。
        """
        loaded = self._loaded
        if loaded is None:
            raise RuntimeError("model is not loaded")
        messages, max_tokens, temperature = validate_chat_request(request)
        prompt = build_prompt_tokens(loaded.tokenizer, messages)
        with contextlib.redirect_stdout(sys.stderr):
            from mlx_lm import stream_generate
            from mlx_lm.sample_utils import make_sampler

            # mlx-lm 0.31+ 的采样参数是 sampler 对象；temp=0 时 make_sampler
            # 返回 None，对应库内的贪心解码路径。
            responses = stream_generate(
                loaded.model,
                loaded.tokenizer,
                prompt=prompt,
                max_tokens=max_tokens,
                sampler=make_sampler(temp=temperature),
            )
        return responses, prompt, max_tokens, temperature


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
