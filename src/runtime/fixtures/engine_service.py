#!/usr/bin/python3
"""Local runtime-engine fixture; no real rendering, network or credentials."""
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
write({
    "type": "ready",
    "version": 99 if mode == "bad_version" else 1,
    "owner": hello["owner"],
    "service": hello["service"],
    "nonce": hello["nonce"],
    "operations": ["render", "render_strict", "render_strict_candidate"],
    "controls": [],
})
(root / "worker.ready").touch()
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
        path = request["input"].get("path", "")
        result = "bridged:%s:%s" % (request["operation"], path)
    else:
        result = {"healthy": True}
    write({"type": "completed", "result": result, **identity})
