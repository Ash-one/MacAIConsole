#!/usr/bin/env python3
"""Generate small audio fixtures for the daemon's audio normalization tests.

Each fixture is a 500 ms 440 Hz sine tone at 16 kHz mono, encoded with a real
encoder (PyAV's bundled FFmpeg for lossy formats, libsndfile for FLAC) so the
Rust tests exercise the full container-probe + codec-decode path.

Run with a Python that has av + numpy (e.g. .build/qwen3-asr-venv/bin/python):

    python3 scripts/tests/generate_audio_fixtures.py

Outputs to crates/ai-daemon/tests/fixtures/ and is committed alongside the
fixtures it produces.
"""

from __future__ import annotations

import pathlib
import sys

import numpy as np

RATE = 16_000
DURATION_MS = 500
FREQ = 440.0
AMP = 0.5

OUT_DIR = pathlib.Path(__file__).resolve().parent.parent.parent / "crates" / "ai-daemon" / "tests" / "fixtures"


def sine_i16() -> np.ndarray:
    t = np.arange(RATE * DURATION_MS // 1000) / RATE
    wave = AMP * np.sin(2 * np.pi * FREQ * t)
    return (wave * 32767).astype(np.int16)


def encode_with_av(samples: np.ndarray, path: pathlib.Path, format_name: str, codec: str) -> None:
    import av

    container = av.open(str(path), "w", format=format_name)
    stream = container.add_stream(codec, rate=RATE)
    stream.layout = "mono"
    frame = av.AudioFrame.from_ndarray(samples.reshape(1, -1), format="s16", layout="mono")
    frame.sample_rate = RATE
    for packet in stream.encode(frame):
        container.mux(packet)
    for packet in stream.encode(None):
        container.mux(packet)
    container.close()


def encode_sndfile(samples: np.ndarray, path: pathlib.Path, fmt: str, subtype: str) -> None:
    import soundfile as sf

    sf.write(str(path), samples, RATE, format=fmt, subtype=subtype)


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    samples = sine_i16()

    encode_with_av(samples, OUT_DIR / "tone-440.mp3", "mp3", "mp3")
    encode_with_av(samples, OUT_DIR / "tone-440.m4a", "ipod", "aac")
    # PyAV 打包的 ffmpeg 没有 libvorbis，原生 vorbis 编码器被标记 experimental；
    # libsndfile 自带 OGG/Vorbis 编码，走 soundfile。
    encode_sndfile(samples, OUT_DIR / "tone-440.ogg", "OGG", "VORBIS")
    encode_sndfile(samples, OUT_DIR / "tone-440.flac", "FLAC", "PCM_16")

    for path in sorted(OUT_DIR.glob("tone-440.*")):
        print(f"wrote {path} ({path.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
