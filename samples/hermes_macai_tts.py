#!/usr/bin/env python3
"""Send a Hermes command-provider TTS request to MacAI."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path


def api_base(base_url: str) -> str:
    base = base_url.rstrip("/")
    return base if base.endswith("/v1") else f"{base}/v1"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, help="UTF-8 text file written by Hermes")
    parser.add_argument("--output", required=True, help="Audio output path expected by Hermes")
    parser.add_argument("--base-url", default="http://127.0.0.1:11435")
    parser.add_argument("--model", required=True)
    parser.add_argument("--voice", default="")
    parser.add_argument("--format", default="wav")
    parser.add_argument("--speed", type=float, default=1.0)
    args = parser.parse_args()

    text = Path(args.input).read_text(encoding="utf-8").strip()
    if not text:
        print("TTS input is empty", file=sys.stderr)
        return 2

    payload: dict[str, object] = {
        "model": args.model,
        "input": text,
        "format": args.format,
        "speed": args.speed,
    }
    if args.voice:
        payload["voice"] = args.voice

    request = urllib.request.Request(
        f"{api_base(args.base_url)}/audio/speech",
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            audio = response.read()
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        print(f"MacAI TTS returned HTTP {exc.code}: {detail}", file=sys.stderr)
        return 1
    except urllib.error.URLError as exc:
        print(f"Cannot reach MacAI TTS: {exc.reason}", file=sys.stderr)
        return 1

    if not audio:
        print("MacAI TTS returned an empty audio body", file=sys.stderr)
        return 1

    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(audio)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
