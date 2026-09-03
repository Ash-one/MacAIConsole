"""真实接线证据：SmolLM2 经 mlx-lm Runner 协议 load → chat infer 闭环。

协议交互与 daemon 侧 supervisor 相同（length-prefixed frames）。
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODEL = Path.home() / "Library/Application Support/MacAIConsole/Models/llm/SmolLM2-135M-Instruct-8bit"

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
    proc = subprocess.Popen(
        [str(ROOT / ".venv/bin/python"), "-m", "macai_mlx_lm_runner"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=False,
    )
    hello = read_frame(proc.stdout)
    assert hello["type"] == "hello", hello
    print(f"[1] hello: capabilities={hello['payload']['capabilities']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "initialize", "id": "startup", "payload": {"runner_id": "org.macai.mlx-lm", "log_level": "info", "temp_root": "/tmp", "network": False}})
    print(f"[2] {read_frame(proc.stdout)['type']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "load", "id": "load-1", "payload": {"model_id": "SmolLM2-135M-Instruct-8bit", "adapter": "mlx-lm-chat", "model_root": str(MODEL)}})
    loaded = read_frame(proc.stdout)
    assert loaded["type"] == "loaded", loaded
    print(f"[3] loaded: device={loaded['payload']['effective_device']}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "infer", "id": "infer-1", "payload": {"capability": "chat.v1", "request": {"messages": [{"role": "user", "content": "用一句话介绍你自己"}], "max_tokens": 64, "temperature": 0.0}}})
    deltas = []
    while True:
        frame = read_frame(proc.stdout)
        if frame["type"] == "accepted":
            continue
        if frame["type"] == "delta":
            deltas.append(frame["payload"]["text"])
            print(f"    delta: {frame['payload']['text']!r}")
        elif frame["type"] == "result":
            usage = frame["payload"]["usage"]
            print(f"[4] result: finish={frame['payload']['finish_reason']} usage={usage}")
            break
        else:
            raise AssertionError(frame)
    text = "".join(deltas)
    assert text == frame["payload"]["text"], "delta stream must concat to result text"
    print(f"[5] full text ({len(text)} chars): {text}")

    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "unload", "id": "unload-1", "payload": {}})
    print(f"[6] {read_frame(proc.stdout)['type']}")
    send(proc.stdin, {"protocol": "macai.runner.v1", "type": "shutdown", "id": "shutdown-1", "payload": {}})
    print(f"[7] {read_frame(proc.stdout)['type']}")
    proc.wait(timeout=10)
    print("WIRING OK")


if __name__ == "__main__":
    main()
