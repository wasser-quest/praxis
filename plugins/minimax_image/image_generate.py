#!/usr/bin/env python3
"""Praxis tool for MiniMax image generation (POST /v1/text/image)."""
import base64
import binascii
import json
import re
import sys
from media_common import (
    MediaError,
    OutputBatch,
    api_key,
    base_url,
    inputs,
    main,
    post,
    setting,
    text,
    timeout_seconds,
)

MAX_IMAGE_BYTES = 16 * 1024 * 1024


def decode_image(encoded):
    if len(encoded) > ((MAX_IMAGE_BYTES + 2) // 3) * 4:
        raise MediaError("response_too_large", "Generated image exceeds the 16 MiB limit")
    try:
        data = base64.b64decode(encoded, validate=True)
    except (ValueError, binascii.Error):
        raise MediaError("invalid_response", "MiniMax returned invalid image encoding") from None
    if len(data) > MAX_IMAGE_BYTES:
        raise MediaError("response_too_large", "Generated image exceeds the 16 MiB limit")
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return data, ".png", "image/png"
    if data.startswith(b"\xff\xd8\xff"):
        return data, ".jpg", "image/jpeg"
    if len(data) >= 12 and data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return data, ".webp", "image/webp"
    raise MediaError("invalid_response", "MiniMax returned data that is not a PNG, JPEG or WebP image")


def generate():
    args, context, secrets = inputs()
    prompt = text(args.get("prompt"), "prompt", 12000)
    body = {"prompt": prompt}
    for field, default in (("width", 1024), ("height", 1024)):
        value = args.get(field, default)
        if type(value) is not int or not 128 <= value <= 4096:
            raise MediaError("invalid_arguments", field + " must be an integer between 128 and 4096")
        body[field] = value
    model = setting(args, context, "model", "minimax_image_model", "MINIMAX_IMAGE_MODEL", "image-01")
    model = text(model, "model", 256)
    if not re.fullmatch(r"[A-Za-z0-9_.:-]+", model):
        raise MediaError("invalid_arguments", "model must be a MiniMax model id")
    body["model"] = model
    key = api_key(secrets, "minimax_api_key", "MINIMAX_API_KEY")
    base = base_url("MINIMAX_BASE_URL", "https://api.minimax.chat")
    timeout = timeout_seconds("MINIMAX_TIMEOUT_SECONDS", 300)
    with OutputBatch() as output:
        raw = post(
            base + "/v1/text/image",
            {"Authorization": "Bearer " + key, "Accept": "application/json"},
            body,
            timeout,
            MAX_IMAGE_BYTES,
        )
        try:
            response = json.loads(raw)
        except (ValueError, UnicodeError):
            raise MediaError("invalid_response", "MiniMax returned invalid image JSON") from None
        error = response.get("error") if isinstance(response, dict) else None
        if isinstance(error, dict) and error:
            raise MediaError(
                "provider_error",
                "MiniMax image API error: " + str(error.get("message") or "unknown"),
                retry_safe=False,
            ) from None
        data = response.get("data") if isinstance(response, dict) else None
        if not isinstance(data, list) or not data or not isinstance(data[0], dict):
            raise MediaError("invalid_response", "MiniMax returned no image data")
        image = data[0]
        # Provider URLs are reported, never fetched; only returned bytes are
        # saved, and only after the whole response validates.
        if isinstance(image.get("b64_json"), str):
            files = output.save([decode_image(image["b64_json"])], "minimax")
            return {"provider": "minimax", "model": model, "images": files}
        if isinstance(image.get("url"), str) and image["url"].startswith("https://"):
            return {"provider": "minimax", "model": model, "image_url": image["url"]}
        raise MediaError("invalid_response", "MiniMax returned neither image bytes nor an HTTPS URL")


if __name__ == "__main__":
    sys.exit(main(generate))
