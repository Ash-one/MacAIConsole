import importlib.util
import io
import json
import sys
import types
import unittest
from pathlib import Path
from unittest import mock

WORKER_PATH = Path(__file__).resolve().parents[1] / "mlx_lm_worker.py"
SPEC = importlib.util.spec_from_file_location("mlx_lm_worker", WORKER_PATH)
assert SPEC is not None and SPEC.loader is not None
worker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = worker
SPEC.loader.exec_module(worker)


class FakeTokenizer:
    def apply_chat_template(self, messages, add_generation_prompt=True):
        self.calls = (list(messages), add_generation_prompt)
        return [101, 7, 9]


class RequestValidationTests(unittest.TestCase):
    def test_defaults_and_normalization(self):
        messages, max_tokens, temperature = worker.validate_chat_request(
            {"messages": [{"role": "user", "content": "hi"}]}
        )
        self.assertEqual(messages, [{"role": "user", "content": "hi"}])
        self.assertEqual(max_tokens, 1024)
        self.assertEqual(temperature, 0.7)

        _, max_tokens, temperature = worker.validate_chat_request(
            {"messages": [{"role": "user", "content": "hi"}], "max_tokens": None, "temperature": None}
        )
        self.assertEqual(max_tokens, 1024)
        self.assertEqual(temperature, 0.7)

    def test_rejects_structurally_invalid_requests(self):
        cases = [
            {"messages": []},
            {"messages": "hi"},
            {"messages": [{"role": "", "content": "hi"}]},
            {"messages": [{"role": "user"}]},
            {"messages": [{"role": "user", "content": 3}]},
            {"messages": [{"role": "user", "content": "hi"}], "max_tokens": 0},
            {"messages": [{"role": "user", "content": "hi"}], "max_tokens": True},
            {"messages": [{"role": "user", "content": "hi"}], "max_tokens": "64"},
            {"messages": [{"role": "user", "content": "hi"}], "temperature": -0.1},
            {"messages": [{"role": "user", "content": "hi"}], "temperature": 3.0},
        ]
        for request in cases:
            with self.assertRaises(worker.ChatRequestError):
                worker.validate_chat_request(request)


class PromptBuildingTests(unittest.TestCase):
    def test_applies_chat_template(self):
        tokenizer = FakeTokenizer()
        messages = [{"role": "user", "content": "hi"}]
        tokens = worker.build_prompt_tokens(tokenizer, messages)
        self.assertEqual(tokens, [101, 7, 9])
        self.assertEqual(tokenizer.calls, (messages, True))

    def test_falls_back_to_encode_when_template_breaks(self):
        class BrokenTemplate:
            def apply_chat_template(self, *_args, **_kwargs):
                raise RuntimeError("no template")

            def encode(self, text):
                return [len(text)]

        tokens = worker.build_prompt_tokens(BrokenTemplate(), [{"role": "user", "content": "hi"}])
        self.assertEqual(tokens, [len("user: hi\n")])

    def test_ignores_empty_template_result(self):
        class EmptyTemplate:
            def apply_chat_template(self, *_args, **_kwargs):
                return []

            def encode(self, text):
                return [1, 2]

        self.assertEqual(
            worker.build_prompt_tokens(EmptyTemplate(), [{"role": "user", "content": "hi"}]), [1, 2]
        )


class FakeMlxLm:
    """serve() 内部延迟导入 mlx_lm；这里用 sys.modules 注入 fake 控制生成行为。"""

    def __init__(self, responses):
        self.responses = responses

    def stream_generate(self, model, tokenizer, *, prompt, max_tokens, sampler):
        self.observed = {"prompt": list(prompt), "max_tokens": max_tokens, "sampler": sampler}
        return iter(self.responses)


class FakeSampleUtils:
    def make_sampler(self, temp):
        return {"temp": temp}


class Response:
    def __init__(self, text, generation_tokens, finish_reason=None):
        self.text = text
        self.generation_tokens = generation_tokens
        self.finish_reason = finish_reason


