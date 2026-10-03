"""Dependency-free media I/O. Kept identical in both standalone plugin folders."""
import datetime
import email.utils
import json
import math
import os
from pathlib import Path
import queue
import tempfile
import threading
import urllib.error
import urllib.parse
import urllib.request
import uuid


class MediaError(Exception):
    def __init__(self, code, message, **details):
        super().__init__(message)
        self.result = {"error": message, "code": code, "retry_safe": False, **details}


def inputs():
    values = []
    for key in ("PLUGIN_ARGS", "PLUGIN_CONTEXT", "PLUGIN_SECRETS"):
        try:
            value = json.loads(os.environ.get(key) or "{}")
            if not isinstance(value, dict):
                raise ValueError()
        except (ValueError, TypeError):
            raise MediaError("invalid_arguments" if key == "PLUGIN_ARGS" else "configuration", key + " must be a JSON object") from None
        values.append(value)
    return values


def text(value, field, maximum=128):
    if not isinstance(value, str) or not value.strip() or len(value) > maximum:
        raise MediaError("invalid_arguments", field + " must be a non-empty string within its documented length limit")
    return value


def setting(args, context, argument, context_key, environment, default=""):
    if argument in args:
        return args[argument]  # Explicit malformed/empty values must fail validation.
    for value in (context.get(context_key), os.environ.get(environment), default):
        if value is not None and value != "":
            return value
    return ""


def api_key(secrets, key, environment):
    for value in (secrets.get(key), os.environ.get(environment)):
        if isinstance(value, str) and value.strip() and value.strip() != "CHANGE_ME":
            value = value.strip()
            if not value.isascii() or any(ord(c) < 33 or ord(c) == 127 for c in value):
                break
            return value
    raise MediaError("configuration", "Configure " + key + " in Praxis secrets or " + environment)


def base_url(environment, default):
    value = os.environ.get(environment, default).rstrip("/")
    try:
        parsed = urllib.parse.urlsplit(value)
        # Plain HTTP is ONLY a loopback test/development option. Never model input.
        secure = parsed.scheme == "https" or (parsed.scheme == "http" and parsed.hostname in ("localhost", "127.0.0.1", "::1"))
        if not secure or not parsed.hostname or parsed.username is not None or parsed.password is not None or parsed.query or parsed.fragment:
            raise ValueError()
        _ = parsed.port
    except ValueError:
        raise MediaError("configuration", environment + " must be an HTTPS base URL without credentials/query/fragment (HTTP allowed on loopback)") from None
    return value


def timeout_seconds(environment, default):
    try:
        value = float(os.environ.get(environment, default))
        if not math.isfinite(value) or not 0.1 <= value <= 600:
            raise ValueError()
        return value
    except ValueError:
        raise MediaError("configuration", environment + " must be between 0.1 and 600 seconds") from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def post(url, headers, body, timeout, limit):
    """Exactly one POST, no redirects/retries; a wall-clock budget includes DNS/body.

    The daemon only does HTTP. It never writes output files. On deadline the
    caller exits, so a slow peer cannot keep the tool alive via trickle reads.
    """
    results = queue.Queue(maxsize=1)

    def request():
        try:
            req = urllib.request.Request(url, json.dumps(body, ensure_ascii=False, allow_nan=False).encode(),
                                         {"Content-Type": "application/json", "User-Agent": "Praxis (+https://getpraxis.boo)", **headers}, method="POST")
            with urllib.request.build_opener(NoRedirect()).open(req, timeout=timeout) as response:
                length = response.headers.get("Content-Length", "")
                if length.isdecimal() and (len(length) > 12 or int(length) > limit):
                    raise MediaError("response_too_large", "Provider response exceeds the media byte limit")
                data = response.read(limit + 1)
                if len(data) > limit:
                    raise MediaError("response_too_large", "Provider response exceeds the media byte limit")
                results.put(data)
        except urllib.error.HTTPError as error:
            status = error.code
            code = ("redirect" if 300 <= status < 400 else "authentication" if status in (401, 403)
                    else "quota" if status == 402 else "rate_limited" if status == 429
                    else "unavailable" if status >= 500 else "invalid_request")
            details = {"http_status": status}
            retry = error.headers.get("Retry-After", "").strip()
            try:
                if retry.isdecimal() and len(retry) <= 10:
                    seconds = int(retry)
                else:
                    when = email.utils.parsedate_to_datetime(retry)
                    seconds = math.ceil((when - datetime.datetime.now(datetime.timezone.utc)).total_seconds())
                details["retry_after_seconds"] = max(0, seconds)
            except (ValueError, TypeError, OverflowError):
                pass
            error.close()  # Never read/echo provider error bodies, URLs or credentials.
            results.put(MediaError(code, "Provider HTTP " + str(status) + "; check credentials, quota, model and parameters. No automatic retry.", **details))
        except MediaError as error:
            results.put(error)
        except (TimeoutError, urllib.error.URLError) as error:
            is_timeout = isinstance(error, TimeoutError) or isinstance(getattr(error, "reason", None), TimeoutError)
            results.put(MediaError("timeout" if is_timeout else "transport", "Media request timed out" if is_timeout else "Media connection failed; check network and endpoint"))
        except Exception:
            results.put(MediaError("transport", "Media request failed; no automatic retry"))

    threading.Thread(target=request, daemon=True).start()
    try:
        result = results.get(timeout=timeout)
    except queue.Empty:
        raise MediaError("timeout", "Media request exceeded its total time budget; no automatic retry") from None
    if isinstance(result, MediaError):
        raise result
    return result


class OutputBatch:
    """Preflight storage before spending; exclusive publication, cleanup on error."""
    def __enter__(self):
        self.root = Path(os.environ.get("DATA_DIR", "./data")).resolve() / "uploads"
        self.root.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix=".praxis-media-", dir=self.root)
        return self

    def __exit__(self, *_):
        self.temp.cleanup()

    def save(self, items, prefix):
        files = []
        created = []
        try:
            for data, extension, mime in items:
                name = prefix + "_" + uuid.uuid4().hex + extension
                destination = self.root / name
                with tempfile.NamedTemporaryFile(dir=self.temp.name, delete=False) as staged:
                    staged.write(data)
                    staged.flush()
                    os.fsync(staged.fileno())
                # link() is atomic and refuses an existing destination, including symlinks.
                os.link(staged.name, destination)
                created.append(destination)
                files.append({"path": str(destination), "download_url": "/api/files/" + name,
                              "mime_type": mime, "bytes": len(data)})
            return files
        except OSError:
            for path in created:
                path.unlink(missing_ok=True)
            raise


def main(generate):
    try:
        result = generate()
    except MediaError as error:
        print(json.dumps(error.result))
        return 1
    except OSError:
        print(json.dumps({"error": "Cannot prepare or save media output; check DATA_DIR storage and permissions", "code": "storage", "retry_safe": False}))
        return 1
    except Exception:
        print(json.dumps({"error": "Invalid media response or configuration; no automatic retry", "code": "invalid_response", "retry_safe": False}))
        return 1
    print(json.dumps(result, ensure_ascii=False))
    return 0
