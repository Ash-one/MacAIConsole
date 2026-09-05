from __future__ import annotations

import json
import os
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
import wave
from pathlib import Path


class WhisperServerError(RuntimeError):
    pass


def validate_pcm_wav(path: str) -> Path:
    audio = Path(path)
    if not audio.is_file() or audio.stat().st_size == 0:
        raise ValueError("audio file is missing or empty")
    try:
        with wave.open(str(audio), "rb") as wav:
            if wav.getcomptype() != "NONE" or wav.getnframes() <= 0:
                raise ValueError("audio must be non-empty PCM WAV")
    except (EOFError, OSError, wave.Error) as error:
        raise ValueError("audio cannot be decoded as PCM WAV") from error
    return audio


def detect_device(logs: str) -> str:
    value = logs.lower()
    if "coreml = 1" in value or "core ml model loaded" in value:
        return "coreml"
    if any(token in value for token in ("metal = 1", "metal: true", "ggml_metal", "mtl : embed_library")):
        return "metal"
    return "cpu"


def resolve_server_binary() -> Path:
    override = os.environ.get("MACAI_WHISPER_SERVER", "").strip()
    if override:
        path = Path(override).expanduser()
        if path.is_file():
            return path
        raise WhisperServerError(f"MACAI_WHISPER_SERVER points to a missing file: {path}")
    fingerprint = (
        Path.home()
        / "Library/Application Support/MacAIConsole/Engines/org.macai.whisper.cpp/.macai-engine.json"
    )
    try:
        relative = json.loads(fingerprint.read_text())["binary"]
    except (OSError, ValueError, KeyError) as error:
        raise WhisperServerError(
            "whisper-server is not installed; install org.macai.whisper.cpp from Runner environments"
        ) from error
    binary = fingerprint.parent / relative
    if not binary.is_file():
        raise WhisperServerError(f"managed whisper-server is missing: {binary}")
    return binary


def resolve_model_file(model_root: str) -> Path:
    root = Path(model_root)
    if root.is_file() and root.suffix.lower() == ".bin":
        return root
    matches = sorted(root.glob("*.bin")) if root.is_dir() else []
    if not matches:
        raise WhisperServerError(f"no .bin Whisper model at {root}")
    return matches[0]


def encode_multipart(audio: Path, fields: dict[str, str]) -> tuple[bytes, str]:
    boundary = f"macai-{uuid.uuid4().hex}"
    chunks: list[bytes] = []
    for name, value in fields.items():
        chunks.extend([
            f"--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n".encode(),
            value.encode(),
            b"\r\n",
        ])
    chunks.extend([
        f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n".encode(),
        audio.read_bytes(),
        f"\r\n--{boundary}--\r\n".encode(),
    ])
    return b"".join(chunks), boundary


def parse_response(payload: bytes) -> tuple[str, str | None]:
    try:
        value = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise WhisperServerError("whisper-server returned invalid JSON") from error
    text = str(value.get("text") or "").strip()
    if not text:
        raise WhisperServerError(str(value.get("error") or "whisper-server returned empty text"))
    language = value.get("language") or value.get("detected_language")
    return text, str(language) if language else None


class WhisperCppEngine:
    def __init__(self, model_root: str) -> None:
        self.model_path = resolve_model_file(model_root)
        self.port = self._free_port()
        self.process: subprocess.Popen | None = None
        self._log_path: Path | None = None
        self._log_file = None

    @staticmethod
    def _free_port() -> int:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            return int(sock.getsockname()[1])

    @staticmethod
    def _opener():
        return urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def start(self) -> str:
        binary = resolve_server_binary()
        fd, name = tempfile.mkstemp(prefix="macai-whisper-server-", suffix=".log")
        os.close(fd)
        self._log_path = Path(name)
        self._log_file = self._log_path.open("ab", buffering=0)
        command = [
            str(binary), "--host", "127.0.0.1", "--port", str(self.port),
            "--model", str(self.model_path), "--language", "auto",
        ]
        try:
            self.process = subprocess.Popen(
                command, stdin=subprocess.DEVNULL, stdout=self._log_file,
                stderr=subprocess.STDOUT,
            )
            self._wait_ready()
            return self.effective_device()
        except Exception:
            self.stop()
            raise

    def _wait_ready(self) -> None:
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            if self.process is not None and self.process.poll() is not None:
                raise WhisperServerError(f"whisper-server exited during load: {self.logs().strip()}")
            try:
                with self._opener().open(f"http://127.0.0.1:{self.port}/health", timeout=1) as response:
                    if response.status == 200:
                        return
            except (urllib.error.URLError, OSError):
                pass
            time.sleep(0.2)
        raise WhisperServerError("whisper-server did not become ready within 300 seconds")

    def transcribe(self, audio_path: str, language: str | None) -> tuple[str, str | None]:
        audio = validate_pcm_wav(audio_path)
        fields = {"response_format": "verbose_json", "no_context": "true"}
        if language:
            fields["language"] = language
        body, boundary = encode_multipart(audio, fields)
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.port}/inference",
            data=body,
            headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
            method="POST",
        )
        try:
            with self._opener().open(request, timeout=300) as response:
                return parse_response(response.read())
        except urllib.error.HTTPError as error:
            raise WhisperServerError(f"whisper-server HTTP {error.code}: {error.read().decode(errors='replace')}") from error
        except (urllib.error.URLError, OSError) as error:
            raise WhisperServerError(f"whisper-server request failed: {error}") from error

    def logs(self) -> str:
        if self._log_path is None:
            return ""
        try:
            return self._log_path.read_text(errors="replace")
        except OSError:
            return ""

    def effective_device(self) -> str:
        return detect_device(self.logs())

    @property
    def running(self) -> bool:
        return self.process is not None and self.process.poll() is None

    def stop(self) -> None:
        process, self.process = self.process, None
        if process is not None and process.poll() is None:
            try:
                process.send_signal(signal.SIGTERM)
                process.wait(timeout=5)
            except (ProcessLookupError, PermissionError):
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if self._log_file is not None:
            self._log_file.close()
            self._log_file = None
        if self._log_path is not None:
            self._log_path.unlink(missing_ok=True)
            self._log_path = None
