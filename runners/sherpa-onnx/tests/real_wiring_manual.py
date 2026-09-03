"""真实接线证据：sherpa-onnx 经 Runner 协议 load → stt infer → 文本闭环。

用模型仓库自带的 test_wavs/0.wav（中文）作输入，协议交互与 daemon 侧
supervisor 相同（length-prefixed frames）。
"""
import json
import os
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODEL = Path.home() / "Library/Application Support/MacAIConsole/Models/stt/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30"
TEST_WAV = MODEL / "test_wavs" / "0.wav"

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
    if not TEST_WAV.is_file():
        # 模型目录没有 test_wavs（HF 镜像有，GitHub tar.bz2 没有）；用 16kHz
        # 正弦静音会转写为空，因此失败前先下载 test wav。
        print(f"missing test wav: {TEST_WAV}")
        return 1

    proc = subprocess.Popen(
        [str(ROOT / ".venv/bin/python"), "-m", "macai_sherpa_onnx_runner"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=False,
    )
    hello = read_frame(proc.stdout)
    assert hello["type"] == "hello", hello
    print(f"[1] hello: capabilities={hello['payload']['capabilities']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "initialize", "id": "startup", "payload": {"runner_id": "org.macai.sherpa-onnx", "log_level": "info", "temp_root": "/tmp", "network": False}})
    print(f"[2] {read_frame(proc.stdout)['type']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "load", "id": "load-1", "payload": {"model_id": "sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30", "adapter": "sherpa-onnx-transducer", "model_root": str(MODEL)}})
    loaded = read_frame(proc.stdout)
    assert loaded["type"] == "loaded", loaded
    print(f"[3] loaded: device={loaded['payload']['effective_device']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "infer", "id": "infer-1", "payload": {"capability": "stt.v1", "request": {"audio": str(TEST_WAV), "language": "Chinese"}, "output": {}}})
    while True:
        frame = read_frame(proc.stdout)
        if frame["type"] == "accepted":
            continue
        if frame["type"] == "result":
            print(f"[4] result: language={frame['payload']['language']} text={frame['payload']['text']!r}")
            break
        if frame["type"] == "error":
            raise AssertionError(frame)
        raise AssertionError(frame)
    assert frame["payload"]["text"], "transcription must not be empty"

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "unload", "id": "unload-1", "payload": {}})
    print(f"[5] {read_frame(proc.stdout)['type']}")
    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "shutdown", "id": "shutdown-1", "payload": {}})
    print(f"[6] {read_frame(proc.stdout)['type']}")
    proc.wait(timeout=10)
    print("WIRING OK")


if __name__ == "__main__":
    raise SystemExit(main())
