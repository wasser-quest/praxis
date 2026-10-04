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

## Trust

The package token is never sent to browsers in the example; browser calls are
authenticated with the operator login (`auth/verify`) before forwarding.
Installed executables are still operator-trusted processes: scopes limit what
the Host API grants, but they are not an OS sandbox.

## Not yet in v1

Uploads/media, STT/TTS, chat audio, the decision probe, and feature page slots
(e.g. the VM page) are still only in the built-in dashboard. Provider setup
stays on the gateway client API (`/v1/...`). They move into services and
Host API scopes next; the built-in dashboard then becomes a package itself.
