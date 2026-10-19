"""Tests our generated proxy route without installing or trusting any CA."""
import asyncio
import json
from pathlib import Path
import sys
import types
import unittest


class Response:
    def __init__(self, status, content, headers=None):
        self.status_code, self.content, self.headers = status, content, headers or {}
        self.stream = False

    @property
    def raw_content(self):
        return self.content

    @classmethod
    def make(cls, status, content, headers=None):
        return cls(status, content, headers)

    def get_text(self, strict=False):
        return self.content.decode()

    def set_text(self, text):
        self.content = text.encode()


sys.modules.setdefault("mitmproxy", types.SimpleNamespace(http=types.SimpleNamespace(Response=Response, HTTPFlow=object)))
CONFIG = {"host": "live.douyin.com", "room_path": "/123", "prefix": "/__live_archive/moderation/v1/", "bridge_host": "127.0.0.1", "bridge_port": 1, "relay_token": "privateRelay"}


def addon():
    source = (Path(__file__).parent.parent / "assets/moderation/proxy/addon.py").read_text()
    source = source.replace("__CONFIG_JSON_STRING__", json.dumps(json.dumps(CONFIG)))
    result = {}
    exec(compile(source, "generated_moderation_addon.py", "exec"), result)
    return result


def flow(path, *, room="123", method="POST", scheme="https"):
    return types.SimpleNamespace(request=types.SimpleNamespace(
        host="live.douyin.com", path=path, method=method, scheme=scheme,
        pretty_url="https://live.douyin.com/" + room,
        raw_content=b"{}", headers={"referer": "https://live.douyin.com/" + room,
        "origin": "https://live.douyin.com", "content-type": "application/json",
        "authorization": "Bearer pageToken", "cookie": "must-not-forward"}), response=None, metadata={})


