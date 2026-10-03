#!/usr/bin/env python3
"""Offline contracts: real plugin subprocesses, only loopback HTTP, fake credentials."""
import base64
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parents[1]
SECRET = "synthetic-secret-never-print"
PRIVATE = "private-prompt-never-echo"
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl2nCIAAAAASUVORK5CYII=")
JPEG = b"\xff\xd8\xff\xe0" + b"synthetic-jpeg"
WEBP = b"RIFF\x10\x00\x00\x00WEBP" + b"synthetic-webp"
MP3 = b"ID3\x04\x00\x00\x00\x00\x00\x00" + b"synthetic-audio"
PLUGINS = {
    "elevenlabs_tts": ("elevenlabs_api_key", "ELEVENLABS_API_BASE", {"text": "Grüße 日本語", "voice_id": "test-voice"}),
    "openrouter_image": ("openrouter_api_key", "OPENROUTER_IMAGE_API_BASE", {"prompt": "A red panda"}),
}


def image_response(*images):
    return json.dumps({"data": [{"b64_json": base64.b64encode(data).decode(), "media_type": mime} for data, mime in images],
                       "usage": {"total_tokens": 42, "cost": 0.01}}).encode()


@contextlib.contextmanager
def mock_api(status=200, body=MP3, headers=None, delay=0, trickle=False):
    calls = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            calls.append((self.path, dict(self.headers), json.loads(self.rfile.read(int(self.headers["Content-Length"])))))
            time.sleep(delay)
            try:
                self.send_response(status)
                for name, value in (headers or {}).items():
                    self.send_header(name, value)
                self.end_headers()
                if trickle:
                    for byte in body:
                        self.wfile.write(bytes([byte]))
                        self.wfile.flush()
                        time.sleep(0.03)
                else:
                    self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def do_GET(self):
            calls.append((self.path, {}, {}))
            self.send_response(500)
            self.end_headers()

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", calls
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


