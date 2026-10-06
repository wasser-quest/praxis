# MiniMax image plugin

Two independently installable Praxis tools over the MiniMax APIs (Python 3.9+,
`python3` on PATH, no extra packages):

| Tool | Output |
| --- | --- |
| `minimax_image_generate` | saved PNG/JPEG/WebP under `DATA_DIR/uploads` (or the provider's HTTPS URL when the model returns one) |
| `minimax_image_analyze` | text analysis of an HTTPS image URL or base64 data URI |

Install the complete folder (`plugin.json`, `image_generate.py`,
`image_analyze.py` and `media_common.py` stay together):

```bash
./praxis plugin install /path/to/praxis-source/plugins/minimax_image
```

## Credentials and settings

- `minimax_api_key` in the Praxis secret store, or `MINIMAX_API_KEY` in the
  service environment. The gateway passes only the secrets declared in the
  manifest (`PLUGIN_SECRETS`). Never put API keys in tool arguments,
  `plugin.json`, POML or user context. Plugin processes are trusted local code,
  **not a sandbox**.
- Models: tool argument `model` → non-empty `custom_data.minimax_image_model` /
  `custom_data.minimax_vision_model` → `MINIMAX_IMAGE_MODEL` /
  `MINIMAX_VISION_MODEL` → `image-01` / `MiniMax-VL-01`.
- `MINIMAX_BASE_URL` overrides the API base (HTTPS only, loopback HTTP for
  tests) and `MINIMAX_TIMEOUT_SECONDS` the request timeout (0.1–600).

## Arguments

`minimax_image_generate` (a real call costs credits): `prompt` (1–12000
characters), optional `width`/`height` (128–4096, default 1024).

`minimax_image_analyze`: `image_url` (HTTPS URL or `data:image/...;base64` data
URI; local paths are not supported), optional `question` (1–4000 characters,
default "Describe this image in detail").

Both tools are paid calls with **no automatic retries and no provider
fallback**. Returned provider URLs are reported, never fetched; only image bytes
the provider returns are saved, and only after the whole response validates.
