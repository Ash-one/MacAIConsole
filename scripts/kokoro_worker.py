#!/usr/bin/env python3
"""Kokoro-82M-zh-MLX TTS worker。

由 aiworkd 在模型加载时拉起，通过 stdin/stdout 的 JSON 行协议通信：
  请求：{"id": 1, "text": "...", "voice": "zf_001", "speed": 1.0}
  响应：{"id": 1, "ok": true, "wav": "/tmp/xxx.wav"}
       {"id": 1, "ok": false, "error": "..."}
stdin EOF 时退出。音频写入临时文件，路径通过响应返回。
"""

import contextlib
import json
import os
import sys
import tempfile
import time


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _patch_misaki_zh_version() -> None:
    """修复 mlx-audio kokoro 中文 G2P 的 vocab 错位。

    mlx-audio 0.5.0 的 KokoroPipeline 调用 `ZHG2P()` 时不带 version 参数，
    默认输出 IPA 音素（如 tu↗ʂu→），与 v1.1-zh 模型的注音符号 vocab
    （ㄅㄆㄇ 体系）不匹配——未知字符被 filter 静默丢弃，声调全部丢失。
    官方 kokoro 使用 `ZHG2P(version='1.1')`。这里把默认 version 钉为 '1.1'。

    同时补两个缺口：
    - 混入英文时需要 en_callable（官方同样传入），否则英文段被丢弃；
      这里默认用 misaki 的 EN G2P 构造。
    - misaki 未把 unk 参数存为实例属性，英文回退路径会 AttributeError，兜底补上。
    """

    def _default_en_callable():
        try:
            from misaki import en as misaki_en

            g2p = misaki_en.G2P(trf=False, british=False, fallback=None, unk="")

            def en_callable(text):
                _, tokens = g2p(text)
                return "".join(
                    (t.phonemes or "") + (" " if t.whitespace else "") for t in tokens
                ).strip()

            return en_callable
        except Exception as error:  # noqa: BLE001
            log(f"[kokoro-worker] en_callable unavailable: {error}")
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
        log("[kokoro-worker] misaki ZHG2P patched to version='1.1'")
    except Exception as error:  # noqa: BLE001
        log(f"[kokoro-worker] misaki zh patch failed: {error}")


def main() -> None:
    _patch_misaki_zh_version()
    model_dir = sys.argv[sys.argv.index("--model") + 1]

    log(f"[kokoro-worker] loading model from {model_dir}")
    start = time.time()
    from mlx_audio.tts.generate import load_model

    model = load_model(model_dir)
    log(f"[kokoro-worker] model ready in {time.time() - start:.1f}s")

    counter = 0
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError as error:
            print(json.dumps({"id": None, "ok": False, "error": f"bad json: {error}"}), flush=True)
            continue

        request_id = request.get("id")
        text = (request.get("text") or "").strip()
        voice = request.get("voice") or "zf_001"
        speed = float(request.get("speed") or 1.0)
        lang_code = request.get("lang_code")
        if not text:
            print(json.dumps({"id": request_id, "ok": False, "error": "text must not be empty"}), flush=True)
            continue

        try:
            start = time.time()
            # 本地仓库有语音文件时直接传绝对路径（mlx-audio 支持直连 .safetensors），
            # 避免它去 HF 缓存目录找。
            voice_ref = os.path.join(model_dir, "voices", f"{voice}.safetensors")
            if not os.path.isfile(voice_ref):
                voice_ref = voice

            kwargs = {"text": text, "voice": voice_ref, "speed": speed}
            # 语言自动检测：含 CJK 时用中文管线。
            if lang_code:
                kwargs["lang_code"] = lang_code
            elif any("\u4e00" <= ch <= "\u9fff" for ch in text):
                kwargs["lang_code"] = "z"

            # 库内部（如 "Creating new KokoroPipeline..."）会直接 print 到 stdout，
            # 污染 JSON 行协议；合成期间全部重定向到 stderr。
            with contextlib.redirect_stdout(sys.stderr):
                results = list(model.generate(**kwargs))
            audio = results[0].audio if results else None
            if audio is None:
                raise RuntimeError("model produced no audio")

            sample_rate = int(getattr(results[0], "sample_rate", 24000))
            counter += 1
            fd, wav_path = tempfile.mkstemp(prefix=f"kokoro-{counter}-", suffix=".wav")
            os.close(fd)
            from mlx_audio.audio_io import write as audio_write

            audio_write(wav_path, audio, sample_rate, format="wav")
            duration = len(audio) / sample_rate
            log(f"[kokoro-worker] synthesized {duration:.1f}s audio in {time.time() - start:.1f}s")
            print(json.dumps({"id": request_id, "ok": True, "wav": wav_path}), flush=True)
        except Exception as error:  # noqa: BLE001 — 单次合成失败不能杀死 worker
            log(f"[kokoro-worker] synthesis failed: {error}")
            print(json.dumps({"id": request_id, "ok": False, "error": str(error)}), flush=True)


if __name__ == "__main__":
    main()
