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


_LETTER_PHONEMES = {
    "A": "ɐ", "B": "bˈi", "C": "sˈi", "D": "dˈi", "E": "ˈi",
    "F": "ˈɛf", "G": "ʤˈi", "H": "ˈAʧ", "I": "ˌI", "J": "ʤˈA",
    "K": "kˈA", "L": "ˈɛl", "M": "ˈɛm", "N": "ˈɛn", "O": "ˈO",
    "P": "pˈi", "Q": "kjˈu", "R": "ˈɑɹ", "S": "ˈɛs", "T": "tˈi",
    "U": "jˈu", "V": "vˈi", "W": "dˈʌbᵊlju", "X": "ˈɛks", "Y": "wˈI", "Z": "zˈi",
}


def _pure_python_spelling_fallback(token) -> tuple[str | None, int | None]:
    """针对不在 CMU 词典里的生僻英文单词，按字母发音拼读，杜绝崩溃。"""
    raw = getattr(token, "text", str(token)).strip()
    if not raw:
        return None, None
    phones = [_LETTER_PHONEMES.get(ch.upper(), "") for ch in raw if ch.isalpha()]
    res = "".join(phones)
    return (res, 2) if res else (None, None)


def _is_espeak_data_intact(path: str) -> bool:
    """检查 espeak 数据目录的核心数据文件是否完整。"""
    if not os.path.isdir(path):
        return False
    required = ("phontab", "phondata", "phonindex")
    return all(
        os.path.isfile(os.path.join(path, item)) and os.path.getsize(os.path.join(path, item)) > 0
        for item in required
    )


def _touch_espeak_data(path: str) -> None:
    """刷新目录和关键文件的 mtime/atime，防止 macOS periodic 任务清理。"""
    try:
        now = time.time()
        os.utime(path, (now, now))
        for item in ("phontab", "phondata", "phonindex"):
            file_path = os.path.join(path, item)
            if os.path.isfile(file_path):
                os.utime(file_path, (now, now))
    except Exception:  # noqa: BLE001
        pass


def _get_espeak_target_root(digest: str) -> str:
    """获取 espeak 数据目录目标根路径，优先使用 ~/.cache/macai/espeak-<digest>。"""
    cache_base = os.environ.get("MACAI_CACHE_DIR") or os.environ.get("XDG_CACHE_HOME")
    if not cache_base:
        cache_base = os.path.expanduser("~/.cache")
    candidate_root = os.path.join(cache_base, "macai", f"espeak-{digest}")
    candidate_target = os.path.join(candidate_root, "espeak-ng-data")
    if len(candidate_target) <= 200:
        return candidate_root
    # 极罕见场景下用户 HOME 路径过长时回退至 /tmp
    return os.path.join("/tmp", f"macai-espeak-{digest}")


