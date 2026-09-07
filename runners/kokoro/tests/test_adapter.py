"""Kokoro Runner adapter 的可独立判定逻辑测试（不加载真实模型）。"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from macai_kokoro_runner import protocol
from macai_kokoro_runner.engine import (  # noqa: E402
    _contains_cjk,
    _ensure_short_espeak_data,
    _get_espeak_target_root,
    _is_espeak_data_intact,
    _patch_misaki_zh_version,
    _pure_python_spelling_fallback,
    _resolve_voice_path,
    _split_long_text_for_kokoro,
)


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


class TestVoiceResolution:
    def test_voice_file_in_model_root_wins(self, tmp_path):
        voices = tmp_path / "voices"
        voices.mkdir()
        (voices / "zf_001.safetensors").write_bytes(b"voice")
        resolved = _resolve_voice_path(str(tmp_path), "zf_001")
        assert resolved == str(voices / "zf_001.safetensors")

    def test_missing_voice_file_falls_back_to_literal_name(self, tmp_path):
        # 本地 voices 目录无该音色时原样透传，由引擎决定（远程/内置音色）。
        assert _resolve_voice_path(str(tmp_path), "zf_002") == "zf_002"

    def test_missing_voices_directory_falls_back_to_literal_name(self, tmp_path):
        assert _resolve_voice_path(str(tmp_path), "af_heart") == "af_heart"


class TestLanguageDetection:
    def test_cjk_text_is_detected(self):
        assert _contains_cjk("你好，世界")
        assert _contains_cjk("Kokoro 你好 mixed")

    def test_plain_latin_text_is_not_cjk(self):
        assert not _contains_cjk("hello world")
        assert not _contains_cjk("")


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


class TestEspeakDataAndG2PResilience:
    def test_spelling_fallback_maps_letters(self):
        class DummyToken:
            def __init__(self, text):
                self.text = text

        phones, rating = _pure_python_spelling_fallback(DummyToken("Mio"))
        assert phones == "ˈɛmˌIˈO"
        assert rating == 2

    def test_spelling_fallback_handles_empty_or_non_alpha(self):
        class DummyToken:
            def __init__(self, text):
                self.text = text

        assert _pure_python_spelling_fallback(DummyToken("")) == (None, None)
        assert _pure_python_spelling_fallback(DummyToken("123")) == (None, None)

    def test_is_espeak_data_intact_validates_required_files(self, tmp_path):
        assert not _is_espeak_data_intact(str(tmp_path / "missing"))

        espeak_dir = tmp_path / "espeak-ng-data"
        espeak_dir.mkdir()
        assert not _is_espeak_data_intact(str(espeak_dir))

        (espeak_dir / "phontab").write_bytes(b"data")
        assert not _is_espeak_data_intact(str(espeak_dir))

        (espeak_dir / "phondata").write_bytes(b"data")
        assert not _is_espeak_data_intact(str(espeak_dir))

        (espeak_dir / "phonindex").write_bytes(b"data")
        assert _is_espeak_data_intact(str(espeak_dir))

        # 0 字节损坏文件应当拒绝
        (espeak_dir / "phontab").write_bytes(b"")
        assert not _is_espeak_data_intact(str(espeak_dir))

    def test_get_espeak_target_root(self, tmp_path, monkeypatch):
        # 1. 默认优先使用 XDG_CACHE_HOME / MACAI_CACHE_DIR
        cache_dir = tmp_path / "custom_cache"
        monkeypatch.setenv("XDG_CACHE_HOME", str(cache_dir))
        root = _get_espeak_target_root("abcd1234")
        assert root == str(cache_dir / "macai" / "espeak-abcd1234")

        # 2. 如果路径超长 (>200 字符)，回退到 /tmp
        long_cache = tmp_path / ("very_long_directory_" * 15)
        monkeypatch.setenv("XDG_CACHE_HOME", str(long_cache))
        root_fallback = _get_espeak_target_root("abcd1234")
        assert root_fallback == "/tmp/macai-espeak-abcd1234"

    def test_ensure_short_espeak_data_self_heals_when_pruned(self, tmp_path, monkeypatch):
        import hashlib
        import espeakng_loader

        # 设置 XDG_CACHE_HOME 指向 tmp_path / "cache"
        cache_home = tmp_path / "cache"
        monkeypatch.setenv("XDG_CACHE_HOME", str(cache_home))

        # 构造假的源目录（包含完整数据文件，路径长度 > 200 模拟受管深路径）
        deep_prefix = "deep_subpath_" * 15
        source_dir = tmp_path / deep_prefix / "espeak-ng-data"
        source_dir.mkdir(parents=True)
        for f in ("phontab", "phondata", "phonindex"):
            (source_dir / f).write_bytes(b"valid-content")

        # 模拟 espeakng_loader.get_data_path 返回这个较长路径
        long_source_str = str(source_dir)
        assert len(long_source_str) > 200
        monkeypatch.setattr(espeakng_loader, "get_data_path", lambda: long_source_str)

        # 模拟 target 目录（模拟被清理部分文件后的残缺状态）
        digest = hashlib.sha1(long_source_str.encode("utf-8")).hexdigest()[:10]
        target_root = cache_home / "macai" / f"espeak-{digest}"
        target = target_root / "espeak-ng-data"
        target.mkdir(parents=True)
        (target / "lang").mkdir()

        # 目标原本损坏（缺少 phontab）
        assert not _is_espeak_data_intact(str(target))

        # 执行自愈
        result = _ensure_short_espeak_data()
        assert result is True
        # 目标已被自愈重建
        assert _is_espeak_data_intact(str(target))
        assert (target / "phontab").read_bytes() == b"valid-content"

    def test_patch_misaki_zh_version_with_espeak_not_ready(self):
        # 当 espeak_ready=False 时，必须安全使用纯 Python fallback，不触发 espeak_Initialize
        _patch_misaki_zh_version(espeak_ready=False)
        from misaki import zh as misaki_zh

        g2p = misaki_zh.ZHG2P()
        # 测试中英混合文本包含 OOD 词汇 Mio，确保不抛异常且成功音素化
        res, _ = g2p("你好 Mio 世界")
        assert res
        assert "ˈɛmˌIˈO" in res



