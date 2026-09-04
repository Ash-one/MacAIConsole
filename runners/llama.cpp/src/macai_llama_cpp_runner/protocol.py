"""length-prefixed JSON frame codec（Runner Protocol v1 传输层）。

与 daemon 侧 `ai-daemon::runners::protocol` 保持同一 wire format：
4-byte unsigned big-endian payload length + N-byte UTF-8 JSON object，
上限 8 MiB。daemon 不在 frame 后写换行，Reader 必须按头部长度精确读取。
"""

from __future__ import annotations

import json
import struct

HEADER_SIZE = 4
MAX_FRAME_BYTES = 8 * 1024 * 1024


def encode_frame(frame: dict) -> bytes:
    payload = json.dumps(frame, ensure_ascii=False).encode("utf-8")
    if len(payload) > MAX_FRAME_BYTES:
        raise ValueError(f"frame too large: {len(payload)} > {MAX_FRAME_BYTES}")
    return struct.pack(">I", len(payload)) + payload


def write_frame(stream, frame: dict) -> None:
    """写入一个完整 frame（头部 + payload）并 flush。"""
    stream.write(encode_frame(frame))
    stream.flush()


def read_frame(stream) -> dict | None:
    """精确读取一个 frame；流 EOF（无数据）返回 None。

    daemon 的 frame 不以换行结尾；按 4 字节头声明长度读取 payload，
    避免 readline 语义与 wire format 不匹配。
    """
    header = stream.read(HEADER_SIZE)
    if not header:
        return None
    if len(header) < HEADER_SIZE:
        raise ValueError(f"truncated frame header: {len(header)} bytes")
    (length,) = struct.unpack(">I", header)
    if length > MAX_FRAME_BYTES:
        raise ValueError(f"frame too large: {length} > {MAX_FRAME_BYTES}")
    payload = stream.read(length)
    if len(payload) < length:
        raise ValueError(f"truncated frame payload: {len(payload)} < {length}")
    return json.loads(payload.decode("utf-8"))
