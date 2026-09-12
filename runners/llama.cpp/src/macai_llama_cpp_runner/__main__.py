"""Runner Protocol v1 入口：stdin/stdout length-prefixed JSON frame。

用法：
    python -m macai_llama_cpp_runner            # 常驻协议服务
    python -m macai_llama_cpp_runner --probe    # 只读环境探针（manifest probe 用）
"""

from __future__ import annotations

import os
import sys

if __package__ in (None, ""):  # pragma: no cover - direct script execution
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    from macai_llama_cpp_runner import protocol
else:
    from macai_llama_cpp_runner import protocol

from macai_llama_cpp_runner.engine import (
    LlamaCppEngine,
    LlamaServerError,
)

PROTOCOL_VERSION = "macai.runner.v1"
RUNNER_ID = "org.macai.llama.cpp"
RUNNER_VERSION = "0.1.0"
CAPABILITIES = ["chat.v1"]


def _send(frame: dict) -> None:
    protocol.write_frame(sys.stdout.buffer, frame)


def _recv() -> dict | None:
    return protocol.read_frame(sys.stdin.buffer)


def run_probe() -> int:
    """manifest probe：验证受管解释器与适配器可导入，不启动引擎。"""
    print("llama.cpp runner probe ok")
    return 0


def main() -> int:
    if "--probe" in sys.argv:
        return run_probe()

    engine: LlamaCppEngine | None = None
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
                engine = LlamaCppEngine(model_root)
                engine.start()
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
            except LlamaServerError as error:
                engine = None
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
            if engine is None or not engine.running:
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
                stream = engine.stream_chat(request)
                emitted = ""
                reasoning_emitted = ""
                usage: dict | None = None
                finish_reason = "stop"
                for event in stream:
                    if "text" in event:
                        delta = event["text"]
                        emitted += delta
                        _send(
                            {
                                "protocol": PROTOCOL_VERSION,
                                "type": "delta",
                                "id": frame_id,
                                "payload": {"text": delta},
                            }
                        )
                    if "reasoning_text" in event:
                        delta = event["reasoning_text"]
                        reasoning_emitted += delta
                        _send(
                            {
                                "protocol": PROTOCOL_VERSION,
                                "type": "delta",
                                "id": frame_id,
                                "payload": {"reasoning_text": delta},
                            }
                        )
                    if "finish_reason" in event:
                        finish_reason = event["finish_reason"]
                        usage = event.get("usage")
                if not emitted and not reasoning_emitted:
                    raise RuntimeError("model produced no output tokens")
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "result",
                        "id": frame_id,
                        "payload": {
                            "text": emitted,
                            "reasoning_text": reasoning_emitted,
                            "finish_reason": finish_reason,
                            "usage": usage
                            or {
                                "prompt_tokens": 0,
                                "completion_tokens": 0,
                                "total_tokens": 0,
                            },
                            "effective_device": "metal",
                        },
                    }
                )
            except LlamaServerError as error:
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "error",
                        "id": frame_id,
                        "payload": {"code": "inference_error", "message": str(error), "retryable": True},
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
            if engine is not None:
                engine.stop()
            engine = None
            _send({"protocol": PROTOCOL_VERSION, "type": "unloaded", "id": frame_id, "payload": {}})
        elif frame_type == "shutdown":
            if engine is not None:
                engine.stop()
            engine = None
            _send({"protocol": PROTOCOL_VERSION, "type": "shutdown_complete", "id": frame_id, "payload": {}})
            break
        elif frame_type == "health":
            _send(
                {
                    "protocol": PROTOCOL_VERSION,
                    "type": "healthy",
                    "id": frame_id,
                    "payload": {"model_loaded": engine is not None and engine.running},
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
