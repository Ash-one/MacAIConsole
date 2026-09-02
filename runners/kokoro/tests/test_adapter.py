"""Kokoro Runner adapter 的可独立判定逻辑测试（不加载真实模型）。"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from macai_kokoro_runner import protocol
from macai_kokoro_runner.engine import _split_long_text_for_kokoro  # noqa: E402


class TestTextSplitting:
    def test_short_text_passes_through_unchanged(self):
        assert _split_long_text_for_kokoro("你好") == "你好"

    def test_empty_text_is_rejected(self):
        assert _split_long_text_for_kokoro("") == ""
        assert _split_long_text_for_kokoro(None) == ""

    def test_long_chinese_text_splits_without_losing_characters(self):
        text = "今天天气真好，我们一起去公园散步，然后吃午饭。" * 20
        split = _split_long_text_for_kokoro(text, max_chars=150)
        # 全部字符保留（换行是新增的切分符，不计入原文字符）
        assert split.replace("\n", "") == text
        for line in split.split("\n"):
            assert len(line) <= 150

    def test_single_sentence_over_limit_is_hard_split(self):
        text = "汉" * 400
        split = _split_long_text_for_kokoro(text, max_chars=150)
        assert split.replace("\n", "") == text
        assert all(len(line) <= 150 for line in split.split("\n"))

    def test_mixed_cjk_and_latin_is_handled(self):
        text = "Hello world，你好世界。Kokoro TTS 是本地语音合成。" * 10
        split = _split_long_text_for_kokoro(text, max_chars=150)
        assert split.replace("\n", "") == text


class TestProtocolCodec:
    def test_frame_round_trip(self):
        frame = {"protocol": "macai.runner.v1", "type": "hello", "id": "startup", "payload": {"pid": 1}}
        assert protocol.decode_frame(protocol.encode_frame(frame)) == frame

    def test_wire_format_matches_daemon_codec(self):
        # 与 Rust 侧 codec 对齐：4 字节 big-endian 长度 + UTF-8 JSON。
        wire = protocol.encode_frame({"payload": {"a": 1}})
        import struct

        (length,) = struct.unpack(">I", wire[:4])
        assert length == len(wire) - 4

    def test_oversize_frame_is_rejected(self, monkeypatch):
        monkeypatch.setattr(protocol, "MAX_FRAME_BYTES", 16)
        with pytest.raises(ValueError):
            protocol.encode_frame({"payload": "x" * 32})

    def test_unicode_payload_survives_round_trip(self):
        frame = {"payload": {"text": "你好世界，Mio"}}
        assert protocol.decode_frame(protocol.encode_frame(frame)) == frame

    def test_read_frame_parses_a_full_wire_frame(self):
        import io

        frame = {"type": "infer", "id": "infer-1", "payload": {"text": "你好"}}
        stream = io.BytesIO(protocol.encode_frame(frame))
        assert protocol.read_frame(stream) == frame
        # 流耗尽后返回 None（EOF 语义与 daemon 对齐）。
        assert protocol.read_frame(stream) is None
