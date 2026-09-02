"""macai_kokoro_runner — Kokoro-82M-zh-MLX 的 Runner Protocol v1 adapter。

stdout 只承载 length-prefixed JSON frame（Runner Protocol v1）；
所有第三方库日志留在 stderr。模型语义（G2P v1.1 patch、长文本切分、
多段音频拼接）迁移自 scripts/kokoro_worker.py，cutover 后旧 worker 删除。
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import re
import shutil
import sys
import time

import numpy as np


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _ensure_short_espeak_data() -> None:
    """espeak-ng 固定缓冲会截断过长的 data 路径（实测 ~255 字符以上失败）：
    受管环境位于 `Runtimes/python/<env-id>/<64-hex-fingerprint>/.venv/…` 时
    espeak-ng-data 绝对路径超过限制，espeak_Initialize 回退编译期默认路径并
    exit(1) 杀死整个 worker。修复：把 espeakng_loader 的 data 复制到 `/tmp`
    短路径，并把 phonemizer 的 data path 指向副本；路径本身够短时零开销跳过。

    只复制 21MB 目录一次（按源路径 hash 去重），由 OS 清理临时副本。
    """
    try:
        import espeakng_loader
        from phonemizer.backend.espeak.wrapper import EspeakWrapper
    except Exception:  # noqa: BLE001 — espeak 不可用时保持原状
        return
    # misaki/espeak.py 在模块级执行 set_data_path(长路径)；必须先触发它，
    # 否则后续首次 import 会把下面设置的短路径覆盖回去。
    try:
        from misaki import espeak as _misaki_espeak  # noqa: F401
    except Exception:  # noqa: BLE001
        pass

    source = espeakng_loader.get_data_path()
    # 阈值取 200，远低于 espeak-ng 实测截断点（~255）。
    if len(source) <= 200 and os.path.isdir(source):
        EspeakWrapper.set_data_path(source)
        return
    digest = hashlib.sha1(source.encode("utf-8")).hexdigest()[:10]
    target_root = os.path.join("/tmp", f"macai-espeak-{digest}")
    target = os.path.join(target_root, "espeak-ng-data")
    if not os.path.isdir(target):
        staging = f"{target_root}.staging-{os.getpid()}"
        shutil.rmtree(staging, ignore_errors=True)
        os.makedirs(staging, exist_ok=True)
        shutil.copytree(source, os.path.join(staging, "espeak-ng-data"))
        try:
            os.replace(staging, target_root)
        except FileExistsError:
            shutil.rmtree(staging, ignore_errors=True)
    EspeakWrapper.set_data_path(target)
    log(f"[kokoro-runner] espeak data relocated to short path: {target}")


def _split_long_text_for_kokoro(text: str, max_chars: int = 150) -> str:
    """把长文本切成短行，规避 mlx-audio 中文管线的音素硬截断。"""
    text = (text or "").strip()
    if not text or max_chars <= 0 or len(text) <= max_chars:
        return text

    units = [part for part in re.split(r"(?<=[。！？!?；;，,\.\\s])", text) if part]
    if not units:
        units = [text]

    expanded = []
    for unit in units:
        unit = unit.strip()
        if not unit:
            continue
        expanded.extend(unit[i : i + max_chars] for i in range(0, len(unit), max_chars))

    lines = []
    current = ""
    for unit in expanded:
        candidate = current + unit
        if current and len(candidate) > max_chars:
            lines.append(current)
            current = unit
        else:
            current = candidate
    if current:
        lines.append(current)
    return "\n".join(lines)


def _patch_misaki_zh_version() -> None:
    """修复 mlx-audio 0.5.x 中文 G2P 的 vocab 错位（钉为 v1.1，补英文 fallback）。"""

    def _default_en_callable():
        try:
            from misaki import en as misaki_en
            from misaki import espeak

            fallback = espeak.EspeakFallback(british=False)
            g2p = misaki_en.G2P(trf=False, british=False, fallback=fallback, unk="")

            def en_callable(text):
                _, tokens = g2p(text)
                return "".join(
                    (t.phonemes or "") + (" " if t.whitespace else "") for t in tokens
                ).strip()

            return en_callable
        except Exception as error:  # noqa: BLE001
            log(f"[kokoro-runner] en_callable unavailable: {error}")
            return None

    try:
        from misaki import zh as misaki_zh

        original = misaki_zh.ZHG2P

        class ZHG2PV11(original):
            def __init__(self, version=None, en_callable=None, **kwargs):
                super().__init__(
                    version="1.1" if version is None else version,
                    en_callable=en_callable if en_callable else _default_en_callable(),
                    **kwargs,
                )
                if not hasattr(self, "unk"):
                    self.unk = "❓"

        misaki_zh.ZHG2P = ZHG2PV11
        log("[kokoro-runner] misaki ZHG2P patched to version='1.1'")
    except Exception as error:  # noqa: BLE001
        log(f"[kokoro-runner] misaki zh patch failed: {error}")


class KokoroEngine:
    """单实例模型引擎：load 一次，常驻服务所有 infer。"""

    def __init__(self, model_root: str) -> None:
        self.model_root = model_root
        # 必须先于任何 EspeakBackend 构造修复 espeak data 路径（短路径），
        # 长路径会被 espeak-ng 固定缓冲截断并 exit(1) 杀死 worker。
        _ensure_short_espeak_data()
        _patch_misaki_zh_version()
        from mlx_audio.tts.generate import load_model

        self._model = load_model(model_root)

    def synthesize(
        self,
        text: str,
        voice: str,
        speed: float,
        output_wav: str,
    ) -> dict:
        start = time.time()
        voice_ref = os.path.join(self.model_root, "voices", f"{voice}.safetensors")
        if not os.path.isfile(voice_ref):
            voice_ref = voice

        kwargs = {"text": text, "voice": voice_ref, "speed": speed}
        if any("\u4e00" <= ch <= "\u9fff" for ch in text):
            kwargs["lang_code"] = "z"
            text = _split_long_text_for_kokoro(text, max_chars=150)
            kwargs["text"] = text

        with contextlib.redirect_stdout(sys.stderr):
            results = list(self._model.generate(**kwargs))
        parts = [r.audio for r in results if r.audio is not None]
        audio = None
        if parts:
            audio = parts[0] if len(parts) == 1 else np.concatenate(parts)
        if audio is None:
            raise RuntimeError("model produced no audio")

        sample_rate = int(getattr(results[0], "sample_rate", 24000))
        from mlx_audio.audio_io import write as audio_write

        audio_write(output_wav, audio, sample_rate, format="wav")
        duration = len(audio) / sample_rate
        log(f"[kokoro-runner] synthesized {duration:.1f}s in {time.time() - start:.1f}s")
        return {
            "content_type": "audio/wav",
            "path": output_wav,
            "bytes": os.path.getsize(output_wav),
            "duration_ms": int(duration * 1000),
            "sample_rate": sample_rate,
        }
