"""Generated read-only, same-origin moderation proxy. Never forwards cookies."""
import asyncio
import json
import re
import time
import urllib.parse
from html import escape
from mitmproxy import http

CONFIG = json.loads(__CONFIG_JSON_STRING__)
LIMIT = 128 * 1024
diagnostic_count = 0
diagnostic_stages = set()
transport = {"requests": 0, "response_headers": 0, "completed": 0,
             "bytes": 0, "http_errors": 0, "client_tls_failures": 0,
             "upstream_tls_failures": 0, "transport_failures": 0,
             "headers_wait_max_ms": 0, "slow_headers": 0}
transport_task = None


async def running():
    global transport_task
    if CONFIG.get("inject_page", True):
        return
    async def report():
        while True:
            await asyncio.sleep(10)
            report_transport()
    transport_task = asyncio.create_task(report())


def report_transport():
    # Numeric session aggregates only; no paths, domains, headers or bodies.
    print("live-archive moderation: transport summary " + json.dumps(transport, sort_keys=True), flush=True)


def done():
    if transport_task is not None:
        transport_task.cancel()
    report_transport()


def diagnostic(stage):
    # Fixed messages only. Do not log URLs, headers, certificates or response bodies.
    global diagnostic_count
    if diagnostic_count < 40 and stage not in diagnostic_stages:
        diagnostic_stages.add(stage)
        print("live-archive moderation: " + stage, flush=True)
        diagnostic_count += 1


def http_connect(flow):
    if flow.request.host == CONFIG["host"]:
        diagnostic("platform CONNECT received")


def http_connect_error(flow):
    if flow.request.host == CONFIG["host"]:
        diagnostic("platform CONNECT failed")


def platform_tls(data):
    server = data.context.server
    return (getattr(server, "sni", None) == CONFIG["host"]
            or (getattr(server, "address", None) or (None,))[0] == CONFIG["host"])


def tls_failed_client(data):
    if platform_tls(data):
        transport["client_tls_failures"] += 1
        diagnostic("platform client TLS failed")
        # Inspect locally, but never print arbitrary OpenSSL error strings.
        error = str(getattr(getattr(data, "conn", None), "error", "") or "").lower()
        if any(value in error for value in ("unknown ca", "unknown_ca", "certificate unknown", "certificate_unknown", "does not trust", "bad certificate")):
            diagnostic("platform client TLS category: certificate rejected")
        elif any(value in error for value in ("protocol version", "unsupported protocol", "no shared cipher")):
            diagnostic("platform client TLS category: protocol mismatch")
        elif any(value in error for value in ("closed", "disconnect", "eof", "reset")):
            diagnostic("platform client TLS category: connection closed")
        else:
            diagnostic("platform client TLS category: unclassified")


def tls_failed_server(data):
    if platform_tls(data):
        transport["upstream_tls_failures"] += 1
        diagnostic("platform upstream TLS failed")


def error(flow):
    if flow.request.host == CONFIG["host"]:
        transport["transport_failures"] += 1
        diagnostic("platform HTTP transport failed")


def page_nonce(html, policy):
    # The packaged mitmdump runtime omits html.parser. Consume entire script
    # bodies/comments so nonce-looking markup inside JS isn't treated as a tag.
    scripts = re.finditer(r"<!--[\s\S]*?-->|<script\b([^>]*)>[\s\S]*?(?:</script\s*>|$)", html, re.I)
    for script in scripts:
        attrs = script.group(1)
        if attrs is None:
            continue
        match = re.search(r'''(?:^|\s)nonce\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))''', attrs, re.I)
        if not match:
            continue
        nonce = next(value for value in match.groups() if value is not None)
        if re.fullmatch(r"[a-zA-Z0-9+/_=-]{1,256}", nonce) and (not policy or "'nonce-" + nonce + "'" in policy):
            return nonce
    return ""


def room_matches(url):
    parsed = urllib.parse.urlsplit(url)
    return (parsed.scheme == "https" and parsed.netloc == CONFIG["host"]
            and parsed.path.rstrip("/") == CONFIG["room_path"])


async def forward(path, body, bearer="", origin=""):
    reader, writer = await asyncio.wait_for(
        asyncio.open_connection(CONFIG["bridge_host"], CONFIG["bridge_port"]), 1)
    try:
        headers = {
            "Host": f'{CONFIG["bridge_host"]}:{CONFIG["bridge_port"]}',
            "Content-Type": "application/json", "Content-Length": str(len(body)),
            "Connection": "close",
        }
        if bearer:
            headers["Authorization"] = "Bearer " + bearer
        if origin:
            headers["Origin"] = origin
        head = f"POST {path} HTTP/1.1\r\n" + "".join(f"{k}: {v}\r\n" for k, v in headers.items()) + "\r\n"
        writer.write(head.encode("ascii") + body)
        await writer.drain()
        response_head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 2)
        lines = response_head.decode("ascii").split("\r\n")
        status = int(lines[0].split()[1])
        response_headers = dict(line.split(": ", 1) for line in lines[1:] if ": " in line)
        size = int(response_headers.get("Content-Length", "0"))
        if size > 256 * 1024:
            raise ValueError("bridge response too large")
        response_body = await asyncio.wait_for(reader.readexactly(size), 2)
        return status, response_headers.get("Content-Type", "application/json"), response_body
    finally:
        writer.close()
        await writer.wait_closed()


