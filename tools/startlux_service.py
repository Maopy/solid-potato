"""Start the isolated, loopback-only official StartLux server for local scoring."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys


def tools_directory():
    here = Path(__file__).resolve().parent
    for candidate in (here, here.parent / 'dist/.live-archive-tools', here.parent.parent / '.live-archive-tools'):
        if (candidate / 'StartLux-Decision').is_dir():
            return candidate
    raise RuntimeError('StartLux local runtime is not installed')


def main():
    base = tools_directory()
    config = json.loads((base / 'startlux-runtime.json').read_text(encoding='utf-8-sig'))
    if '--serve' not in sys.argv:
        with socket.socket() as probe:
            if probe.connect_ex(('127.0.0.1', config['port'])) == 0:
                return
        env = {k: v for k, v in os.environ.items() if not k.upper().startswith('PYTHON') and k.upper() not in
               {'VIRTUAL_ENV', '__PYVENV_LAUNCHER__', '_PYTHON_PROJECT_BASE'}}
        env.update(PYTHONUTF8='1', PYTHONNOUSERSITE='1', HF_HUB_OFFLINE='1', TRANSFORMERS_OFFLINE='1', USE_TF='0')
        with (base / 'startlux.log').open('ab') as log:
            subprocess.Popen([str(base / 'python312/python.exe'), str(Path(__file__).resolve()), '--serve'],
                             cwd=base, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                             creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        return
    # Keep the lock until the server exits, including the time spent loading weights.
    lock = (base / 'startlux-server.lock').open('a+b')
    lock.seek(0)
    if os.name == 'nt':
        import msvcrt
        try:
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            return
    import site
    site.addsitedir(str(base / 'startlux-venv/Lib/site-packages'))
    sys.path.insert(0, str(base / 'StartLux-Decision'))
    import torch
    if not torch.cuda.is_available():
        raise RuntimeError('StartLux CUDA runtime is unavailable')
    torch.set_num_threads(4)
    if os.name == 'nt':
        # Windows PyTorch has no fused Flash Attention. Keep the same SDPA math,
        # including GQA and non-causal attention, at a bounded short context.
        from contextlib import nullcontext
        from startlux_decide.backends import bidirectional
        bidirectional._kernel_guard = lambda device: nullcontext()
    from startlux_decide.cli import main as serve
    serve(['serve', '--model', config['model'], '--backend', 'transformers', '--device', 'cuda',
           '--host', '127.0.0.1', '--port', str(config['port']), '--max-model-len', '2048',
           '--api-keys-file', config['api_keys_file'], '--max-inflight', '2', '--no-images'])


if __name__ == '__main__':
    main()
