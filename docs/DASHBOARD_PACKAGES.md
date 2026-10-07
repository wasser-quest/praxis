# Dashboard packages and Host API v1

The dashboard is replaceable. Any installed package that declares a
`"dashboard"` block can serve the UI instead of the built-in one, using only
Host API v1 — no Praxis source changes or rebuilds.

## Selecting a dashboard

```bash
cp -r examples/dashboard-package/minimal_dashboard plugins/
DASHBOARD_PACKAGE=minimal_dashboard praxis run
```

- Unset `DASHBOARD_PACKAGE`: the built-in dashboard starts (if compiled with
  the `dashboard` feature, which is the default).
- Set: the built-in dashboard is not started; exactly one dashboard is active.
- `--no-dashboard` disables both.
- The package binds the configured `DASHBOARD_PORT` (default `0.0.0.0:1337`).
  The gateway stays on `0.0.0.0:3537`.
- If the package is missing, disabled, invalid or crashes, Praxis logs an error
  and the gateway, CLI and workflows keep running. It is not restarted or
  silently replaced by the built-in dashboard.
- With `DASHBOARD_TLS=true` the package receives `DATA_DIR/tls/cert.pem` and
  `key.pem` (they must exist; the built-in dashboard generates them on first
  TLS start).

A headless build (`--no-default-features`) plus a dashboard package is a fully
working UI setup without the built-in dashboard code.

## Declaring a dashboard

```json
{
  "name": "my_dashboard",
  "version": "0.1.0",
  "description": "...",
  "tools": [],
  "dashboard": {
    "executable": "bin/server",
    "args": [],
    "scopes": ["sessions:read", "sessions:write", "agent", "events", "auth"]
  }
}
```

The executable must live inside the package directory. It is launched with an
empty environment (only `PATH`), speaks the existing process protocol on
stdin/stdout (`hello` → `ready`, `health`, `shutdown`; service `dashboard`,
operations `["status"]`, no controls), and receives `DashboardInit`:

```json
{"listen":"0.0.0.0:1337","tls":null,"gateway_port":3537,
 "host_api":{"version":1,"url":"http://127.0.0.1:<port>","token":"<per-launch>","scopes":[...]}}
```

Rust packages can use `praxis_plugin_api::host::{DashboardInit, HOST_API_VERSION}`
and `praxis_plugin_api::serve`. The example is Python standard library only.

## Host API v1

Loopback only, `Authorization: Bearer <token>`, new token per launch. Each
route requires a scope; undeclared scopes return 403.

| Route | Scope |
|---|---|
| `GET /host/v1/info` | — |
| `GET /host/v1/sessions`, `/contexts`, `/messages/:user?chat_only=1` | `sessions:read` |
| `GET /host/v1/graphs/:user?workflow=`, `/execution/:user`, `/usage/:user` | `sessions:read` |
| `POST /host/v1/sessions/:user/fork`, `/messages/:user/clear-chat` | `sessions:write` |
| `GET /host/v1/agent/:user`, `POST …/begin`, `…/input`, `…/stop` | `agent` |
| `GET /host/v1/events/:user` (SSE) | `events` |
| `POST /host/v1/auth/login`, `/auth/verify` | `auth` |
| `GET /host/v1/admin/tools`, `/admin/templates`, `/admin/templates/*name` | `admin:read` |
| `GET /host/v1/admin/workflows`, `/admin/workflows/:name`, `/admin/memory/:user?profile=` | `admin:read` |
| `GET /host/v1/admin/pairings`, `/admin/pending-pairings`, `/admin/cron`, `/admin/delegations/:user` | `admin:read` |
| `POST /admin/tools/:name` `{is_enabled}` | `admin:write` |
| `POST /admin/templates`, `PUT`/`DELETE /admin/templates/*name` | `admin:write` |
| `PUT /admin/workflows/:name`, `PUT /admin/memory/:user` | `admin:write` |
| `DELETE /admin/pairings/:user`, `POST`/`DELETE /admin/pending-pairings/:code` | `admin:write` |
| `GET /admin/skills`, `/admin/router`, `/admin/profiles`, `/admin/decision-profiles(/:name)` | `admin:read` |
| `POST /admin/profiles` `{name, source_user_id}`, `DELETE /admin/profiles/:name`, `POST /admin/profiles/:name/apply/:user` | `admin:write` |
| `PUT /admin/decision-profiles/:name` | `admin:write` |
| `GET /contexts/:user` | `sessions:read` |
| `PUT`/`DELETE /contexts/:user` (context writes can change workflow and permissions) | `admin:write` |
| `DELETE /messages/:user`, `POST /messages/:user/compact` | `sessions:write` |
| `GET /secrets` (masked only), `PUT /secrets` | `secrets` |
| `GET /media?q=`, `GET /media/files/:name`, `POST /media/files?name=` (raw body, 50 MB) | `media` |
| `GET`/`POST /media/avatars/:name` (PNG/JPG/GIF/WebP, 2 MB) | `media` |
| `GET /media/screenshots/vm/:guest/screenshots/:file` | `media` |
| `GET /media/audio/:user/:message_id`, `POST /media/stt/:user` (raw audio, 25 MB) | `media` |
| `POST /decision-probe` `{profile, contexts}` (classification only, spends inference) | `agent` |
| `GET /features` (installed feature pages: id, title, entry, API, sockets) | `features` |
| `ANY /features/:owner/<service path>` (HTTP and WebSocket) | `features` |
| `GET /sm/:user` (workflow file, template, skill, state, SM data) | `sessions:read` |
| `POST /contexts/:user/exec` `{line}` (the `/context` command language) | `admin:write` |
| `POST /chat/send` (chat box semantics: question replies, options, start or inject) | `agent` |
| `GET /admin/tool-records`, `GET /admin/tool-activity?…` | `admin:read` |
| `GET /admin/tool-packages`, `POST /admin/tool-packages/:id` `{enabled}` | `admin:read` / `admin:write` |

