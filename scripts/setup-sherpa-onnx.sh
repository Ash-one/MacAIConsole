#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PYTHON="${AIWORK_SHERPA_ONNX_PYTHON:-$ROOT_DIR/.build/sherpa-onnx-venv/bin/python}"
VENV_DIR="$(dirname "$(dirname "$PYTHON")")"

if [[ -n "${AIWORK_SHERPA_ONNX_PYTHON:-}" ]]; then
  if [[ ! -x "$PYTHON" ]]; then
    echo "AIWORK_SHERPA_ONNX_PYTHON is not executable: $PYTHON" >&2
    exit 1
  fi
else
  if ! command -v python3.12 >/dev/null 2>&1; then
    echo "python3.12 is required; install it or set AIWORK_SHERPA_ONNX_PYTHON" >&2
    exit 1
  fi
  if [[ ! -x "$PYTHON" ]]; then
    python3.12 -m venv "$VENV_DIR"
  fi
fi

"$PYTHON" -m pip install --upgrade 'sherpa-onnx==1.13.6' 'numpy>=1.26,<3'
"$PYTHON" -c 'import sherpa_onnx; print("sherpa-onnx", sherpa_onnx.__version__ if hasattr(sherpa_onnx, "__version__") else "installed")'
printf 'Ready: %s\n' "$PYTHON"
