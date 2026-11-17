import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('funasr_worker', Path(__file__).resolve().parents[1] / 'tools/funasr_asr_worker.py')
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)

class HotwordsTest(unittest.TestCase):
    def test_editing_file_is_reflected_on_next_read(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'hotwords.txt'
            self.assertIsNone(worker.read_hotwords(path))
            path.write_text('# comment\n可可\n品牌名\n', encoding='utf-8-sig')
            self.assertEqual(worker.read_hotwords(path), '可可 品牌名')
            path.write_text('新词\n', encoding='utf-8')
            self.assertEqual(worker.read_hotwords(path), '新词')
            path.write_text('不 合规\n', encoding='utf-8')
            with self.assertRaises(ValueError):
                worker.read_hotwords(path)

if __name__ == '__main__':
    unittest.main()
