#!/usr/bin/env python3
"""Bounded, one-shot MCP stdio client; executable selection is operator-only."""
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import time

LIMIT = 512 * 1024
WIRE_LIMIT = 2 * 1024 * 1024
PROTOCOL = "2024-11-05"
NAME = re.compile(r"^[A-Za-z0-9_.-]{1,128}$")


class MCPError(Exception):
    pass


def fail(code):
    raise MCPError(code)


def decode(raw):
    try:
        if isinstance(raw, bytes):
            raw = raw.decode("utf-8")
        return json.loads(raw, parse_constant=lambda _: fail("invalid_json"))
    except (ValueError, UnicodeError, RecursionError):
        fail("invalid_json")


def encode(value):
    try:
        return json.dumps(value, ensure_ascii=False, allow_nan=False).encode("utf-8")
    except (ValueError, TypeError, UnicodeError, RecursionError):
        fail("invalid_json")


def load_config():
    # Never read PLUGIN_CONTEXT or model arguments for process configuration.
    path = Path(os.environ.get("PRAXIS_MCP_CONFIG", str(Path(__file__).with_name("servers.json"))))
    with path.open("rb") as source:
        raw = source.read(LIMIT + 1)
    if len(raw) > LIMIT:
        fail("configuration")
    config = decode(raw)
    if not isinstance(config, dict) or set(config) != {"servers"} or not isinstance(config["servers"], dict) or len(config["servers"]) > 64:
        fail("configuration")
    servers = {}
    for name, spec in config["servers"].items():
        if not NAME.fullmatch(name) or not isinstance(spec, dict) or set(spec) - {"command", "args", "cwd", "env_from", "allow_tools", "timeout_secs"}:
            fail("configuration")
        command = spec.get("command")
        args = spec.get("args", [])
        tools = spec.get("allow_tools")
        env_from = spec.get("env_from", {})
        timeout = spec.get("timeout_secs", 20)
        if not isinstance(command, str) or not os.path.isabs(command) or not isinstance(args, list) or len(args) > 64 or any(not isinstance(a, str) or len(a) > 4096 or "\0" in a for a in [command] + args):
            fail("configuration")
        if not isinstance(tools, list) or len(tools) > 256 or any(not isinstance(t, str) or not NAME.fullmatch(t) for t in tools):
            fail("configuration")
        if not isinstance(env_from, dict) or len(env_from) > 64 or any(not isinstance(k, str) or not isinstance(v, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", v) for k, v in env_from.items()):
            fail("configuration")
        if type(timeout) not in (int, float) or not 0.1 <= timeout <= 40:
            fail("configuration")
        cwd = spec.get("cwd")
        if cwd is not None and (not isinstance(cwd, str) or not os.path.isabs(cwd) or "\0" in cwd):
            fail("configuration")
        servers[name] = spec
    return servers


def environment(spec):
    # No provider/admin environment or plugin envelopes reach the MCP child.
    env = {k: os.environ[k] for k in ("PATH", "SYSTEMROOT", "WINDIR") if k in os.environ}
    secrets = []
    for target, source in spec.get("env_from", {}).items():
        value = os.environ.get(source)
        if not value:
            fail("missing_environment")
        env[target] = value
        secrets.append(value)
    envelope = decode(os.environ.get("PLUGIN_SECRETS", "{}"))
    if isinstance(envelope, dict):
        secrets.extend(v for v in envelope.values() if isinstance(v, str) and v)
    if sum(len(s.encode("utf-8")) for s in secrets) > LIMIT:
        fail("configuration")
    return env, secrets


def redact(value, secrets, remaining):
    # One pass: replacement markers are never themselves searched/replaced.
    patterns = sorted(set(secrets), key=len, reverse=True)
    budget = LIMIT

    def redact_text(text):
        spans = []
        for secret in patterns:
            remaining()
            start = 0
            while True:
                index = text.find(secret, start)
                if index < 0:
                    break
                spans.append((index, index + len(secret)))
                if len(spans) > LIMIT:
                    fail("output_limit")
                if len(spans) % 1024 == 0:
                    remaining()
                start = index + len(secret)
        remaining()
        spans.sort()
        output, position = [], 0
        for start, end in spans:
            if end <= position:
                continue
            if start >= position:
                output.extend((text[position:start], "[REDACTED]"))
            position = end
        output.append(text[position:])
        remaining()
        return "".join(output)

    def visit(item):
        nonlocal budget
        remaining()
        if isinstance(item, str):
            item = redact_text(item) if patterns else item
            budget -= len(encode(item))
            if budget < 0:
                fail("output_limit")
            return item
        if isinstance(item, list):
            return [visit(v) for v in item]
        if isinstance(item, dict):
            result = {}
            for key, val in item.items():
                key = visit(key)
                if key in result:
                    fail("credential_in_structure")
                result[key] = visit(val)
            return result
        if type(item) in (int, float) and str(item) in secrets:
            return "[REDACTED]"
        return item

    return visit(value)


class Client:
    def __init__(self, spec, env):
        if os.name != "posix":
            fail("unsupported_platform")
        self.deadline = time.monotonic() + spec.get("timeout_secs", 20)
        self.process = subprocess.Popen([spec["command"], *spec.get("args", [])], cwd=spec.get("cwd"), env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                        start_new_session=True, bufsize=0)
        self.selector = None
        try:
            self.selector = selectors.DefaultSelector()
            for stream in (self.process.stdin, self.process.stdout):
                os.set_blocking(stream.fileno(), False)
            self.selector.register(self.process.stdout, selectors.EVENT_READ)
        except BaseException:
            self.close()
            raise
        self.buffer = bytearray()
        self.received = 0
        self.next_id = 0

    def remaining(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            fail("timeout")
        return remaining

    def read_ready(self):
        data = os.read(self.process.stdout.fileno(), 65536)
        if not data:
            fail("server_closed")
        self.received += len(data)
        if self.received > WIRE_LIMIT:
            fail("output_limit")
        self.buffer.extend(data)
        if len(self.buffer) > LIMIT:
            fail("output_limit")

    def send(self, message):
        data = encode(message) + b"\n"
        if len(data) > LIMIT:
            fail("input_limit")
        self.selector.register(self.process.stdin, selectors.EVENT_WRITE)
        try:
            while data:
                events = self.selector.select(self.remaining())
                if not events:
                    fail("timeout")
                for key, _ in events:
                    if key.fileobj is self.process.stdout:
                        self.read_ready()
                    else:
                        count = os.write(self.process.stdin.fileno(), data)
                        data = data[count:]
        finally:
            self.selector.unregister(self.process.stdin)

    def message(self):
        while b"\n" not in self.buffer:
            if not self.selector.select(self.remaining()):
                fail("timeout")
            self.read_ready()
        line, _, rest = self.buffer.partition(b"\n")
        self.buffer = bytearray(rest)
        message = decode(line)
        if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
            fail("protocol_error")
        return message

    def request(self, method, params):
        self.next_id += 1
        request_id = self.next_id
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        while True:
            self.remaining()
            message = self.message()
            if "method" in message:
                if not isinstance(message["method"], str):
                    fail("protocol_error")
                if "id" in message:
                    # No sampling, elicitation, roots or host authority is granted.
                    if message["method"] == "ping":
                        self.send({"jsonrpc": "2.0", "id": message["id"], "result": {}})
                    else:
                        self.send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Client capability unavailable"}})
                continue
            if type(message.get("id")) is not int or message["id"] != request_id or ("result" in message) == ("error" in message):
                fail("protocol_error")
            if "error" in message:
                fail("server_error")  # Never return server error text/data.
            if not isinstance(message["result"], dict):
                fail("protocol_error")
            return message["result"]

    def initialize(self):
        result = self.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "praxis-mcp", "version": "1.0.0"}})
        if result.get("protocolVersion") != PROTOCOL or not isinstance(result.get("serverInfo"), dict) or not isinstance(result.get("capabilities"), dict) or not isinstance(result["capabilities"].get("tools"), dict):
            fail("unsupported_server")
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def tools(self, allowed):
        tools, names, cursors = [], set(), set()
        params = {}
        for _ in range(16):
            result = self.request("tools/list", params)
            page = result.get("tools")
            if not isinstance(page, list):
                fail("protocol_error")
            for tool in page:
                if not isinstance(tool, dict) or not isinstance(tool.get("name"), str) or tool["name"] in names or not isinstance(tool.get("inputSchema"), dict):
                    fail("protocol_error")
                names.add(tool["name"])
                if tool["name"] in allowed:
                    tools.append(tool)
            cursor = result.get("nextCursor")
            if cursor is None:
                return tools
            if not isinstance(cursor, str) or not cursor or cursor in cursors:
                fail("protocol_error")
            cursors.add(cursor)
            params = {"cursor": cursor}
        fail("pagination_limit")

    def close(self):
        if self.selector is not None:
            self.selector.close()
        # EOF permits normal MCP cleanup before bounded group termination.
        self.process.stdin.close()
        try:
            self.process.wait(timeout=0.2)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                self.process.wait(timeout=0.2)
            except subprocess.TimeoutExpired:
                pass
        # Reap descendants too, even if the direct server already exited.
        try:
            os.killpg(self.process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        self.process.wait(timeout=0.5)
        self.process.stdout.close()


def run(action):
    raw = os.environ.get("PLUGIN_ARGS", "{}")
    if len(raw.encode("utf-8")) > LIMIT:
        fail("input_limit")
    args = decode(raw)
    required = {"server", "tool", "arguments"} if action == "call" else ({"server"} if action == "tools" else set())
    if not isinstance(args, dict) or set(args) != required:
        fail("invalid_arguments")
    if action != "servers" and (not isinstance(args["server"], str) or not NAME.fullmatch(args["server"])):
        fail("invalid_arguments")
    if action == "call" and (not isinstance(args["tool"], str) or not NAME.fullmatch(args["tool"]) or not isinstance(args["arguments"], dict)):
        fail("invalid_arguments")
    servers = load_config()
    if action == "servers":
        return {"servers": sorted(servers), "transport": "stdio"}
    spec = servers.get(args["server"])
    if spec is None:
        fail("unknown_server")
    if action == "call" and args["tool"] not in spec["allow_tools"]:
        fail("tool_not_allowed")
    env, secrets = environment(spec)
    client = Client(spec, env)
    try:
        client.initialize()
        tools = client.tools(spec["allow_tools"])
        if action == "tools":
            result = {"tools": tools}
        else:
            if args["tool"] not in {tool["name"] for tool in tools}:
                fail("unknown_tool")
            result = client.request("tools/call", {"name": args["tool"], "arguments": args["arguments"]})
            if not isinstance(result.get("content"), list) or ("isError" in result and type(result["isError"]) is not bool):
                fail("protocol_error")
        safe = redact(result, secrets, client.remaining)
        if action == "tools":
            for original, scrubbed in zip(result["tools"], safe["tools"]):
                if original["name"] != scrubbed["name"] or original["inputSchema"] != scrubbed["inputSchema"]:
                    fail("credential_in_structure")
        output = {"server": args["server"], "result": safe, "untrusted": True, "verified": False}
        client.remaining()
        if len(encode(output)) > LIMIT:
            fail("output_limit")
        return output
    finally:
        client.close()


def main(action):
    try:
        print(encode(run(action)).decode("utf-8"))
    except MCPError as error:
        print(json.dumps({"error": str(error), "retry_safe": False, "verified": False}))
        sys.exit(1)
    except Exception:
        # Paths, OS errors, command lines and server stderr may contain secrets.
        print(json.dumps({"error": "mcp_failure", "retry_safe": False, "verified": False}))
        sys.exit(1)


if __name__ == "__main__":
    main("servers")