class ServeProtocolTests(unittest.TestCase):
    """daemon 按 delta 帧 -> 终帧的顺序驱动 SSE 流，并按错误码分流
    invalid_request / inference_error；这里固化 serve() 的帧序列契约。"""

    def run_serve(self, lines, loaded, fake_mlx):
        stdin = io.StringIO("".join(line + "\n" for line in lines))
        stdout = io.StringIO()
        with mock.patch.dict(
            sys.modules,
            {"mlx_lm": fake_mlx, "mlx_lm.sample_utils": FakeSampleUtils()},
        ), mock.patch.object(sys, "stdin", stdin), mock.patch.object(sys, "stdout", stdout):
            worker.serve(loaded)
        return [json.loads(line) for line in stdout.getvalue().splitlines()]

    def test_success_streams_deltas_then_final_frame(self):
        fake_mlx = FakeMlxLm([Response("Hel", 1), Response("lo", 2)])
        loaded = worker.LoadedModel(model=object(), tokenizer=FakeTokenizer(), device="metal")
        request = json.dumps(
            {"id": 5, "messages": [{"role": "user", "content": "hi"}], "max_tokens": 8, "temperature": 0.0}
        )
        frames = self.run_serve([request], loaded, fake_mlx)

        self.assertEqual(len(frames), 3)
        self.assertEqual(frames[0], {"id": 5, "delta": "Hel"})
        self.assertEqual(frames[1], {"id": 5, "delta": "lo"})
        self.assertTrue(frames[2]["ok"])
        self.assertEqual(frames[2]["text"], "Hello")
        self.assertEqual(frames[2]["prompt_tokens"], 3)
        self.assertEqual(frames[2]["tokens"], 2)
        self.assertEqual(frames[2]["device"], "metal")
        # fake 响应未携带 finish_reason 时按自然结束处理。
        self.assertEqual(frames[2]["finish_reason"], "stop")
        self.assertEqual(fake_mlx.observed, {"prompt": [101, 7, 9], "max_tokens": 8, "sampler": {"temp": 0.0}})

    def test_max_tokens_truncation_reports_length_finish_reason(self):
        fake_mlx = FakeMlxLm([Response("蓝", 1, finish_reason="length")])
        loaded = worker.LoadedModel(model=object(), tokenizer=FakeTokenizer(), device="metal")
        request = json.dumps({"id": 6, "messages": [{"role": "user", "content": "hi"}], "max_tokens": 1})
        frames = self.run_serve([request], loaded, fake_mlx)

        self.assertEqual(len(frames), 2)
        self.assertEqual(frames[0], {"id": 6, "delta": "蓝"})
        self.assertTrue(frames[1]["ok"])
        self.assertEqual(frames[1]["finish_reason"], "length")

    def test_error_frames_use_documented_codes_and_keep_worker_alive(self):
        fake_mlx = FakeMlxLm([])
        loaded = worker.LoadedModel(model=object(), tokenizer=FakeTokenizer())
        frames = self.run_serve(
            [
                "not json",
                '{"id": "x"}',
                '{"id": 1}',
                '{"id": 2, "messages": [{"role": "user", "content": "hi"}]}',
            ],
            loaded,
            fake_mlx,
        )
        self.assertIsNone(frames[0]["id"])
        self.assertEqual(frames[0]["error"]["code"], "invalid_request")
        self.assertEqual(frames[1]["error"]["code"], "invalid_request")
        self.assertEqual(frames[2]["error"]["code"], "invalid_request")
        # 空生成属于 inference_error 而非 invalid_request，且 worker 继续存活。
        self.assertEqual(frames[3]["error"]["code"], "inference_error")

    def test_generation_failure_reports_error_after_partial_deltas(self):
        class Exploding:
            def stream_generate(self, *_args, **_kwargs):
                yield Response("par", 1)
                raise RuntimeError("metal OOM")

        loaded = worker.LoadedModel(model=object(), tokenizer=FakeTokenizer())
        request = json.dumps({"id": 9, "messages": [{"role": "user", "content": "hi"}]})
        frames = self.run_serve([request], loaded, Exploding())

        self.assertEqual(len(frames), 2)
        self.assertEqual(frames[0], {"id": 9, "delta": "par"})
        self.assertFalse(frames[1]["ok"])
        self.assertEqual(frames[1]["error"]["code"], "inference_error")


if __name__ == "__main__":
    unittest.main()
