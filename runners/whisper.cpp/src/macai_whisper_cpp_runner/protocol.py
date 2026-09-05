from __future__ import annotations

import json
import struct

MAX_FRAME_BYTES = 8 * 1024 * 1024


def write_frame(stream, frame: dict) -> None:
    payload = json.dumps(frame, ensure_ascii=False).encode("utf-8")
    if len(payload) > MAX_FRAME_BYTES:
        raise ValueError("frame too large")
    stream.write(struct.pack(">I", len(payload)) + payload)
    stream.flush()


def read_frame(stream) -> dict | None:
    header = stream.read(4)
    if not header:
        return None
    if len(header) != 4:
        raise ValueError("truncated frame header")
    (length,) = struct.unpack(">I", header)
    if length > MAX_FRAME_BYTES:
        raise ValueError("frame too large")
    payload = stream.read(length)
    if len(payload) != length:
        raise ValueError("truncated frame payload")
    return json.loads(payload.decode("utf-8"))
