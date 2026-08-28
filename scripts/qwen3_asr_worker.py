#!/usr/bin/env python3
"""Persistent Qwen3-ASR worker for aiworkd.

Protocol (one JSON object per line):
  readiness: {"ready": true, "device": "mps"}
  request:   {"id": 1, "audio": "/tmp/input.wav", "language": "Chinese"}
  success:   {"id": 1, "ok": true, "text": "...", "language": "Chinese", "device": "mps"}
  failure:   {"id": 1, "ok": false, "error": {"code": "invalid_audio", "message": "..."}}

stdout is reserved for protocol frames. Library diagnostics are redirected to stderr.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import math
import os
import sys
import wave
from array import array
from dataclasses import dataclass
from typing import Any, Iterable, Optional

TARGET_SAMPLE_RATE = 16_000
MIN_AUDIO_SAMPLES = TARGET_SAMPLE_RATE // 2


class AudioInputError(ValueError):
    """The supplied file is not a usable PCM WAV input."""


@dataclass(frozen=True)
class LoadedModel:
    model: Any
    device: str


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _decode_pcm(data: bytes, sample_width: int) -> array:
    """Decode little-endian PCM bytes into normalized float samples."""
    samples = array("f")
    if sample_width == 1:
        samples.extend((value - 128) / 128.0 for value in data)
    elif sample_width == 2:
        values = array("h")
        values.frombytes(data)
        if sys.byteorder != "little":
            values.byteswap()
        samples.extend(value / 32768.0 for value in values)
    elif sample_width == 3:
        for offset in range(0, len(data), 3):
            raw = int.from_bytes(data[offset : offset + 3], "little", signed=False)
            if raw & 0x800000:
                raw -= 1 << 24
            samples.append(raw / 8388608.0)
    elif sample_width == 4:
        values = array("i")
        values.frombytes(data)
        if sys.byteorder != "little":
            values.byteswap()
        samples.extend(value / 2147483648.0 for value in values)
    else:
        raise AudioInputError("PCM WAV sample width must be 8, 16, 24, or 32 bits")
    return samples


def _downmix(samples: array, channels: int) -> array:
    if channels == 1:
        return samples
    if channels <= 0 or len(samples) % channels:
        raise AudioInputError("PCM WAV channel layout is invalid")
    mono = array("f")
    for offset in range(0, len(samples), channels):
        mono.append(sum(samples[offset : offset + channels]) / channels)
    return mono


def _resample_linear(samples: array, source_rate: int, target_rate: int) -> array:
    if source_rate == target_rate:
        return samples
    if source_rate <= 0 or target_rate <= 0:
        raise AudioInputError("PCM WAV sample rate is invalid")
    output_length = max(1, round(len(samples) * target_rate / source_rate))
    if len(samples) == 1:
        return array("f", [samples[0]] * output_length)
    scale = source_rate / target_rate
    output = array("f")
    for index in range(output_length):
        position = min(index * scale, len(samples) - 1)
        left = int(position)
        right = min(left + 1, len(samples) - 1)
        fraction = position - left
        output.append(samples[left] + (samples[right] - samples[left]) * fraction)
    return output


def preprocess_pcm_wav(path: str, target_rate: int = TARGET_SAMPLE_RATE) -> tuple[array, int]:
    """Decode PCM WAV, downmix to mono, resample, clamp, and pad to 0.5 s."""
    if not path or not os.path.isfile(path) or os.path.getsize(path) == 0:
        raise AudioInputError("audio file is missing or empty")
    try:
        with wave.open(path, "rb") as wav:
            if wav.getcomptype() != "NONE":
                raise AudioInputError("compressed WAV input is not supported")
            channels = wav.getnchannels()
            sample_width = wav.getsampwidth()
            source_rate = wav.getframerate()
            frame_count = wav.getnframes()
            if frame_count <= 0:
                raise AudioInputError("audio contains no samples")
            pcm = wav.readframes(frame_count)
    except AudioInputError:
        raise
    except (EOFError, OSError, wave.Error) as error:
        raise AudioInputError("audio cannot be decoded as PCM WAV") from error

    samples = _downmix(_decode_pcm(pcm, sample_width), channels)
    if not samples or any(not math.isfinite(value) for value in samples):
        raise AudioInputError("audio contains no finite samples")
    samples = array("f", (max(-1.0, min(1.0, value)) for value in samples))
    samples = _resample_linear(samples, source_rate, target_rate)
    if len(samples) < target_rate // 2:
        samples.extend([0.0] * (target_rate // 2 - len(samples)))
    return samples, target_rate


def _model_candidates(torch_module: Any, requested_device: str) -> Iterable[tuple[str, Any]]:
    if requested_device == "auto":
        if torch_module.backends.mps.is_available():
            yield "mps", torch_module.float16
        yield "cpu", torch_module.float32
        return
    if requested_device == "mps":
        if not torch_module.backends.mps.is_available():
            raise RuntimeError("MPS is not available on this host")
        yield "mps", torch_module.float16
        return
    if requested_device == "cpu":
        yield "cpu", torch_module.float32
        return
    raise ValueError("device must be auto, mps, or cpu")


def load_model(model_dir: str, requested_device: str) -> LoadedModel:
    """Load once, preferring MPS and explicitly falling back to CPU in auto mode."""
    with contextlib.redirect_stdout(sys.stderr):
        import torch
        from qwen_asr import Qwen3ASRModel

    failures = []
    for device, dtype in _model_candidates(torch, requested_device):
        try:
            log(f"[qwen3-asr] loading model on {device}")
            with contextlib.redirect_stdout(sys.stderr):
                model = Qwen3ASRModel.from_pretrained(
                    model_dir,
                    dtype=dtype,
                    device_map=device,
                    max_inference_batch_size=1,
                    max_new_tokens=256,
                )
            return LoadedModel(model=model, device=device)
        except Exception as error:  # model/device failures are reported at readiness
            failures.append(f"{device}: {type(error).__name__}: {error}")
            log(f"[qwen3-asr] model load failed on {device}: {error}")
            if requested_device != "auto":
                break
    raise RuntimeError("; ".join(failures) or "model could not be loaded")


def transcribe(loaded: LoadedModel, samples: array, sample_rate: int, language: Optional[str]) -> tuple[str, str]:
    """Run one inference and normalize qwen-asr's public result object."""
    with contextlib.redirect_stdout(sys.stderr):
        import numpy as np

        result = loaded.model.transcribe(
            audio=(np.asarray(samples, dtype=np.float32), sample_rate),
            language=language,
            return_time_stamps=False,
        )
    if not result:
        raise RuntimeError("model returned no transcription result")
    item = result[0]
    text = str(getattr(item, "text", "") or "").strip()
    detected_language = str(getattr(item, "language", "") or "").strip()
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
            samples, sample_rate = preprocess_pcm_wav(audio)
            text, detected_language = transcribe(loaded, samples, sample_rate, language)
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
            log(f"[qwen3-asr] inference failed: {type(error).__name__}: {error}")
            print(
                json.dumps(error_frame(request_id, "inference_error", f"Qwen3-ASR inference failed: {error}")),
                flush=True,
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--device", choices=("auto", "mps", "cpu"), default="auto")
    args = parser.parse_args()
    try:
        loaded = load_model(args.model, args.device)
    except Exception as error:
        log(f"[qwen3-asr] startup failed: {type(error).__name__}: {error}")
        print(
            json.dumps(
                {
                    "ready": False,
                    "error": {"code": "model_load_failed", "message": f"Qwen3-ASR model load failed: {error}"},
                }
            ),
            flush=True,
        )
        return 1

    print(json.dumps({"ready": True, "device": loaded.device}), flush=True)
    serve(loaded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
