#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODEL_NAME="${1:-base}"
MODEL_REVISION="5359861c739e955e79d9a303bcbc70fb988958b1"
MODEL_DIR="${AIWORK_WHISPER_MODEL_DIR:-$ROOT_DIR/.build/models}"

if [[ ! "$MODEL_NAME" =~ ^[a-zA-Z0-9._-]+$ ]]; then
  echo "Invalid whisper model name: $MODEL_NAME" >&2
  exit 1
fi

FILE_NAME="ggml-$MODEL_NAME.bin"
DESTINATION="$MODEL_DIR/$FILE_NAME"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/$MODEL_REVISION/$FILE_NAME?download=true"

mkdir -p "$MODEL_DIR"
curl -L --fail --retry 3 --continue-at - "$URL" -o "$DESTINATION"
test -s "$DESTINATION"

printf 'Whisper model ready: %s\n' "$DESTINATION"
shasum -a 256 "$DESTINATION"
