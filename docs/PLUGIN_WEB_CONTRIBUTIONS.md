# Feature web contributions

The VM package now owns its administration API, VNC bridge, dashboard page and
noVNC assets. Both the native compatibility backend and the installed worker use
the same `praxis-vm-web` implementation. A host built with `--no-default-features`
can display and administer an installed VM worker without linking either VM crate.

| Component | Responsibility |
| --- | --- |
| `praxis-plugin-api` | Web metadata/version validation and packaged asset type |
| `praxis-vm` | Headless VM engine and tools; no web/database/provider dependency |
| `praxis-vm-web` | VM administration, private loopback listener, VNC and embedded UI/assets |
| `praxis-vm-worker` | Installed `praxis-vm-service`, hosting the engine and web contribution |
| `src/dashboard/extensions.rs` | Host authentication, bounded HTTP/WebSocket proxy and extension listing |
| `static/extensions.js` | Generic navigation/page loading and show/hide/logout cleanup |

The host selects a fresh private web nonce during initialization. The worker's
`web_info` control returns web API version 1, a loopback port and public metadata;
it never returns the nonce. The host validates the contribution against the bound
owner and expected descriptor. Browser metadata contains neither the listener port
nor private credentials. Process metadata cannot install host route aliases.

## Routes and authentication

| Public route | Behavior |
| --- | --- |
| `/api/dashboard/extensions` | Authenticated descriptors for ready, enabled bindings |
| `/api/plugins/vm` and its child routes | Authenticated VM administration |
| `/api/plugins/vm/vnc/ws` | Authenticated VNC WebSocket; dashboard token may be supplied in query |
| `/plugins/vm/ui/*` | Package page, script, stylesheet and standalone viewer |
| `/plugins/vm/novnc/*` | Embedded noVNC modules and their licenses |
| `/api/tool-activity` | Authenticated host tool activity; the VM service has no DB access |

The VM adapter explicitly contributes compatibility aliases for `/api/vm`,
`/api/vm/activity`, `/websockify`, `/vnc` and `/static/novnc/*`. These aliases use
the same binding and authentication policy. No VM binding means no VM routes or
navigation contribution. An unready/stopped binding cannot initialize itself
through a request. A disabled or unreachable backend returns an unavailable
result and cannot fall back to native VM or host execution.

Administrator API calls require a Bearer dashboard JWT or gateway key. Only
declared WebSockets accept query-token authentication. Viewer HTML and assets are
public read-only resources; an actual desktop connection still requires login.
The internal loopback listener requires its private header even for assets. The
host discards public authorization, cookies, query tokens and caller-supplied
private headers before forwarding with its own credential. Request/response
bodies and socket messages are bounded to 2 MiB. Proxy requests have a 300-second
budget and socket connection setup a five-second budget.

Requests and desktop connections count as in-flight service work. Disabling a
binding rejects new work immediately and drains active work until grace expires;
forced cancellation closes sockets and stops the owned listener/worker. This does
not stop a committed guest, delete its disk or undo external effects. Explicit
Stop VM remains an administrator action.

## Dashboard and installation

The host loads only metadata at login. Opening the contributed VM page loads its
HTML, stylesheet and script once. Leaving the page or logging out stops refresh
timers and disconnects VNC. Package code cannot re-open a desktop when a late
module/request finishes after the page was hidden. Failed/stopped bindings are
removed when the extension list is refreshed.

These are **trusted first-party pages**, running with the dashboard's origin and
shared host helpers. General third-party UI isolation and immutable package
publication remain future work; manifest-v2 contributions are now loaded.

## Manifest-declared contributions

Any package can contribute a web service without a host-native adapter. Declare
it in the manifest and have the worker expose the matching `web_info` control:

```json
"provides": {
  "routes": ["ext"],
  "ui": ["ext.panel"],
  "web": {"service": "ext", "title": "Extension"}
}
```

The host derives the descriptor from the owner (default
`/plugins/<owner>/ui/page.html`, `.js`, `.css`; optional `websockets` list) and
requires the worker's `web_info` descriptor to match it byte for byte. The
worker gets a per-launch `web_token` in its initialization, must bind a
loopback listener, and must reject requests without the private
`x-praxis-plugin-key` header. From then on the service is listed and proxied by
the same authenticated dashboard/Host API paths and with the same bounded
transport, drain and forced-disconnect behavior as the VM. One owner per
namespace; a descriptor mismatch, a missing control or a bad handshake fails
startup closed, and install/enable never starts the worker.

Guest operations preserve
the existing global administrator policy; per-user guest ownership is a remaining
PR 3 milestone.

Follow [worker installation](INSTALLATION_PRESETS.md#install-the-headless-vm-worker).
The worker embeds its UI/noVNC modules and licenses, so no VM UI directory needs
to be copied into a core-only installation. The compatibility build can also
install those assets under `plugins/vm`; `--update-dashboard` backs up changes
without replacing operator manifests. Existing `static/novnc` files may remain
on upgrades; the registered alias serves the bound package's current assets.
Old host bundles need `static/extensions.js` and the updated `app.js`/`index.html`.
Update the host and worker from the same checkout: older workers do not declare
the new `web_info` control and fail the startup handshake.

Manual dashboard starts use the explicitly selected keyboard layout (default
`us`) and no implicit credential grants. Model tool calls and configured
autostart retain authenticated preferences and explicit per-guest grants.

## Verification and remaining work

```bash
cargo test -p praxis-plugin-api -p praxis-vm -p praxis-vm-web -p praxis-vm-worker --locked
cargo test --lib --locked --no-default-features dashboard::extensions
cargo test --lib --locked runtime::vm_tests
node tests/test_dashboard_extensions.js
cargo tree -p praxis --no-default-features
```

Tests serve real package assets, launch the actual worker, relay binary VNC data
against a local TCP/QMP fixture, check authentication and forced disconnect, and
preserve existing storage. Real guest installation, keyboard/mouse interaction
and desktop rendering remain operator smoke checks. Independent VM CLI and
screenshot delivery are implemented; guest ownership and complete upgrade/recovery
policy finish PR 3. The whole
dashboard listener/UI becomes optional in PR 4. POML, contexts, states, Decision
IR and verified execution remain runtime services throughout.