class ProxyTests(unittest.IsolatedAsyncioTestCase):
    async def test_upstream_protocol_and_error_logs_only_export_fixed_categories(self):
        import contextlib
        import io
        module = addon()
        page = flow("/api/private?token=private", room="api/private")
        page.response = Response(503, b"private-cookie", {"content-type":"application/json"})
        page.response.http_version = "HTTP/2.0"
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            module["responseheaders"](page)
        self.assertIn("platform upstream protocol: HTTP/2", output.getvalue())
        self.assertIn("platform upstream response: 5xx", output.getvalue())
        self.assertNotIn("private", output.getvalue())
        self.assertTrue(page.response.stream)

    async def test_tls_only_forwards_without_html_cache_changes_or_bridge_calls(self):
        module = addon()
        module["CONFIG"]["inject_page"] = False
        async def forward(*args):
            self.fail("TLS-only mode must not invoke the bridge")
        module["forward"] = forward
        page = flow("/123", method="GET")
        page.request.headers["if-none-match"] = "original-etag"
        html = b'<html><head></head><body>original</body></html>'
        page.response = Response(200, html, {"content-type": "text/html", "etag": "original-etag"})
        await module["request"](page)
        module["requestheaders"](page)
        module["responseheaders"](page)
        module["response"](page)
        self.assertTrue(page.response.stream)
        self.assertEqual(page.response.content, html)
        self.assertEqual(page.response.headers["etag"], "original-etag")
        self.assertEqual(page.request.headers["if-none-match"], "original-etag")
        reserved = flow(CONFIG["prefix"] + "bootstrap.js", method="GET")
        await module["request"](reserved)
        self.assertIsNone(reserved.response)

    async def test_headers_stream_platform_api_and_media_but_buffer_document_and_bridge(self):
        module = addon()
        for path, content_type, expected in [
            ("/123", "text/html; charset=utf-8", False),
            ("/123", "video/mp2t", True),
            ("/api/poll", "text/event-stream", True),
            ("/api/events", "application/json", True),
            (CONFIG["prefix"] + "events", "application/json", False),
        ]:
            page = flow(path, room=path.lstrip('/'))
            page.response = Response(200, b"fixture", {"content-type": content_type})
            module["requestheaders"](page)
            module["responseheaders"](page)
            self.assertEqual(bool(page.response.stream), expected)
            self.assertEqual(page.request.stream, not path.startswith(CONFIG["prefix"]))

    async def test_tls_error_categories_do_not_disclose_error_text(self):
        import contextlib
        import io
        module = addon()
        for message, category in [
            ("private-token tlsv1 alert unknown ca", "certificate rejected"),
            ("private-cookie unsupported protocol", "protocol mismatch"),
            ("private-url connection closed", "connection closed"),
            ("private-unknown", "unclassified"),
        ]:
            tls = types.SimpleNamespace(
                conn=types.SimpleNamespace(error=message),
                context=types.SimpleNamespace(server=types.SimpleNamespace(sni=CONFIG["host"])))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                module["tls_failed_client"](tls)
            self.assertIn("category: " + category, output.getvalue())
            self.assertNotIn("private", output.getvalue())

    async def test_fixed_diagnostics_hide_values_and_fresh_page_skips_cache(self):
        import contextlib
        import io
        module = addon()
        page = flow("/123?private=query", method="GET")
        page.request.pretty_url = "https://live.douyin.com/123?private=query"
        page.request.headers["if-none-match"] = "private-etag"
        page.request.headers["if-modified-since"] = "private-date"
        page.error = types.SimpleNamespace(msg="private-cookie private-token")
        tls = types.SimpleNamespace(context=types.SimpleNamespace(server=types.SimpleNamespace(address=(CONFIG["host"],443),sni=CONFIG["host"])))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            await module["request"](page)
            module["http_connect"](page)
            module["http_connect_error"](page)
            module["tls_failed_client"](tls)
            module["tls_failed_server"](tls)
            module["error"](page)
            for _ in range(100):
                module["diagnostic"]("bounded diagnostic")
            for index in range(100):
                module["diagnostic"]("bounded fixture " + str(index))
        self.assertNotIn("if-none-match", page.request.headers)
        self.assertNotIn("if-modified-since", page.request.headers)
        self.assertNotIn("private", output.getvalue())
        self.assertEqual(len(output.getvalue().splitlines()),40)
        self.assertEqual(output.getvalue().count("bounded diagnostic"), 1)
        self.assertIn("platform client TLS failed",output.getvalue())

    async def test_relay_keeps_auth_scopes_separate_and_never_forwards_cookie(self):
        module, calls = addon(), []

        async def forward(*args):
            calls.append(args)
            return 200, "application/json", b"{}"

        module["forward"] = forward
        page = flow(CONFIG["prefix"] + "events")
        await module["request"](page)
        self.assertEqual(calls[0], ("/v1/events", b"{}", "pageToken", "https://live.douyin.com"))
        boot = flow(CONFIG["prefix"] + "bootstrap.js", method="GET")
        await module["request"](boot)
        self.assertEqual(calls[1], ("/v1/bootstrap", b"{}", "privateRelay", ""))
        self.assertNotIn(b"privateRelay", page.response.content)

    async def test_rejected_reserved_requests_never_reach_platform_or_local_bridge(self):
        module = addon()

        async def forward(*args):
            self.fail("invalid request reached bridge")

        module["forward"] = forward
        for request in [flow(CONFIG["prefix"] + "events", room="456"),
                        flow(CONFIG["prefix"] + "execute"),
                        flow(CONFIG["prefix"] + "events", scheme="http")]:
            await module["request"](request)
            self.assertEqual(request.response.status_code, 403)
        invalid = flow(CONFIG["prefix"] + "events")
        invalid.request.headers["origin"] = "https://attacker.example"
        await module["request"](invalid)
        self.assertEqual(invalid.response.status_code, 403)

    async def test_fill_receipts_use_only_the_local_authenticated_route(self):
        module, calls = addon(), []

        async def forward(*args):
            calls.append(args)
            return 200, "application/json", b'{"send_chat":false}'

        module["forward"] = forward
        receipt = flow(CONFIG["prefix"] + "results")
        await module["request"](receipt)
        self.assertEqual(calls, [("/v1/results", b"{}", "pageToken", "https://live.douyin.com")])
        other_room = flow(CONFIG["prefix"] + "results", room="456")
        await module["request"](other_room)
        self.assertEqual(other_room.response.status_code, 403)
        self.assertEqual(len(calls), 1)

    async def test_html_injection_is_early_idempotent_scoped_and_preserves_csp(self):
        module = addon()
        html = b'<HTML><HEAD><script src="platform.js"></script></HEAD><body></body></HTML>'
        page = flow("/123", method="GET")
        page.response = Response(200, html, {"content-type": "text/html", "content-security-policy": "script-src 'self'", "etag": "old"})
        module["response"](page)
        injected = page.response.content
        self.assertLess(injected.index(b"bootstrap.js"), injected.index(b"platform.js"))
        self.assertEqual(page.response.headers["content-security-policy"], "script-src 'self'")
        self.assertNotIn("etag", page.response.headers)
        module["response"](page)
        self.assertEqual(page.response.content, injected)
        other = flow("/456", room="456", method="GET")
        other.response = Response(200, html, {"content-type": "text/html"})
        module["response"](other)
        self.assertEqual(other.response.content, html)

    async def test_wechat_desktop_origin_uses_same_scoped_nonce_and_local_relay(self):
        module = addon()
        module["CONFIG"] = {**CONFIG, "host": "channels.weixin.qq.com", "room_path": "/web/pages/live"}
        base = "https://channels.weixin.qq.com"
        page = flow("/web/pages/live", method="GET")
        page.request.host = "channels.weixin.qq.com"
        page.request.pretty_url = base + "/web/pages/live?private=query"
        policy = "script-src 'nonce-currentPageToken' 'strict-dynamic'"
        page.response = Response(200, b'<head><script nonce="currentPageToken" src="platform.js"></script></head>', {"content-type": "text/html", "content-security-policy": policy})
        module["response"](page)
        self.assertIn(b'nonce="currentPageToken" src="/__live_archive/moderation/v1/bootstrap.js"', page.response.content)
        self.assertEqual(page.response.headers["content-security-policy"], policy)
        calls = []

        async def forward(*args):
            calls.append(args)
            return 200, "application/json", b'{}'

        module["forward"] = forward
        route = flow(CONFIG["prefix"] + "hello")
        route.request.host = "channels.weixin.qq.com"
        route.request.headers["referer"] = base + "/web/pages/live?private=query"
        route.request.headers["origin"] = base
        await module["request"](route)
        self.assertEqual(route.response.status_code, 200)
        self.assertEqual(calls, [("/v1/hello", b"{}", "pageToken", base)])
        route.request.headers["origin"] = "https://live.douyin.com"
        await module["request"](route)
        self.assertEqual(route.response.status_code, 403)
        self.assertEqual(len(calls), 1)

    async def test_bridge_failure_returns_local_503(self):
        module = addon()

        async def forward(*args):
            raise OSError("bridge stopped")

        module["forward"] = forward
        page = flow(CONFIG["prefix"] + "events")
        await module["request"](page)
        self.assertEqual(page.response.status_code, 503)
        self.assertNotIn(b"privateRelay", page.response.content)

    async def test_strict_dynamic_uses_existing_authorized_nonce_without_changing_policy(self):
        module = addon()
        policy = "script-src 'nonce-currentPageToken' 'strict-dynamic' 'self'"
        html = b'<html><head><script nonce="otherToken"></script><script nonce="currentPageToken" src="platform.js"></script></head></html>'
        page = flow("/123", method="GET")
        page.response = Response(200, html, {"content-type": "text/html", "content-security-policy": policy})
        module["response"](page)
        self.assertIn(b'data-live-archive-moderation="v1" nonce="currentPageToken"', page.response.content)
        self.assertEqual(page.response.headers["content-security-policy"], policy)
        self.assertLess(page.response.content.index(b"bootstrap.js"), page.response.content.index(b"platform.js"))

    async def test_nonce_is_not_taken_from_javascript_strings_or_untrusted_attributes(self):
        module = addon()
        for html in [b'<head><script>const text = \'<script nonce="fake">\';</script></head>',
                     b'<head><script nonce="not-in-policy" src="platform.js"></script></head>',
                     b'<head><script nonce="bad&amp;quot;value" src="platform.js"></script></head>']:
            page = flow("/123", method="GET")
            page.response = Response(200, html, {"content-type": "text/html", "content-security-policy": "script-src 'nonce-required' 'strict-dynamic'"})
            module["response"](page)
            self.assertIn(b'data-live-archive-moderation="v1" src=', page.response.content)


if __name__ == "__main__":
    unittest.main()
