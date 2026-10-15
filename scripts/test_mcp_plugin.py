#!/usr/bin/env python3
"""Offline black-box tests; this file also serves as the mock MCP server."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def mock():
    mode = sys.argv[2]
    for line in sys.stdin:
        request = json.loads(line)
        with open(os.environ["MOCK_LOG"], "a") as log:
            log.write(json.dumps({**request, "mock_pid": os.getpid()}) + "\n")
        method = request["method"]
        if method == "notifications/initialized":
            continue
        if method == "initialize":
            assert request["params"]["protocolVersion"] == "2024-11-05"
            assert request["params"]["capabilities"] == {}
            if mode == "timeout":
                time.sleep(10)
            if mode == "close":
                return
            if mode == "flood":
                sys.stdout.write("x" * (600 * 1024))
                sys.stdout.flush()
                time.sleep(10)
            if mode == "invalid_json":
                print("not JSON", flush=True)
                continue
            result = {"protocolVersion": "wrong" if mode == "version" else "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "mock", "version": "1"}}
        elif method == "tools/list":
            if mode == "ping":
                print(json.dumps({"jsonrpc": "2.0", "id": "ping-1", "method": "ping"}), flush=True)
                answer = json.loads(sys.stdin.readline())
                assert answer == {"jsonrpc": "2.0", "id": "ping-1", "result": {}}
            if mode == "request":
                print(json.dumps({"jsonrpc": "2.0", "id": "host-request", "method": "sampling/createMessage", "params": {}}), flush=True)
                answer = json.loads(sys.stdin.readline())
                assert answer["error"]["code"] == -32601
            result = {"tools": [{"name": "echo", "description": "Mock tool " + os.environ.get("TOKEN", ""), "inputSchema": {"type": "object"}}]}
            if mode == "pagination" and not request["params"]:
                result["nextCursor"] = "page2"
            elif mode == "pagination":
                result = {"tools": [{"name": "blocked", "inputSchema": {"type": "object"}}]}
            elif mode == "cursor_loop":
                result = {"tools": [], "nextCursor": "same"}
            elif mode == "missing":
                result = {"tools": []}
            elif mode == "schema_secret":
                result["tools"][0]["inputSchema"] = {"properties": {os.environ["TOKEN"]: {"type": "string"}}}
        elif method == "tools/call":
            assert request["params"]["name"] == "echo"
            if mode == "error":
                print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -1, "message": os.environ.get("TOKEN")}}), flush=True)
                continue
            if mode == "env":
                value = json.dumps(sorted(os.environ))
            elif mode == "path":
                value = os.environ["PATH"]
            elif mode == "secret":
                value = os.environ["TOKEN"]
                print(value, file=sys.stderr, flush=True)
            elif mode == "numeric_secret":
                value = 123456
            else:
                value = json.dumps(request["params"]["arguments"], ensure_ascii=False)
            result = {"content": [{"type": "text", "text": value}], "isError": mode == "tool_error"}
        else:
            raise AssertionError(method)
        print(json.dumps({"jsonrpc": "2.0", "id": 999 if mode == "wrong_id" else request["id"], "result": result}), flush=True)
        if mode == "blocked_stdin" and method == "tools/list":
            time.sleep(10)
    if mode == "graceful":
        Path(os.environ["MOCK_LOG"] + ".eof").write_text("clean shutdown")


class MCPTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.config = Path(self.temp.name) / "servers.json"
        self.log = Path(self.temp.name) / "requests.jsonl"

    def invoke(self, action="call", args=None, mode="normal", spec=None, env_overrides=None):
        server = {"command": sys.executable, "args": [str(Path(__file__).resolve()), "--mock", mode],
                  "allow_tools": ["echo"], "env_from": {"TOKEN": "mcp_service_token", "MOCK_LOG": "mock_log"}, "timeout_secs": 1}
        if spec:
            server.update(spec)
        self.config.write_text(json.dumps({"servers": {"local": server}}))
        env = {**os.environ, "PRAXIS_MCP_CONFIG": str(self.config),
               "PRIVATE_ADMIN_KEY": "must-not-inherit", "PRIVATE_PROVIDER_KEY": "provider-must-not-inherit",
               "PYTHONDONTWRITEBYTECODE": "1",
               "PLUGIN_SECRETS": json.dumps({"mcp_service_token": "synthetic-private-token", "mock_log": str(self.log),
                                             "other_key": "another-private-token"}),
               "PLUGIN_ARGS": json.dumps(args if args is not None else {"server": "local", "tool": "echo", "arguments": {"text": "Grüße"}})}
        for key in ("mcp_service_token", "mock_log"):
            env.pop(key, None)
        for key, value in (env_overrides or {}).items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
        result = subprocess.run([sys.executable, str(ROOT / "plugins/mcp" / (action + ".py"))], env=env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.stderr, "")
        self.assertNotIn("synthetic-private-token", result.stdout)
        self.assertNotIn("another-private-token", result.stdout)
        for request in self.requests():
            with self.assertRaises(ProcessLookupError):
                os.kill(request["mock_pid"], 0)
        return result.returncode, json.loads(result.stdout)

    def requests(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_manifest(self):
        manifest = json.loads((ROOT / "plugins/mcp/plugin.json").read_text())
        self.assertEqual(manifest["name"], "mcp")
        self.assertEqual({t["name"] for t in manifest["tools"]}, {"mcp_list_servers", "mcp_list_tools", "mcp_call_tool"})
        for tool in manifest["tools"]:
            self.assertTrue((ROOT / "plugins/mcp" / tool["handler"]["path"]).is_file())
            self.assertFalse(tool["parameters"]["additionalProperties"])

    def test_list_servers_no_spawn(self):
        code, result = self.invoke("servers", {})
        self.assertEqual(code, 0)
        self.assertEqual(result["servers"], ["local"])
        self.assertEqual(self.requests(), [])
        self.assertNotIn(sys.executable, json.dumps(result))

    def test_handshake_and_call(self):
        code, result = self.invoke()
        self.assertEqual(code, 0)
        self.assertTrue(result["untrusted"])
        self.assertFalse(result["verified"])
        self.assertEqual(json.loads(result["result"]["content"][0]["text"]), {"text": "Grüße"})
        self.assertEqual([r["method"] for r in self.requests()], ["initialize", "notifications/initialized", "tools/list", "tools/call"])

    def test_tools_pagination_and_allowlist(self):
        code, result = self.invoke("tools", {"server": "local"}, "pagination")
        self.assertEqual(code, 0)
        self.assertEqual([t["name"] for t in result["result"]["tools"]], ["echo"])
        self.assertEqual(self.requests()[-1]["params"], {"cursor": "page2"})

    def test_input_rejected_without_spawn(self):
        for args in ({"server": "local", "tool": "blocked", "arguments": {}},
                     {"server": "missing", "tool": "echo", "arguments": {}},
                     {"server": "local", "tool": "echo", "arguments": {}, "command": "/bin/sh"},
                     {"server": "local", "tool": "echo", "arguments": []},
                     {"server": {"command": "/bin/sh"}, "tool": "echo", "arguments": {}}):
            with self.subTest(args=args):
                code, _ = self.invoke(args=args)
                self.assertNotEqual(code, 0)
                self.assertEqual(self.requests(), [])

    def test_configuration_rejected(self):
        for spec in ({"command": "python3"}, {"allow_tools": None}, {"timeout_secs": 100}, {"env_from": {"TOKEN": "NOT SET"}}, {"cwd": "relative"}):
            with self.subTest(spec=spec):
                code, _ = self.invoke(spec=spec)
                self.assertNotEqual(code, 0)
                self.assertEqual(self.requests(), [])

    def test_scoped_secret_only_and_ambient_conflict(self):
        for overrides in ({}, {"mcp_service_token": "ambient-must-not-win"}):
            with self.subTest(overrides=overrides):
                code, result = self.invoke(mode="secret", env_overrides=overrides)
                self.assertEqual(code, 0)
                self.assertEqual(result["result"]["content"][0]["text"], "[REDACTED]")

    def test_ambient_secret_never_used(self):
        for envelope in (None, "{}", json.dumps({"mock_log": str(self.log)})):
            with self.subTest(envelope=envelope):
                code, result = self.invoke(env_overrides={"PLUGIN_SECRETS": envelope,
                                                         "mcp_service_token": "synthetic-private-token",
                                                         "mock_log": str(self.log)})
                self.assertNotEqual(code, 0)
                self.assertEqual(result["error"], "missing_environment")
                self.assertEqual(self.requests(), [])

    def test_invalid_secret_envelope_fails_before_spawn(self):
        valid = {"mcp_service_token": "synthetic-private-token", "mock_log": str(self.log)}
        envelopes = ["not JSON", "[]", "null", '"string"', "123", "NaN",
                     '{"key":"first","key":"second"}']
        envelopes.extend(json.dumps({**valid, "other_key": value}) for value in (None, 1, [], {}, "bad\0value"))
        envelopes.extend(json.dumps({**valid, "mcp_service_token": value}) for value in ("", "bad\0value"))
        for envelope in envelopes:
            with self.subTest(envelope=envelope[:100]):
                code, _ = self.invoke(env_overrides={"PLUGIN_SECRETS": envelope,
                                                    "mcp_service_token": "synthetic-private-token",
                                                    "mock_log": str(self.log)})
                self.assertNotEqual(code, 0)
                self.assertEqual(self.requests(), [])

    def test_secret_envelope_size_limit(self):
        # A >512 KiB env entry exceeds Linux exec limits, so test in-process.
        sys.path.insert(0, str(ROOT / "plugins/mcp"))
        try:
            import bridge
            with patch.dict(os.environ, {"PLUGIN_SECRETS": " " * (bridge.LIMIT + 1)}):
                with self.assertRaises(bridge.MCPError):
                    bridge.environment({})
        finally:
            sys.path.pop(0)

    def test_safe_path_forwarding(self):
        code, result = self.invoke(mode="path", env_overrides={"PATH": "/safe/bin:/usr/bin"})
        self.assertEqual(code, 0)
        self.assertEqual(result["result"]["content"][0]["text"], "/safe/bin:/usr/bin")

    def test_untransferred_scoped_secret_redacted(self):
        code, result = self.invoke(args={"server": "local", "tool": "echo", "arguments": {"text": "another-private-token"}})
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(result["result"]["content"][0]["text"]), {"text": "[REDACTED]"})

    def test_environment_isolation_and_secret_redaction(self):
        code, result = self.invoke(mode="env")
        self.assertEqual(code, 0)
        child_env = json.loads(result["result"]["content"][0]["text"])
        for key in ("PRIVATE_ADMIN_KEY", "PLUGIN_ARGS", "PLUGIN_CONTEXT", "PLUGIN_SECRETS", "MOCK_TOKEN", "mcp_service_token", "mock_log", "PRIVATE_PROVIDER_KEY", "PRAXIS_MCP_CONFIG"):
            self.assertNotIn(key, child_env)
        code, result = self.invoke(mode="secret")
        self.assertEqual(code, 0)
        self.assertEqual(result["result"]["content"][0]["text"], "[REDACTED]")

    def test_failures_are_bounded_and_sanitized(self):
        for mode, expected in (("timeout", "timeout"), ("flood", "output_limit"), ("close", "server_closed"),
                               ("version", "unsupported_server"), ("wrong_id", "protocol_error"),
                               ("invalid_json", "invalid_json"), ("error", "server_error"),
                               ("cursor_loop", "protocol_error"), ("missing", "unknown_tool")):
            with self.subTest(mode=mode):
                started = time.monotonic()
                code, result = self.invoke(mode=mode, spec={"timeout_secs": 0.3})
                self.assertNotEqual(code, 0)
                self.assertEqual(result["error"], expected)
                self.assertFalse(result["retry_safe"])
                self.assertLess(time.monotonic() - started, 3)

    def test_blocked_stdin_deadline(self):
        code, result = self.invoke(mode="blocked_stdin", args={"server": "local", "tool": "echo", "arguments": {"text": "x" * 100000}}, spec={"timeout_secs": 0.3})
        self.assertNotEqual(code, 0)
        self.assertEqual(result["error"], "timeout")

    def test_schema_credentials_fail_closed(self):
        code, result = self.invoke("tools", {"server": "local"}, "schema_secret")
        self.assertNotEqual(code, 0)
        self.assertEqual(result["error"], "credential_in_structure")

    def test_redaction_no_marker_replacement_or_collisions(self):
        sys.path.insert(0, str(ROOT / "plugins/mcp"))
        try:
            import bridge
            self.assertEqual(bridge.redact("secret secret", ["secret", "R", "E"], lambda: 1), "[REDACTED] [REDACTED]")
            self.assertEqual(bridge.redact(123456, ["123456"], lambda: 1), "[REDACTED]")
            started = time.monotonic()
            text = "a" * 400000
            self.assertEqual(bridge.redact(text, ["a" * 50000 + "b", "x"], lambda: 1), text)
            self.assertLess(time.monotonic() - started, 1)
            with self.assertRaises(bridge.MCPError):
                bridge.redact({"secret": 1, "[REDACTED]": 2}, ["secret"], lambda: 1)
        finally:
            sys.path.pop(0)

    def test_graceful_stdio_shutdown(self):
        code, _ = self.invoke(mode="graceful")
        self.assertEqual(code, 0)
        self.assertEqual(Path(str(self.log) + ".eof").read_text(), "clean shutdown")

    def test_server_ping_supported(self):
        code, _ = self.invoke(mode="ping")
        self.assertEqual(code, 0)

    def test_server_requests_not_granted(self):
        code, _ = self.invoke(mode="request")
        self.assertEqual(code, 0)

    def test_tool_error_preserved(self):
        code, result = self.invoke(mode="tool_error")
        self.assertEqual(code, 0)
        self.assertTrue(result["result"]["isError"])


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--mock":
        mock()
    else:
        unittest.main()
