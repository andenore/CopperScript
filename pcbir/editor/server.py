"""Loopback-only editor transport; capability-authenticated session operations."""
from __future__ import annotations

import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from importlib.resources import files
import json
import secrets
from threading import RLock
from urllib.parse import urlsplit

from .session import EditorError, EditorSession, StaleRevision


def create_server(session: EditorSession, port: int = 0) -> ThreadingHTTPServer:
    if type(port) is not int or not 0 <= port <= 65535:
        raise ValueError("editor port must be between 0 and 65535")
    token = secrets.token_urlsafe(32)
    lock = RLock()

    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(10)

        def log_message(self, *_):
            pass  # No source identities, capability or requests in access logs.

        def respond(self, status, body, content_type="application/json"):
            if not isinstance(body, bytes):
                body = json.dumps(body, allow_nan=False).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
            self.end_headers()
            self.wfile.write(body)

        def allowed(self, api=False):
            expected = f"127.0.0.1:{self.server.server_port}"
            if self.headers.get_all("Host") != [expected]:
                self.respond(403, {"error": "invalid Host"})
                return False
            origin = self.headers.get("Origin")
            if origin is not None and origin != f"http://{expected}":
                self.respond(403, {"error": "cross-origin request rejected"})
                return False
            if api and not hmac.compare_digest(self.headers.get("X-Copper-Token", "").encode("utf-8"), token.encode("utf-8")):
                self.respond(403, {"error": "session capability required"})
                return False
            return True

        def do_GET(self):
            path = urlsplit(self.path).path
            if not self.allowed(api=path.startswith("/api/")):
                return
            if path == "/api/scene":
                try:
                    with lock:
                        self.respond(200, session.scene())
                except (OSError, ValueError) as exc:
                    self.respond(400, {"error": str(exc)})
                return
            assets = {"/": ("index.html", "text/html; charset=utf-8"),
                      "/editor.js": ("editor.js", "text/javascript; charset=utf-8"),
                      "/editor.css": ("editor.css", "text/css; charset=utf-8")}
            if path not in assets:
                self.respond(404, {"error": "not found"})
                return
            name, mime = assets[path]
            self.respond(200, files("pcbir.editor").joinpath("assets", name).read_bytes(), mime)

        def do_POST(self):
            if not self.allowed(api=True):
                return
            if urlsplit(self.path).path != "/api/operation":
                self.respond(404, {"error": "not found"})
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if (not 0 < length <= 4096 or self.headers.get("Content-Type") != "application/json"
                        or self.headers.get("Transfer-Encoding") is not None
                        or len(self.headers.get_all("Content-Length", ())) != 1):
                    raise EditorError("bounded application/json body required")

                def unique(pairs):
                    data = {}
                    for key, value in pairs:
                        if key in data:
                            raise EditorError("duplicate JSON fields")
                        data[key] = value
                    return data

                request = json.loads(self.rfile.read(length), object_pairs_hook=unique,
                    parse_constant=lambda _: (_ for _ in ()).throw(EditorError("nonfinite JSON value")))
                if not isinstance(request, dict):
                    raise EditorError("operation must be an object")
                with lock:
                    self.respond(200, session.operation(request))
            except StaleRevision as exc:
                self.respond(409, {"error": str(exc)})
            except (ValueError, OSError, TypeError) as exc:
                self.respond(400, {"error": str(exc)})

    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    server.editor_url = f"http://127.0.0.1:{server.server_port}/#token={token}"
    server.editor_token = token
    return server


def serve(session: EditorSession, *, port: int = 0, open_browser: bool = True):
    import webbrowser
    with create_server(session, port) as server:
        print(f"Mechanical editor (source read-only): {server.editor_url}", flush=True)
        print("Preview changes are temporary. Ctrl+C stops the local service.", flush=True)
        if open_browser:
            webbrowser.open(server.editor_url)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass
