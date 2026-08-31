#!/usr/bin/env python3
"""Persistent Qwen3-TTS CustomVoice worker for aiworkd.

stdin/stdout use one JSON object per line. stdout is reserved for protocol
frames; mlx-audio diagnostics are redirected to stderr.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import sys
import tempfile
from typing import Any, Optional

import numpy as np

DEFAULT_VOICE = "Vivian"


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def error_frame(request_id: Any, code: str, message: str) -> dict[str, Any]:
    return {"id": request_id, "ok": False, "error": {"code": code, "message": message}}


def split_voice(voice: Optional[str]) -> tuple[str, Optional[str]]:
    """Return the speaker and optional instruction encoded in a voice value.

    SpeechRequest intentionally has no provider-specific instruct field.  The
    first comma therefore provides a small, backwards-compatible escape hatch:
    ``Vivian, very happy`` becomes speaker ``Vivian`` and that instruction.
    """
    value = (voice or DEFAULT_VOICE).strip()
    if not value:
        value = DEFAULT_VOICE
    speaker, separator, instruction = value.partition(",")
    speaker = speaker.strip() or DEFAULT_VOICE
    instruction = instruction.strip() if separator else None
    return speaker, instruction or None


def load_model(model_dir: str) -> Any:
    """Load a Qwen3-TTS model using mlx-audio's model auto-dispatch."""
    with contextlib.redirect_stdout(sys.stderr):
        from mlx_audio.tts.generate import load_model as mlx_load_model

        log(f"[qwen3-tts] loading model from {model_dir}")
        model = mlx_load_model(model_dir)
    return model


def generate_audio(
    model: Any,
    text: str,
    voice: Optional[str],
    speed: float = 1.0,
    language: str = "Auto",
    instruct: Optional[str] = None,
) -> tuple[Any, int]:
    """Generate one complete non-streaming CustomVoice result.

    Current mlx-audio exposes speed on the generic ``generate`` API, while
    ``generate_custom_voice`` has no speed argument and documents speed as not
    directly supported.  Keep accepting and validating speed at the daemon
    boundary; use the native CustomVoice call here without silently resampling.
    """
    del speed
    speaker, encoded_instruct = split_voice(voice)
    instruction = instruct.strip() if isinstance(instruct, str) and instruct.strip() else encoded_instruct
    with contextlib.redirect_stdout(sys.stderr):
        results = list(
            model.generate_custom_voice(
                text=text,
                speaker=speaker,
                language=language or "Auto",
                instruct=instruction,
                verbose=False,
            )
        )
    parts = [result.audio for result in results if getattr(result, "audio", None) is not None]
    if not parts:
        raise RuntimeError("model produced no audio")
    audio = parts[0] if len(parts) == 1 else np.concatenate(parts)
    sample_rate = int(getattr(results[0], "sample_rate", 24000))
    if sample_rate <= 0:
        raise RuntimeError("model returned an invalid sample rate")
    return audio, sample_rate


def serve(model: Any) -> None:
    counter = 0
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            print(json.dumps(error_frame(None, "invalid_request", "request must be valid JSON")), flush=True)
            continue

        request_id = request.get("id") if isinstance(request, dict) else None
        try:
            if not isinstance(request, dict):
                raise ValueError("request must be a JSON object")
            if not isinstance(request_id, int):
                raise ValueError("request id must be an integer")
            text = request.get("text")
            if not isinstance(text, str) or not text.strip():
                raise ValueError("text must not be empty")
            voice = request.get("voice")
            if voice is not None and not isinstance(voice, str):
                raise ValueError("voice must be a string")
            speed = request.get("speed", 1.0)
            if not isinstance(speed, (int, float)):
                raise ValueError("speed must be a number")
            if not 0.25 <= float(speed) <= 4.0:
                raise ValueError("speed must be between 0.25 and 4.0")
            if len(text) > 5_000:
                raise ValueError("text exceeds the 5000 character limit")
            language = request.get("language", "auto")
            if not isinstance(language, str):
                raise ValueError("language must be a string")
            instruct = request.get("instruct")
            if instruct is not None and not isinstance(instruct, str):
                raise ValueError("instruct must be a string")

            audio, sample_rate = generate_audio(
                model,
                text.strip(),
                voice,
                float(speed),
                language.strip() or "Auto",
                instruct,
            )
            counter += 1
            fd, wav_path = tempfile.mkstemp(prefix=f"qwen3-tts-{counter}-", suffix=".wav")
            os.close(fd)
            with contextlib.redirect_stdout(sys.stderr):
                from mlx_audio.audio_io import write as audio_write

                audio_write(wav_path, audio, sample_rate, format="wav")
            print(json.dumps({"id": request_id, "ok": True, "wav": wav_path}), flush=True)
        except ValueError as error:
            print(json.dumps(error_frame(request_id, "invalid_request", str(error))), flush=True)
        except Exception as error:  # one failed request must not kill the worker
            log(f"[qwen3-tts] synthesis failed: {type(error).__name__}: {error}")
            print(
                json.dumps(error_frame(request_id, "inference_error", f"Qwen3-TTS inference failed: {error}")),
                flush=True,
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    args = parser.parse_args()
    try:
        model = load_model(args.model)
    except Exception as error:
        log(f"[qwen3-tts] startup failed: {type(error).__name__}: {error}")
        print(
            json.dumps(
                {
                    "ready": False,
                    "error": {
                        "code": "model_load_failed",
                        "message": f"Qwen3-TTS model load failed: {error}",
                    },
                }
            ),
            flush=True,
        )
        return 1

    print(json.dumps({"ready": True, "device": "gpu"}), flush=True)
    serve(model)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
