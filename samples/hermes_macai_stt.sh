#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "Usage: $0 <input-audio> <output-text> <base-url> <model> <language>" >&2
  exit 2
fi

INPUT_PATH="$1"
OUTPUT_PATH="$2"
BASE_URL="${3%/}"
MODEL="$4"
LANGUAGE="$5"

if [[ "$BASE_URL" != */v1 ]]; then
  BASE_URL="$BASE_URL/v1"
fi

for command in ffmpeg curl; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command is missing: $command" >&2
    exit 1
  fi
done

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
WAV_PATH="$WORK_DIR/input.wav"

ffmpeg \
  -nostdin \
  -hide_banner \
  -loglevel error \
  -y \
  -i "$INPUT_PATH" \
  -ar 16000 \
  -ac 1 \
  -c:a pcm_s16le \
  "$WAV_PATH"

CURL_ARGS=(
  --fail-with-body
  --silent
  --show-error
  "$BASE_URL/audio/transcriptions"
  -F "file=@$WAV_PATH"
  -F "model=$MODEL"
  -F "response_format=text"
  -o "$OUTPUT_PATH"
)

if [[ -n "$LANGUAGE" ]]; then
  CURL_ARGS+=( -F "language=$LANGUAGE" )
fi

curl "${CURL_ARGS[@]}"

if [[ ! -s "$OUTPUT_PATH" ]]; then
  echo "MacAI STT returned an empty transcript" >&2
  exit 1
fi
