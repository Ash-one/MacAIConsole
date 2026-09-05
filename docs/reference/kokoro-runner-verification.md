# Kokoro Runner 验证参考

Status: current

Architecture owner: [`Runner 插件架构`](../decisions/2026-09-02-runner-plugin-architecture.md)

Environment owner: [`uv Python 环境`](../decisions/2026-09-02-uv-python-environments.md)

本文档保留首个真实 Runner 切片的可重复验证路径。它不拥有 Runner
Protocol、manifest、Model Profile 或 uv 环境契约。

## Current path

```text
HTTP / GUI / macai
  → aiworkd model registration
  → immutable Kokoro Model Profile snapshot
  → org.macai.kokoro RunnerProvider
  → daemon-owned uv environment
  → supervised macai_kokoro_runner worker
  → tts.v1
  → validated and cleaned WAV output
```

Runner package 位于 `runners/kokoro/`，包含 `runner.toml`、`pyproject.toml`、
`uv.lock`、`profiles/kokoro-82m-zh.toml`、`src/macai_kokoro_runner/` 和
`tests/test_adapter.py`。legacy `KokoroMlxProvider`、`.build/kokoro-venv`、
`scripts/kokoro_worker.py` 及其测试已删除。

## Automated evidence

```bash
cargo test -p ai-daemon --test runner_runtime_composition
cargo test -p ai-daemon --test runner_kokoro_real_wiring

uv lock --check --project runners/kokoro
PYTHONPATH=runners/kokoro/src \
  python3.12 -m pytest runners/kokoro/tests -v
```

`runner_kokoro_real_wiring` 只有在 `MACAI_KOKORO_WIRING_MODEL` 指向完整模型时
才执行真实 MLX 接线；未设置环境变量时的空跑不能作为真实模型证据。

## Real-model smoke

```bash
MACAI_KOKORO_SMOKE_MODEL=/absolute/path/to/kokoro-82m-zh \
  scripts/tests/kokoro_runner_smoke.sh
```

smoke 需验证：

- 短中文、混合中英文和长中文都生成非空 PCM WAV；
- sample rate、duration、voice 和 speed 契约有效；
- 推理期间 `/api/runtime` 与 Runner status 不被协议 I/O 锁阻塞；
- worker crash 返回 `backend_crashed`，清除 resident，后续 load 可恢复；
- unload / shutdown 后无孤儿 worker 和请求输出目录。

2026-09-02 的目标 Apple Silicon 验证曾覆盖短中文、混合中英文、
116 字长中文、24 kHz WAV、crash/reload 与 unload/shutdown 清理。该结果
是当时的目标机证据；引擎、lock、模型或 macOS 变化后应重跑 smoke。

## Boundary

adapter 单测只验证切分、voice、wire codec 和错误处理。Rust fake Runner
composition 只验证 daemon 组合与进程生命周期。这两层证据都不替代
真实 Kokoro/MLX 模型 smoke。
