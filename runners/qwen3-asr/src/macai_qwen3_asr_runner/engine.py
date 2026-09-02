"""Qwen3-ASR-0.6B-MLX Runner engine：legacy `qwen3_asr_mlx_worker.py` 语义迁移。

保持：MLX/Metal 加载、PCM WAV 前置校验、deterministic generate
（temperature=0、batch_size=1、max_tokens=8192）、单次失败不杀 worker。
"""

from __future__ import annotations

import os
import wave
from dataclasses import dataclass
from typing import Any


class AudioInputError(ValueError):
    """The supplied file is not a usable PCM WAV input."""


@dataclass(frozen=True)
class LoadedModel:
    model: Any
    device: str = "metal"


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


class Qwen3AsrEngine:
    """单实例 STT 引擎：load 一次，常驻服务所有 infer。"""

    def __init__(self, model_root: str) -> None:
        self.model_root = model_root
        self._loaded: LoadedModel | None = None
        self._load()

    def _load(self) -> None:
        import contextlib

        import sys

        with contextlib.redirect_stdout(sys.stderr):
            import mlx.core as mx
            from mlx_audio.stt.utils import load_model as mlx_load_model

            print(f"[qwen3-asr-runner] loading MLX model from {self.model_root}", file=sys.stderr)
            model = mlx_load_model(self.model_root, lazy=False, strict=False)
            mx.eval(model.parameters())
        self._loaded = LoadedModel(model=model)

    def transcribe(self, audio: str, language: str | None) -> tuple[str, str]:
        """deterministic MLX inference；返回 (text, detected_language)。"""
        loaded = self._loaded
        if loaded is None:
            raise RuntimeError("model is not loaded")
        validate_pcm_wav(audio)
        import contextlib

        import sys

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
        detected = str(language_value).strip()
        if not text:
            raise RuntimeError("model returned an empty transcription")
        return text, detected
