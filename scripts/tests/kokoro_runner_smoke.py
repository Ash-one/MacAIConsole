#!/usr/bin/env python3
"""Kokoro Runner Protocol v1 真实模型端到端 smoke（Phase 2 证据）。

用法：
    MACAI_KOKORO_SMOKE_MODEL=/absolute/path/to/kokoro-82m-zh \
        scripts/tests/kokoro_runner_smoke.sh

流程：spawn → hello → initialize → load → infer（短中文 / 混合中英 / 长中文）
→ health → unload → shutdown → 进程退出。本脚本自带一份独立的 frame codec
实现，从第三方视角复核 stdout 只承载 wire format、stderr 只承载日志。

验收关注 WAV header、sample rate、时长与进程生命周期；不比较音频内容。
"""

from __future__ import annotations

import json
import os
import struct
import subprocess
import sys
import tempfile
import time

HEADER_SIZE = 4
INFER_TIMEOUT_S = 300


class SmokeError(AssertionError):
    pass


def read_frame(stream):
    header = stream.read(HEADER_SIZE)
    if not header:
        return None
    if len(header) < HEADER_SIZE:
        raise SmokeError(f"truncated frame header: {len(header)!r}")
    (length,) = struct.unpack(">I", header)
    payload = stream.read(length)
    if len(payload) < length:
        raise SmokeError(f"truncated frame payload: {len(payload)} < {length}")
    return json.loads(payload.decode("utf-8"))


def write_frame(stream, frame):
    payload = json.dumps(frame, ensure_ascii=False).encode("utf-8")
    stream.write(struct.pack(">I", len(payload)) + payload)
    stream.flush()


def recv_until(proc, frame_type, frame_id, deadline):
    """读取 frame 直到命中 (type, id)；error frame 直接判失败。"""
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise SmokeError(f"timeout waiting for {frame_type}/{frame_id}")
        frame = read_frame(proc.stdout)
        if frame is None:
            raise SmokeError(f"runner stdout EOF while waiting for {frame_type}/{frame_id}")
        ftype, fid = frame.get("type"), frame.get("id")
        if ftype == "error":
            raise SmokeError(f"runner error frame: {frame.get('payload')}")
        if ftype == frame_type and (frame_id is None or fid == frame_id):
            return frame


def send(proc, frame_type, frame_id, payload=None):
    write_frame(
        proc.stdin,
        {"protocol": "macai.runner.v1", "type": frame_type, "id": frame_id, "payload": payload or {}},
    )


def check_wav(path, expected_rate, expected_bytes):
    if not os.path.isfile(path):
        raise SmokeError(f"result WAV missing: {path}")
    size = os.path.getsize(path)
    if expected_bytes and size != expected_bytes:
        raise SmokeError(f"WAV size mismatch: file={size} result={expected_bytes}")
    with open(path, "rb") as handle:
        header = handle.read(44)
    if header[0:4] != b"RIFF" or header[8:12] != b"WAVE":
        raise SmokeError(f"invalid RIFF/WAVE header: {header[:12]!r}")
    (rate,) = struct.unpack("<I", header[24:28])
    if rate != expected_rate:
        raise SmokeError(f"sample rate mismatch: header={rate} result={expected_rate}")


def run_infer(proc, frame_id, text, out_dir, deadline, min_duration_ms=200):
    send(
        proc,
        "infer",
        frame_id,
        {
            "request": {"text": text, "voice": "zf_001", "speed": 1.0},
            "output": {"directory": out_dir},
        },
    )
    recv_until(proc, "accepted", frame_id, deadline)
    result = recv_until(proc, "result", frame_id, deadline)["payload"]
    out_wav = result["path"]
    duration_ms = int(result["duration_ms"])
    if duration_ms < min_duration_ms:
        raise SmokeError(f"{frame_id}: duration too short: {duration_ms}ms for {len(text)} chars")
    check_wav(out_wav, int(result["sample_rate"]), int(result["bytes"]))
    print(f"[smoke] {frame_id}: {len(text)} chars -> {duration_ms}ms @ {result['sample_rate']}Hz")
    return duration_ms


def main() -> int:
    model_root = os.environ.get("MACAI_KOKORO_SMOKE_MODEL", "").strip()
    if not model_root or not os.path.isdir(model_root):
        print("usage: MACAI_KOKORO_SMOKE_MODEL=/path/to/kokoro-82m-zh kokoro_runner_smoke.sh", file=sys.stderr)
        return 2

    runner_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "runners", "kokoro")
    python = os.path.join(runner_dir, ".venv", "bin", "python")
    if not os.path.isfile(python):
        print(f"runner venv missing: {python}（先 uv sync --project runners/kokoro --locked --no-dev）", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix="kokoro-runner-smoke-") as workdir:
        out_dir = os.path.join(workdir, "output")
        deadline = time.monotonic() + INFER_TIMEOUT_S

        proc = subprocess.Popen(
            [python, "-m", "macai_kokoro_runner"],
            cwd=os.path.abspath(runner_dir),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,
        )
        try:
            hello = recv_until(proc, "hello", "startup", deadline)
            payload = hello["payload"]
            if payload.get("runner_id") != "org.macai.kokoro" or "tts.v1" not in payload.get("capabilities", []):
                raise SmokeError(f"unexpected hello payload: {payload}")
            print(f"[smoke] hello: {payload['runner_id']} v{payload['runner_version']} pid={payload['pid']}")

            send(proc, "initialize", "init-1")
            recv_until(proc, "initialized", "init-1", deadline)

            send(proc, "load", "load-1", {"model_id": "kokoro-82m-zh", "model_root": model_root})
            loaded = recv_until(proc, "loaded", "load-1", deadline)["payload"]
            print(f"[smoke] loaded: device={loaded.get('effective_device')}")

            run_infer(proc, "infer-zh", "你好，这里是 Kokoro Runner 的中文合成验证。", out_dir, deadline)
            run_infer(
                proc,
                "infer-mixed",
                "你好 hello world，这是 mixed text 混合中英文 smoke test。",
                out_dir,
                deadline,
            )
            long_text = (
                "月光落在艾欧尼翁的原野上，风穿过剑与剑之间的缝隙。"
                "我们走过许多日子，也送别过许多同伴；每一段旅程都值得被记住，"
                "每一个名字都值得被轻轻念出。哪怕前路仍然漫长，只要还有人愿意吹响笛声，"
                "夜晚就不会真正安静下来，希望也总会跟着火光一起醒来。"
            )
            long_ms = run_infer(
                proc,
                "infer-long",
                long_text,
                out_dir,
                deadline,
                min_duration_ms=20000,
            )
            print(f"[smoke] long text complete: {len(long_text)} chars -> {long_ms}ms，无截断")

            send(proc, "health", "health-1")
            health = recv_until(proc, "healthy", "health-1", deadline)["payload"]
            if health.get("model_loaded") is not True:
                raise SmokeError(f"health reported model not loaded: {health}")

            send(proc, "unload", "unload-1")
            recv_until(proc, "unloaded", "unload-1", deadline)
            send(proc, "shutdown", "shutdown-1")
            recv_until(proc, "shutdown_complete", "shutdown-1", deadline)
            if proc.stdin is not None:
                proc.stdin.close()
        except Exception:
            proc.kill()
            raise
        finally:
            if proc.poll() is None:
                try:
                    proc.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    raise SmokeError("runner did not exit after shutdown")
        if proc.returncode != 0:
            raise SmokeError(f"runner exit code: {proc.returncode}")

    print("[smoke] PASS: Runner Protocol v1 端到端 load/infer/unload/shutdown 全部通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
