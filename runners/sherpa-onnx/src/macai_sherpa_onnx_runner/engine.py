"""macai_sherpa_onnx_runner — sherpa-onnx streaming Zipformer 的 Runner adapter。

stdout 只承载 length-prefixed JSON frame（Runner Protocol v1）；
所有第三方库日志留在 stderr。模型语义（PCM16 WAV 校验、多声道下混、
0.032s 分块流式解码、0.66s 右上下文 flush、仅中文）迁移自
scripts/sherpa_onnx_worker.py，cutover 后旧 worker 删除。
"""

from __future__ import annotations

import contextlib
import sys
import wave
from dataclasses import dataclass
from typing import Any


class AudioInputError(ValueError):
    """The supplied file is not a usable PCM WAV input."""


@dataclass(frozen=True)
class LoadedModel:
    recognizer: Any
    device: str


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def model_files(model_dir: str) -> tuple[str, str, str, str]:
    import os

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


def read_pcm_wav(path: str) -> tuple[Any, int]:
    """Read PCM16 WAV and downmix multi-channel input for the ASR model."""
    import os

    if not path or not os.path.isfile(path) or os.path.getsize(path) == 0:
        raise AudioInputError("audio file is missing or empty")
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


class SherpaOnnxEngine:
    """单实例 STT 引擎：load 一次，常驻服务所有 infer。"""

    def __init__(self, model_root: str, device: str = "cpu", num_threads: int = 4) -> None:
        self.model_root = model_root
        tokens, encoder, decoder, joiner = model_files(model_root)
        with contextlib.redirect_stdout(sys.stderr):
            import sherpa_onnx

            log(f"[sherpa-onnx-runner] loading zh-int8-2025 on {device}")
            self._recognizer = sherpa_onnx.OnlineRecognizer.from_transducer(
                tokens=tokens,
                encoder=encoder,
                decoder=decoder,
                joiner=joiner,
                num_threads=max(1, num_threads),
                provider=device,
                sample_rate=16_000,
                feature_dim=80,
                decoding_method="greedy_search",
            )
        self._loaded = LoadedModel(recognizer=self._recognizer, device=device)

    def transcribe(self, audio: str) -> str:
        """Decode a complete file through the streaming recognizer."""
        loaded = self._loaded
        if loaded is None:
            raise RuntimeError("model is not loaded")
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