class MediaPlugins(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def run_plugin(self, name, url, args=None, secrets=None, context=None, extra=None, script=None):
        key, base_env, defaults = PLUGINS[name]
        env = {"PATH": os.environ.get("PATH", os.defpath), "PYTHONDONTWRITEBYTECODE": "1",
               "PLUGIN_ARGS": json.dumps(defaults if args is None else args), "PLUGIN_CONTEXT": json.dumps(context or {}),
               "PLUGIN_SECRETS": json.dumps({key: SECRET} if secrets is None else secrets), "DATA_DIR": str(self.root), base_env: url}
        env.update(extra or {})
        result = subprocess.run([sys.executable, str(script or ROOT / "plugins" / name / "generate.py")],
                                env=env, cwd=self.root, capture_output=True, text=True, timeout=8)
        self.assertNotIn(SECRET, result.stdout + result.stderr)
        self.assertNotIn(PRIVATE, result.stdout + result.stderr)
        self.assertEqual(result.stderr, "", result.stderr)
        output = json.loads(result.stdout)
        self.assertNotIn("b64_json", result.stdout)
        self.assertNotIn("data:image/", result.stdout)
        return result.returncode, output

    def assert_failure(self, result, code):
        status, output = result
        self.assertNotEqual(status, 0)
        self.assertEqual(output["code"], code, output)
        self.assertFalse(output["retry_safe"])
        self.assertEqual(list(self.root.rglob("*.part")), [])
        self.assertEqual(list((self.root / "uploads").glob("*")), [])

    def assert_file(self, file, data, suffix):
        path = Path(file["path"])
        self.assertTrue(path.is_absolute())
        self.assertEqual(path.parent, self.root / "uploads")
        self.assertEqual(path.read_bytes(), data)
        self.assertEqual(path.suffix, suffix)
        self.assertEqual(file["bytes"], len(data))
        self.assertEqual(file["download_url"], "/api/files/" + path.name)

    def test_manifests_are_standalone_and_only_declare_own_key(self):
        for name, (key, _, _) in PLUGINS.items():
            manifest = json.loads((ROOT / "plugins" / name / "plugin.json").read_text())
            self.assertEqual(manifest["name"], name)
            self.assertEqual(manifest["secrets"], [key])
            self.assertTrue(manifest["enabled"])
            for tool in manifest["tools"]:
                self.assertTrue((ROOT / "plugins" / name / tool["handler"]["path"]).is_file())
                self.assertNotIn("api_key", tool["parameters"]["properties"])
        self.assertEqual((ROOT / "plugins/elevenlabs_tts/media_common.py").read_bytes(),
                         (ROOT / "plugins/openrouter_image/media_common.py").read_bytes())

    def test_elevenlabs_contract_unicode_voice_settings_and_unique_mp3(self):
        args = {"text": "Grüße 日本語", "voice_id": "test-voice", "model_id": "eleven_flash_v2_5",
                "language_code": "de", "stability": 0.5, "similarity_boost": 0.75, "style": 0.2, "speed": 1.2,
                "use_speaker_boost": True}
        with mock_api(headers={"Content-Type": "audio/mpeg"}) as (url, calls):
            first = self.run_plugin("elevenlabs_tts", url, args)
            second = self.run_plugin("elevenlabs_tts", url, args)
        self.assertEqual(first[0], 0, first)
        self.assertEqual(second[0], 0, second)
        self.assert_file(first[1], MP3, ".mp3")
        self.assert_file(second[1], MP3, ".mp3")
        self.assertNotEqual(first[1]["path"], second[1]["path"])
        endpoint, headers, body = calls[0]
        self.assertEqual(endpoint, "/v1/text-to-speech/test-voice?output_format=mp3_44100_128")
        self.assertEqual(headers["Xi-Api-Key"], SECRET)
        self.assertIn("Praxis", headers["User-Agent"])
        self.assertIn("https://getpraxis.boo", headers["User-Agent"])
        self.assertEqual(body["text"], args["text"])
        self.assertEqual(body["model_id"], args["model_id"])
        self.assertEqual(body["language_code"], "de")
        self.assertEqual(body["voice_settings"]["speed"], 1.2)
        self.assertNotIn("language_code", body["voice_settings"])
        self.assertEqual(len(calls), 2)

    def test_context_then_environment_defaults_and_placeholder_secret(self):
        with mock_api() as (url, calls):
            result = self.run_plugin("elevenlabs_tts", url, {"text": "hello"},
                                     secrets={"elevenlabs_api_key": "CHANGE_ME"},
                                     context={"elevenlabs_tts_voice_id": "context-voice", "elevenlabs_tts_model_id": "context-model"},
                                     extra={"ELEVENLABS_API_KEY": SECRET, "ELEVENLABS_VOICE_ID": "env-voice"})
        self.assertEqual(result[0], 0, result)
        self.assertIn("/context-voice?", calls[0][0])
        self.assertEqual(calls[0][2]["model_id"], "context-model")

    def test_openrouter_dedicated_image_api_multiple_formats_and_metadata(self):
        args = {"prompt": "Grüße 日本語", "model": "openai/gpt-image-2", "n": 3,
                "resolution": "2K", "aspect_ratio": "16:9", "quality": "high", "output_format": "png",
                "background": "opaque", "reference_images": ["https://example.org/reference.png"]}
        with mock_api(body=image_response((PNG, "image/png"), (JPEG, "image/jpeg"), (WEBP, "image/webp"))) as (url, calls):
            result = self.run_plugin("openrouter_image", url + "/api/v1", args)
        self.assertEqual(result[0], 0, result)
        for file, data, suffix in zip(result[1]["images"], [PNG, JPEG, WEBP], [".png", ".jpg", ".webp"]):
            self.assert_file(file, data, suffix)
        endpoint, headers, body = calls[0]
        self.assertEqual(endpoint, "/api/v1/images")
        self.assertEqual(headers["Authorization"], "Bearer " + SECRET)
        self.assertEqual(headers["Http-Referer"], "https://getpraxis.boo")
        self.assertEqual(headers["X-Openrouter-Title"], "Praxis")
        self.assertIn("https://getpraxis.boo", headers["User-Agent"])
        self.assertEqual(body["model"], "openai/gpt-image-2")
        self.assertEqual(body["prompt"], args["prompt"])
        self.assertEqual(body["n"], 3)
        self.assertEqual(body["resolution"], "2K")
        self.assertEqual(body["aspect_ratio"], "16:9")
        self.assertEqual(body["input_references"][0]["image_url"]["url"], args["reference_images"][0])
        self.assertEqual(body["provider"], {"allow_fallbacks": False})
        self.assertNotIn("messages", body)
        self.assertEqual(result[1]["usage"]["cost"], 0.01)
        self.assertEqual(len(calls), 1)

    def test_image_env_defaults_and_format_inference(self):
        body = json.dumps({"data": [{"b64_json": base64.b64encode(JPEG).decode()}]}).encode()
        with mock_api(body=body) as (url, calls):
            result = self.run_plugin("openrouter_image", url, extra={"OPENROUTER_IMAGE_MODEL": "test/image-model"})
        self.assertEqual(result[0], 0, result)
        self.assert_file(result[1]["images"][0], JPEG, ".jpg")
        self.assertEqual(calls[0][2]["model"], "test/image-model")
        self.assertEqual(calls[0][2]["n"], 1)
        self.assertNotIn("resolution", calls[0][2])

    def test_invalid_arguments_and_missing_keys_make_no_requests(self):
        cases = [("elevenlabs_tts", {}), ("elevenlabs_tts", {"text": "hello"}),
                 ("elevenlabs_tts", {"text": "x", "voice_id": "v", "speed": True}),
                 ("elevenlabs_tts", {"text": "x", "voice_id": "v", "stability": 2}),
                 ("elevenlabs_tts", {"text": "x", "voice_id": "../other"}),
                 ("elevenlabs_tts", {"text": "x", "voice_id": "v", "language_code": "de"}),
                 ("openrouter_image", {}), ("openrouter_image", {"prompt": "x", "n": 5}),
                 ("openrouter_image", {"prompt": "x", "output_format": "svg"}),
                 ("openrouter_image", {"prompt": "x", "reference_images": ["file:///etc/passwd"]}),
                 ("openrouter_image", {"prompt": "x", "background": "transparent", "output_format": "jpeg"}),
                 ("openrouter_image", {"prompt": "x", "api_key": "not-accepted"})]
        with mock_api() as (url, calls):
            for name, args in cases:
                with self.subTest(name=name, args=args):
                    self.assert_failure(self.run_plugin(name, url, args), "invalid_arguments")
            for name in PLUGINS:
                self.assert_failure(self.run_plugin(name, url, secrets={}), "configuration")
            self.assertEqual(calls, [])

    def test_provider_errors_are_redacted_never_retried_and_expose_retry_after(self):
        for name in PLUGINS:
            for status, code in [(400, "invalid_request"), (401, "authentication"), (402, "quota"), (429, "rate_limited"), (503, "unavailable")]:
                with self.subTest(name=name, status=status), mock_api(status, (SECRET + PRIVATE).encode(), {"Retry-After": "17"}) as (url, calls):
                    result = self.run_plugin(name, url)
                    self.assert_failure(result, code)
                    self.assertEqual(result[1]["http_status"], status)
                    self.assertEqual(result[1]["retry_after_seconds"], 17)
                    self.assertEqual(len(calls), 1)

    def test_redirects_never_forward_credentials(self):
        for name in PLUGINS:
            with mock_api() as (other, leaked):
                with mock_api(307, b"", {"Location": other + "/leak"}) as (url, calls):
                    self.assert_failure(self.run_plugin(name, url), "redirect")
                    self.assertEqual(len(calls), 1)
                self.assertEqual(leaked, [])

    def test_total_timeout_and_connection_failure(self):
        for name in PLUGINS:
            key = "ELEVENLABS_TTS_TIMEOUT_SECONDS" if name == "elevenlabs_tts" else "OPENROUTER_IMAGE_TIMEOUT_SECONDS"
            with mock_api(delay=0.7) as (url, calls):
                self.assert_failure(self.run_plugin(name, url, extra={key: "0.1"}), "timeout")
                self.assertEqual(len(calls), 1)
            self.assert_failure(self.run_plugin(name, url), "transport")
            with mock_api(body=b"x" * 300, trickle=True) as (url, calls):
                self.assert_failure(self.run_plugin(name, url, extra={key: "0.1"}), "timeout")
                self.assertEqual(len(calls), 1)

    def test_malformed_envelopes_config_and_explicit_null_make_no_requests(self):
        with mock_api() as (url, calls):
            for name in PLUGINS:
                for key, value, code in [("PLUGIN_ARGS", "[]", "invalid_arguments"),
                                         ("PLUGIN_CONTEXT", "{", "configuration"),
                                         ("PLUGIN_SECRETS", "null", "configuration")]:
                    self.assert_failure(self.run_plugin(name, url, extra={key: value}), code)
                self.assert_failure(self.run_plugin(name, url + "?private=" + SECRET), "configuration")
            self.assert_failure(self.run_plugin("openrouter_image", url, {"prompt": "x", "model": None}), "invalid_arguments")
            self.assertEqual(calls, [])

    def test_common_read_limit_without_content_length_and_failed_publish_cleanup(self):
        spec = importlib.util.spec_from_file_location("media_common_fixture", ROOT / "plugins/elevenlabs_tts/media_common.py")
        common = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(common)
        with mock_api(body=b"123456789") as (url, calls):
            with self.assertRaises(common.MediaError) as caught:
                common.post(url, {}, {}, 2, 4)
            self.assertEqual(caught.exception.result["code"], "response_too_large")
            self.assertEqual(len(calls), 1)
        original_link = os.link
        links = []
        def fail_second(source, destination):
            links.append(destination)
            if len(links) == 2:
                raise OSError("synthetic disk failure")
            return original_link(source, destination)
        with patch.dict(os.environ, {"DATA_DIR": str(self.root)}), patch.object(common.os, "link", side_effect=fail_second):
            with common.OutputBatch() as output:
                with self.assertRaises(OSError):
                    output.save([(PNG, ".png", "image/png")] * 2, "test")
        self.assertEqual(list((self.root / "uploads").iterdir()), [])

    def test_empty_invalid_or_oversized_responses_leave_no_artifacts(self):
        cases = [("elevenlabs_tts", b""), ("elevenlabs_tts", b'{"error":"invalid audio"}'),
                 ("openrouter_image", b"not-json"), ("openrouter_image", b'{"data":[]}'),
                 ("openrouter_image", b'{"error":{"message":"private-prompt-never-echo"}}'),
                 ("openrouter_image", b'{"data":[{"b64_json":"!!"}]}'),
                 ("openrouter_image", image_response((PNG, "image/jpeg"))),
                 ("openrouter_image", image_response((b"<svg/>", "image/svg+xml"))),
                 ("openrouter_image", b'{"data":[{"url":"http://127.0.0.1/private"}]}')]
        for name, body in cases:
            with self.subTest(name=name, body=body), mock_api(body=body) as (url, _):
                self.assert_failure(self.run_plugin(name, url), "invalid_response")
        for name in PLUGINS:
            with mock_api(body=b"", headers={"Content-Length": "999999999"}) as (url, _):
                self.assert_failure(self.run_plugin(name, url), "response_too_large")

    def test_image_batch_is_validated_before_writing_any_files(self):
        with mock_api(body=image_response((PNG, "image/png"), (b"invalid", "image/png"))) as (url, _):
            self.assert_failure(self.run_plugin("openrouter_image", url, {"prompt": "x", "n": 2}), "invalid_response")

    def test_output_preflight_fails_before_paid_request(self):
        (self.root / "uploads").write_text("not a directory")
        with mock_api() as (url, calls):
            for name in PLUGINS:
                status, result = self.run_plugin(name, url)
                self.assertNotEqual(status, 0)
                self.assertEqual(result["code"], "storage")
            self.assertEqual(calls, [])

    def test_install_copy_runs_without_repository_imports(self):
        for name in PLUGINS:
            installed = self.root / "installed" / name
            shutil.copytree(ROOT / "plugins" / name, installed)
            data = MP3 if name == "elevenlabs_tts" else image_response((PNG, "image/png"))
            with mock_api(body=data) as (url, _):
                self.assertEqual(self.run_plugin(name, url, script=installed / "generate.py")[0], 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
