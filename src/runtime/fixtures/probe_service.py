#!/usr/bin/python3
"""Local declared-service fixture; no external network, credentials or host access."""
import json
import pathlib
import struct
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

root = pathlib.Path.cwd()
web = {"port": 0, "key": None}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        # Enforce the host-issued private key, like a real contribution.
        if self.headers.get("x-praxis-plugin-key") != web["key"]:
            self.send_response(403)
            self.end_headers()
            return
        body = json.dumps({
            "path": self.path,
            "principal": self.headers.get("x-praxis-principal"),
        }).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def read():
    header = sys.stdin.buffer.read(4)
    if len(header) != 4:
        raise EOFError()
    size = struct.unpack(">I", header)[0]
    return json.loads(sys.stdin.buffer.read(size))


def write(value):
    data = json.dumps(value).encode()
    sys.stdout.buffer.write(struct.pack(">I", len(data)) + data)
    sys.stdout.buffer.flush()


hello = read()
mode = (root / "worker.mode").read_text()
if mode == "web":
    web["key"] = hello["initialization"].get("web_token")
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    web["port"] = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
(root / "worker.ready").touch()
write({
    "type": "ready",
    "version": 99 if mode == "bad_version" else 1,
    "owner": "probe",
    "service": "probe",
    "nonce": hello["nonce"],
    "operations": ["probe_echo"],
    "controls": ["web_info"] if mode == "web" else [],
})
while True:
    request = read()
    identity = {"id": request["id"], "nonce": request["nonce"]}
    if request["type"] == "shutdown":
        write({"type": "stopped", **identity})
        break
    if request["type"] == "invoke":
        with (root / "worker.calls").open("a") as log:
            log.write("invoke\n")
        if mode == "crash":
            sys.exit(1)
        context = request["context"]
        result = {
            "caller": {k: v for k, v in context.items() if k not in ("attributes", "secrets")},
            "initialization": hello["initialization"],
        }
    elif request["type"] == "control" and request["operation"] == "web_info":
        result = {
            "version": 1,
            "port": web["port"],
            "descriptor": {
                "id": "probe",
                "title": "Probe",
                "page": "/plugins/probe/ui/page.html",
                "script": "/plugins/probe/ui/probe.js",
                "style": "/plugins/probe/ui/probe.css",
                "websockets": [],
            },
        }
    else:
        result = {"healthy": True}
    write({"type": "completed", "result": result, **identity})
