#!/usr/bin/env python3
"""Persistent sherpa-onnx worker for the zh-int8-2025 streaming model.

stdout is reserved for one JSON object per line:
  readiness: {"ready": true, "device": "cpu"}
  success:   {"id": 1, "ok": true, "text": "...", "language": "Chinese"}
  failure:   {"id": 1, "ok": false, "error": {"code": "invalid_audio", ...}}

The worker owns the sherpa-onnx import and recognizer lifetime.  aiworkd only
supervises this process and exchanges file paths, keeping the Rust daemon
independent of the Python extension ABI.
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
    recognizer: Any
    device: str


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def model_files(model_dir: str) -> tuple[str, str, str, str]:
    root = os.path.abspath(model_dir)
    required = (
        "tokens.txt",
        "encoder.int8.onnx",
        "decoder.onnx",
        "joiner.int8.onnx",
    )
    missing = [name for name in required if not os.path.isfile(os.path.join(root, name))]
    if missing:
        raise FileNotFoundError(
            f"sherpa-onnx model directory is missing: {', '.join(missing)}"
        )
    return tuple(os.path.join(root, name) for name in required)  # type: ignore[return-value]


def load_model(model_dir: str, device: str, num_threads: int) -> LoadedModel:
    """Create one long-lived OnlineRecognizer and load its ONNX sessions."""
    tokens, encoder, decoder, joiner = model_files(model_dir)
    with contextlib.redirect_stdout(sys.stderr):
        import sherpa_onnx

        log(f"[sherpa-onnx] loading zh-int8-2025 on {device}")
        recognizer = sherpa_onnx.OnlineRecognizer.from_transducer(
            tokens=tokens,
            encoder=encoder,
            decoder=decoder,
            joiner=joiner,
            num_threads=num_threads,
            provider=device,
            sample_rate=16_000,
            feature_dim=80,
            decoding_method="greedy_search",
        )
    return LoadedModel(recognizer=recognizer, device=device)


def read_pcm_wav(path: str) -> tuple[Any, int]:
    """Read PCM16 WAV and downmix multi-channel input for the ASR model."""
    try:
        with wave.open(path, "rb") as wav:
            channels = wav.getnchannels()
            sample_width = wav.getsampwidth()
            sample_rate = wav.getframerate()
            frames = wav.getnframes()
            if channels <= 0 or sample_rate <= 0 or frames <= 0:
                raise AudioInputError("audio contains no samples")
            if wav.getcomptype() != "NONE" or sample_width != 2:
                raise AudioInputError("sherpa-onnx requires uncompressed PCM16 WAV input")
            raw = wav.readframes(frames)
    except AudioInputError:
        raise
    except (EOFError, OSError, wave.Error) as error:
        raise AudioInputError("audio cannot be decoded as PCM WAV") from error

    import numpy as np

    samples = np.frombuffer(raw, dtype="<i2")
    expected = frames * channels
    if samples.size != expected:
        raise AudioInputError("audio data is truncated")
    samples = samples.astype("float32") / 32768.0
    if channels > 1:
        samples = samples.reshape((-1, channels)).mean(axis=1)
    return samples, sample_rate


def transcribe(loaded: LoadedModel, audio: str) -> str:
    """Decode a complete file through the streaming recognizer."""
    samples, sample_rate = read_pcm_wav(audio)
    import numpy as np

    stream = loaded.recognizer.create_stream()

    # Feed short chunks just as a microphone would.  This keeps the streaming
    # model's memory behavior independent of the length of a meeting recording.
    chunk_size = max(1, int(0.032 * sample_rate))
    for start in range(0, len(samples), chunk_size):
        stream.accept_waveform(sample_rate, samples[start : start + chunk_size])
        while loaded.recognizer.is_ready(stream):
            loaded.recognizer.decode_stream(stream)

    # Flush the right context before marking EOF; without this, the last words
    # of a file can remain in the streaming decoder's look-ahead buffer.
    stream.accept_waveform(
        sample_rate, np.zeros(max(1, int(0.66 * sample_rate)), dtype="float32")
    )
    stream.input_finished()
    while loaded.recognizer.is_ready(stream):
        loaded.recognizer.decode_stream(stream)

    result = loaded.recognizer.get_result(stream)
    text = str(getattr(result, "text", result) or "").strip()
    if not text:
        raise RuntimeError("sherpa-onnx returned an empty transcription")
    return text


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

        request_id = request.get("id") if isinstance(request, dict) else None
        try:
            if not isinstance(request_id, int):
                raise ValueError("request id must be an integer")
            audio = request.get("audio")
            if not isinstance(audio, str) or not audio:
                raise AudioInputError("audio path is required")
            language = request.get("language")
            if language not in (None, "Chinese"):
                raise ValueError("sherpa-onnx zh-int8-2025 accepts only Chinese")
            text = transcribe(loaded, audio)
            print(
                json.dumps(
                    {
                        "id": request_id,
                        "ok": True,
                        "text": text,
                        "language": "Chinese",
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
        except Exception as error:  # one bad file must not kill the resident worker
            log(f"[sherpa-onnx] inference failed: {type(error).__name__}: {error}")
            print(
                json.dumps(
                    error_frame(
                        request_id,
                        "inference_error",
                        f"sherpa-onnx inference failed: {error}",
                    )
                ),
                flush=True,
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--device", choices=("cpu", "coreml"), default="cpu")
    parser.add_argument("--num-threads", type=int, default=4)
    args = parser.parse_args()
    try:
        loaded = load_model(args.model_dir, args.device, max(1, args.num_threads))
    except Exception as error:
        log(f"[sherpa-onnx] startup failed: {type(error).__name__}: {error}")
        print(
            json.dumps(
                {
                    "ready": False,
                    "error": {
                        "code": "model_load_failed",
                        "message": f"sherpa-onnx model load failed: {error}",
                    },
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
