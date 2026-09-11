"""Runner Protocol v1 入口：stdin/stdout length-prefixed JSON frame。

用法：
    python -m macai_mlx_lm_runner            # 常驻协议服务
    python -m macai_mlx_lm_runner --probe    # 只读环境探针（manifest probe 用）
"""

from __future__ import annotations

import os
import sys

if __package__ in (None, ""):  # pragma: no cover - direct script execution
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    from macai_mlx_lm_runner import protocol
else:
    from macai_mlx_lm_runner import protocol

from macai_mlx_lm_runner.engine import (
    ChatRequestError,
    MlxLmEngine,
    next_response,
)

PROTOCOL_VERSION = "macai.runner.v1"
RUNNER_ID = "org.macai.mlx-lm"
RUNNER_VERSION = "0.1.0"
CAPABILITIES = ["chat.v1"]


def _send(frame: dict) -> None:
    protocol.write_frame(sys.stdout.buffer, frame)


def _recv() -> dict | None:
    return protocol.read_frame(sys.stdin.buffer)


def run_probe() -> int:
    """manifest probe：验证受管解释器与运行依赖，不加载模型。"""
    import mlx.core  # noqa: F401
    import mlx_lm  # noqa: F401

    print("mlx-lm runner probe ok")
    return 0


def main() -> int:
    if "--probe" in sys.argv:
        return run_probe()

    engine: MlxLmEngine | None = None
    _send(
        {
            "protocol": PROTOCOL_VERSION,
            "type": "hello",
            "id": "startup",
            "payload": {
                "runner_id": RUNNER_ID,
                "runner_version": RUNNER_VERSION,
                "protocol_versions": [PROTOCOL_VERSION],
                "capabilities": CAPABILITIES,
                "pid": os.getpid(),
            },
        }
    )

    while True:
        frame = _recv()
        if frame is None:
            break
        frame_type = frame.get("type", "")
        frame_id = frame.get("id", "")
        payload = frame.get("payload") or {}

        if frame_type == "initialize":
            _send({"protocol": PROTOCOL_VERSION, "type": "initialized", "id": frame_id, "payload": {}})
        elif frame_type == "load":
            try:
                model_root = payload["model_root"]
                engine = MlxLmEngine(model_root)
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "loaded",
                        "id": frame_id,
                        "payload": {
                            "model_id": payload.get("model_id", ""),
                            "effective_device": "metal",
                            "capabilities": CAPABILITIES,
                            "limits": {"max_concurrency": 1},
                        },
                    }
                )
            except Exception as error:  # noqa: BLE001
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "error",
                        "id": frame_id,
                        "payload": {
                            "code": "model_load_failed",
                            "message": str(error),
                            "retryable": True,
                        },
                    }
                )
        elif frame_type == "infer":
            if engine is None:
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "error",
                        "id": frame_id,
                        "payload": {"code": "provider_unavailable", "message": "model not loaded", "retryable": False},
                    }
                )
                continue
            request = payload.get("request") or {}
            _send({"protocol": PROTOCOL_VERSION, "type": "accepted", "id": frame_id, "payload": {}})
            try:
                responses, prompt, _max_tokens, _temperature, _top_p = engine.stream_chat(request)
                emitted = ""
                generation_tokens: int | None = None
                finish_reason: str | None = None
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
                    _send(
                        {
                            "protocol": PROTOCOL_VERSION,
                            "type": "delta",
                            "id": frame_id,
                            "payload": {"text": delta},
                        }
                    )
                if not emitted:
                    raise RuntimeError("model produced no output tokens")
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "result",
                        "id": frame_id,
                        "payload": {
                            "text": emitted,
                            "finish_reason": finish_reason or "stop",
                            "usage": {
                                "prompt_tokens": len(prompt),
                                "completion_tokens": generation_tokens or 0,
                                "total_tokens": len(prompt) + (generation_tokens or 0),
                            },
                            "effective_device": "metal",
                        },
                    }
                )
            except ChatRequestError as error:
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "error",
                        "id": frame_id,
                        "payload": {"code": "invalid_request", "message": str(error), "retryable": False},
                    }
                )
            except Exception as error:  # noqa: BLE001 — 单次失败不杀 worker
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "error",
                        "id": frame_id,
                        "payload": {"code": "inference_error", "message": str(error), "retryable": True},
                    }
                )
        elif frame_type == "unload":
            engine = None
            _send({"protocol": PROTOCOL_VERSION, "type": "unloaded", "id": frame_id, "payload": {}})
        elif frame_type == "shutdown":
            _send({"protocol": PROTOCOL_VERSION, "type": "shutdown_complete", "id": frame_id, "payload": {}})
            break
        elif frame_type == "health":
            _send(
                {
                    "protocol": PROTOCOL_VERSION,
                    "type": "healthy",
                    "id": frame_id,
                    "payload": {"model_loaded": engine is not None},
                }
            )
        else:
            _send(
                {
                    "protocol": PROTOCOL_VERSION,
                    "type": "error",
                    "id": frame_id,
                    "payload": {"code": "invalid_request", "message": f"unknown frame type {frame_type}", "retryable": False},
                }
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
