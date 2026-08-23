#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANAGED_SOURCE_DIR="$ROOT_DIR/.build/whisper.cpp-src"
SOURCE_DIR="${AIWORK_WHISPER_SOURCE:-$MANAGED_SOURCE_DIR}"
BUILD_DIR="${AIWORK_WHISPER_BUILD:-$ROOT_DIR/.build/whisper.cpp}"
WHISPER_REPO="https://github.com/ggml-org/whisper.cpp.git"
WHISPER_COMMIT="371b5a7561823ab2bb32142d2751e35e7534727b"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "This build script targets Apple Silicon macOS." >&2
  exit 1
fi

if [[ -n "${AIWORK_WHISPER_SOURCE:-}" ]]; then
  if ! git -C "$SOURCE_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "AIWORK_WHISPER_SOURCE is not a Git checkout: $SOURCE_DIR" >&2
    exit 1
  fi
  ACTUAL_COMMIT="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
  if [[ "$ACTUAL_COMMIT" != "$WHISPER_COMMIT" ]]; then
    echo "Expected whisper.cpp $WHISPER_COMMIT, got $ACTUAL_COMMIT" >&2
    exit 1
  fi
else
  if [[ ! -d "$SOURCE_DIR/.git" ]]; then
    mkdir -p "$(dirname "$SOURCE_DIR")"
    git clone --filter=blob:none --no-checkout "$WHISPER_REPO" "$SOURCE_DIR"
  fi
  git -C "$SOURCE_DIR" fetch --depth 1 origin "$WHISPER_COMMIT"
  git -C "$SOURCE_DIR" checkout --detach "$WHISPER_COMMIT"
fi

cmake -S "$SOURCE_DIR" -B "$BUILD_DIR" \
  -DCMAKE_BUILD_TYPE=Release \
  -DWHISPER_BUILD_TESTS=OFF \
  -DWHISPER_BUILD_EXAMPLES=ON \
  -DWHISPER_BUILD_SERVER=OFF \
  -DGGML_METAL=ON \
  -DGGML_ACCELERATE=ON
cmake --build "$BUILD_DIR" --target whisper-cli -j "${AIWORK_BUILD_JOBS:-8}"

BINARY="$BUILD_DIR/bin/whisper-cli"
test -x "$BINARY"
"$BINARY" --help >/dev/null
printf 'whisper-cli ready: %s\n' "$BINARY"