| `GET /media?q=`, `GET /media/files/:name`, `POST /media/files?name=` (raw body, 50 MB) | `media` |
| `GET`/`POST /media/avatars/:name` (PNG/JPG/GIF/WebP, 2 MB) | `media` |
| `GET /media/screenshots/vm/:guest/screenshots/:file` | `media` |
| `GET /media/audio/:user/:message_id`, `POST /media/stt/:user` (raw audio, 25 MB) | `media` |
| `POST /decision-probe` `{profile, contexts}` (classification only, spends inference) | `agent` |
| `GET /features` (installed feature pages: id, title, entry, API, sockets) | `features` |
| `ANY /features/:owner/<service path>` (HTTP and WebSocket) | `features` |
| `GET /sm/:user` (workflow file, template, skill, state, SM data) | `sessions:read` |
| `POST /contexts/:user/exec` `{line}` (the `/context` command language) | `admin:write` |
| `POST /chat/send` (chat box semantics: question replies, options, start or inject) | `agent` |
| `GET /admin/tool-records`, `GET /admin/tool-activity?…` | `admin:read` |
| `GET /admin/tool-packages`, `POST /admin/tool-packages/:id` `{enabled}` | `admin:read` / `admin:write` |

### Feature page slots

Feature packages such as VM declare a web descriptor (page title, entry asset,
API prefix, WebSocket paths). A dashboard package lists them with
`GET /features` and reaches them through
`/host/v1/features/<owner>/<service path>`, for example
`/host/v1/features/vm/plugins/vm/index.html` (assets, GET/HEAD only),
`/host/v1/features/vm/api/plugins/vm/guests` (API) or
`/host/v1/features/vm/api/plugins/vm/vnc/ws` (noVNC WebSocket). Only the
owner's `/api/plugins/<owner>` and `/plugins/<owner>` namespaces and
host-registered alias targets are reachable. The host forwards over loopback
with the service's private key and the operator principal, strips the
package's credentials, and applies the same 2 MB/300 s limits as the built-in
dashboard. WebSockets authenticate with the `Authorization` header only (no
query tokens), so a package proxies sockets from its own backend and never
hands the Host API token to a browser. The built-in dashboard's extension
routes use the same transport (`runtime::web_proxy`).

Media names are a single `[A-Za-z0-9._:@+-]` component (uploads are
sanitized to that); screenshots are served only from
`vm/<guest>/screenshots/`, never other DATA_DIR content.

`PUT /secrets` keeps the built-in guards: empty `gateway_api_key` /
`dashboard_admin_password` are ignored (no lockout), and with
`master_password` the store is only re-encrypted if that password opens the
existing store; provider keys reload the LLM router without restart.

Admin writes go through `services::admin`, the same code the built-in
dashboard now uses: template updates are rendered against a real context
before saving, workflow files are parsed before an atomic write, shared memory
needs a reason, unknown tools return 404, and template names cannot leave
`templates/` (this also fixes a path traversal in the old dashboard
create/delete handlers).

Messages include prompt/completion tokens and `generation_ms`; graphs come from
the same state-machine parser as the built-in dashboard (active node, edges,
state-local Decision IR); execution includes transitions, receipts and
compaction telemetry. `agent/begin` goes through the gateway chat API, so
routing, budgets, IR enforcement and receipts are unchanged.

## The standard dashboard package

The full Praxis dashboard ships as a package too (`crates/praxis-dashboard`,
manifest in `plugins/dashboard/plugin.json`):

```bash
praxis plugin install --build ./plugins/dashboard            # builds, installs plugins/dashboard
cargo build --release --no-default-features       # optional: core without dashboard code
DASHBOARD_PACKAGE=dashboard praxis run
```

It serves the unchanged browser UI (`static/`, copied into the package) and
implements every URL the UI uses — `/api/*`, the chat SSE streams,
multipart uploads/STT, `/api/dashboard/extensions`, `/plugins/<owner>/…`,
`/api/plugins/<owner>/…` and host aliases such as `/api/vm`, `/websockify`
(HTTP and WebSocket) — as an adapter over Host API v1. It has no database,
secrets or workflow access of its own. Browser requests carry the operator
login token, which the package verifies with `auth/verify` (cached 30 s)
before forwarding; the Host API token stays in the package process. A
different dashboard is just a different package in `DASHBOARD_PACKAGE`.

The built-in dashboard (`dashboard` feature) stays available for one release
as the compatibility path and is now a thin layer over the same services.

## Trust

The package token is never sent to browsers in the example; browser calls are
authenticated with the operator login (`auth/verify`) before forwarding.
Installed executables are still operator-trusted processes: scopes limit what
the Host API grants, but they are not an OS sandbox.

## Not yet in v1

Everything the built-in dashboard does is now reachable through Host API v1.
Provider setup stays on the gateway client API (`/v1/...`). They move into services and
Host API scopes next; the built-in dashboard then becomes a package itself.
