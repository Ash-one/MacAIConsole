#!/usr/bin/env python3
"""Persistent MLX Qwen3-ASR worker for aiworkd.

Protocol (one JSON object per line):
  readiness: {"ready": true, "device": "metal", "precision": "8bit"}
  request:   {"id": 1, "audio": "/tmp/input.wav", "language": "Chinese"}
  success:   {"id": 1, "ok": true, "text": "...", "language": "Chinese", "device": "metal"}
  failure:   {"id": 1, "ok": false, "error": {"code": "invalid_audio", "message": "..."}}

stdout is reserved for protocol frames. Library diagnostics are redirected to stderr.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import sys
import wave
from dataclasses import dataclass
from typing import Any, Optional


class AudioInputError(ValueError):
    """The supplied file is not a usable PCM WAV input."""


@dataclass(frozen=True)
class LoadedModel:
    model: Any
    device: str = "metal"
    precision: str = "8bit"


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def validate_pcm_wav(path: str) -> None:
    if not path or not os.path.isfile(path) or os.path.getsize(path) == 0:
        raise AudioInputError("audio file is missing or empty")
    try:
        with wave.open(path, "rb") as wav:
            if wav.getcomptype() != "NONE":
                raise AudioInputError("compressed WAV input is not supported")
            if wav.getnchannels() <= 0 or wav.getframerate() <= 0 or wav.getnframes() <= 0:
                raise AudioInputError("audio contains no samples")
    except AudioInputError:
        raise
    except (EOFError, OSError, wave.Error) as error:
        raise AudioInputError("audio cannot be decoded as PCM WAV") from error


def load_model(model_dir: str) -> LoadedModel:
    """Load one quantized Qwen3-ASR model into MLX/Metal."""
    with contextlib.redirect_stdout(sys.stderr):
        import mlx.core as mx
        from mlx_audio.stt.utils import load_model as mlx_load_model

        log("[qwen3-asr-mlx] loading 8-bit model on Metal")
        model = mlx_load_model(model_dir, lazy=False, strict=False)
        mx.eval(model.parameters())
    return LoadedModel(model=model)


def transcribe(loaded: LoadedModel, audio: str, language: Optional[str]) -> tuple[str, str]:
    """Run deterministic MLX inference and normalize its STTOutput."""
    with contextlib.redirect_stdout(sys.stderr):
        result = loaded.model.generate(
            audio,
            max_tokens=8192,
            batch_size=1,
            temperature=0.0,
            language=language,
            verbose=False,
        )
    text = str(getattr(result, "text", "") or "").strip()
    language_value = getattr(result, "language", "") or language or ""
    if isinstance(language_value, (list, tuple)):
        language_value = language_value[0] if language_value else ""
    detected_language = str(language_value).strip()
    if not text:
        raise RuntimeError("model returned an empty transcription")
    return text, detected_language


def error_frame(request_id: Any, code: str, message: str) -> dict[str, Any]:
    return {"id": request_id, "ok": False, "error": {"code": code, "message": message}}


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
                raise ValueError("request id must be an integer")
            audio = request.get("audio")
            if not isinstance(audio, str) or not audio:
                raise AudioInputError("audio path is required")
            language = request.get("language")
            if language is not None and not isinstance(language, str):
                raise ValueError("language must be a string")
            validate_pcm_wav(audio)
            text, detected_language = transcribe(loaded, audio, language)
            print(
                json.dumps(
                    {
                        "id": request_id,
                        "ok": True,
                        "text": text,
                        "language": detected_language,
                        "device": loaded.device,
                    },
                    ensure_ascii=False,
                ),
                flush=True,
            )
        except AudioInputError as error:
            print(json.dumps(error_frame(request_id, "invalid_audio", str(error))), flush=True)
        except ValueError as error:
            print(json.dumps(error_frame(request_id, "invalid_request", str(error))), flush=True)
        except Exception as error:  # one inference failure must not terminate the worker
            log(f"[qwen3-asr-mlx] inference failed: {type(error).__name__}: {error}")
            print(
                json.dumps(error_frame(request_id, "inference_error", f"Qwen3-ASR MLX inference failed: {error}")),
                flush=True,
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--device", choices=("auto", "metal"), default="auto")
    args = parser.parse_args()
    try:
        loaded = load_model(args.model)
    except Exception as error:
        log(f"[qwen3-asr-mlx] startup failed: {type(error).__name__}: {error}")
        print(
            json.dumps(
                {
                    "ready": False,
                    "error": {
                        "code": "model_load_failed",
                        "message": f"Qwen3-ASR MLX model load failed: {error}",
                    },
                }
            ),
            flush=True,
        )
        return 1

    print(
        json.dumps(
            {"ready": True, "device": loaded.device, "precision": loaded.precision}
        ),
        flush=True,
    )
    serve(loaded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
