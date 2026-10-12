# Brave Web Search for Praxis

A standard-library Python plugin with the `brave_web_search` tool. It performs
one bounded Brave Web Search API request, never follows result links, never
executes page instructions, and does not retry automatically.

## Setup

The bundled installation/`repair-assets` supplies `plugins/brave_search/`.
Alternatively copy this folder into the service's `PLUGINS_DIR` or use
`praxis plugin install /path/to/brave_search`. Restart Praxis to load plugins.

Configure the **Brave Search API** subscription key (not a browser account):
- Dashboard Secrets: custom key `brave_search_api_key`; save using your normal
  encrypted-secret workflow and restart to refresh startup credentials.
- Or service environment `BRAVE_SEARCH_API_KEY`.

Never place the key in context, POML, tool arguments or Git. The plugin declares
only its own credential. No key is bundled or provisioned. Brave requests may
incur charges according to your subscription; tests use loopback fixtures only.

## Mandatory safe-search skill

Praxis automatically loads `safe_web_search` as a task-local system-policy
layer before inference whenever this plugin is installed. It stays active while
web results are consumed, and is removed when the task finishes, fails or is
cancelled. The user's persistent skill selection is never replaced.

The host rejects Brave execution unless the skill was loaded and included in a
successful model phase. Missing/changed skill files, disabled `use_skill`, and
insufficient context budget fail closed. Direct registry calls cannot bypass
this check; both chat and agent/Decision-IR paths use the same host gate.

This is a Praxis capability boundary, not an OS-wide internet filter. Operator
scripts, arbitrary custom plugins, external MCP servers and direct HTTP clients
are outside that boundary. Keep Brave credentials in encrypted plugin secrets,
not a globally inherited environment, when those other capabilities are enabled.
Skill activation also does not guarantee factual accuracy or eliminate prompt
injection; result text remains untrusted evidence.

## Use

First call `search_tools` with `{"query":"brave web search","limit":1}`.
On the next model turn, call e.g.:

```json
{"query":"Microsoft POML documentation", "count":5, "country":"DE", "search_lang":"en", "freshness":"py", "safesearch":"moderate"}
```

Only `query` is required. `count` is 1–20 (default 5); freshness supports past
day/week/month/year (`pd`, `pw`, `pm`, `py`). Results contain titles, source URLs,
plain-text descriptions and available age labels. Snippets are untrusted data,
not verified full pages. Cite URLs and explicitly distinguish snippets from
primary-source verification. No page-fetch capability is implied.

Timeout: 20 seconds. Response limit: 2 MiB. HTTP error bodies and credentials
are never forwarded. Redirects are refused so credentials cannot follow them.
Missing keys, malformed inputs/responses, authorization/quota and network errors
are explicit failures. No automatic provider fallback.

An operator-only `BRAVE_SEARCH_API_BASE` override supports an HTTPS proxy or
HTTP loopback fixtures; it is not accepted as a tool argument.

## Offline test

```bash
python3 scripts/test_brave_search.py
```
