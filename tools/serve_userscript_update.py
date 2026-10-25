"""Offer the current bundled userscript to this machine's Tampermonkey."""
import argparse
import http.server
import json
import re
from pathlib import Path
import secrets
import time

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--key-file', type=Path, required=True)
    parser.add_argument('--backend', default='http://127.0.0.1:2025')
    parser.add_argument('--port', type=int, default=2027)
    args = parser.parse_args()
    key = args.key_file.read_text(encoding='utf-8').strip()
    assets = ROOT / 'assets/moderation'
    script = (assets / 'userscript/douyin.user.js').read_text(encoding='utf-8')
    script = script.replace('__BASE_JSON__', json.dumps(args.backend)).replace('__KEY_JSON__', json.dumps(key))
    files = dict(SDK='sdk/probe', RAW='sdk/raw', SUBMIT='sdk/submit',
        DECODER='adapters/douyin-decoder', ADAPTER='adapters/douyin-live',
        FILL='adapters/douyin-fill', RECORDING='adapters/douyin-recording')
    for marker, path in files.items():
        script = script.replace('/*__' + marker + '__*/', (assets / (path + '.js')).read_text(encoding='utf-8'))
    panel = '\n'.join((assets / ('sdk/' + name + '.js')).read_text(encoding='utf-8') for name in ('speech', 'likes', 'panel'))
    body = script.replace('/*__PANEL__*/', panel).encode('utf-8')
    version = re.search(r'^// @version\s+(\S+)', script, re.MULTILINE).group(1)
    route = '/install/' + secrets.token_hex(16) + f'/live-archive-douyin-{version}.user.js'

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != route or self.headers.get('Host') != f'127.0.0.1:{args.port}':
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header('Content-Type', 'text/javascript; charset=utf-8')
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    server = http.server.HTTPServer(('127.0.0.1', args.port), Handler)
    server.timeout = 1
    print(f'http://127.0.0.1:{args.port}{route}', flush=True)
    deadline = time.monotonic() + 3600
    while time.monotonic() < deadline:
        server.handle_request()
    server.server_close()


if __name__ == '__main__':
    main()
