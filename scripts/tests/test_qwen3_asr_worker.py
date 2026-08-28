import importlib.util
import math
import os
import struct
import sys
import tempfile
import types
import unittest
import wave
from pathlib import Path
from unittest import mock

WORKER_PATH = Path(__file__).resolve().parents[1] / "qwen3_asr_worker.py"
SPEC = importlib.util.spec_from_file_location("qwen3_asr_worker", WORKER_PATH)
assert SPEC is not None and SPEC.loader is not None
worker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = worker
SPEC.loader.exec_module(worker)


def write_wav(path, channels, sample_rate, frames):
    with wave.open(path, "wb") as wav:
        wav.setnchannels(channels)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        flattened = [sample for frame in frames for sample in frame]
        wav.writeframes(struct.pack(f"<{len(flattened)}h", *flattened))


class AudioPreprocessingTests(unittest.TestCase):
    def test_downmixes_resamples_normalizes_and_pads_pcm_wav(self):
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "stereo.wav")
            # 0.25 s at 8 kHz. Opposing channels downmix to silence.
            frames = [(16_384, -16_384)] * 2_000
            write_wav(path, 2, 8_000, frames)

            samples, sample_rate = worker.preprocess_pcm_wav(path)

            self.assertEqual(sample_rate, 16_000)
            self.assertEqual(len(samples), 8_000)  # padded to the 0.5 s minimum
            self.assertTrue(all(math.isfinite(value) for value in samples))
            self.assertLess(max(abs(value) for value in samples), 1e-6)

    def test_rejects_empty_and_non_wav_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            empty = os.path.join(directory, "empty.wav")
            Path(empty).touch()
            with self.assertRaisesRegex(worker.AudioInputError, "missing or empty"):
                worker.preprocess_pcm_wav(empty)

            invalid = os.path.join(directory, "invalid.wav")
            Path(invalid).write_bytes(b"not a wave file")
            with self.assertRaisesRegex(worker.AudioInputError, "cannot be decoded"):
                worker.preprocess_pcm_wav(invalid)


class InferenceLogicTests(unittest.TestCase):
    def test_formats_fake_model_result(self):
        case = self

        class FakeNumpy:
            float32 = "float32"

            @staticmethod
            def asarray(samples, dtype=None):
                case.assertEqual(dtype, "float32")
                return list(samples)

        class FakeResult:
            text = "  你好，世界。  "
            language = "Chinese"

        class FakeModel:
            def transcribe(self, **kwargs):
                case.assertEqual(kwargs["language"], "Chinese")
                case.assertFalse(kwargs["return_time_stamps"])
                audio, sample_rate = kwargs["audio"]
                case.assertEqual(sample_rate, 16_000)
                case.assertEqual(audio, [0.0, 0.5])
                return [FakeResult()]

        fake_numpy_module = types.ModuleType("numpy")
        setattr(fake_numpy_module, "float32", "float32")
        setattr(fake_numpy_module, "asarray", FakeNumpy.asarray)
        loaded = worker.LoadedModel(FakeModel(), "cpu")
        with mock.patch.dict(sys.modules, {"numpy": fake_numpy_module}):
            text, language = worker.transcribe(loaded, [0.0, 0.5], 16_000, "Chinese")

        self.assertEqual(text, "你好，世界。")
        self.assertEqual(language, "Chinese")

    def test_rejects_empty_model_output(self):
        class FakeModel:
            def transcribe(self, **_kwargs):
                return []

        fake_numpy_module = types.ModuleType("numpy")
        setattr(fake_numpy_module, "float32", "float32")
        setattr(fake_numpy_module, "asarray", lambda samples, dtype=None: samples)
        loaded = worker.LoadedModel(FakeModel(), "cpu")
        with mock.patch.dict(sys.modules, {"numpy": fake_numpy_module}):
            with self.assertRaisesRegex(RuntimeError, "no transcription result"):
                worker.transcribe(loaded, [0.0], 16_000, None)


if __name__ == "__main__":
    unittest.main()
