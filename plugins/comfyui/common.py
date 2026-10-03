"""Standalone ComfyUI plugin I/O. Standard library only; no GPU/provider fallback."""
import hashlib
import ipaddress
import json
import math
import mimetypes
import os
from pathlib import Path
import queue
import re
import stat
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

JSON_LIMIT = 16 * 1024 * 1024
WORKFLOW_LIMIT = 4 * 1024 * 1024
MAX_FILES = 32


class ComfyError(Exception):
    def __init__(self, code, message, **details):
        super().__init__(message)
        self.result = {"error": message, "code": code, "retry_safe": False, **details}


def text(value, field, maximum=4096):
    if not isinstance(value, str) or not value.strip() or len(value) > maximum or any(ord(c) < 32 for c in value):
        raise ComfyError("invalid_arguments", field + " must be a nonempty string without control characters")
    return value


def number(value, field, low, high, integer=False):
    if type(value) not in ((int,) if integer else (int, float)) or not math.isfinite(value) or not low <= value <= high:
        raise ComfyError("invalid_arguments", f"{field} must be {'an integer' if integer else 'a number'} in {low}..{high}")
    return value


def boolean(value, field):
    if type(value) is not bool:
        raise ComfyError("invalid_arguments", field + " must be boolean")
    return value


def fields(args, allowed, required=()):
    if not isinstance(args, dict) or set(args) - set(allowed) or set(required) - set(args):
        raise ComfyError("invalid_arguments", "Missing required or unexpected arguments; follow this tool's schema")


def encode(value):
    try:
        return json.dumps(value, ensure_ascii=False, allow_nan=False).encode("utf-8")
    except (ValueError, TypeError, UnicodeError):
        raise ComfyError("invalid_arguments", "Expected finite, UTF-8 JSON data") from None


def decode(data):
    try:
        def bad_constant(_):
            raise ValueError()
        return json.loads(data, parse_constant=bad_constant)
    except (ValueError, UnicodeError, RecursionError):
        raise ComfyError("invalid_response", "Expected valid JSON data") from None


def inputs():
    values = []
    for key in ("PLUGIN_ARGS", "PLUGIN_CONTEXT", "PLUGIN_SECRETS"):
        value = decode(os.environ.get(key) or "{}")
        if not isinstance(value, dict):
            raise ComfyError("configuration", key + " must contain a JSON object")
        values.append(value)
    return values


def setting(context, key, env, default=""):
    for value in (context.get(key), os.environ.get(env), default):
        if value is not None and value != "":
            return value
    return ""


def safe_relative(value, field, empty=False):
    if empty and value == "":
        return value
    text(value, field, 512)
    # ComfyUI interprets [input]/[output] suffixes, and userdata can unquote twice.
    if any(c in value for c in "\\:%[]\x7f") or any(part in ("", ".", "..") for part in value.split("/")):
        raise ComfyError("unsafe_path", field + " must be a safe relative path without annotations or traversal")
    return value


def descriptor(value, upload=False):
    if not isinstance(value, dict):
        raise ComfyError("invalid_response", "Expected a ComfyUI file descriptor")
    name = safe_relative(value.get("name" if upload else "filename"), "remote filename")
    if "/" in name:
        raise ComfyError("unsafe_path", "Remote filename must not contain a directory")
    folder = safe_relative(value.get("subfolder", ""), "remote subfolder", empty=True)
    kind = value.get("type")
    if kind not in (("input",) if upload else ("output", "temp", "input")):
        raise ComfyError("invalid_response", "Unexpected ComfyUI file descriptor type")
    return {"filename": name, "subfolder": folder, "type": kind}


def file_extension(filename):
    suffix = Path(filename).suffix.lower()
    return suffix if re.fullmatch(r"\.[a-z0-9_-]{1,32}", suffix) else ".bin"


def data_root():
    return Path(os.environ.get("DATA_DIR", "./data")).expanduser().resolve()


def local_path(value):
    value = text(value, "local file path")
    if "://" in value or value.startswith("data:"):
        raise ComfyError("invalid_arguments", "Use a file already on the Praxis host, not a URL or base64 data")
    restricted = None
    if value.startswith("/mnt/shared/"):
        restricted = data_root() / "shared"
        relative = safe_relative(value[len("/mnt/shared/"):], "shared path")
        path = restricted / relative
    elif value.startswith("/api/files/"):
        relative = safe_relative(value[len("/api/files/"):], "download path")
        if "/" in relative:
            raise ComfyError("unsafe_path", "Dashboard file reference must be one filename")
        restricted = data_root() / "uploads"
        path = restricted / relative
    else:
        path = Path(value).expanduser()
    path = path.resolve()
    if restricted is not None and not path.is_relative_to(restricted.resolve()):
        raise ComfyError("unsafe_path", "File reference escapes its shared directory")
    # Plugins are trusted host code, but do not defeat explicit VM-only isolation.
    if os.environ.get("VM_ENABLED", "").lower() in ("true", "1") and os.environ.get("VM_MODE") == "vm":
        if not path.is_relative_to((data_root() / "shared").resolve()):
            raise ComfyError("unsafe_path", "VM-only mode permits only files in DATA_DIR/shared")
    if not path.is_file():
        raise ComfyError("file_not_found", "File is missing or not a regular file; use its exact existing local path")
    return path


