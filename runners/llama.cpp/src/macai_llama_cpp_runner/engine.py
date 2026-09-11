"""llama.cpp 引擎适配器：定位 llama-server 二进制并管理其子进程生命周期。

Runner 模型目录约定：`model_root` 指向包含 .gguf 的目录（llama.cpp 的模型
是用户任意注册的单文件，daemon 的 ad-hoc 绑定把注册目录作为 model_root 传
入）。二进制查找顺序：
1. 环境变量 `MACAI_LLAMA_SERVER`（显式覆盖，迁移自 legacy `AIWORK_LLAMA_SERVER`）；
2. `<repo>/.build/llama.cpp/bin/llama-server`（脚本安装产物，向后兼容）；
3. Runner 受管产物目录（install 下载的预编译产物，见 daemon 侧安装流程）。

加载失败（二进制缺失、启动失败、health 超时）抛 :class:`LlamaServerError`，
由协议层映射为 `model_load_failed` 帧。
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

HEALTH_TIMEOUT_SECONDS = 120.0
HEALTH_POLL_INTERVAL_SECONDS = 0.2
DEFAULT_PORT = 11436


def _direct_opener() -> urllib.request.OpenerDirector:
    """绕过一切代理的 opener，仅用于本机回环的引擎 HTTP 调用。

    urllib 在环境变量没有代理时会读 macOS 系统代理（scutil），把
    127.0.0.1 的引擎请求发给外部代理转发——代理无法回连本机引擎，
    返回的 503 HTML 会被 health 轮询误判为「模型加载中」。引擎流量
    是 loopback，必须 ProxyHandler({}) 显式直连。
    """
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


class LlamaServerError(RuntimeError):
    """引擎生命周期错误：二进制缺失、启动失败或 health 探测超时。"""


@dataclass
class LoadedServer:
    process: subprocess.Popen
    model_path: Path
    base_url: str


def resolve_server_binary(model_root: Path) -> Path:
    """按覆盖 → 脚本产物 → 受管产物的顺序定位 llama-server。

    `model_root` 可以是 .gguf 文件（ad-hoc 绑定把注册文件作为 artifact root）
    或包含 .gguf 的目录（catalog 绑定）。
    """
    override = os.environ.get("MACAI_LLAMA_SERVER", "").strip()
    if override:
        candidate = Path(override).expanduser()
        if not candidate.is_file():
            raise LlamaServerError(
                f"MACAI_LLAMA_SERVER points to a missing file: {candidate}"
            )
        return candidate

    repo_build = _repo_build_binary(model_root)
    if repo_build is not None:
        return repo_build

    managed = model_root / ".macai" / "llama.cpp" / "bin" / "llama-server"
    if managed.is_file():
        return managed

    path_hit = shutil.which("llama-server")
    if path_hit:
        return Path(path_hit)

    raise LlamaServerError(
        "llama-server executable not found; install it via the Runner "
        "environments page or set MACAI_LLAMA_SERVER"
    )


def _repo_build_binary(model_root: Path) -> Path | None:
    """脚本安装产物定位：model_root 之外的仓库 `.build/` 布局。

    Finder/launchd 启动时 cwd 不可靠，这里只接受显式 override 之外的两条
    可靠路径：从 model_root 上溯找到仓库根（含 `.git` 或 `Cargo.toml`），
    以及 daemon 侧 install 写入的指纹文件（`.macai-engine.json`，含受管
    二进制相对路径）。
    """
    for parent in [model_root, *model_root.parents]:
        candidate = parent / ".build" / "llama.cpp" / "bin" / "llama-server"
        if candidate.is_file():
            return candidate
        if (parent / "Cargo.toml").is_file() or (parent / ".git").is_dir():
            break
    # daemon install 产物：指纹文件记录 sha256 与包内二进制相对路径，
    # 引擎升级 = manifest 换 [engine] 表重装。HOME 须在 runner.toml 的
    # inherit_environment 白名单里（supervisor env_clear 后按白名单注入）。
    fingerprint = (
        Path.home()
        / "Library"
        / "Application Support"
        / "MacAIConsole"
        / "Engines"
        / "org.macai.llama.cpp"
        / ".macai-engine.json"
    )
    try:
        binary_rel = json.loads(fingerprint.read_text())["binary"]
    except (OSError, ValueError, KeyError):
        return None
    managed = fingerprint.parent / binary_rel
    if managed.is_file():
        return managed
    return None


class LlamaCppEngine:
    """一个已加载模型的 llama-server 子进程句柄。"""

    def __init__(self, model_root: str, port: int = DEFAULT_PORT) -> None:
        self.model_root = Path(model_root)
        self.port = port
        self.base_url = f"http://127.0.0.1:{port}"
        self._server: LoadedServer | None = None

    # -- 生命周期 ---------------------------------------------------------

    def start(self) -> LoadedServer:
        if self._server is not None:
            return self._server
        binary = resolve_server_binary(self.model_root)
        model_path = self._resolve_model_file()
        command = [
            str(binary),
            "--host",
            "127.0.0.1",
            "--port",
            str(self.port),
            "--model",
            str(model_path),
        ]
        try:
            process = subprocess.Popen(
                command,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
        except OSError as error:
            raise LlamaServerError(f"cannot start llama-server: {error}") from error
        try:
            self._wait_health()
        except LlamaServerError:
            self.stop()
            raise
        self._server = LoadedServer(
            process=process,
            model_path=model_path,
            base_url=self.base_url,
        )
        return self._server

    def stop(self) -> None:
        server, self._server = self._server, None
        if server is None:
            return
        process = server.process
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except (ProcessLookupError, PermissionError):
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    process.kill()
                process.wait(timeout=5)

    @property
    def running(self) -> bool:
        server = self._server
        return server is not None and server.process.poll() is None

    # -- 内部 -------------------------------------------------------------

    def _resolve_model_file(self) -> Path:
        # ad-hoc 绑定：model_root 就是注册的 .gguf 文件本身。
        if self.model_root.is_file():
            return self.model_root
        ggufs = sorted(self.model_root.glob("*.gguf"))
        if not ggufs:
            raise LlamaServerError(
                f"no .gguf file under model directory: {self.model_root}"
            )
        # 目录绑定约定为目录内首个 .gguf；排序保证确定性。
        return ggufs[0]

    def _wait_health(self) -> None:
        deadline = time.monotonic() + HEALTH_TIMEOUT_SECONDS
        url = f"{self.base_url}/health"
        while time.monotonic() < deadline:
            if self._poll_health_once(url):
                return
            time.sleep(HEALTH_POLL_INTERVAL_SECONDS)
        raise LlamaServerError(
            f"llama-server /health not ready within {HEALTH_TIMEOUT_SECONDS:.0f}s"
        )

    def _poll_health_once(self, url: str) -> bool:
        try:
            with _direct_opener().open(url, timeout=2) as response:
                return 200 <= response.status < 300
        except urllib.error.HTTPError as error:
            # llama-server 健康检查在模型加载中返回 503：继续等待。
            return error.code == 503
        except (urllib.error.URLError, OSError):
            return False

    # -- 推理 -------------------------------------------------------------

    def stream_chat(self, request: dict) -> "LlamaChatStream":
        payload = _chat_payload(request)
        return LlamaChatStream(self, payload)

    def chat_completion(self, request: dict) -> dict:
        """非流式便捷路径：stream_chat 的聚合形式（engine 单测用）。"""
        stream = self.stream_chat(request)
        text_parts: list[str] = []
        finish_reason = "stop"
        usage = {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
        for event in stream:
            if "text" in event:
                text_parts.append(event["text"])
            if "finish_reason" in event:
                finish_reason = event["finish_reason"]
            if "usage" in event:
                usage = event["usage"]
        return {
            "text": "".join(text_parts),
            "finish_reason": finish_reason,
            "usage": usage,
        }


def _chat_payload(request: dict) -> dict:
    """Runner chat 请求 → OpenAI /v1/chat/completions 载荷。"""
    messages = request.get("messages") or []
    if not isinstance(messages, list) or not messages:
        raise LlamaServerError("chat request requires a non-empty messages array")
    payload: dict = {"messages": messages, "stream": True}
    if "temperature" in request and request["temperature"] is not None:
        payload["temperature"] = request["temperature"]
    if "top_p" in request and request["top_p"] is not None:
        payload["top_p"] = request["top_p"]
    if "max_tokens" in request and request["max_tokens"] is not None:
        payload["max_tokens"] = request["max_tokens"]
    return payload


class LlamaChatStream:
    """SSE 流迭代器：把 llama-server 的 chat chunks 转成引擎事件字典。"""

    def __init__(self, engine: LlamaCppEngine, payload: dict) -> None:
        self._engine = engine
        self._payload = payload

    def __iter__(self):
        body = json.dumps(self._payload).encode("utf-8")
        request = urllib.request.Request(
            f"{self._engine.base_url}/v1/chat/completions",
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with _direct_opener().open(request, timeout=600) as response:
                prompt_tokens = 0
                completion_tokens = 0
                for raw_line in response:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                    if not line.startswith("data:"):
                        continue
                    data = line[len("data:") :].strip()
                    if data == "[DONE]":
                        break
                    chunk = json.loads(data)
                    usage = chunk.get("usage") or {}
                    prompt_tokens = usage.get("prompt_tokens", prompt_tokens)
                    completion_tokens = usage.get(
                        "completion_tokens", completion_tokens
                    )
                    choices = chunk.get("choices") or []
                    if not choices:
                        continue
                    delta = choices[0].get("delta") or {}
                    text = delta.get("content")
                    if text:
                        yield {"text": text}
                    reason = choices[0].get("finish_reason")
                    if reason:
                        yield {
                            "finish_reason": reason,
                            "usage": {
                                "prompt_tokens": prompt_tokens,
                                "completion_tokens": completion_tokens,
                                "total_tokens": prompt_tokens + completion_tokens,
                            },
                        }
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")[:500]
            raise LlamaServerError(
                f"llama-server chat failed ({error.code}): {detail}"
            ) from error
        except (urllib.error.URLError, OSError) as error:
            raise LlamaServerError(f"llama-server connection lost: {error}") from error
