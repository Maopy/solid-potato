"""Exercise generated proxy logging without a proxy or any network requests."""
import contextlib
import io
import os
import sys
import types

source = sys.stdin.read()
class Response:
    def __init__(self, status=200, body=b'', headers=None):
        self.status_code = status
        self.text = body.decode()
        self.headers = headers or {}
    make = staticmethod(lambda *args: Response(*args))
    def get_text(self, strict=False):
        return self.text
    def set_text(self, text):
        self.text = text

sys.modules['mitmproxy'] = types.SimpleNamespace(
    http=types.SimpleNamespace(HTTPFlow=object, Response=Response))

def flow(path, html=False):
    return types.SimpleNamespace(
        request=types.SimpleNamespace(host='channels.weixin.qq.com', path=path,
            method='GET', get_text=lambda strict=False: ''),
        response=Response(body=b'<html><head></head></html>',
            headers={'content-type': 'text/html' if html else 'application/json'}))

for verbose in ('', '1'):
    os.environ['LIVE_ARCHIVE_VERBOSE'] = verbose
    addon = {}
    exec(source, addon)
    addon['forward_to_loopback'] = lambda *args: (200, b'{}')
    logs = io.StringIO()
    with contextlib.redirect_stdout(logs):
        addon['request'](flow('/api/v1/download_task/list'))
        page = flow('/web/pages/live', html=True)
        page.response.headers.update({'cache-control': 'max-age=86400', 'etag': 'old', 'last-modified': 'old'})
        addon['response'](page)
    assert '/__assets/inject/channels.live.js' in page.response.text
    assert page.response.headers['cache-control'] == 'no-store'
    assert 'etag' not in page.response.headers and 'last-modified' not in page.response.headers
    assert bool(logs.getvalue()) == bool(verbose), logs.getvalue()
    probe = flow('/api/status?download_service_probe=123', html=True)
    original = probe.response.text
    with contextlib.redirect_stdout(logs):
        addon['response'](probe)
    assert probe.response.text == original
    assert 'cache-control' not in probe.response.headers
    assert '/api/status' not in logs.getvalue()
    logs = io.StringIO()
    addon['forward_to_loopback'] = lambda *args: (502, b'{}')
    with contextlib.redirect_stdout(logs):
        addon['request'](flow('/api/v1/download_task/list'))
    assert 'bridge returned 502' in logs.getvalue()
