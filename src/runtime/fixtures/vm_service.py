#!/usr/bin/python3
"""Local transport fixture; no QEMU, network, provider or credential store."""
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
manifest = json.loads((root / "plugins/vm/plugin.json").read_text())
write({"type": "ready", "version": 99 if mode == "bad_version" else 1,
       "owner": "vm", "service": "vm", "nonce": hello["nonce"],
       "operations": [t["name"] for t in manifest["tools"]], "controls": ["autostart"]})
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
        if mode == "cancel":
            cancellation = read()
            assert cancellation["type"] == "cancel"
            (root / "worker.cancelled").touch()
            write({"type": "cancelled", **identity})
            break
        context = request["context"]
        grants = context["attributes"]["grants"]
        result = {"scope": {"kind": "guest", "user": context["user"], "task": context["task_id"]},
                  "outcome": "succeeded", "verified": False,
                  "caller": {k: v for k, v in context.items() if k not in ("attributes", "secrets")},
                  "preferences": context["attributes"]["preferences"],
                  "grant_names": sorted(grants), "credentials_match": grants == {"VM_TOKEN": "permitted-fixture-value"},
                  "secret_count": len(context["secrets"])}
    else:
        result = {"healthy": True}
    write({"type": "completed", "result": result, **identity})
