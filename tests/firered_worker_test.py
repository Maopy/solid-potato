import json
import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

WORKER = Path(__file__).resolve().parents[1] / 'tools/firered_asr_worker.py'


class WorkerTransportTests(unittest.TestCase):
    def test_native_process_failure_returns_exit_code_and_bounded_stderr(self):
        spec = importlib.util.spec_from_file_location('launcher', WORKER.with_name('firered_asr_launcher.py'))
        launcher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(launcher)
        process = type('Process', (), {'stdout': io.BytesIO(),
            'stderr': io.BytesIO(b'x' * 40000 + b' native failure'),
            'wait': lambda self: 3221225477})()
        buffer = io.BytesIO()
        output = io.TextIOWrapper(buffer, encoding='utf-8')
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            (base / 'firered-venv').mkdir()
            (base / 'firered-venv/pyvenv.cfg').write_text('home = "C:/isolated-python"\n')
            with patch.object(launcher, '__file__', str(base / 'launcher.py')), patch.object(launcher.os, 'environ', {'PYTHONHOME': '"bad"', '__PYVENV_LAUNCHER__': '"bad"', 'PATH': 'keep'}), patch.object(launcher.subprocess, 'Popen', return_value=process) as spawn, patch.object(sys, 'stdout', output):
                self.assertEqual(launcher.main(), 3221225477)
            args, kwargs = spawn.call_args
            self.assertEqual(args[0][0], str(Path('C:/isolated-python') / 'python.exe'))
            self.assertNotIn('PYTHONHOME', kwargs['env'])
            self.assertNotIn('__PYVENV_LAUNCHER__', kwargs['env'])
            self.assertEqual(kwargs['env']['PATH'], 'keep')
        output.flush()
        message = json.loads(buffer.getvalue())['error']
        self.assertIn('3221225477', message)
        self.assertTrue(message.endswith(' native failure'))
        self.assertLess(len(message), 3100)

    def invoke(self, failure=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'torch.py').write_text('def set_num_threads(n): pass\n')
            package = root / 'fireredasr2s'
            package.mkdir()
            (package / '__init__.py').write_text('')
            (package / 'fireredasr2.py').write_text('''
class FireRedAsr2Config:
    def __init__(self, **kwargs): pass
class FireRedAsr2:
    @staticmethod
    def from_pretrained(*args):
        print('upstream startup log')
        FAILURE
        return FireRedAsr2()
    def transcribe(self, *args):
        print('upstream inference log')
        return [{'text': '\u6d4b\u8bd5'}]
'''.replace('FAILURE', "raise RuntimeError('model load failed')" if failure else 'pass'), encoding='utf-8')
            result = subprocess.run([sys.executable, str(WORKER), '--source', directory,
                '--model', directory], input='{"wav":"test.wav"}\n' * 2,
                capture_output=True, text=True, encoding='utf-8', timeout=15)
            return result, [json.loads(line) for line in result.stdout.splitlines()]

    def test_startup_failure_reaches_the_panel_as_json(self):
        result, messages = self.invoke(failure=True)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(messages, [{'error': 'model load failed'}])
        self.assertIn('RuntimeError: model load failed', result.stderr)

    def test_logs_cannot_corrupt_utf8_protocol_and_requests_reuse_model(self):
        result, messages = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(messages, [{'ready': True, 'gpu': False}, {'text': '测试'}, {'text': '测试'}])
        self.assertEqual(result.stderr.count('upstream startup log'), 1)


if __name__ == '__main__':
    unittest.main()
