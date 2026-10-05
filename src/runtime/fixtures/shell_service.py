#!/usr/bin/python3
"""Local shell worker fixture; no real jobs, network or credentials."""
import json
import pathlib
import struct
import sys

root = pathlib.Path.cwd()


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
(root / "worker.ready").touch()
write({
    "type": "ready",
    "version": 99 if mode == "bad_version" else 1,
    "owner": "shell",
    "service": "shell",
    "nonce": hello["nonce"],
    "operations": ["run_background", "background_status"],
    "controls": ["cleanup", "drain_completions"],
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
            "operation": request["operation"],
        }
    elif request["type"] == "control" and request["operation"] == "drain_completions":
        result = []
    elif request["type"] == "control":
        result = {"removed": 0}
    else:
        result = {"healthy": True}
    write({"type": "completed", "result": result, **identity})
