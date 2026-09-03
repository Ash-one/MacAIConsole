"""engine 校验语义单测（不需要 sherpa-onnx，只锁音频校验与必需文件清单）。"""

from __future__ import annotations

import sys
import wave
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from macai_sherpa_onnx_runner.engine import (  # noqa: E402
    AudioInputError,
    model_files,
    read_pcm_wav,
)


def write_wav(path, channels=1, rate=16000, frames=1600):
    import struct

    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(channels)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        wav.writeframes(b"\x00\x00" * frames * channels)


def test_model_files_requires_all_four(tmp_path):
    with pytest.raises(FileNotFoundError):
        model_files(str(tmp_path))
    for name in ("tokens.txt", "encoder.int8.onnx", "decoder.onnx"):
        (tmp_path / name).write_text("x")
    with pytest.raises(FileNotFoundError):
        model_files(str(tmp_path))
    (tmp_path / "joiner.int8.onnx").write_text("x")
    files = model_files(str(tmp_path))
    assert len(files) == 4


def test_read_pcm_wav_rejects_compressed_or_empty(tmp_path):
    empty = tmp_path / "empty.wav"
    empty.write_bytes(b"")
    with pytest.raises(AudioInputError):
        read_pcm_wav(str(empty))

    pcm = tmp_path / "mono.wav"
    write_wav(pcm)
    samples, rate = read_pcm_wav(str(pcm))
    assert rate == 16000
    assert samples.size == 1600


def test_read_pcm_wav_downmixes_stereo(tmp_path):
    import numpy as np

    stereo = tmp_path / "stereo.wav"
    write_wav(stereo, channels=2)
    samples, _ = read_pcm_wav(str(stereo))
    # 双声道下混后帧数不变。
    assert samples.size == 1600
    assert samples.dtype == np.float32
