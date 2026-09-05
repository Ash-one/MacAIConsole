import json
import wave

import pytest

from macai_whisper_cpp_runner.engine import (
    detect_device,
    encode_multipart,
    parse_response,
    resolve_model_file,
    validate_pcm_wav,
)


def test_device_detection_prefers_coreml_then_metal():
    assert detect_device("CORE ML model loaded; ggml_metal") == "coreml"
    assert detect_device("system_info: METAL = 1") == "metal"
    assert detect_device("no accelerator") == "cpu"


def test_model_resolution_accepts_only_bin(tmp_path):
    model = tmp_path / "ggml-base.bin"
    model.write_bytes(b"model")
    assert resolve_model_file(str(model)) == model
    assert resolve_model_file(str(tmp_path)) == model
    with pytest.raises(Exception):
        resolve_model_file(str(tmp_path / "missing"))


def test_pcm_validation_and_multipart(tmp_path):
    audio = tmp_path / "audio.wav"
    with wave.open(str(audio), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16000)
        wav.writeframes(b"\0\0" * 16)
    assert validate_pcm_wav(str(audio)) == audio
    body, boundary = encode_multipart(audio, {"response_format": "verbose_json", "language": "zh"})
    assert boundary.encode() in body
    assert b'name="file"' in body
    assert b'name="language"' in body


def test_invalid_audio_and_response_are_rejected(tmp_path):
    bad = tmp_path / "bad.wav"
    bad.write_bytes(b"bad")
    with pytest.raises(ValueError):
        validate_pcm_wav(str(bad))
    with pytest.raises(Exception):
        parse_response(json.dumps({"text": ""}).encode())


def test_verbose_json_response_maps_text_and_language():
    assert parse_response(json.dumps({"text": " 你好 ", "language": "Chinese"}).encode()) == ("你好", "Chinese")
