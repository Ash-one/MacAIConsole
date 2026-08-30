import importlib.util
import io
import json
import os
import struct
import sys
import tempfile
import types
import unittest
import wave
from pathlib import Path
from unittest import mock

WORKER_PATH = Path(__file__).resolve().parents[1] / "qwen3_asr_mlx_worker.py"
SPEC = importlib.util.spec_from_file_location("qwen3_asr_mlx_worker", WORKER_PATH)
assert SPEC is not None and SPEC.loader is not None
worker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = worker
SPEC.loader.exec_module(worker)


def write_wav(path: str) -> None:
    with wave.open(path, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16_000)
        wav.writeframes(struct.pack("<1600h", *([0] * 1600)))


class AudioValidationTests(unittest.TestCase):
    def test_accepts_pcm_wav_and_rejects_invalid_input(self):
        with tempfile.TemporaryDirectory() as directory:
            valid = os.path.join(directory, "valid.wav")
            write_wav(valid)
            worker.validate_pcm_wav(valid)

            invalid = os.path.join(directory, "invalid.wav")
            Path(invalid).write_bytes(b"not a wave file")
            with self.assertRaisesRegex(worker.AudioInputError, "cannot be decoded"):
                worker.validate_pcm_wav(invalid)

            empty = os.path.join(directory, "empty.wav")
            Path(empty).touch()
            with self.assertRaisesRegex(worker.AudioInputError, "missing or empty"):
                worker.validate_pcm_wav(empty)


class InferenceLogicTests(unittest.TestCase):
    def test_formats_fake_model_result_and_passes_deterministic_options(self):
        class Result:
            text = "  你好，世界。  "
            language = "Chinese"

        class Model:
            def generate(self, audio, **kwargs):
                self.audio = audio
                self.kwargs = kwargs
                return Result()

        model = Model()
        loaded = worker.LoadedModel(model=model)
        text, language = worker.transcribe(loaded, "/tmp/input.wav", "Chinese")

        self.assertEqual(text, "你好，世界。")
        self.assertEqual(language, "Chinese")
        self.assertEqual(model.audio, "/tmp/input.wav")
        self.assertEqual(model.kwargs["max_tokens"], 8192)
        self.assertEqual(model.kwargs["batch_size"], 1)
        self.assertEqual(model.kwargs["temperature"], 0.0)
        self.assertEqual(model.kwargs["language"], "Chinese")
        self.assertFalse(model.kwargs["verbose"])

    def test_normalizes_list_language_from_mlx_output(self):
        class Result:
            text = "你好"
            language = ["Chinese"]

        class Model:
            def generate(self, *_args, **_kwargs):
                return Result()

        text, language = worker.transcribe(
            worker.LoadedModel(model=Model()), "/tmp/input.wav", None
        )
        self.assertEqual(text, "你好")
        self.assertEqual(language, "Chinese")

    def test_uses_requested_language_when_result_omits_detection(self):
        class Result:
            text = "hello"
            language = ""

        class Model:
            def generate(self, *_args, **_kwargs):
                return Result()

        text, language = worker.transcribe(
            worker.LoadedModel(model=Model()), "/tmp/input.wav", "English"
        )
        self.assertEqual(text, "hello")
        self.assertEqual(language, "English")

    def test_rejects_empty_model_output(self):
        class Result:
            text = ""
            language = "English"

        class Model:
            def generate(self, *_args, **_kwargs):
                return Result()

        with self.assertRaisesRegex(RuntimeError, "empty transcription"):
            worker.transcribe(
                worker.LoadedModel(model=Model()), "/tmp/input.wav", None
            )


class ServeProtocolTests(unittest.TestCase):
    """daemon 按错误码分流 invalid_request / invalid_audio / inference_error，
    这里在 worker 侧固化 serve() 协议帧的产生逻辑（单次失败不退出 worker）。"""

    def run_serve(self, lines, loaded):
        stdin = io.StringIO("".join(line + "\n" for line in lines))
        stdout = io.StringIO()
        with mock.patch.object(sys, "stdin", stdin), mock.patch.object(
            sys, "stdout", stdout
        ):
            worker.serve(loaded)
        return [json.loads(line) for line in stdout.getvalue().splitlines()]

    def test_error_frames_use_documented_codes_and_keep_worker_alive(self):
        loaded = worker.LoadedModel(model=None)
        frames = self.run_serve(
            [
                "not json",
                '{"id": "x"}',
                '{"id": 1}',
                '{"id": 2, "audio": "/nonexistent.wav", "language": 3}',
            ],
            loaded,
        )
        self.assertIsNone(frames[0]["id"])
        self.assertEqual(frames[0]["error"]["code"], "invalid_request")
        self.assertEqual(frames[1]["error"]["code"], "invalid_request")
        self.assertEqual(frames[2]["error"]["code"], "invalid_audio")
        # language 非字符串在打开音频文件之前就被拒绝。
        self.assertEqual(frames[3]["error"]["code"], "invalid_request")

    def test_success_frame_round_trips_transcription(self):
        class Result:
            text = "  hello  "
            language = ""

        class Model:
            def generate(self, *_args, **_kwargs):
                return Result()

        with tempfile.TemporaryDirectory() as directory:
            wav = os.path.join(directory, "in.wav")
            write_wav(wav)
            loaded = worker.LoadedModel(model=Model(), device="cpu")
            request = json.dumps({"id": 5, "audio": wav, "language": "English"})
            frames = self.run_serve([request], loaded)

        self.assertEqual(len(frames), 1)
        frame = frames[0]
        self.assertTrue(frame["ok"])
        self.assertEqual(frame["id"], 5)
        self.assertEqual(frame["text"], "hello")
        self.assertEqual(frame["language"], "English")
        self.assertEqual(frame["device"], "cpu")


if __name__ == "__main__":
    unittest.main()
