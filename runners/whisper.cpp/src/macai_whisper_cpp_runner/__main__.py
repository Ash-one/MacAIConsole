from __future__ import annotations

import os
import sys

from . import protocol
from .engine import WhisperCppEngine

PROTOCOL = "macai.runner.v1"
RUNNER_ID = "org.macai.whisper.cpp"
CAPABILITIES = ["stt.v1"]


def send(frame_type: str, frame_id: str, payload: dict) -> None:
    protocol.write_frame(sys.stdout.buffer, {"protocol": PROTOCOL, "type": frame_type, "id": frame_id, "payload": payload})


def main() -> int:
    engine: WhisperCppEngine | None = None
    send("hello", "startup", {"runner_id": RUNNER_ID, "runner_version": "0.1.0", "protocol_versions": [PROTOCOL], "capabilities": CAPABILITIES, "pid": os.getpid()})
    while True:
        frame = protocol.read_frame(sys.stdin.buffer)
        if frame is None:
            break
        kind, frame_id, payload = frame.get("type", ""), frame.get("id", ""), frame.get("payload") or {}
        if kind == "initialize":
            send("initialized", frame_id, {})
        elif kind == "load":
            try:
                if engine is not None:
                    engine.stop()
                engine = WhisperCppEngine(payload["model_root"])
                device = engine.start()
                send("loaded", frame_id, {"model_id": payload.get("model_id", ""), "effective_device": device, "capabilities": CAPABILITIES, "limits": {"max_concurrency": 1}})
            except Exception as error:
                if engine is not None:
                    engine.stop()
                    engine = None
                send("error", frame_id, {"code": "model_load_failed", "message": str(error), "retryable": True})
        elif kind == "infer":
            if engine is None or not engine.running:
                send("error", frame_id, {"code": "provider_unavailable", "message": "model not loaded", "retryable": True})
                continue
            request = payload.get("request") or {}
            send("accepted", frame_id, {})
            try:
                text, language = engine.transcribe(str(request.get("audio") or ""), request.get("language"))
                send("result", frame_id, {"text": text, "language": language, "device": engine.effective_device()})
            except ValueError as error:
                send("error", frame_id, {"code": "invalid_audio", "message": str(error), "retryable": False})
            except Exception as error:
                send("error", frame_id, {"code": "inference_error", "message": str(error), "retryable": True})
        elif kind == "health":
            send("healthy", frame_id, {"model_loaded": engine is not None and engine.running})
        elif kind == "unload":
            if engine is not None:
                engine.stop()
                engine = None
            send("unloaded", frame_id, {})
        elif kind == "shutdown":
            if engine is not None:
                engine.stop()
            send("shutdown_complete", frame_id, {})
            break
        else:
            send("error", frame_id, {"code": "invalid_request", "message": f"unknown frame type {kind}", "retryable": False})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
