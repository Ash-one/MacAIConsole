import importlib.util
import sys
import unittest
from pathlib import Path

WORKER_PATH = Path(__file__).resolve().parents[1] / "kokoro_worker.py"
SPEC = importlib.util.spec_from_file_location("kokoro_worker", WORKER_PATH)
assert SPEC is not None and SPEC.loader is not None
kokoro_worker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = kokoro_worker
SPEC.loader.exec_module(kokoro_worker)

split_long_text = kokoro_worker._split_long_text_for_kokoro


class LongTextSplitTests(unittest.TestCase):
    """mlx-audio 中文管线会对超长行做音素硬截断（表现为 ~31s 截断），
    worker 先把文本切成 <=150 字符的短行是规避该缺陷的核心步骤。"""

    def test_short_empty_and_unsplittable_inputs_pass_through(self):
        self.assertEqual(split_long_text("你好"), "你好")
        self.assertEqual(split_long_text(""), "")
        self.assertEqual(split_long_text(None), "")
        self.assertEqual(split_long_text("   "), "")
        # max_chars <= 0 时按约定不切分。
        self.assertEqual(split_long_text("一" * 500, max_chars=0), "一" * 500)

    def test_splits_on_cjk_and_latin_punctuation_without_losing_content(self):
        text = "第一句。第二句！第三句？第四句；第五句，第六句最后一点.End."
        result = split_long_text(text, max_chars=10)
        self.assertTrue(result)
        for line in result.splitlines():
            self.assertLessEqual(len(line), 10)
        # 断句只插入换行、不丢字符：去掉换行后必须与原文一致。
        self.assertEqual(result.replace("\n", ""), text)

    def test_short_text_is_not_split(self):
        text = "一。二。三。"
        self.assertEqual(split_long_text(text, max_chars=150), text)

    def test_overlong_sentence_without_punctuation_is_hard_split(self):
        text = "汉" * 400
        result = split_long_text(text, max_chars=150)
        lines = result.splitlines()
        self.assertEqual([len(line) for line in lines], [150, 150, 100])
        self.assertEqual(result.replace("\n", ""), text)

    def test_units_merge_greedily_up_to_the_limit(self):
        # 60 个 3 字符短句（180 字符）贪心合并成 150 + 30 两行。
        text = "一句。" * 60
        result = split_long_text(text, max_chars=150)
        lines = result.splitlines()
        self.assertEqual(len(lines), 2)
        self.assertEqual(len(lines[0]), 150)
        self.assertEqual(len(lines[1]), 30)
        self.assertEqual(result.replace("\n", ""), text)


if __name__ == "__main__":
    unittest.main()
