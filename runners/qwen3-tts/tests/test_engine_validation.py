"""engine 校验语义单测（不需要 mlx-audio，只锁请求校验与 voice 拆分语义）。"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from macai_qwen3_tts_runner.engine import (  # noqa: E402
    ChatRequestError,
    split_voice,
    validate_tts_request,
)


def test_split_voice_defaults_and_instruction_escape_hatch():
    assert split_voice(None) == ("Vivian", None)
    assert split_voice("") == ("Vivian", None)
    assert split_voice("Serena") == ("Serena", None)
    assert split_voice("Vivian, very happy") == ("Vivian", "very happy")
    assert split_voice(", whisper") == ("Vivian", "whisper")


def test_valid_request_applies_defaults():
    request = validate_tts_request({"text": "你好"})
    assert request.text == "你好"
    assert request.speaker == "Vivian"
    assert request.instruction is None


def test_instruct_field_overrides_voice_encoding():
    request = validate_tts_request(
        {"text": "hi", "voice": "Serena", "instruct": " calm tone "}
    )
    assert request.speaker == "Serena"
    assert request.instruction == "calm tone"


def test_text_limits_and_speed_boundaries():
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "   "})
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "x" * 5001})
    ok = validate_tts_request({"text": "x" * 5000, "speed": 4.0})
    assert ok.text
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "hi", "speed": 4.1})
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "hi", "speed": 0.2})
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "hi", "speed": True})


def test_invalid_types_raise():
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "hi", "voice": 1})
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "hi", "instruct": []})
    with pytest.raises(ChatRequestError):
        validate_tts_request({"text": "hi", "language": 3})
