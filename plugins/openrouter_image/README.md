# OpenRouter image plugin

Install this complete folder with `praxis plugin install /path/to/openrouter_image` and restart the gateway when appropriate. Requires Python 3.9+ (`python3`), no pip packages.

Tool: **`openrouter_image_generate`**. Supply `prompt`; optional `model`, `n` (1–4), `resolution`, `aspect_ratio`, `quality`, `output_format`, `background` and `reference_images` (1–4 HTTPS URLs). Use only parameters supported by the selected model. Default model: `openai/gpt-image-2`; override with `model`, `custom_data.openrouter_image_model` or `OPENROUTER_IMAGE_MODEL`.

Credential: Praxis secret `openrouter_api_key`, or `OPENROUTER_API_KEY`. The updated gateway reuses the native secret and allows an explicit Custom Secret override. Do not put keys in tool arguments or context. Keep `media_common.py` alongside `generate.py`.

Uses **`POST https://openrouter.ai/api/v1/images`**, not Chat Completions. Discover image model capabilities via `GET /api/v1/images/models`. Returns saved PNG/JPEG/WebP files in `$DATA_DIR/uploads` with absolute paths, authenticated dashboard download URLs and available usage/cost metadata. No base64 in tool results; no SVG or automatic downloads of returned URLs.

**Paid calls.** One POST per tool invocation, no automatic retries/redirects and no provider fallback. HTTP budget 300 s (`OPENROUTER_IMAGE_TIMEOUT_SECONDS`, max 600 s). Cap 16 MiB/image and 64 MiB/JSON response. Operator endpoint override: `OPENROUTER_IMAGE_API_BASE` (HTTPS, except loopback test servers). Existing files are never overwritten.

Full installation, configuration, safeguards and offline tests: `docs/MEDIA_PLUGINS.md` in the Praxis source distribution. This plugin is independent of the configured Chat LLM provider.

Requests attribute the app to **Praxis** with `HTTP-Referer: https://getpraxis.boo` and `X-OpenRouter-Title: Praxis`, following [OpenRouter app attribution](https://openrouter.ai/docs/app-attribution). The shared HTTP helper also sends a Praxis User-Agent. Reinstall the complete plugin folder to update an existing installation.
