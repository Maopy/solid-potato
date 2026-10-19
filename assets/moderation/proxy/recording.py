"""Compose recording and moderation in separate namespaces on one proxy."""
import inspect
import json
import re
import urllib.parse
from html import escape
from mitmproxy import http

moderation = {'__name__': 'integrated_moderation'}
capture = {'__name__': 'integrated_capture'}
exec(compile(__MODERATION_SOURCE__, '<moderation>', 'exec'), moderation)
exec(compile(__CAPTURE_SOURCE__, '<recording>', 'exec'), capture)
ROOMS = json.loads(__ROOMS__)
DOUYIN_SCRIPT = __DOUYIN_SCRIPT__

def room_allowed(url):
    p = urllib.parse.urlsplit(url)
    return any(p.scheme == 'https' and p.netloc == urllib.parse.urlsplit(r).netloc and p.path.rstrip('/') == urllib.parse.urlsplit(r).path.rstrip('/') for r in ROOMS)

def local_recording(flow):
    path = flow.request.path.split('?', 1)[0]
    return path.startswith(('/api/v1/download_task/', '/__assets/', '/__live_archive/capture', '/__live_archive/live-info', '/api/channels/live/profile', '/__live_archive/recording.js'))

async def request(flow):
    if not local_recording(flow):
        await moderation['request'](flow)
        return
    host = flow.request.host
    if host == 'channels.weixin.qq.com' and any(urllib.parse.urlsplit(r).hostname == host for r in ROOMS):
        capture['request'](flow)
        return
    if host != 'live.douyin.com' or not room_allowed(flow.request.headers.get('referer', '')):
        flow.response = http.Response.make(403, b'{"code":403}', {'Content-Type':'application/json'})
        return
    path = flow.request.path.split('?', 1)[0]
    if path == '/__live_archive/recording.js' and flow.request.method == 'GET':
        flow.response = http.Response.make(200, DOUYIN_SCRIPT.encode(), {'Content-Type':'application/javascript','Cache-Control':'no-store'})
    elif path.startswith('/api/v1/download_task/') and flow.request.method in ('GET','POST'):
        if len(flow.request.raw_content or b'') > 1024 * 1024:
            flow.response=http.Response.make(413,b'{}');return
        try:
            status, body = capture['forward_to_loopback'](flow.request.path,flow.request.get_text(strict=False),flow.request.method)
            flow.response=http.Response.make(status,body,{'Content-Type':'application/json','Cache-Control':'no-store'})
        except Exception:
            flow.response=http.Response.make(502,b'{"code":502}',{'Content-Type':'application/json'})
    else:
        flow.response=http.Response.make(404,b'{}')

def requestheaders(flow):
    if local_recording(flow):
        flow.request.stream = False
        return
    moderation['requestheaders'](flow)
    if flow.request.host in ('channels.weixin.qq.com','res.wx.qq.com','finder.video.qq.com'):
        flow.request.headers.pop('if-none-match',None)
        flow.request.headers.pop('if-modified-since',None)

def responseheaders(flow):
    if local_recording(flow):
        flow.response.stream=False;return
    moderation['responseheaders'](flow)
    content=flow.response.headers.get('content-type','').lower()
    if flow.request.host in ('channels.weixin.qq.com','res.wx.qq.com','finder.video.qq.com') and ('javascript' in content or 'ecmascript' in content or 'text/html' in content or '.js' in flow.request.path.split('?',1)[0]):
        flow.response.stream=False

def response(flow):
    if local_recording(flow) or not flow.response or flow.response.stream:return
    if any(urllib.parse.urlsplit(r).hostname == 'channels.weixin.qq.com' for r in ROOMS):
        original=flow.response.get_text(strict=False) if 'text/html' in flow.response.headers.get('content-type','').lower() else ''
        nonce_match=re.search(r'<script\b[^>]*\bnonce=["\']([a-zA-Z0-9+/_=-]{1,256})["\']',original,re.I)
        capture['response'](flow)
        if nonce_match and original:
            nonce=' nonce="'+escape(nonce_match.group(1),quote=True)+'"'
            updated=flow.response.get_text(strict=False)
            updated=re.sub(r'<script(?![^>]*\bnonce=)([^>]*src=["\']/__assets/[^>]*>)',lambda m:'<script'+nonce+m.group(1),updated)
            updated=updated.replace('<script>window.__d_config=', '<script'+nonce+'>window.__d_config=')
            flow.response.set_text(updated)
    moderation['response'](flow)
    if flow.request.host=='live.douyin.com' and room_allowed(flow.request.pretty_url) and 'text/html' in flow.response.headers.get('content-type','').lower():
        html=flow.response.get_text(strict=False)
        if '/__live_archive/recording.js' not in html:
            # Copy the nonce from the moderation bootstrap injected into this document.
            match=re.search(r'<script\b[^>]*\bnonce=["\']([^"\']+)["\'][^>]*>',html,re.I)
            nonce=' nonce="'+escape(match.group(1),quote=True)+'"' if match else ''
            tag='<script'+nonce+' src="/__live_archive/recording.js"></script>'
            html,count=re.subn(r'(<head(?:\s[^>]*)?>)',lambda m:m.group(0)+tag,html,count=1,flags=re.I)
            if not count:return
            flow.response.set_text(html)

async def running():
    if 'running' in moderation:
        result=moderation['running']()
        if inspect.isawaitable(result):await result

def done():
    if 'done' in moderation:moderation['done']()
def load(loader):
    if 'load' in capture:capture['load'](loader)
def http_connect(flow):
    moderation['http_connect'](flow)
def http_connect_error(flow):
    moderation['http_connect_error'](flow)
def error(flow):
    moderation['error'](flow)
def tls_failed_client(data):
    moderation['tls_failed_client'](data)
def tls_failed_server(data):
    moderation['tls_failed_server'](data)
