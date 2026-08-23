#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANAGED_SOURCE_DIR="$ROOT_DIR/.build/llama.cpp-src"
SOURCE_DIR="${AIWORK_LLAMA_SOURCE:-$MANAGED_SOURCE_DIR}"
BUILD_DIR="${AIWORK_LLAMA_BUILD:-$ROOT_DIR/.build/llama.cpp}"
LLAMA_REPO="https://github.com/ggml-org/llama.cpp.git"
LLAMA_COMMIT="8086439a4cea94c71a5dfb8fe4ad1546aebd640f"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "This milestone builds llama.cpp for Apple Silicon (Darwin arm64)." >&2
  exit 1
fi

mkdir -p "$(dirname "$SOURCE_DIR")" "$BUILD_DIR"

if [[ -n "${AIWORK_LLAMA_SOURCE:-}" ]]; then
  if ! git -C "$SOURCE_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "AIWORK_LLAMA_SOURCE is not a Git checkout: $SOURCE_DIR" >&2
    exit 1
  fi
  ACTUAL_COMMIT="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
  if [[ "$ACTUAL_COMMIT" != "$LLAMA_COMMIT" ]]; then
    echo "AIWORK_LLAMA_SOURCE must be at $LLAMA_COMMIT (found $ACTUAL_COMMIT)" >&2
    exit 1
  fi
else
  if [[ ! -d "$SOURCE_DIR/.git" ]]; then
    git init "$SOURCE_DIR"
    git -C "$SOURCE_DIR" remote add origin "$LLAMA_REPO"
  fi

  if ! git -C "$SOURCE_DIR" cat-file -e "$LLAMA_COMMIT^{commit}" 2>/dev/null; then
    git -C "$SOURCE_DIR" fetch --depth 1 origin "$LLAMA_COMMIT"
  fi

  git -C "$SOURCE_DIR" checkout --detach "$LLAMA_COMMIT"
fi

cmake -S "$SOURCE_DIR" -B "$BUILD_DIR" \
  -DLLAMA_BUILD_SERVER=ON \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_EXAMPLES=OFF \
  -DLLAMA_BUILD_TOOLS=ON \
  -DLLAMA_BUILD_UI=OFF \
  -DLLAMA_USE_PREBUILT_UI=OFF \
  -DGGML_METAL=ON \
  -DGGML_ACCELERATE=ON \
  -DCMAKE_BUILD_TYPE=Release

cmake --build "$BUILD_DIR" --target llama-server -j "$(sysctl -n hw.ncpu)"

BINARY="$BUILD_DIR/bin/llama-server"
"$BINARY" --version
echo "llama-server ready: $BINARY"
