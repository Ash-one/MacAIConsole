"""macai_llama_cpp_runner 引擎单测：纯逻辑边界，不启动真实 llama-server。"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from macai_llama_cpp_runner.engine import (  # noqa: E402
    LlamaCppEngine,
    LlamaServerError,
    _chat_delta_events,
    _chat_payload,
)


def test_chat_payload_requires_messages() -> None:
    with pytest.raises(LlamaServerError):
        _chat_payload({"messages": []})
    with pytest.raises(LlamaServerError):
        _chat_payload({})


def test_chat_payload_maps_openai_fields() -> None:
    payload = _chat_payload(
        {
            "messages": [{"role": "user", "content": "hi"}],
            "temperature": 0.2,
            "top_p": 0.95,
            "max_tokens": 128,
        }
    )
    assert payload["stream"] is True
    assert payload["reasoning_format"] == "auto"
    assert payload["messages"] == [{"role": "user", "content": "hi"}]
    assert payload["temperature"] == 0.2
    assert payload["top_p"] == 0.95
    assert payload["max_tokens"] == 128


def test_chat_delta_separates_reasoning_and_content() -> None:
    assert _chat_delta_events({"reasoning_content": "why", "content": "answer"}) == [
        {"reasoning_text": "why"},
        {"text": "answer"},
    ]
    assert _chat_delta_events({"reasoning": "legacy"}) == [
        {"reasoning_text": "legacy"}
    ]


def test_resolve_model_file_requires_gguf(tmp_path: Path) -> None:
    engine = LlamaCppEngine(str(tmp_path))
    with pytest.raises(LlamaServerError):
        engine._resolve_model_file()


def test_resolve_model_file_prefers_single_gguf(tmp_path: Path) -> None:
    (tmp_path / "model.gguf").write_bytes(b"1")
    engine = LlamaCppEngine(str(tmp_path))
    assert engine._resolve_model_file() == tmp_path / "model.gguf"


def test_resolve_model_file_is_deterministic(tmp_path: Path) -> None:
    (tmp_path / "b.gguf").write_bytes(b"1")
    (tmp_path / "a.gguf").write_bytes(b"2")
    engine = LlamaCppEngine(str(tmp_path))
    assert engine._resolve_model_file() == tmp_path / "a.gguf"


def test_engine_starts_idle() -> None:
    engine = LlamaCppEngine("/nonexistent")
    assert engine.running is False