def _ensure_short_espeak_data() -> bool:
    """espeak-ng 固定缓冲会截断过长的 data 路径（实测 ~255 字符以上失败）：
    受管环境位于 `Runtimes/python/<env-id>/<64-hex-fingerprint>/.venv/…` 时
    espeak-ng-data 绝对路径超过限制，espeak_Initialize 回退编译期默认路径并
    exit(1) 杀死整个 worker。修复：优先把 espeakng_loader 的 data 复制到
    `~/.cache/macai/espeak-<hash>` 短路径（持久、受用户缓存控制、免于 /tmp 定期清理）；
    极深路径下回退 `/tmp`；路径本身够短时零开销跳过。

    同时具备文件完整性自愈：如果数据文件被误删，自动重新全量复制，避免
    espeak_Initialize 找不到文件触发 C exit(1)。
    返回 True 表示 espeak 可用，False 表示不可用（应降级纯 Python 兜底）。
    """
    try:
        import espeakng_loader
        from phonemizer.backend.espeak.wrapper import EspeakWrapper
    except Exception:  # noqa: BLE001 — espeak 不可用时保持原状
        return False
    # misaki/espeak.py 在模块级执行 set_data_path(长路径)；必须先触发它，
    # 否则后续首次 import 会把下面设置的短路径覆盖回去。
    try:
        from misaki import espeak as _misaki_espeak  # noqa: F401
    except Exception:  # noqa: BLE001
        pass

    try:
        source = espeakng_loader.get_data_path()
    except Exception:  # noqa: BLE001
        return False

    if not _is_espeak_data_intact(source):
        log(f"[kokoro-runner] source espeak data is incomplete: {source}")
        return False

    # 阈值取 200，远低于 espeak-ng 实测截断点（~255）。
    if len(source) <= 200:
        EspeakWrapper.set_data_path(source)
        return True

    digest = hashlib.sha1(source.encode("utf-8")).hexdigest()[:10]
    target_root = _get_espeak_target_root(digest)
    target = os.path.join(target_root, "espeak-ng-data")
    if not _is_espeak_data_intact(target):
        os.makedirs(target_root, exist_ok=True)
        staging = f"{target_root}.staging-{os.getpid()}"
        shutil.rmtree(staging, ignore_errors=True)
        os.makedirs(staging, exist_ok=True)
        shutil.copytree(source, os.path.join(staging, "espeak-ng-data"))
        shutil.rmtree(target_root, ignore_errors=True)
        try:
            os.replace(staging, target_root)
        except Exception:
            shutil.rmtree(staging, ignore_errors=True)
            if not _is_espeak_data_intact(target):
                log(f"[kokoro-runner] failed to establish espeak data at {target}")
                return False

    if _is_espeak_data_intact(target):
        _touch_espeak_data(target)
        EspeakWrapper.set_data_path(target)
        log(f"[kokoro-runner] espeak data verified at short path: {target}")
        return True

    return False


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


def _patch_misaki_zh_version(espeak_ready: bool = True) -> None:
    """修复 mlx-audio 0.5.x 中文 G2P 的 vocab 错位（钉为 v1.1，补英文 fallback）。"""

    def _default_en_callable():
        try:
            from misaki import en as misaki_en

            fallback = None
            if espeak_ready:
                try:
                    from misaki import espeak

                    fallback = espeak.EspeakFallback(british=False)
                except Exception as error:  # noqa: BLE001
                    log(f"[kokoro-runner] espeak fallback init failed: {error}")
                    fallback = None

            if fallback is None:
                fallback = _pure_python_spelling_fallback
                log("[kokoro-runner] using pure-python letter spelling fallback for OOD English words")

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

    if not espeak_ready:
        try:
            from misaki import espeak as _misaki_espeak

            class SafeEspeakFallback:
                def __init__(self, *args, **kwargs):
                    pass

                def __call__(self, token):
                    return _pure_python_spelling_fallback(token)

            _misaki_espeak.EspeakFallback = SafeEspeakFallback
        except Exception:  # noqa: BLE001
            pass

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


def _contains_cjk(text: str) -> bool:
    return any("\u4e00" <= ch <= "\u9fff" for ch in text)


def _resolve_voice_path(model_root: str, voice: str) -> str:
    """voice 解析：模型 voices 目录里的 `{voice}.safetensors` 优先（本地部署绕过
    HF 缓存），否则把 voice 原样交给引擎（兼容远程/内置音色名）。"""
    voice_ref = os.path.join(model_root, "voices", f"{voice}.safetensors")
    if os.path.isfile(voice_ref):
        return voice_ref
    return voice


class KokoroEngine:
    """单实例模型引擎：load 一次，常驻服务所有 infer。"""

    def __init__(self, model_root: str) -> None:
        self.model_root = model_root
        # 必须先于任何 EspeakBackend 构造修复 espeak data 路径（短路径），
        # 长路径会被 espeak-ng 固定缓冲截断并 exit(1) 杀死 worker。
        self._espeak_ready = _ensure_short_espeak_data()
        _patch_misaki_zh_version(espeak_ready=self._espeak_ready)
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
        voice_ref = _resolve_voice_path(self.model_root, voice)

        kwargs = {"text": text, "voice": voice_ref, "speed": speed}
        if _contains_cjk(text):
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
