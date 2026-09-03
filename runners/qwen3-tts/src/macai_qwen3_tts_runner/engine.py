"""macai_qwen3_tts_runner — Qwen3-TTS CustomVoice 的 Runner Protocol v1 adapter。

stdout 只承载 length-prefixed JSON frame（Runner Protocol v1）；
所有第三方库日志留在 stderr。模型语义（speaker/instruct 拆分、
generate_custom_voice、5000 字限制、speed 校验边界）迁移自
scripts/qwen3_tts_worker.py，cutover 后旧 worker 删除。
"""

from __future__ import annotations

import contextlib
import sys
from dataclasses import dataclass
from typing import Any

import numpy as np

DEFAULT_VOICE = "Vivian"
MAX_TEXT_CHARS = 5_000
MIN_SPEED = 0.25
MAX_SPEED = 4.0


class ChatRequestError(ValueError):
    """The TTS request frame violates the worker protocol."""


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def split_voice(voice: str | None) -> tuple[str, str | None]:
    """Return the speaker and optional instruction encoded in a voice value.

    SpeechRequest intentionally has no provider-specific instruct field. The
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


@dataclass(frozen=True)
class TtsRequest:
    text: str
    speaker: str
    instruction: str | None


def validate_tts_request(request: dict[str, Any]) -> TtsRequest:
    """Return the normalized request or raise ChatRequestError."""
    text = request.get("text")
    if not isinstance(text, str) or not text.strip():
        raise ChatRequestError("text must not be empty")
    if len(text) > MAX_TEXT_CHARS:
        raise ChatRequestError(f"text exceeds the {MAX_TEXT_CHARS} character limit")

    voice = request.get("voice")
    if voice is not None and not isinstance(voice, str):
        raise ChatRequestError("voice must be a string")

    speed = request.get("speed", 1.0)
    if speed is None:
        speed = 1.0
    if not isinstance(speed, (int, float)) or isinstance(speed, bool):
        raise ChatRequestError("speed must be a number")
    if not MIN_SPEED <= float(speed) <= MAX_SPEED:
        raise ChatRequestError(f"speed must be between {MIN_SPEED} and {MAX_SPEED}")

    language = request.get("language", "auto")
    if language is not None and not isinstance(language, str):
        raise ChatRequestError("language must be a string")

    instruct = request.get("instruct")
    if instruct is not None and not isinstance(instruct, str):
        raise ChatRequestError("instruct must be a string")

    speaker, encoded_instruct = split_voice(voice)
    instruction = (
        instruct.strip()
        if isinstance(instruct, str) and instruct.strip()
        else encoded_instruct
    )
    return TtsRequest(text=text.strip(), speaker=speaker, instruction=instruction)


class Qwen3TtsEngine:
    """单实例 TTS 引擎：load 一次，常驻服务所有 infer。"""

    def __init__(self, model_root: str) -> None:
        self.model_root = model_root
        self._model: Any | None = None
        self._load()

    def _load(self) -> None:
        with contextlib.redirect_stdout(sys.stderr):
            from mlx_audio.tts.generate import load_model as mlx_load_model

            log(f"[qwen3-tts-runner] loading model from {self.model_root}")
            self._model = mlx_load_model(self.model_root)

    def synthesize(
        self,
        request: dict[str, Any],
        output_wav: str,
    ) -> dict[str, Any]:
        """Generate one complete non-streaming CustomVoice result.

        Current mlx-audio exposes speed on the generic ``generate`` API, while
        ``generate_custom_voice`` has no speed argument and documents speed as
        not directly supported. Keep accepting and validating speed at the
        daemon boundary; use the native CustomVoice call here without silently
        resampling.
        """
        if self._model is None:
            raise RuntimeError("model is not loaded")
        normalized = validate_tts_request(request)
        with contextlib.redirect_stdout(sys.stderr):
            results = list(
                self._model.generate_custom_voice(
                    text=normalized.text,
                    speaker=normalized.speaker,
                    language=(request.get("language") or "Auto").strip() or "Auto",
                    instruct=normalized.instruction,
                    verbose=False,
                )
            )
        parts = [
            result.audio for result in results if getattr(result, "audio", None) is not None
        ]
        if not parts:
            raise RuntimeError("model produced no audio")
        audio = parts[0] if len(parts) == 1 else np.concatenate(parts)
        sample_rate = int(getattr(results[0], "sample_rate", 24000))
        if sample_rate <= 0:
            raise RuntimeError("model returned an invalid sample rate")

        with contextlib.redirect_stdout(sys.stderr):
            from mlx_audio.audio_io import write as audio_write

            audio_write(output_wav, audio, sample_rate, format="wav")

        bytes_written = len(audio) * 2  # 16-bit PCM mono
        return {
            "content_type": "audio/wav",
            "path": output_wav,
            "bytes": bytes_written,
            "sample_rate": sample_rate,
        }
