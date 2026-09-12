"""engine 校验语义单测（不需要 mlx-lm，只锁协议校验与降级逻辑）。"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from macai_mlx_lm_runner.engine import (  # noqa: E402
    ChatRequestError,
    ThinkingStreamParser,
    build_prompt_tokens,
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

    messages, *_ = validate_chat_request(
        {
            "messages": [
                {
                    "role": "assistant",
                    "content": "answer",
                    "reasoning_content": "why",
                }
            ]
        }
    )
    assert messages[0]["reasoning_content"] == "why"


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
    tokenizer = FakeTokenizer(template_fails=True)
    prompt = build_prompt_tokens(
        tokenizer, [{"role": "user", "content": "hello"}]
    )
    assert tokenizer.encoded == ["user: hello\n"]
    assert prompt == [9, 9]


def test_thinking_template_is_enabled_explicitly():
    class ThinkingTokenizer(FakeTokenizer):
        chat_template = "{% if enable_thinking %}<think>{% endif %}"

        def apply_chat_template(self, messages, add_generation_prompt=True, **kwargs):
            assert kwargs == {"enable_thinking": True}
            return [1, 2, 3]

    assert build_prompt_tokens(ThinkingTokenizer(), [{"role": "user", "content": "hi"}]) == [1, 2, 3]


def test_thinking_parser_handles_prefill_split_tag_and_truncation():
    parser = ThinkingStreamParser(enabled=True)
    assert parser.feed("reasoning</thi") == ("reasoning", "")
    assert parser.feed("nk>\n\nanswer") == ("", "answer")
    assert parser.feed(" continues") == ("", " continues")
    assert parser.finish() == ("", "")

    truncated = ThinkingStreamParser(enabled=True)
    assert truncated.feed("unfinished</thi") == ("unfinished", "")
    assert truncated.finish() == ("</thi", "")

    split_whitespace = ThinkingStreamParser(enabled=True)
    assert split_whitespace.feed("why</think>") == ("why", "")
    assert split_whitespace.feed("\n\n") == ("", "")
    assert split_whitespace.feed("answer") == ("", "answer")


def test_non_thinking_parser_is_plain_content_passthrough():
    parser = ThinkingStreamParser(enabled=False)
    assert parser.feed("plain answer") == ("", "plain answer")
