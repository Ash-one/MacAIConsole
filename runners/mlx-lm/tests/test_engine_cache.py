"""prompt cache 复用语义单测：假 stream_generate 让每轮实际 prefill 的
token 数可观测，断言增量 prefill、退化重建与失败重置行为。不依赖 mlx。"""

from __future__ import annotations

import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import macai_mlx_lm_runner.engine as engine_module
from macai_mlx_lm_runner.engine import LoadedModel, MlxLmEngine

GENERATED_TOKENS = [11, 12, 13]


class PromptTokenizer:
    """apply_chat_template 按最后一条消息 content 查表返回确定性的 token 序列。"""

    def __init__(self, prompts: dict[str, list[int]]) -> None:
        self.prompts = prompts

    def apply_chat_template(self, messages, add_generation_prompt=True, **kwargs):
        return list(self.prompts[messages[-1]["content"]])


class FakeCache:
    def __init__(self) -> None:
        self.trims: list[int] = []


class FakeEngine(MlxLmEngine):
    """绕过 mlx 加载；_load 保持与真实实现相同的 cache 初始化契约。"""

    def __init__(self, prompts: dict[str, list[int]]) -> None:
        self.model_root = "fake"
        self._loaded = LoadedModel(model=object(), tokenizer=PromptTokenizer(prompts))
        self._prompt_cache = None
        self._cached_tokens = []
        self._load()

    def _load(self) -> None:
        self._prompt_cache = engine_module._make_prompt_cache(self._loaded.model)
        self._cached_tokens = []


@pytest.fixture
def fake_mlx(monkeypatch: pytest.MonkeyPatch):
    calls: list[dict] = []
    caches: list[FakeCache] = []
    behavior: dict = {"generate": None, "trim": None}

    def fake_make_prompt_cache(model):
        cache = FakeCache()
        caches.append(cache)
        return cache

    def fake_trim_prompt_cache(cache, num_tokens):
        cache.trims.append(num_tokens)
        override = behavior["trim"]
        return num_tokens if override is None else override(cache, num_tokens)

    def fake_stream_generate(model, tokenizer, prompt, **kwargs):
        calls.append(
            {
                "prompt": list(prompt),
                "cache": kwargs.get("prompt_cache"),
                "sampler": kwargs.get("sampler"),
                "max_tokens": kwargs.get("max_tokens"),
            }
        )
        return behavior["generate"](list(prompt))

    monkeypatch.setattr(engine_module, "_make_prompt_cache", fake_make_prompt_cache)
    monkeypatch.setattr(engine_module, "_trim_prompt_cache", fake_trim_prompt_cache)
    monkeypatch.setattr(engine_module, "_stream_generate", fake_stream_generate)
    monkeypatch.setattr(engine_module, "_make_sampler", lambda temperature, top_p: (temperature, top_p))
    return SimpleNamespace(calls=calls, caches=caches, behavior=behavior)


def generation(tokens: list[int], fail_before_last: bool = False):
    """模拟 stream_generate：逐 token yield，末帧携带最后一个 token 与 finish_reason。"""

    def generate(prompt: list[int]):
        def iterate():
            for token in tokens[:-1]:
                yield SimpleNamespace(token=token, text="x", finish_reason=None)
            if fail_before_last:
                raise RuntimeError("generation failed mid-stream")
            yield SimpleNamespace(token=tokens[-1], text="", finish_reason="stop")

        return iterate()

    return generate


def drain(engine: MlxLmEngine, content: str) -> list[int]:
    """走真实 next_response 消费路径跑完一轮，返回各帧 token。"""
    responses, _prompt, *_rest = engine.stream_chat(
        {"messages": [{"role": "user", "content": content}]}
    )
    tokens = []
    while True:
        response = engine_module.next_response(responses)
        if response is None:
            return tokens
        tokens.append(response.token)


def test_first_turn_uses_loaded_cache_and_full_prefill(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5]})

    tokens = drain(engine, "hi")

    assert tokens == GENERATED_TOKENS
    assert len(fake_mlx.caches) == 1, "首轮必须复用 _load 建的 cache"
    assert fake_mlx.calls[0]["prompt"] == [1, 2, 3, 4, 5]
    assert fake_mlx.calls[0]["cache"] is fake_mlx.caches[0]
    assert engine._cached_tokens == [1, 2, 3, 4, 5, *GENERATED_TOKENS]


