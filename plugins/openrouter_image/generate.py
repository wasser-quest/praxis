#!/usr/bin/env python3
"""Praxis tool for OpenRouter's dedicated Image API (POST /api/v1/images)."""
import base64
import binascii
import json
import math
import re
import sys
import urllib.parse
from media_common import MediaError, OutputBatch, api_key, base_url, inputs, main, post, setting, text, timeout_seconds

MAX_IMAGE_BYTES = 16 * 1024 * 1024
ENUMS = {
    "resolution": {"512", "1K", "2K", "4K"},
    "aspect_ratio": {"auto", "1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "4:5", "5:4", "1:2", "2:1", "1:4", "4:1", "1:8", "8:1", "9:21", "21:9"},
    "quality": {"auto", "low", "medium", "high"},
    "output_format": {"png", "jpeg", "webp"},
    "background": {"auto", "transparent", "opaque"},
}


def decode_image(item):
    if not isinstance(item, dict) or not isinstance(item.get("b64_json"), str):
        raise MediaError("invalid_response", "OpenRouter image response has no base64 image data")
    encoded = item["b64_json"]
    if len(encoded) > ((MAX_IMAGE_BYTES + 2) // 3) * 4:
        raise MediaError("response_too_large", "Generated image exceeds the 16 MiB limit")
    try:
        data = base64.b64decode(encoded, validate=True)
    except (ValueError, binascii.Error):
        raise MediaError("invalid_response", "OpenRouter returned invalid image encoding") from None
    if len(data) > MAX_IMAGE_BYTES:
        raise MediaError("response_too_large", "Generated image exceeds the 16 MiB limit")
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        extension, mime = ".png", "image/png"
    elif data.startswith(b"\xff\xd8\xff"):
        extension, mime = ".jpg", "image/jpeg"
    elif len(data) >= 12 and data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        extension, mime = ".webp", "image/webp"
    else:
        raise MediaError("invalid_response", "Expected a PNG, JPEG or WebP image; empty/unsupported formats are rejected")
    declared = item.get("media_type")
    if declared is not None and declared != mime:
        raise MediaError("invalid_response", "Generated image format does not match its media_type")
    return data, extension, mime


def generate():
    args, context, secrets = inputs()
    if set(args) - ({"prompt", "model", "n", "reference_images"} | set(ENUMS)):
        raise MediaError("invalid_arguments", "Unexpected OpenRouter image tool arguments")
    prompt = text(args.get("prompt"), "prompt", 12000)
    model = text(setting(args, context, "model", "openrouter_image_model", "OPENROUTER_IMAGE_MODEL", "openai/gpt-image-2"), "model", 256)
    if not re.fullmatch(r"[A-Za-z0-9_.:-]+/[A-Za-z0-9_.:/-]+", model):
        raise MediaError("invalid_arguments", "model must be an OpenRouter provider/model slug")
    count = args.get("n", 1)
    if type(count) is not int or not 1 <= count <= 4:
        raise MediaError("invalid_arguments", "n must be an integer between 1 and 4")
    body = {"model": model, "prompt": prompt, "n": count, "provider": {"allow_fallbacks": False}}
    for field, values in ENUMS.items():
        if field in args:
            if not isinstance(args[field], str) or args[field] not in values:
                raise MediaError("invalid_arguments", field + " is not a supported value")
            body[field] = args[field]
    if body.get("background") == "transparent" and body.get("output_format") == "jpeg":
        raise MediaError("invalid_arguments", "Transparent background requires PNG or WebP, not JPEG")
    if "reference_images" in args:
        references = args["reference_images"]
        if not isinstance(references, list) or not 1 <= len(references) <= 4:
            raise MediaError("invalid_arguments", "reference_images must contain 1 to 4 HTTPS URLs")
        body["input_references"] = []
        for reference in references:
            reference = text(reference, "reference_images URL", 4096)
            try:
                parsed = urllib.parse.urlsplit(reference)
                if parsed.scheme != "https" or not parsed.hostname or parsed.username is not None or parsed.password is not None:
                    raise ValueError()
                _ = parsed.port
            except ValueError:
                raise MediaError("invalid_arguments", "Reference images must be HTTPS URLs without embedded credentials") from None
            body["input_references"].append({"type": "image_url", "image_url": {"url": reference}})
    key = api_key(secrets, "openrouter_api_key", "OPENROUTER_API_KEY")
    base = base_url("OPENROUTER_IMAGE_API_BASE", "https://openrouter.ai/api/v1")
    timeout = timeout_seconds("OPENROUTER_IMAGE_TIMEOUT_SECONDS", 300)
    usage_metadata = {}
    with OutputBatch() as output:
        raw = post(base + "/images", {
            "Authorization": "Bearer " + key, "Accept": "application/json",
            "HTTP-Referer": "https://getpraxis.boo", "X-OpenRouter-Title": "Praxis",
        }, body, timeout, 64 * 1024 * 1024)
        try:
            response = json.loads(raw)
        except (ValueError, UnicodeError):
            raise MediaError("invalid_response", "OpenRouter returned invalid image JSON") from None
        if not isinstance(response, dict) or response.get("error") is not None:
            raise MediaError("invalid_response", "OpenRouter returned an image generation error; no automatic retry")
        images = response.get("data")
        if not isinstance(images, list) or not 1 <= len(images) <= count:
            raise MediaError("invalid_response", "OpenRouter returned no images or more than requested")
        # Validate the ENTIRE batch before publishing any files. Never follow URLs
        # returned by the provider or accidentally write active SVG/HTML content.
        decoded = [decode_image(image) for image in images]
        usage = response.get("usage")
        if isinstance(usage, dict):
            usage_metadata = {key: value for key, value in usage.items()
                              if key in {"prompt_tokens", "completion_tokens", "total_tokens", "cost"}
                              and type(value) in (int, float) and 0 <= value <= 10**15 and math.isfinite(value)}
        files = output.save(decoded, "openrouter")
    return {"provider": "openrouter", "model": model, "images": files, "usage": usage_metadata}


if __name__ == "__main__":
    sys.exit(main(generate))
