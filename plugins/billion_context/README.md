# billion_context — long conversations for local models

> **Credits:** this plugin uses [billion-context](https://github.com/ranxianglei/billion-context)
> and its compression kernel [acp-kernel](https://github.com/ranxianglei/acp-kernel)
> by ranxianglei (MIT License plus an attribution term). See [NOTICE.md](NOTICE.md)
> and [THIRD_PARTY_LICENSE-billion-context](THIRD_PARTY_LICENSE-billion-context).

This package adds a chat provider, `bili_llamacpp`, that sends requests to your
local OpenAI-compatible model server (llama.cpp) **through a
[billion-context](https://github.com/ranxianglei/billion-context) proxy**.
The proxy compresses older parts of the conversation into summaries and gives the
model tools to fold, restore and search them (`compress`, `decompress`,
`search_context`, `acp_status`). It runs those tools itself; Praxis never sees
them. The result: long sessions with a small context window.

Praxis contains no bili code. The package is a declaration over the generic
OpenAI-compatible provider worker:

```
Praxis ──praxis.provider.v1──▶ bin/praxis-provider-openai
                                  │ POST http://127.0.0.1:8787/bili/<your model>/chat/completions
                                  │ x-acp-session: <opaque conversation key>
                                  ▼
                               bili proxy ──▶ llama.cpp
```

`x-acp-session` carries an opaque per-conversation key derived by Praxis
(a keyed hash; never the context, user or session id), so bili keeps one
compression state per Praxis conversation.

## 1. Install and start bili (separate Node.js tool)

```bash
npm install -g billion-context --prefix=~/.local
bili start                     # proxy on http://127.0.0.1:8787
curl -s http://127.0.0.1:8787/__bili/health
```

bili is third-party software with its own license (MIT plus an attribution
term, see [NOTICE.md](NOTICE.md)) and its own auto-update mechanism; review
both before use. If you build a product on top of this plugin, keep the
billion-context and acp-kernel credits with their links visible on its home
page, documentation or About/Credits page. Keep it bound
to `127.0.0.1` (the default): its HTTP endpoints have no authentication.

Tell bili your model's real context window, otherwise it cannot judge when to
compress. In `~/.config/billion-context/billion-context.json`:

```json
{
  "providers": {
    "http://127.0.0.1:8080/v1": {
      "models": { "local": { "context": 32768, "output": 4096 } }
    }
  }
}
```

Use your llama.cpp URL and its `--ctx-size`; `local` is the model name this
package sends (see below).

## 2. Point the package at your model

Edit `args` in this folder's `plugin.json`:

| Position | Meaning | Default |
|---|---|---|
| 1 | `<bili origin>/bili/<llama.cpp base URL>/v1` | `http://127.0.0.1:8787/bili/http://127.0.0.1:8080/v1` |
| 2 | model name sent upstream (llama.cpp accepts any) | `local` |
| 3–4 | forward the conversation key as this header | `--conversation-header x-acp-session` |

## 3. Install, trust and select

```bash
./praxis plugin install plugins/billion_context --build   # builds bin/praxis-provider-openai
./praxis plugin trust billion_context --role runtime
./praxis plugin enable billion_context
PRAXIS_REGISTRY_CHAT_PROVIDER=bili_llamacpp ./praxis run
```

Then select the provider for a conversation (or set `USE_PROVIDER=bili_llamacpp`):

```text
/context set settings.provider=bili_llamacpp
```

## 4. Let bili do the compressing

Praxis has its own compaction and trims history to `history_token_limit`. With
bili both would fight, and bili needs the full history to recognize what it has
already folded. For conversations on this provider:

```text
/context set settings.compaction_enabled=false settings.history_token_limit=10000000
```

When you switch back to a direct provider, turn Praxis compaction on again:
without bili, the full history no longer fits the model.

## Notes and limits

- If bili is not running, requests fail with a connection error. There is no
  silent fallback to the direct endpoint.
- Chat only, non-streaming (the generic worker's capabilities). One request may
  take up to 300 s, including bili's own compress round trips.
- bili may add a confirmation line (`📦 [ACP] Compressed …`) to replies; it is
  the proxy's, not the model's. Check `acp_status` or `/__bili/stats` instead of
  trusting such text.
- Status: billion-context describes itself as early software. Measure with your
  own model before relying on it.

## Credits

This plugin uses billion-context (https://github.com/ranxianglei/billion-context)
and acp-kernel (https://github.com/ranxianglei/acp-kernel). License and
attribution terms: [NOTICE.md](NOTICE.md).
