#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HERMES_BIN="${HERMES_BIN:-hermes}"
MACAI_BASE_URL="${MACAI_BASE_URL:-http://127.0.0.1:11435}"
MACAI_STT_LANGUAGE="${MACAI_STT_LANGUAGE:-zh}"
MACAI_TTS_VOICE="${MACAI_TTS_VOICE:-}"

if [[ "${1:-}" == "--help" ]]; then
  cat <<'EOF'
Configure Hermes command providers for MacAI TTS and STT.

Environment variables:
  HERMES_BIN          Hermes executable (default: hermes)
  MACAI_BASE_URL      MacAI root URL or /v1 API URL
  MACAI_TTS_MODEL     TTS model ID; auto-detected when omitted
  MACAI_TTS_VOICE     Optional MacAI voice ID
  MACAI_STT_MODEL     STT model ID; auto-detected when omitted
  MACAI_STT_LANGUAGE  STT language hint (default: zh)
EOF
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "Unknown argument: $1 (use --help)" >&2
  exit 2
fi

for command in "$HERMES_BIN" python3 curl ffmpeg; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command is missing: $command" >&2
    exit 1
  fi
done

ROOT_URL="${MACAI_BASE_URL%/}"
if [[ "$ROOT_URL" == */v1 ]]; then
  API_URL="$ROOT_URL"
  ROOT_URL="${ROOT_URL%/v1}"
else
  API_URL="$ROOT_URL/v1"
fi

curl --fail --silent --show-error --max-time 5 "$ROOT_URL/health" >/dev/null

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
MODELS_FILE="$WORK_DIR/models.json"
curl --fail --silent --show-error --max-time 10 "$API_URL/models" -o "$MODELS_FILE"

discover_model() {
  python3 - "$MODELS_FILE" "$1" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    payload = json.load(stream)
model_type = sys.argv[2]
for model in payload.get("data", []):
    if model.get("type") == model_type and model.get("id"):
        print(model["id"])
        break
PY
}

TTS_MODEL="${MACAI_TTS_MODEL:-$(discover_model tts)}"
STT_MODEL="${MACAI_STT_MODEL:-$(discover_model stt)}"

if [[ -z "$TTS_MODEL" ]]; then
  echo "No TTS model found. Register a MacAI TTS model or set MACAI_TTS_MODEL." >&2
  exit 1
fi
if [[ -z "$STT_MODEL" ]]; then
  echo "No STT model found. Register a MacAI STT model or set MACAI_STT_MODEL." >&2
  exit 1
fi

TTS_COMMAND="python3 \"$SCRIPT_DIR/hermes_macai_tts.py\" --input \"{input_path}\" --output \"{output_path}\" --base-url \"$API_URL\" --model \"{model}\" --voice \"{voice}\" --format \"{format}\" --speed \"{speed}\""
STT_COMMAND="bash \"$SCRIPT_DIR/hermes_macai_stt.sh\" \"{input_path}\" \"{output_path}\" \"$API_URL\" \"{model}\" \"{language}\""

set_config() {
  "$HERMES_BIN" config set --force "$1" "$2" >/dev/null
}

set_config tts.provider macai
set_config tts.providers.macai.type command
set_config tts.providers.macai.command "$TTS_COMMAND"
set_config tts.providers.macai.model "$TTS_MODEL"
set_config tts.providers.macai.voice "$MACAI_TTS_VOICE"
set_config tts.providers.macai.output_format wav
set_config tts.providers.macai.timeout 150
set_config tts.providers.macai.max_text_length 3000
set_config tts.providers.macai.voice_compatible true

set_config stt.enabled true
set_config stt.echo_transcripts true
set_config stt.provider macai
set_config stt.language "$MACAI_STT_LANGUAGE"
set_config stt.providers.macai.type command
set_config stt.providers.macai.command "$STT_COMMAND"
set_config stt.providers.macai.model "$STT_MODEL"
set_config stt.providers.macai.language "$MACAI_STT_LANGUAGE"
set_config stt.providers.macai.format txt
set_config stt.providers.macai.timeout 300

"$HERMES_BIN" tools enable tts >/dev/null

[[ "$("$HERMES_BIN" config get tts.provider)" == "macai" ]]
[[ "$("$HERMES_BIN" config get stt.provider)" == "macai" ]]

printf 'Hermes audio providers configured successfully.\n'
printf '  MacAI API: %s\n' "$API_URL"
printf '  TTS model: %s\n' "$TTS_MODEL"
printf '  STT model: %s\n' "$STT_MODEL"
printf '  STT language: %s\n' "$MACAI_STT_LANGUAGE"
printf 'Restart Hermes Desktop or the Hermes gateway before the first voice request.\n'
