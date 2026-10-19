"""Real mitmdump + local continuous response; no platform, CA or system proxy changes."""
import http.client
import http.server
import json
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import unittest


class StreamingIntegration(unittest.TestCase):
    def test_media_and_event_stream_arrive_before_upstream_finishes(self):
        executable = shutil.which("mitmdump")
        self.assertIsNotNone(executable, "install mitmdump to run this integration test")
        release = threading.Event()

        class Upstream(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", "video/mp2t" if self.path == "/media" else "text/event-stream")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                self.wfile.write(b"5\r\nfirst\r\n")
                self.wfile.flush()
                release.wait(5)
                try:
                    self.wfile.write(b"4\r\nlast\r\n0\r\n\r\n")
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass

        upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        thread = threading.Thread(target=upstream.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                with socket.socket() as probe:
                    probe.bind(("127.0.0.1", 0))
                    port = probe.getsockname()[1]
                config = {"host": "127.0.0.1", "room_path": "/web/pages/live",
                          "prefix": "/__live_archive/moderation/v1/", "bridge_host": "127.0.0.1",
                          "bridge_port": 1, "relay_token": "synthetic", "inject_page": False}
                template = (Path(__file__).parent.parent / "assets/moderation/proxy/addon.py").read_text()
                addon = Path(directory) / "addon.py"
                addon.write_text(template.replace("__CONFIG_JSON_STRING__", json.dumps(json.dumps(config))))
                process = subprocess.Popen([executable, "-q", "--listen-host", "127.0.0.1",
                    "--listen-port", str(port), "--set", "confdir=" + directory,
                    "--set", "block_global=false", "-s", str(addon)],
                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                try:
                    deadline = time.monotonic() + 8
                    while True:
                        self.assertIsNone(process.poll(), "proxy exited during startup")
                        try:
                            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                                break
                        except OSError:
                            if time.monotonic() > deadline:
                                self.fail("proxy startup timed out")
                            time.sleep(0.05)
                    for path in ("/media", "/events"):
                        with self.subTest(path=path):
                            release.clear()
                            client = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                            try:
                                client.request("GET", f"http://127.0.0.1:{upstream.server_port}{path}")
                                response = client.getresponse()
                                self.assertEqual(response.status, 200)
                                self.assertEqual(response.read(5), b"first")
                                self.assertFalse(release.is_set(), "upstream must still be open")
                                release.set()
                                self.assertEqual(response.read(), b"last")
                            finally:
                                release.set()
                                client.close()
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        process.kill()
                    output = process.communicate(timeout=3)[0].decode()
                    summaries = [json.loads(line.split("transport summary ", 1)[1])
                                 for line in output.splitlines() if "transport summary " in line]
                    self.assertTrue(summaries, "proxy must report numeric transport aggregates")
                    self.assertEqual(summaries[-1]["requests"], 2)
                    self.assertEqual(summaries[-1]["bytes"], 18)
        finally:
            release.set()
            upstream.shutdown()
            upstream.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
