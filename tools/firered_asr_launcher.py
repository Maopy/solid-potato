"""Isolate native model crashes and retain a bounded in-memory diagnostic tail."""
import collections
import json
import os
from pathlib import Path
import subprocess
import sys
import threading


def main():
    base = Path(__file__).resolve().parent
    venv = base / 'firered-venv'
    config = dict(line.split('=', 1) for line in (venv / 'pyvenv.cfg').read_text(encoding='utf-8').splitlines() if '=' in line)
    home = next(value.strip().strip('"') for key, value in config.items() if key.strip() == 'home')
    python = Path(home) / 'python.exe'
    worker = base / 'firered_asr_worker.py'
    # Bypass the Windows venv redirector. Start the real interpreter and load
    # our isolated site, independently of inherited Python launcher settings.
    bootstrap = 'import sys,site,runpy;site.addsitedir(sys.argv.pop(1));runpy.run_path(sys.argv.pop(1),run_name="__main__")'
    env = os.environ.copy()
    for name in list(env):
        if name.upper().startswith('PYTHON') or name.upper() in {
            '__PYVENV_LAUNCHER__', 'VIRTUAL_ENV', '_PYTHON_PROJECT_BASE',
        }:
            env.pop(name)
    env.update(PYTHONUTF8='1', PYTHONNOUSERSITE='1', HF_HUB_OFFLINE='1')
    tail = collections.deque(maxlen=16384)
    process = subprocess.Popen([str(python), '-u', '-c', bootstrap,
        str(venv / 'Lib/site-packages'), str(worker), *sys.argv[1:]],
        stdin=sys.stdin.buffer, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)

    def drain():
        while chunk := process.stderr.read(1024):
            tail.extend(chunk)

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    while chunk := process.stdout.read1(65536):
        sys.stdout.buffer.write(chunk)
        sys.stdout.buffer.flush()
    code = process.wait()
    reader.join(timeout=2)
    if code:
        detail = bytes(tail).decode('utf-8', errors='replace')[-3000:]
        message = f'FireRed process exited ({code}): {detail}'
        print(json.dumps({'error': message}, ensure_ascii=True), flush=True)
    return code


if __name__ == '__main__':
    try:
        code = main()
    except Exception as error:
        print(json.dumps({'error': f'FireRed launcher failed: {error}'}, ensure_ascii=True), flush=True)
        code = 1
    raise SystemExit(code)
