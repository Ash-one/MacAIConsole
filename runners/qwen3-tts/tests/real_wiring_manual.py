"""真实接线证据：Qwen3-TTS 经 Runner 协议 load → tts infer → WAV 闭环。

协议交互与 daemon 侧 supervisor 相同（length-prefixed frames）。
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODEL = Path.home() / "Library/Application Support/MacAIConsole/Models/tts/Qwen3-TTS-0.6B-CustomVoice-4bit"
OUT = Path("/tmp/qwen3-tts-runner-wiring.wav")

HEADER = 4


def read_exact(stream, n):
    data = b""
    while len(data) < n:
        chunk = stream.read(n - len(data))
        if not chunk:
            raise EOFError("runner closed")
        data += chunk
    return data


def read_frame(stream):
    header = read_exact(stream, HEADER)
    (length,) = __import__("struct").unpack(">I", header)
    return json.loads(read_exact(stream, length))


def send(stream, frame):
    payload = json.dumps(frame, ensure_ascii=False).encode("utf-8")
    stream.write(__import__("struct").pack(">I", len(payload)) + payload)
    stream.flush()


def main():
    OUT.unlink(missing_ok=True)
    proc = subprocess.Popen(
        [str(ROOT / ".venv/bin/python"), "-m", "macai_qwen3_tts_runner"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=False,
    )
    hello = read_frame(proc.stdout)
    assert hello["type"] == "hello", hello
    print(f"[1] hello: capabilities={hello['payload']['capabilities']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "initialize", "id": "startup", "payload": {"runner_id": "org.macai.qwen3-tts", "log_level": "info", "temp_root": "/tmp", "network": False}})
    print(f"[2] {read_frame(proc.stdout)['type']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "load", "id": "load-1", "payload": {"model_id": "Qwen3-TTS-0.6B-CustomVoice-4bit", "adapter": "qwen3-tts-customvoice", "model_root": str(MODEL)}})
    loaded = read_frame(proc.stdout)
    assert loaded["type"] == "loaded", loaded
    print(f"[3] loaded: device={loaded['payload']['effective_device']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "infer", "id": "infer-1", "payload": {"capability": "tts.v1", "request": {"text": "你好，这是 Qwen3 TTS Runner 接线后的声音。", "voice": "Vivian", "speed": 1.0, "format": "wav", "language": "auto"}, "output": {"directory": "/tmp/qwen3-tts-runner-out", "allowed_extensions": ["wav"]}}})
    while True:
        frame = read_frame(proc.stdout)
        if frame["type"] == "accepted":
            continue
        if frame["type"] == "result":
            print(f"[4] result: bytes={frame['payload']['bytes']} sample_rate={frame['payload']['sample_rate']}")
            break
        if frame["type"] == "error":
            raise AssertionError(frame)
        raise AssertionError(frame)

    wav_path = Path(frame["payload"]["path"])
    size = wav_path.stat().st_size
    assert size > 100000, f"wav too small: {size}"
    with open(wav_path, "rb") as f:
        assert f.read(4) == b"RIFF", "not a RIFF wav"
    print(f"[5] wav valid: {size} bytes at {wav_path}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "unload", "id": "unload-1", "payload": {}})
    print(f"[6] {read_frame(proc.stdout)['type']}")
    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "shutdown", "id": "shutdown-1", "payload": {}})
    print(f"[7] {read_frame(proc.stdout)['type']}")
    proc.wait(timeout=10)
    print("WIRING OK")


if __name__ == "__main__":
    main()