def read_file(path, limit):
    # Nonblocking open avoids hanging on a file replaced with a FIFO after preflight.
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ComfyError("invalid_arguments", "Only regular files can be read")
        if info.st_size > limit:
            raise ComfyError("file_too_large", "Local file exceeds the configured byte limit")
        data = source.read(limit + 1)
        if len(data) > limit:
            raise ComfyError("file_too_large", "Local file exceeds the configured byte limit")
        return data


def node_errors(response):
    """Useful validation diagnostics without echoing remote prompts/paths/secrets."""
    errors = response.get("node_errors") if isinstance(response, dict) else None
    if not isinstance(errors, dict):
        return {}
    result = {}
    for node_id, entry in list(errors.items())[:64]:
        if not re.fullmatch(r"[A-Za-z0-9_.:-]{1,128}", node_id) or not isinstance(entry, dict):
            continue
        kinds = [error.get("type") for error in entry.get("errors", [])[:16] if isinstance(error, dict)]
        result[node_id] = [kind for kind in kinds if isinstance(kind, str) and re.fullmatch(r"[A-Za-z0-9_]{1,80}", kind)]
    return result


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Client:
    def __init__(self, context, secrets):
        value = setting(context, "comfyui_workflow_base_url", "COMFYUI_BASE_URL")
        try:
            value = text(value, "COMFYUI_BASE_URL").rstrip("/")
            parsed = urllib.parse.urlsplit(value)
            if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username is not None or parsed.password is not None or parsed.query or parsed.fragment:
                raise ValueError()
            _ = parsed.port
            if parsed.scheme == "http" and parsed.hostname != "localhost":
                ip = ipaddress.ip_address(parsed.hostname)
                private = ip.is_loopback or any(ip in network for network in (
                    ipaddress.ip_network("10.0.0.0/8"), ipaddress.ip_network("172.16.0.0/12"),
                    ipaddress.ip_network("192.168.0.0/16"), ipaddress.ip_network("100.64.0.0/10"),
                    ipaddress.ip_network("fc00::/7")))
                if not private:
                    raise ValueError()
        except (ValueError, ComfyError):
            raise ComfyError("configuration", "Configure COMFYUI_BASE_URL (or custom_data.comfyui_workflow_base_url): HTTPS, or HTTP on a private literal IP/localhost; no credentials/query/fragment") from None
        self.base = value
        try:
            self.timeout = float(os.environ.get("COMFYUI_WORKFLOW_TIMEOUT_SECONDS", "900"))
            number(self.timeout, "COMFYUI_WORKFLOW_TIMEOUT_SECONDS", 0.1, 3600)
            self.file_limit = int(os.environ.get("COMFYUI_MAX_FILE_BYTES", str(64 * 1024 * 1024)))
            number(self.file_limit, "COMFYUI_MAX_FILE_BYTES", 1, 1024 * 1024 * 1024, integer=True)
        except (ValueError, ComfyError):
            raise ComfyError("configuration", "Invalid ComfyUI timeout or file byte limit") from None
        self.deadline = time.monotonic() + self.timeout
        self.headers = {"Accept": "application/json"}
        key = secrets.get("comfyui_api_key") or os.environ.get("COMFYUI_API_KEY", "")
        user = setting(context, "comfyui_workflow_user", "COMFYUI_USER")
        for header, entry in (("Authorization", key), ("Comfy-User", user)):
            if entry and entry != "CHANGE_ME":
                if not isinstance(entry, str) or not entry.isascii() or any(ord(c) < 33 or ord(c) == 127 for c in entry):
                    raise ComfyError("configuration", "Invalid ComfyUI credential/profile header")
                self.headers[header] = "Bearer " + entry if header == "Authorization" else entry
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def request(self, method, route, body=None, query=None, content_type="application/json", limit=JSON_LIMIT):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise ComfyError("timeout", "ComfyUI operation exceeded its total time budget")
        results = queue.Queue(maxsize=1)
        url = self.base + route
        if query:
            url += "?" + urllib.parse.urlencode(query)

        def perform():
            try:
                request = urllib.request.Request(url, body, {"User-Agent": "Praxis (+https://getpraxis.boo)", **self.headers, "Content-Type": content_type}, method=method)
                with self.opener.open(request, timeout=min(30, remaining)) as response:
                    length = response.headers.get("Content-Length", "")
                    if length.isdecimal() and (len(length) > 12 or int(length) > limit):
                        raise ComfyError("response_too_large", "ComfyUI response exceeds the byte limit")
                    data = response.read(limit + 1)
                    if len(data) > limit:
                        raise ComfyError("response_too_large", "ComfyUI response exceeds the byte limit")
                    results.put(data)
            except urllib.error.HTTPError as error:
                status = error.code
                details = {"http_status": status}
                # Only schema error TYPES are returned; never echo arbitrary server bodies.
                if status == 400 and route == "/prompt":
                    try:
                        details["node_errors"] = node_errors(decode(error.read(64 * 1024)))
                    except Exception:
                        pass
                error.close()
                results.put(ComfyError("http_error", f"ComfyUI HTTP {status}; check server, credentials and workflow. No automatic retry.", **details))
            except ComfyError as error:
                results.put(error)
            except (TimeoutError, urllib.error.URLError) as error:
                timed_out = isinstance(error, TimeoutError) or isinstance(getattr(error, "reason", None), TimeoutError)
                results.put(ComfyError("timeout" if timed_out else "transport", "ComfyUI request timed out" if timed_out else "Cannot reach ComfyUI; check the configured endpoint"))
            except Exception:
                results.put(ComfyError("transport", "ComfyUI request failed; no automatic retry"))

        # Worker only performs HTTP, never writes files. Trickle reads/DNS cannot
        # overrun the operation wall-clock budget or publish late local outputs.
        threading.Thread(target=perform, daemon=True).start()
        try:
            result = results.get(timeout=remaining)
        except queue.Empty:
            raise ComfyError("timeout", "ComfyUI operation exceeded its total time budget; remote writes may have succeeded") from None
        if isinstance(result, ComfyError):
            raise result
        return result

    def json(self, method, route, body=None, query=None, limit=JSON_LIMIT):
        return decode(self.request(method, route, None if body is None else encode(body), query, limit=limit))

    def upload(self, path):
        payload = read_file(path, self.file_limit)
        boundary = "praxis_" + uuid.uuid4().hex
        filename = "praxis_" + uuid.uuid4().hex + file_extension(path.name)
        parts = []
        for key, value in (("type", "input"), ("subfolder", "praxis"), ("overwrite", "false")):
            parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
        parts.extend([f'--{boundary}\r\nContent-Disposition: form-data; name="image"; filename="{filename}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode(),
                      payload, f'\r\n--{boundary}--\r\n'.encode()])
        result = decode(self.request("POST", "/upload/image", b"".join(parts), content_type="multipart/form-data; boundary=" + boundary))
        remote = descriptor(result, upload=True)
        return remote, "/".join(filter(None, (remote["subfolder"], remote["filename"])))


