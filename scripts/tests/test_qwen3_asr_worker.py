import importlib.util
import io
import json
import math
import os
import struct
import sys
import tempfile
import types
import unittest
import wave
from array import array
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


class DecodeAndNormalizeTests(unittest.TestCase):
    def test_decode_pcm_sign_extends_24bit_and_rejects_unknown_width(self):
        # 24 位样本的符号扩展：-1 (0xFFFFFF) 与 1 (0x000001)。
        negative = worker._decode_pcm(b"\xff\xff\xff", 3)
        self.assertAlmostEqual(negative[0], -1.0 / 8_388_608.0)
        positive = worker._decode_pcm(b"\x01\x00\x00", 3)
        self.assertAlmostEqual(positive[0], 1.0 / 8_388_608.0)

        with self.assertRaisesRegex(worker.AudioInputError, "8, 16, 24, or 32"):
            worker._decode_pcm(b"\x00" * 5, 5)

    def test_downmix_rejects_invalid_channel_layout(self):
        with self.assertRaisesRegex(worker.AudioInputError, "channel layout"):
            worker._downmix(array("f", [0.0, 0.0, 0.0]), 2)
        with self.assertRaisesRegex(worker.AudioInputError, "channel layout"):
            worker._downmix(array("f", [0.0]), 0)

        mono = array("f", [0.25])
        self.assertEqual(worker._downmix(mono, 1), mono)

    def test_resample_linear_lengths_and_invalid_rates(self):
        samples = array("f", [0.0, 0.5, -0.5])
        # 相同采样率原样返回（同一对象，避免多余拷贝）。
        self.assertIs(worker._resample_linear(samples, 16_000, 16_000), samples)

        # 8k -> 16k 长度翻倍，端点保留。
        up = worker._resample_linear(array("f", [0.0, 1.0]), 8_000, 16_000)
        self.assertEqual(len(up), 4)
        self.assertAlmostEqual(up[0], 0.0)
        self.assertAlmostEqual(up[2], 1.0)

        # 单样本按目标长度复制。
        single = worker._resample_linear(array("f", [0.5]), 8_000, 16_000)
        self.assertEqual(single, array("f", [0.5, 0.5]))

        with self.assertRaisesRegex(worker.AudioInputError, "sample rate"):
            worker._resample_linear(samples, 0, 16_000)


class ServeProtocolTests(unittest.TestCase):
    """daemon（qwen3_asr.rs）按错误码分流 invalid_request / invalid_audio，
    这里在 worker 侧固化 serve() 协议帧的产生逻辑。"""

    def run_serve(self, lines, loaded):
        stdin = io.StringIO("".join(line + "\n" for line in lines))
        stdout = io.StringIO()
        with mock.patch.object(sys, "stdin", stdin), mock.patch.object(
            sys, "stdout", stdout
        ):
            worker.serve(loaded)
        return [json.loads(line) for line in stdout.getvalue().splitlines()]

    def test_error_frames_use_documented_codes(self):
        loaded = worker.LoadedModel(model=None, device="cpu")
        frames = self.run_serve(
            ["not json", '{"id": "x"}', '{"id": 1}'],
            loaded,
        )
        # 非法 JSON：id 为 None 的 invalid_request 帧，且 worker 不退出。
        self.assertEqual(frames[0]["ok"], False)
        self.assertIsNone(frames[0]["id"])
        self.assertEqual(frames[0]["error"]["code"], "invalid_request")
        # id 非整数 → invalid_request；缺 audio 路径 → invalid_audio。
        self.assertEqual(frames[1]["error"]["code"], "invalid_request")
        self.assertEqual(frames[2]["error"]["code"], "invalid_audio")

    def test_error_frame_shape_matches_daemon_decoder(self):
        frame = worker.error_frame(3, "invalid_audio", "bad wav")
        self.assertEqual(
            frame, {"id": 3, "ok": False, "error": {"code": "invalid_audio", "message": "bad wav"}}
        )

    def test_success_frame_round_trips_transcription(self):
        class FakeModel:
            def transcribe(self, **kwargs):
                self.kwargs = kwargs
                return [types.SimpleNamespace(text="  你好  ", language="Chinese")]

        fake_numpy_module = types.ModuleType("numpy")
        setattr(fake_numpy_module, "float32", "float32")
        setattr(fake_numpy_module, "asarray", lambda samples, dtype=None: list(samples))
        with tempfile.TemporaryDirectory() as directory:
            wav = os.path.join(directory, "in.wav")
            write_wav(wav, 1, 16_000, [(1_000,)])
            fake_model = FakeModel()
            loaded = worker.LoadedModel(fake_model, "cpu")
            request = json.dumps({"id": 7, "audio": wav, "language": "Chinese"})
            with mock.patch.dict(sys.modules, {"numpy": fake_numpy_module}):
                frames = self.run_serve([request], loaded)

        self.assertEqual(len(frames), 1)
        frame = frames[0]
        self.assertTrue(frame["ok"])
        self.assertEqual(frame["id"], 7)
        self.assertEqual(frame["text"], "你好")
        self.assertEqual(frame["language"], "Chinese")
        self.assertEqual(frame["device"], "cpu")
        self.assertEqual(fake_model.kwargs["language"], "Chinese")


if __name__ == "__main__":
    unittest.main()
