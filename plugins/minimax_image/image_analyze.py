#!/usr/bin/env python3
"""Praxis tool for MiniMax vision analysis (POST /v1/text/chatcompletion_v2)."""
import json
import re
import sys
import urllib.parse
from media_common import (
    MediaError,
    api_key,
    base_url,
    inputs,
    main,
    post,
    setting,
    text,
    timeout_seconds,
)

MAX_RESPONSE_BYTES = 4 * 1024 * 1024
DATA_URI_TYPES = {"png", "jpeg", "jpg", "webp", "gif"}


def image_source(value):
    value = text(value, "image_url", 4_000_000)
    if value.startswith("data:image/"):
        header, separator, payload = value.partition(";base64,")
        if not separator or not payload or not re.fullmatch(r"[A-Za-z0-9+/=]+", payload):
            raise MediaError("invalid_arguments", "image_url data URI must carry base64 image data")
        if header.split("/", 1)[1].split(";", 1)[0] not in DATA_URI_TYPES:
            raise MediaError("invalid_arguments", "image_url data URI must be PNG, JPEG, WebP or GIF")
        return value
    try:
        parsed = urllib.parse.urlsplit(value)
        if parsed.scheme != "https" or not parsed.hostname or parsed.username is not None or parsed.password is not None:
            raise ValueError()
        _ = parsed.port
    except ValueError:
        raise MediaError(
            "invalid_arguments",
            "image_url must be an HTTPS URL or a base64 data URI (local paths are not supported)",
        ) from None
    return value


def analyze():
    args, context, secrets = inputs()
    source = image_source(args.get("image_url"))
    question = text(args.get("question", "Describe this image in detail"), "question", 4000)
    model = setting(args, context, "model", "minimax_vision_model", "MINIMAX_VISION_MODEL", "MiniMax-VL-01")
    model = text(model, "model", 256)
    key = api_key(secrets, "minimax_api_key", "MINIMAX_API_KEY")
    base = base_url("MINIMAX_BASE_URL", "https://api.minimax.chat")
    timeout = timeout_seconds("MINIMAX_TIMEOUT_SECONDS", 300)
    body = {
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": question},
                {"type": "image_url", "image_url": {"url": source}},
            ],
        }],
    }
    raw = post(
        base + "/v1/text/chatcompletion_v2",
        {"Authorization": "Bearer " + key, "Accept": "application/json"},
        body,
        timeout,
        MAX_RESPONSE_BYTES,
    )
    try:
        response = json.loads(raw)
    except (ValueError, UnicodeError):
        raise MediaError("invalid_response", "MiniMax returned invalid vision JSON") from None
    error = response.get("error") if isinstance(response, dict) else None
    if isinstance(error, dict) and error:
        raise MediaError(
            "provider_error",
            "MiniMax vision API error: " + str(error.get("message") or "unknown"),
            retry_safe=False,
        ) from None
    try:
        content = response["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        raise MediaError("invalid_response", "MiniMax vision returned no analysis") from None
    if not isinstance(content, str) or not content.strip():
        raise MediaError("invalid_response", "MiniMax vision returned an empty analysis")
    return {"provider": "minimax", "model": model, "analysis": content}


if __name__ == "__main__":
    sys.exit(main(analyze))