class OutputBatch:
    """Exclusive atomic publication. Remote filenames never choose local paths."""
    def __enter__(self):
        self.root = data_root() / "uploads"
        self.root.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix=".praxis-comfyui-", dir=self.root)
        self.created = []
        return self

    def __exit__(self, kind, *_):
        if kind is not None:
            for path in self.created:
                path.unlink(missing_ok=True)
        self.temp.cleanup()

    def save(self, data, remote_name):
        name = "comfyui_" + uuid.uuid4().hex + file_extension(remote_name)
        destination = self.root / name
        with tempfile.NamedTemporaryFile(dir=self.temp.name, delete=False) as staged:
            staged.write(data)
            staged.flush()
            os.fsync(staged.fileno())
        os.link(staged.name, destination)
        self.created.append(destination)
        return {"path": str(destination), "download_url": "/api/files/" + name, "bytes": len(data),
                "mime_type": mimetypes.guess_type(remote_name)[0] or "application/octet-stream",
                "sha256": hashlib.sha256(data).hexdigest()}


def main(action):
    try:
        args, context, secrets = inputs()
        result = action(args, Client(context, secrets))
        print(encode(result).decode())
        return 0
    except ComfyError as error:
        result = error.result
    except OSError:
        result = {"code": "storage", "error": "Cannot read/write local files; check paths, DATA_DIR, permissions and space", "retry_safe": False}
    except Exception:
        result = {"code": "invalid_response", "error": "Invalid workflow, response or configuration; no automatic retry", "retry_safe": False}
    print(json.dumps(result))
    return 1
