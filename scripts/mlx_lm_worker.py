#!/usr/bin/env python3
"""Persistent MLX-LM chat worker for aiworkd.

Protocol (one JSON object per line):
  readiness: {"ready": true, "device": "metal", "model": "..."}
  request:   {"id": 1, "messages": [{"role": "user", "content": "hi"}],
              "max_tokens": 1024, "temperature": 0.7}
  delta:     {"id": 1, "delta": "Hel"}
  success:   {"id": 1, "ok": true, "text": "Hello", "prompt_tokens": 9,
              "tokens": 12, "device": "metal"}
  failure:   {"id": 1, "ok": false, "error": {"code": "inference_error", "message": "..."}}

Deltas stream between request and final frame; the Rust provider maps each
delta to one OpenAI chat chunk. stdout is reserved for protocol frames;
library diagnostics are redirected to stderr. The worker exits on stdin EOF.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import sys
from dataclasses import dataclass
from typing import Any, Optional

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


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def error_frame(request_id: Any, code: str, message: str) -> dict[str, Any]:
    return {"id": request_id, "ok": False, "error": {"code": code, "message": message}}


def delta_frame(request_id: Any, text: str) -> dict[str, Any]:
    return {"id": request_id, "delta": text}


def final_frame(
    request_id: Any,
    text: str,
    prompt_tokens: int,
    tokens: int,
    device: str,
    finish_reason: str,
) -> dict[str, Any]:
    return {
        "id": request_id,
        "ok": True,
        "text": text,
        "prompt_tokens": prompt_tokens,
        "tokens": tokens,
        "device": device,
        "finish_reason": finish_reason,
    }


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
            log(f"[mlx-lm] chat template failed ({error}); using plain prompt")
    text = "".join(f"{m['role']}: {m['content']}\n" for m in messages)
    encoded = getattr(tokenizer, "encode", None)
    if encoded is not None:
        return list(encoded(text))
    raise ChatRequestError("tokenizer supports neither chat template nor encode")


def load_model(model_dir: str) -> LoadedModel:
    """Load one MLX-format model directory onto Metal."""
    with contextlib.redirect_stdout(sys.stderr):
        from mlx_lm import load

        log(f"[mlx-lm] loading model from {model_dir}")
        model, tokenizer = load(model_dir)
    return LoadedModel(model=model, tokenizer=tokenizer)


def stream_chat(
    loaded: LoadedModel,
    prompt: list[int],
    max_tokens: int,
    temperature: float,
):
    """Return the raw mlx-lm generation iterator (one GenerationResponse per step)."""
    with contextlib.redirect_stdout(sys.stderr):
        from mlx_lm import stream_generate
        from mlx_lm.sample_utils import make_sampler

        # mlx-lm 0.31+ 的采样参数是 sampler 对象；temp=0 时 make_sampler 返回
        # None，对应库内的贪心解码路径。
        return stream_generate(
            loaded.model,
            loaded.tokenizer,
            prompt=prompt,
            max_tokens=max_tokens,
            sampler=make_sampler(temp=temperature),
        )


def next_response(responses) -> Optional[Any]:
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


def serve(loaded: LoadedModel) -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            print(json.dumps(error_frame(None, "invalid_request", "request must be valid JSON")), flush=True)
            continue

        request_id = request.get("id")
        try:
            if not isinstance(request_id, int):
                raise ChatRequestError("request id must be an integer")
            messages, max_tokens, temperature = validate_chat_request(request)
            prompt = build_prompt_tokens(loaded.tokenizer, messages)
            responses = stream_chat(loaded, prompt, max_tokens, temperature)
            emitted = ""
            generation_tokens: Optional[int] = None
            finish_reason: Optional[str] = None
            while True:
                response = next_response(responses)
                if response is None:
                    break
                text = getattr(response, "text", None)
                if not text:
                    continue
                delta = str(text)
                emitted += delta
                count = getattr(response, "generation_tokens", None)
                if isinstance(count, int):
                    generation_tokens = count
                reason = getattr(response, "finish_reason", None)
                if isinstance(reason, str):
                    finish_reason = reason
                print(json.dumps(delta_frame(request_id, delta), ensure_ascii=False), flush=True)
            if not emitted:
                raise RuntimeError("model produced no output tokens")
            print(
                json.dumps(
                    final_frame(
                        request_id,
                        emitted,
                        len(prompt),
                        generation_tokens or 0,
                        loaded.device,
                        finish_reason or "stop",
                    ),
                    ensure_ascii=False,
                ),
                flush=True,
            )
        except ChatRequestError as error:
            print(json.dumps(error_frame(request_id, "invalid_request", str(error))), flush=True)
        except Exception as error:  # one failed generation must not terminate the worker
            log(f"[mlx-lm] generation failed: {type(error).__name__}: {error}")
            print(
                json.dumps(error_frame(request_id, "inference_error", f"MLX-LM generation failed: {error}")),
                flush=True,
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    args = parser.parse_args()
    try:
        loaded = load_model(args.model)
    except Exception as error:
        log(f"[mlx-lm] startup failed: {type(error).__name__}: {error}")
        print(
            json.dumps(
                {
                    "ready": False,
                    "error": {
                        "code": "model_load_failed",
                        "message": f"MLX-LM model load failed: {error}",
                    },
                }
            ),
            flush=True,
        )
        return 1

    print(
        json.dumps({"ready": True, "device": loaded.device, "model": args.model}),
        flush=True,
    )
    serve(loaded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
