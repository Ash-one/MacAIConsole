import importlib.util
import io
import json
import os
import struct
import sys
import tempfile
import unittest
import wave
from pathlib import Path
from unittest import mock

WORKER_PATH = Path(__file__).resolve().parents[1] / "sherpa_onnx_worker.py"
SPEC = importlib.util.spec_from_file_location("sherpa_onnx_worker", WORKER_PATH)
assert SPEC is not None and SPEC.loader is not None
worker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = worker
SPEC.loader.exec_module(worker)


def write_pcm_wav(path: str, *, channels: int = 1, sample_rate: int = 16_000) -> None:
    frames = [1_000, -1_000] if channels == 1 else [1_000, -1_000, 3_000, 1_000]
    with wave.open(path, "wb") as wav:
        wav.setnchannels(channels)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(struct.pack(f"<{len(frames)}h", *frames))


class ModelLayoutTests(unittest.TestCase):
    def test_requires_the_complete_zh_int8_2025_model_layout(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(FileNotFoundError, "tokens.txt"):
                worker.model_files(directory)

            for name in (
                "tokens.txt",
                "encoder.int8.onnx",
                "decoder.onnx",
                "joiner.int8.onnx",
            ):
                Path(directory, name).write_bytes(b"fixture")

            files = worker.model_files(directory)
            self.assertEqual([Path(path).name for path in files], [
                "tokens.txt",
                "encoder.int8.onnx",
                "decoder.onnx",
                "joiner.int8.onnx",
            ])


class AudioInputTests(unittest.TestCase):
    def test_downmixes_pcm16_wav_and_rejects_unusable_audio(self):
        with tempfile.TemporaryDirectory() as directory:
            stereo = os.path.join(directory, "stereo.wav")
            write_pcm_wav(stereo, channels=2, sample_rate=8_000)
            samples, sample_rate = worker.read_pcm_wav(stereo)
            self.assertEqual(sample_rate, 8_000)
            self.assertEqual(samples.tolist(), [0.0, 2_000 / 32768.0])

            malformed = os.path.join(directory, "malformed.wav")
            Path(malformed).write_bytes(b"not a WAV")
            with self.assertRaisesRegex(worker.AudioInputError, "cannot be decoded"):
                worker.read_pcm_wav(malformed)

            empty = os.path.join(directory, "empty.wav")
            with wave.open(empty, "wb") as wav:
                wav.setnchannels(1)
                wav.setsampwidth(2)
                wav.setframerate(16_000)
            with self.assertRaisesRegex(worker.AudioInputError, "no samples"):
                worker.read_pcm_wav(empty)


class ServeProtocolTests(unittest.TestCase):
    def run_serve(self, lines, loaded):
        stdin = io.StringIO("".join(line + "\n" for line in lines))
        stdout = io.StringIO()
        with mock.patch.object(sys, "stdin", stdin), mock.patch.object(sys, "stdout", stdout):
            worker.serve(loaded)
        return [json.loads(line) for line in stdout.getvalue().splitlines()]

    def test_success_frame_has_the_daemon_transcription_contract(self):
        loaded = worker.LoadedModel(recognizer=object(), device="cpu")
        request = json.dumps({"id": 5, "audio": "/tmp/input.wav", "language": "Chinese"})
        with mock.patch.object(worker, "transcribe", return_value=" 你好，世界。 "):
            frames = self.run_serve([request], loaded)

        self.assertEqual(frames, [{
            "id": 5,
            "ok": True,
            "text": " 你好，世界。 ",
            "language": "Chinese",
            "device": "cpu",
        }])

    def test_validation_failures_are_structured_and_do_not_stop_the_worker(self):
        loaded = worker.LoadedModel(recognizer=object(), device="cpu")
        frames = self.run_serve(
            [
                "not json",
                '{"id": "wrong"}',
                '{"id": 3, "audio": "/tmp/input.wav", "language": "English"}',
            ],
            loaded,
        )

        self.assertEqual([frame["error"]["code"] for frame in frames], [
            "invalid_request",
            "invalid_request",
            "invalid_request",
        ])
        self.assertIsNone(frames[0]["id"])
        self.assertEqual(frames[2]["id"], 3)
        self.assertIn("accepts only Chinese", frames[2]["error"]["message"])


class StartupFailureTests(unittest.TestCase):
    def test_missing_dependency_is_a_structured_model_load_failure(self):
        stdout = io.StringIO()
        stderr = io.StringIO()
        with mock.patch.object(sys, "argv", ["worker", "--model-dir", "/model"]), mock.patch.object(
            sys, "stdout", stdout
        ), mock.patch.object(sys, "stderr", stderr), mock.patch.object(
            worker, "load_model", side_effect=ModuleNotFoundError("No module named 'sherpa_onnx'")
        ):
            self.assertEqual(worker.main(), 1)

        frame = json.loads(stdout.getvalue())
        self.assertFalse(frame["ready"])
        self.assertEqual(frame["error"]["code"], "model_load_failed")
        self.assertIn("sherpa-onnx model load failed", frame["error"]["message"])
        self.assertIn("No module named 'sherpa_onnx'", frame["error"]["message"])


if __name__ == "__main__":
    unittest.main()
