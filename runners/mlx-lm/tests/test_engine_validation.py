"""engine 校验语义单测（不需要 mlx-lm，只锁协议校验与降级逻辑）。"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from macai_mlx_lm_runner.engine import (  # noqa: E402
    ChatRequestError,
    validate_chat_request,
)


class FakeTokenizer:
    """chat template 可注入失败，encode 记录输入。"""

    def __init__(self, template_fails: bool = False) -> None:
        self.template_fails = template_fails
        self.encoded: list[str] = []

    def apply_chat_template(self, messages, add_generation_prompt=True):
        if self.template_fails:
            raise RuntimeError("template broken")
        return [1, 2, 3]

    def encode(self, text):
        self.encoded.append(text)
        return [9, 9]


def test_valid_request_applies_defaults():
    request = {"messages": [{"role": "user", "content": "hi"}]}
    messages, max_tokens, temperature, top_p = validate_chat_request(request)
    assert messages == [{"role": "user", "content": "hi"}]
    assert max_tokens == 1024
    assert temperature == 0.7
    assert top_p == 1.0


def test_temperature_zero_is_greedy_and_boundary_is_two():
    request = {
        "messages": [{"role": "user", "content": "hi"}],
        "temperature": 0,
        "max_tokens": 16,
    }
    _, max_tokens, temperature, top_p = validate_chat_request(request)
    assert (max_tokens, temperature) == (16, 0.0)
    assert top_p == 1.0
    with pytest.raises(ChatRequestError):
        validate_chat_request({**request, "temperature": 2.1})
    with pytest.raises(ChatRequestError):
        validate_chat_request({**request, "temperature": -0.1})


def test_top_p_is_validated_and_returned():
    request = {
        "messages": [{"role": "user", "content": "hi"}],
        "top_p": 0.95,
    }
    *_, top_p = validate_chat_request(request)
    assert top_p == 0.95
    with pytest.raises(ChatRequestError):
        validate_chat_request({**request, "top_p": 1.1})


def test_invalid_messages_raise():
    with pytest.raises(ChatRequestError):
        validate_chat_request({"messages": []})
    with pytest.raises(ChatRequestError):
        validate_chat_request({"messages": ["not-a-dict"]})
    with pytest.raises(ChatRequestError):
        validate_chat_request({"messages": [{"role": "", "content": "x"}]})
    with pytest.raises(ChatRequestError):
        validate_chat_request({"messages": [{"role": "user", "content": 1}]})
    with pytest.raises(ChatRequestError):
        validate_chat_request({"messages": [{"role": "user", "content": "x"}], "max_tokens": 0})
    with pytest.raises(ChatRequestError):
        validate_chat_request(
            {"messages": [{"role": "user", "content": "x"}], "max_tokens": True}
        )


def test_template_failure_degrades_to_plain_join():
    from macai_mlx_lm_runner.engine import build_prompt_tokens

    tokenizer = FakeTokenizer(template_fails=True)
    prompt = build_prompt_tokens(
        tokenizer, [{"role": "user", "content": "hello"}]
    )
    assert tokenizer.encoded == ["user: hello\n"]
    assert prompt == [9, 9]
