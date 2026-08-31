import importlib.util
import io
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

WORKER_PATH = Path(__file__).resolve().parents[1] / "qwen3_tts_worker.py"
SPEC = importlib.util.spec_from_file_location("qwen3_tts_worker", WORKER_PATH)
assert SPEC is not None and SPEC.loader is not None
worker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = worker
SPEC.loader.exec_module(worker)


class VoiceParsingTests(unittest.TestCase):
    def test_defaults_to_vivian_and_preserves_encoded_instruction(self):
        self.assertEqual(worker.split_voice(None), ("Vivian", None))
        self.assertEqual(worker.split_voice(" Vivian, very happy "), ("Vivian", "very happy"))
        self.assertEqual(worker.split_voice("Ryan"), ("Ryan", None))


class GenerationTests(unittest.TestCase):
    def test_calls_native_custom_voice_api_and_combines_segments(self):
        class Result:
            def __init__(self, audio):
                self.audio = audio
                self.sample_rate = 24_000

        class Model:
            def generate_custom_voice(self, **kwargs):
                self.kwargs = kwargs
                return iter([Result([1, 2]), Result([3])])

        model = Model()
        audio, sample_rate = worker.generate_audio(
            model, "hello", "Vivian, very happy", speed=1.5, language="English"
        )
        self.assertEqual(audio.tolist(), [1, 2, 3])
        self.assertEqual(sample_rate, 24_000)
        self.assertEqual(model.kwargs["speaker"], "Vivian")
        self.assertEqual(model.kwargs["language"], "English")
        self.assertEqual(model.kwargs["instruct"], "very happy")

    def test_explicit_instruction_takes_precedence(self):
        class Result:
            audio = [1]
            sample_rate = 24_000

        class Model:
            def generate_custom_voice(self, **kwargs):
                self.kwargs = kwargs
                return iter([Result()])

        model = Model()
        worker.generate_audio(model, "hello", "Vivian, encoded", instruct="explicit")
        self.assertEqual(model.kwargs["instruct"], "explicit")

    def test_empty_generation_is_rejected(self):
        class Model:
            def generate_custom_voice(self, **_kwargs):
                return iter([])

        with self.assertRaisesRegex(RuntimeError, "no audio"):
            worker.generate_audio(Model(), "hello", "Vivian")


class ProtocolTests(unittest.TestCase):
    def run_serve(self, lines, model):
        stdin = io.StringIO("".join(line + "\n" for line in lines))
        stdout = io.StringIO()
        with mock.patch.object(sys, "stdin", stdin), mock.patch.object(sys, "stdout", stdout):
            worker.serve(model)
        return [json.loads(line) for line in stdout.getvalue().splitlines()]

    def test_invalid_frames_are_reported_and_worker_continues(self):
        frames = self.run_serve(
            [
                "not json",
                '{"id": "x"}',
                '{"id": 2, "text": ""}',
                '{"id": 3, "text": "hello", "speed": 0.2}',
                '{"id": 4, "text": "hello", "speed": 4.1}',
                json.dumps({"id": 5, "text": "字" * 5_001}),
            ],
            object(),
        )
        self.assertEqual(len(frames), 6)
        self.assertEqual(frames[0]["error"]["code"], "invalid_request")
        self.assertEqual(frames[1]["error"]["code"], "invalid_request")
        self.assertEqual(frames[2]["error"]["code"], "invalid_request")
        self.assertEqual(frames[3]["error"]["code"], "invalid_request")
        self.assertEqual(frames[4]["error"]["code"], "invalid_request")
        self.assertEqual(frames[5]["error"]["code"], "invalid_request")

    def test_success_frame_writes_wav_and_keeps_protocol_clean(self):
        class Result:
            audio = [0, 1]
            sample_rate = 24_000

        class Model:
            def generate_custom_voice(self, **_kwargs):
                return iter([Result()])

        fake_audio_io = types.ModuleType("mlx_audio.audio_io")
        fake_audio_io.write = lambda path, audio, rate, format: Path(path).write_bytes(
            b"RIFF" + b"\x00" * 4 + b"WAVE" + b"\x00" * 40
        )
        with tempfile.TemporaryDirectory() as directory, mock.patch.dict(
            sys.modules,
            {
                "mlx_audio": types.ModuleType("mlx_audio"),
                "mlx_audio.audio_io": fake_audio_io,
            },
        ):
            frames = self.run_serve(['{"id": 5, "text": "hello", "voice": "Vivian"}'], Model())
        self.assertEqual(len(frames), 1)
        self.assertEqual(frames[0]["id"], 5)
        self.assertTrue(frames[0]["ok"])
        self.assertEqual(frames[0]["wav"].split("/")[-1].startswith("qwen3-tts-"), True)
        Path(frames[0]["wav"]).unlink(missing_ok=True)

    def test_model_exception_is_reported_and_next_request_still_runs(self):
        class Result:
            audio = [0, 1]
            sample_rate = 24_000

        class Model:
            def __init__(self):
                self.calls = 0

            def generate_custom_voice(self, **_kwargs):
                self.calls += 1
                if self.calls == 1:
                    raise RuntimeError("mock inference failure")
                return iter([Result()])

        fake_audio_io = types.ModuleType("mlx_audio.audio_io")
        fake_audio_io.write = lambda path, audio, rate, format: Path(path).write_bytes(
            b"RIFF" + b"\x00" * 4 + b"WAVE" + b"\x00" * 40
        )
        with mock.patch.dict(
            sys.modules,
            {
                "mlx_audio": types.ModuleType("mlx_audio"),
                "mlx_audio.audio_io": fake_audio_io,
            },
        ):
            frames = self.run_serve(
                [
                    '{"id": 1, "text": "first"}',
                    '{"id": 2, "text": "second"}',
                ],
                Model(),
            )
        self.assertEqual(frames[0]["error"]["code"], "inference_error")
        self.assertEqual(frames[1]["id"], 2)
        self.assertTrue(frames[1]["ok"])
        Path(frames[1]["wav"]).unlink(missing_ok=True)

    def test_invalid_model_result_is_reported_without_protocol_corruption(self):
        class Result:
            audio = [0]
            sample_rate = 0

        class Model:
            def generate_custom_voice(self, **_kwargs):
                return iter([Result()])

        frames = self.run_serve(['{"id": 7, "text": "hello"}'], Model())
        self.assertEqual(frames[0]["id"], 7)
        self.assertEqual(frames[0]["error"]["code"], "inference_error")


if __name__ == "__main__":
    unittest.main()
