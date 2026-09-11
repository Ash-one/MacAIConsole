from __future__ import annotations

import importlib.util
import json
import os
import struct
import sys
from pathlib import Path

PROTOCOL = "macai.runner.v1"
MAX_FRAME_BYTES = 8 * 1024 * 1024


def read_frame():
    header = sys.stdin.buffer.read(4)
    if not header:
        return None
    if len(header) != 4:
        raise ValueError("truncated frame header")
    length = struct.unpack(">I", header)[0]
    if length > MAX_FRAME_BYTES:
        raise ValueError("frame too large")
    payload = sys.stdin.buffer.read(length)
    if len(payload) != length:
        raise ValueError("truncated frame payload")
    return json.loads(payload)


def send(kind, request_id, payload=None):
    data = json.dumps(
        {"protocol": PROTOCOL, "type": kind, "id": request_id, "payload": payload or {}},
        ensure_ascii=False,
    ).encode("utf-8")
    sys.stdout.buffer.write(struct.pack(">I", len(data)) + data)
    sys.stdout.buffer.flush()


def load_module(path):
    spec = importlib.util.spec_from_file_location("macai_user_runner", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load Runner script")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def required_hook(module, name):
    hook = getattr(module, name, None)
    if not callable(hook):
        raise RuntimeError(f"Runner script must define {name}()")
    return hook


def error(request_id, code, exc, retryable=False):
    send("error", request_id, {"code": code, "message": str(exc), "retryable": retryable})


def optional_hook(module, name):
    hook = getattr(module, name, None)
    return hook if callable(hook) else None


def infer(module, model, capability, request, output, request_id):
    if capability == "chat.v1":
        hook = required_hook(module, "chat")
        result = hook(model, request.get("messages") or [], request)
        if isinstance(result, str):
            chunks = [result]
        else:
            chunks = result
        text = ""
        for chunk in chunks:
            if not isinstance(chunk, str):
                raise TypeError("chat() must return text or yield text chunks")
            text += chunk
            send("delta", request_id, {"text": chunk})
        return {
            "text": text,
            "finish_reason": "stop",
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
        }
    if capability == "stt.v1":
        text = required_hook(module, "transcribe")(model, request.get("audio"), request)
        if not isinstance(text, str):
            raise TypeError("transcribe() must return text")
        return {"text": text, "language": request.get("language")}
    if capability == "tts.v1":
        directory = Path(output["directory"])
        target = directory / "output.wav"
        returned = required_hook(module, "synthesize")(
            model, request.get("text", ""), str(target), request
        )
        path = Path(returned) if isinstance(returned, str) else target
        return {"path": str(path), "content_type": "audio/wav"}
    raise ValueError(f"unsupported capability {capability}")


def main():
    if len(sys.argv) < 3:
        raise SystemExit("usage: script_host.py runner.py script_config.json [--probe]")
    script_path, config_path = sys.argv[1], sys.argv[2]
    config = json.loads(Path(config_path).read_text(encoding="utf-8"))
    module = load_module(script_path)
    required_hook(module, "load")
    hook = {"chat.v1": "chat", "stt.v1": "transcribe", "tts.v1": "synthesize"}[config["capability"]]
    required_hook(module, hook)
    if "--probe" in sys.argv:
        print("script runner probe ok")
        return 0

    send("hello", "startup", {
        "runner_id": config["id"],
        "runner_version": config["version"],
        "protocol_versions": [PROTOCOL],
        "capabilities": [config["capability"]],
        "pid": os.getpid(),
    })
    loaded = False
    model = None
    while True:
        frame = read_frame()
        if frame is None:
            return 0
        kind = frame.get("type", "")
        request_id = frame.get("id", "")
        payload = frame.get("payload") or {}
        try:
            if kind == "initialize":
                send("initialized", request_id)
            elif kind == "load":
                model = required_hook(module, "load")(payload["model_root"], payload)
                describe = optional_hook(module, "describe")
                details = describe(model) if describe else {}
                if not isinstance(details, dict):
                    raise TypeError("describe() must return a dictionary")
                loaded = True
                send("loaded", request_id, {
                    "model_id": payload.get("model_id", ""),
                    "effective_device": details.get("effective_device", "unknown"),
                    "resident_bytes": details.get("resident_bytes"),
                    "capabilities": [config["capability"]],
                    "limits": {"max_concurrency": 1},
                })
            elif kind == "infer":
                if not loaded:
                    raise RuntimeError("model is not loaded")
                send("accepted", request_id)
                result = infer(
                    module,
                    model,
                    payload.get("capability"),
                    payload.get("request") or {},
                    payload.get("output") or {},
                    request_id,
                )
                send("result", request_id, result)
            elif kind == "unload":
                unload = optional_hook(module, "unload")
                if unload:
                    unload(model)
                model = None
                loaded = False
                send("unloaded", request_id)
            elif kind == "health":
                send("healthy", request_id, {"model_loaded": loaded})
            elif kind == "shutdown":
                if loaded:
                    unload = optional_hook(module, "unload")
                    if unload:
                        unload(model)
                send("shutdown_complete", request_id)
                return 0
            else:
                raise ValueError(f"unknown frame type {kind}")
        except (KeyError, TypeError, ValueError) as exc:
            error(request_id, "invalid_request", exc)
        except Exception as exc:
            error(request_id, "inference_error" if kind == "infer" else "model_load_failed", exc, True)


if __name__ == "__main__":
    raise SystemExit(main())
