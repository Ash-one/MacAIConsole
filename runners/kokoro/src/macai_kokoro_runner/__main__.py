"""Runner Protocol v1 入口：stdin/stdout length-prefixed JSON frame。

用法：
    python -m macai_kokoro_runner            # 常驻协议服务
    python -m macai_kokoro_runner --probe    # 只读环境探针（manifest probe 用）
"""

from __future__ import annotations

import json
import os
import sys

if __package__ in (None, ""):  # pragma: no cover - direct script execution
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    from macai_kokoro_runner import protocol
else:
    from macai_kokoro_runner import protocol

PROTOCOL_VERSION = "macai.runner.v1"
RUNNER_ID = "org.macai.kokoro"
RUNNER_VERSION = "0.1.0"
CAPABILITIES = ["tts.v1"]


def _send(frame: dict) -> None:
    protocol.write_frame(sys.stdout.buffer, frame)


def _recv() -> dict | None:
    return protocol.read_frame(sys.stdin.buffer)


def run_probe() -> int:
    """manifest probe：验证受管解释器与运行依赖，不加载模型。"""
    import mlx_audio  # noqa: F401
    import misaki  # noqa: F401
    import numpy  # noqa: F401
    import phonemizer  # noqa: F401
    import espeakng_loader  # noqa: F401

    print("kokoro runner probe ok")
    return 0


def main() -> int:
    if "--probe" in sys.argv:
        return run_probe()

    from macai_kokoro_runner.engine import KokoroEngine

    engine: KokoroEngine | None = None
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
                engine = KokoroEngine(model_root)
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
            output = payload.get("output") or {}
            text = (request.get("text") or "").strip()
            if not text:
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "error",
                        "id": frame_id,
                        "payload": {"code": "invalid_request", "message": "text must not be empty", "retryable": False},
                    }
                )
                continue
            _send({"protocol": PROTOCOL_VERSION, "type": "accepted", "id": frame_id, "payload": {}})
            directory = output.get("directory", "")
            try:
                os.makedirs(directory, exist_ok=True)
                result = engine.synthesize(
                    text=text,
                    voice=request.get("voice") or "zf_001",
                    speed=float(request.get("speed") or 1.0),
                    output_wav=os.path.join(directory, "output.wav"),
                )
                _send({"protocol": PROTOCOL_VERSION, "type": "result", "id": frame_id, "payload": result})
            except Exception as error:  # noqa: BLE001 — 单次失败不杀 worker
                _send(
                    {
                        "protocol": PROTOCOL_VERSION,
                        "type": "error",
                        "id": frame_id,
                        "payload": {"code": "internal", "message": str(error), "retryable": True},
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