def test_second_turn_prefills_only_new_suffix(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5], "hi again": [1, 2, 3, 4, 5, 6, 7]})
    drain(engine, "hi")

    drain(engine, "hi again")

    assert fake_mlx.calls[1]["prompt"] == [6, 7], "第二轮只 prefill 分歧尾部"
    assert fake_mlx.calls[1]["cache"] is fake_mlx.caches[0], "同一对话延续同一 cache"
    assert fake_mlx.caches[0].trims == [3], "裁掉上一轮生成的 3 个 token"
    assert len(fake_mlx.caches) == 1, "cache 命中时不得重建"
    assert engine._cached_tokens == [1, 2, 3, 4, 5, 6, 7, *GENERATED_TOKENS]


def test_prompt_covered_by_cache_keeps_one_prefill_token(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5], "retry": [1, 2, 3, 4, 5]})
    drain(engine, "hi")

    drain(engine, "retry")

    # 新 prompt 完全落在已缓存序列内：至少留 1 个 token 做 prefill。
    assert fake_mlx.calls[1]["prompt"] == [5]
    assert fake_mlx.caches[0].trims == [4]


def test_unrelated_history_rebuilds_cache_and_full_prefills(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5], "other": [9, 9, 9]})
    drain(engine, "hi")
    first_cache = fake_mlx.caches[0]

    drain(engine, "other")

    assert fake_mlx.calls[1]["prompt"] == [9, 9, 9], "公共前缀为 0 时全量 prefill"
    assert fake_mlx.calls[1]["cache"] is not first_cache
    assert first_cache.trims == [], "退化路径不得触碰旧 cache"
    assert len(fake_mlx.caches) == 2


def test_trim_exception_degrades_and_worker_survives(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5], "hi again": [1, 2, 3, 4, 5, 6, 7]})
    drain(engine, "hi")
    fake_mlx.behavior["trim"] = lambda cache, num_tokens: (_ for _ in ()).throw(
        RuntimeError("trim exploded")
    )

    drain(engine, "hi again")

    assert fake_mlx.calls[1]["prompt"] == [1, 2, 3, 4, 5, 6, 7]
    assert len(fake_mlx.caches) == 2, "trim 异常时重建 cache"
    # worker 存活：恢复 trim 后下一轮回到增量路径。
    fake_mlx.behavior["trim"] = None
    drain(engine, "hi")
    assert fake_mlx.calls[2]["prompt"] == [5]


def test_trim_shortfall_rebuilds_cache(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5], "hi again": [1, 2, 3, 4, 5, 6, 7]})
    drain(engine, "hi")
    fake_mlx.behavior["trim"] = lambda cache, num_tokens: num_tokens - 1

    drain(engine, "hi again")

    assert fake_mlx.calls[1]["prompt"] == [1, 2, 3, 4, 5, 6, 7]
    assert len(fake_mlx.caches) == 2, "实际裁剪数与请求不符时 cache 不可信"


def test_generation_failure_resets_cache_to_full_prefill(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5], "hi again": [1, 2, 3, 4, 5, 6, 7]})
    drain(engine, "hi")
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS, fail_before_last=True)

    with pytest.raises(RuntimeError, match="generation failed"):
        drain(engine, "hi again")

    assert fake_mlx.calls[1]["prompt"] == [6, 7], "失败发生在增量 prefill 之后的迭代中"
    assert engine._prompt_cache is None, "生成失败后不信任 cache 状态"
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    drain(engine, "hi again")
    assert fake_mlx.calls[2]["prompt"] == [1, 2, 3, 4, 5, 6, 7]
    assert len(fake_mlx.caches) == 2


def test_forwarded_generation_arguments(fake_mlx):
    fake_mlx.behavior["generate"] = generation(GENERATED_TOKENS)
    engine = FakeEngine({"hi": [1, 2, 3, 4, 5]})

    drain(engine, "hi")

    call = fake_mlx.calls[0]
    assert call["sampler"] == (0.7, 1.0), "默认温度/top_p 必须仍传给采样器"
    assert call["max_tokens"] == 1024
