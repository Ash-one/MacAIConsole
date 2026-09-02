#!/bin/bash
# Kokoro Runner Protocol v1 真实模型端到端 smoke 入口（Phase 2 证据）。
# 用法：MACAI_KOKORO_SMOKE_MODEL=/absolute/path/to/kokoro-82m-zh scripts/tests/kokoro_runner_smoke.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

if [ -z "${MACAI_KOKORO_SMOKE_MODEL:-}" ]; then
    echo "usage: MACAI_KOKORO_SMOKE_MODEL=/absolute/path/to/kokoro-82m-zh $0" >&2
    exit 2
fi

# uv run --frozen 只用提交的 lock 解析驱动依赖（本 smoke 自身零第三方依赖）。
cd "$REPO_ROOT"
exec uv run --project runners/kokoro --frozen --no-dev python scripts/tests/kokoro_runner_smoke.py
