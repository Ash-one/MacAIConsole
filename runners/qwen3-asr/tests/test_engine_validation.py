"""engine 纯逻辑单测：PCM WAV 前置校验（不加载 MLX）。"""

import wave

import pytest

from macai_qwen3_asr_runner.engine import AudioInputError, validate_pcm_wav


def test_missing_file_is_rejected(tmp_path):
    with pytest.raises(AudioInputError):
        validate_pcm_wav(str(tmp_path / "nope.wav"))


def test_non_wav_bytes_are_rejected(tmp_path):
    path = tmp_path / "not_wav.wav"
    path.write_bytes(b"this is not a wav file at all")
    with pytest.raises(AudioInputError, match="PCM WAV"):
        validate_pcm_wav(str(path))


def test_empty_wav_is_rejected(tmp_path):
    path = tmp_path / "empty.wav"
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(24000)
        wav.setnframes(0)
    with pytest.raises(AudioInputError, match="no samples"):
        validate_pcm_wav(str(path))


def test_valid_pcm_wav_passes(tmp_path):
    path = tmp_path / "ok.wav"
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(24000)
        wav.writeframes(b"\x00\x00" * 8000)
    validate_pcm_wav(str(path))