async def request(flow: http.HTTPFlow):
    if not CONFIG.get("inject_page", True):
        return
    if flow.request.host != CONFIG["host"]:
        return
    path = flow.request.path.split("?", 1)[0]
    if room_matches(flow.request.pretty_url):
        # Force a fresh document only for the configured page, matching recording's cache behavior.
        flow.request.headers.pop("if-none-match", None)
        flow.request.headers.pop("if-modified-since", None)
        diagnostic("platform page request received")
    if not path.startswith(CONFIG["prefix"]):
        return
    # Always intercept reserved local paths, including rejected requests.
    # A failed bridge must never leak local tokens to Douyin's origin server.
    flow.response = http.Response.make(403, b'{"error":"scope_denied"}', {"Content-Type": "application/json"})
    if flow.request.scheme != "https" or not room_matches(flow.request.headers.get("referer", "")):
        return
    endpoint = path[len(CONFIG["prefix"]):]
    if endpoint == "bootstrap.js" and flow.request.method == "GET":
        bridge_path, body = "/v1/bootstrap", b"{}"
        bearer, origin = CONFIG["relay_token"], ""
    elif endpoint in ("control", "hello", "events", "raw", "poll", "results", "delivery", "goodbye") and flow.request.method == "POST":
        if flow.request.headers.get("origin") != "https://" + CONFIG["host"]:
            return
        if flow.request.headers.get("content-type", "").split(";")[0] != "application/json":
            return
        body = flow.request.raw_content or b""
        if len(body) > LIMIT:
            flow.response = http.Response.make(413, b'{"error":"too_large"}')
            return
        bridge_path = "/v1/" + endpoint
        auth = flow.request.headers.get("authorization", "")
        bearer = auth[7:] if auth.startswith("Bearer ") else ""
        # Defense against header injection into the local HTTP connection.
        if not re.fullmatch(r"[a-zA-Z0-9]*", bearer):
            return
        origin = "https://" + CONFIG["host"]
    else:
        return
    try:
        status, content_type, data = await forward(bridge_path, body, bearer, origin)
    except Exception:
        status, content_type, data = 503, "application/json", b'{"error":"bridge_unavailable"}'
    flow.response = http.Response.make(status, data, {
        "Content-Type": content_type, "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff",
    })
    diagnostic("local bootstrap served" if endpoint == "bootstrap.js" and status == 200
               else "local bridge response served" if status == 200 else "local bridge response failed")


def requestheaders(flow: http.HTTPFlow):
    # Only local bridge payloads require buffering for authentication/limits.
    local = CONFIG.get("inject_page", True) and flow.request.host == CONFIG["host"] and flow.request.path.split("?", 1)[0].startswith(CONFIG["prefix"])
    flow.request.stream = not local
    if flow.request.host == CONFIG["host"] and not local:
        transport["requests"] += 1
        flow.metadata["live_archive_transport_start"] = time.monotonic()


def responseheaders(flow: http.HTTPFlow):
    # Never buffer media, event streams or unrelated API responses. Only the
    # selected HTML document needs its body available for script insertion.
    page = CONFIG.get("inject_page", True) and room_matches(flow.request.pretty_url)
    html = "text/html" in flow.response.headers.get("content-type", "").lower()
    local = CONFIG.get("inject_page", True) and flow.request.host == CONFIG["host"] and flow.request.path.split("?", 1)[0].startswith(CONFIG["prefix"])
    flow.response.stream = not (local or (page and html))
    if flow.request.host == CONFIG["host"] and not local:
        transport["response_headers"] += 1
        started = getattr(flow, "metadata", {}).get("live_archive_transport_start")
        if started is not None:
            elapsed = round((time.monotonic() - started) * 1000)
            transport["headers_wait_max_ms"] = max(transport["headers_wait_max_ms"], elapsed)
            if elapsed >= 2000:
                transport["slow_headers"] += 1
        if flow.response.stream:
            def count_chunk(chunk):
                transport["bytes"] += len(chunk)
                return chunk
            flow.response.stream = count_chunk
        version = getattr(flow.response, "http_version", "")
        if version == "HTTP/2.0":
            diagnostic("platform upstream protocol: HTTP/2")
        elif version == "HTTP/1.1":
            diagnostic("platform upstream protocol: HTTP/1.1")
        code = getattr(flow.response, "status_code", 0)
        if code >= 400:
            transport["http_errors"] += 1
        if code >= 500:
            diagnostic("platform upstream response: 5xx")
        elif code >= 400:
            diagnostic("platform upstream response: 4xx")


def response(flow: http.HTTPFlow):
    local = flow.request.host == CONFIG["host"] and flow.request.path.split("?", 1)[0].startswith(CONFIG["prefix"])
    if flow.request.host == CONFIG["host"] and not local and flow.response:
        transport["completed"] += 1
        if not flow.response.stream:
            transport["bytes"] += len(flow.response.raw_content or b"")
    if not CONFIG.get("inject_page", True):
        return
    if not room_matches(flow.request.pretty_url) or not flow.response:
        return
    if "text/html" not in flow.response.headers.get("content-type", "").lower():
        diagnostic("platform page response is not HTML")
        return
    html = flow.response.get_text(strict=False)
    if "data-live-archive-moderation" in html:
        return
    nonce = page_nonce(html, flow.response.headers.get("content-security-policy", ""))
    nonce_attr = ' nonce="' + escape(nonce, quote=True) + '"' if nonce else ""
    tag = '<script data-live-archive-moderation="v1"' + nonce_attr + ' src="' + CONFIG["prefix"] + 'bootstrap.js"></script>'
    modified, count = re.subn(r"(<head(?:\s[^>]*)?>)", lambda match: match[0] + tag, html, count=1, flags=re.I)
    if count:
        flow.response.set_text(modified)
        flow.response.headers["cache-control"] = "no-store"
        flow.response.headers.pop("etag", None)
        diagnostic("platform page injection inserted")
    else:
        diagnostic("platform HTML has no head; injection skipped")
