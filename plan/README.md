# Praxis pluginization plan

This folder contains the implementation roadmap for turning Praxis into a small
runtime with optional feature plugins. The PR 1 and PR 2 progress sections record
the implemented execution boundary and the first optional native feature package.

Start with [the full app roadmap](PLUGINIZATION.md). The order is:

1. Add the shared registry, dispatcher and host interfaces needed by VM plugins.
2. Extract VM support, including its service, tools, routes and noVNC assets.
3. Extract the whole dashboard UI and its listener into an optional plugin.
4. Move the remaining tools, providers, channels and workflow assets into packages.
5. Ship a minimal runtime and a compatibility preset with independent packages.

The intended builtin tool set is `inspect_file`, `write_file` and `apply_patch`
under `file_ops`. POML rendering, context handling, state-machine execution,
workspace authority, permissions and receipt verification remain trusted runtime
services. Feature plugins and the dashboard are not required for local POML/SM
workflows. Model-facing runtime tools can be supplied by `runtime_control`;
optional workflow packs add assets rather than replacing the core interpreter.

## Delivery checklist

- [x] PR 1: canonical registry and shared dispatch/host API.
- [x] PR 2: headless VM adapter extraction, preserving current behavior.
- [ ] PR 3: independently packaged VM service, routes, UI contribution and lifecycle.
- [ ] PR 4: optional dashboard with feature-independent runtime APIs/events.
- [ ] PR 5: runtime-control tools and other tool packages.
- [ ] PR 6: providers, channels, media and workflow/prompt packages.
- [ ] PR 7: minimal distribution, compatibility preset and dependency cleanup.

These are ordered reviewable milestones, not time estimates. Each milestone
includes migration and acceptance checks in the full roadmap. VM is the first
feature extracted into an optional native crate. Independent IPC/UI packaging
is PR 3; the dashboard and remaining features still belong to later milestones.

## PR 1 progress

- [x] Shared owner catalog for discovery, preflight and execution.
- [x] Atomic rejection of duplicate package/tool owners on activation.
- [x] Shared chat/agent dispatcher, including lowered Decision IR and WebSocket tasks.
- [x] Live enable/cancellation checks and authenticated cron/background ownership.
- [x] Native background-service API v1: owner/version checks, drain and bounded stop.
- [x] Task-owned registry snapshots and revision evidence in action receipts.
- [x] Service-backed feature handles and versioned invocation/context API.
- [x] Runtime events, template synchronization and independent housekeeping services.
- [x] Headless fixture-service lifecycle and plugin-free POML/context/SM coverage.
- [x] Provider-independent management startup and lazy inference clients.

The native worker API and feature invocation API v1 now support service-backed
tool calls without UI or providers. Host-issued scope includes persistent storage,
task identity, workspace, credentials, cancellation and deadlines; the existing
core guard APIs remain the authority for later runtime-control wrappers. See
[the native service guide](../docs/NATIVE_FEATURE_SERVICES.md). Generic v2 process
manifests remain later work; the VM worker now uses protocol v1. Registry pins
cover resolved declarations and native
binding generations, not immutable package/script bytes; atomic package
publication is later work.

See [the current execution boundary](../docs/PLUGIN_RUNTIME.md) for implemented
behavior and compatibility limits. PR 1's full compatibility checks passed:
960 library tests with real POML, 15 CLI tests and seven selected integration
cases, including real Cargo. No live model inference was used.

## PR 2 progress

- [x] Optional `praxis-vm` crate owns QEMU/QMP/serial and all 25 VM tools.
- [x] Explicit host configuration, authenticated callers and user preferences;
  no global manager, environment reads or default-user fallback in the backend.
- [x] Native service ownership, live tool flags and explicit guest execution backend.
- [x] Guest-scoped results cannot satisfy verified host-workspace guards.
- [x] Optional screenshot hooks and explicit per-guest credential grants.
- [x] Headless CLI extraction, read-only initialization, safe QMP reattachment and
  cancellation cleanup of newly spawned children.
- [x] Legacy dashboard/VNC adapters use the bound service and authenticated tokens.
- [x] `compatibility` build and offline installation preset, including shipped
  plugin helpers, workflows and noVNC assets; explicit one-time VM tool activation.

Use [installation presets](../docs/INSTALLATION_PRESETS.md) to retain the previous
feature set. The [VM migration guide](../docs/VM_PLUGIN.md) explains the native
package and its transitional boundary. The preset is available now; PR 7 still
owns the final minimal distribution and app-wide dependency cleanup. Real QEMU
guest/OS/noVNC interaction remains an operator smoke check.

Compatibility verification: 972 library tests with VM and 965 without VM, 16 CLI
tests in each build, 10 native VM package tests and seven selected integration
tests passed. The integrations use real POML/Cargo and synthetic provider
responses; no live model inference was used. The compiled preset CLI was checked
for fresh install, upgrade/backups, complete plugin helpers, separate `DATA_DIR`
and preservation of disabled tools. The no-VM dependency tree excludes `praxis-vm`.

## PR 3 progress — process and web contributions

- [x] Versioned local pipe protocol and reusable host/worker API crate.
- [x] Installed `praxis-vm-service` for all 25 tools, without a QEMU dependency in the host.
- [x] Explicit host initialization, declaration handshake, health and live availability.
- [x] Host-issued identity/preferences, scoped guest credentials and unchanged receipt authority.
- [x] Bounded calls, cooperative cancellation, owned-child cleanup and crash detection.
- [x] No automatic replay, native fallback or reactivation of a stopped binding.
- [x] Installation/upgrade instructions; existing native compatibility stays the default.
- [x] Package-owned VM routes, authenticated VNC sessions and embedded noVNC assets/licenses.
- [x] VM dashboard contribution via generic host extension slots, with native or process backend.
- [x] HTTP/VNC sessions participate in service drain/forced disconnect; guests/disks survive.
- [x] Screenshot delivery and image-context loading with native or process backend.
- [x] Package-owned VM CLI parsing/execution, with a core-only forwarding entry point.
- [x] Persisted guest records (owner, shares, endpoints, pending/interrupted ops).
- [x] Ownership enforced for tools, HTTP routes, VNC, screenshots and listing,
  including explicit and `*` shares; host-asserted principal header, fail closed.
- [x] Restart/upgrade recovery: legacy adoption, interrupted-operation records,
  verified QMP reattach via persisted endpoints; no spawn/kill/delete.
- [x] Real QEMU verification (opt-in test, TCG, Alpine ISO): boot, PNG screenshots,
  keyboard, absolute mouse, VNC RFB handshake, ownership denials, worker
  restart with the guest still running, verified reattach and disk preservation.
  This found and fixed a missing absolute pointer device (`usb-tablet`).

See [process hosting](../docs/PLUGIN_PROCESS_PROTOCOL.md). The full PR 3 checkbox
remains open for guest ownership and full upgrade/recovery policy. Generic v2
process declarations and immutable package publication are still future work.

Verification: 978 library tests with the native VM engine and 971 without it,
16 CLI tests in each build, 9 protocol tests, 12 VM package tests and 9 selected
integration tests passed. The integrations use real POML/Cargo and synthetic
providers, covering IR/normal coding, graph navigation, language learning,
rollback/completion and retry/WebSocket behavior. The compiled no-VM CLI installed
the preset/VM flags, handshook an actual installed worker, and served management
without provider credentials; missing-QEMU autostart remained non-fatal. The
no-VM host dependency tree excludes `praxis-vm`. No live inference was used.

The web slice passed 980 native-build library tests, 975 core-only library tests,
28 protocol/engine/web/worker tests, 16 CLI tests in each build, all four browser
helper test scripts, and two real-POML/Cargo workflow integrations with scripted
providers. A compiled core-only host plus the actual installed worker passed VM
API/UI/assets and legacy-alias smoke checks without provider credentials.
The actual worker serves its embedded UI, and a TCP/QMP fixture verifies binary
VNC relay and forced disconnect without stopping a committed guest. See
[web contribution architecture and migration](../docs/PLUGIN_WEB_CONTRIBUTIONS.md).

The CLI/screenshot slice passed 984 native-build and 979 core-only library tests
with real POML (48 integration tests remain opt-in), 17 CLI tests in each build,
and 37 protocol/engine/web/worker tests. Actual-worker QMP fixtures cover PNG
capture, PNG/PPM retention, media insertion, force-stop request acknowledgement,
read-only attachment and unchanged guest credentials. A compiled core-only host
and installed worker passed CLI help/status, QMP-to-PNG delivery, VM API/UI/assets
and legacy-alias smoke checks without provider credentials. Its dependency tree
excludes all three VM crates. No live model inference or real guest boot was used.

The ownership/recovery slice is described in
[the VM guide](../docs/VM_PLUGIN.md#guest-ownership-and-recovery). It passed
the VM package tests (store, route ownership/sharing, VNC denial, restart
reattach through a persisted non-default QMP port) and the host `vm` library
tests, including process-worker and native dispatch. The real-QEMU check runs
with `PRAXIS_REAL_QEMU_ISO=/path/to.iso cargo test -p praxis-vm-web --test web
real_qemu -- --ignored`. Desktop-OS interaction (installed GUI, guest agent,
clipboard) remains an operator smoke check.

## PR 4 progress — optional dashboard

- [x] `dashboard` Cargo feature (in `compatibility`/default). Without it the
  dashboard module, `:1337` listener, TLS/self-signed cert generation and the
  `axum-server`/`rcgen`/`hostname` dependencies are absent; `praxis run` logs
  that the dashboard isn't installed and keeps gateway/CLI/workflows running.
- [x] Host services (`src/services`): sessions/contexts/messages (incl. token,
  generation-speed telemetry and chat-clear marker), fork, workflow graphs
  (shared SM parser, active node, preview), execution events/receipts and
  usage limits, agent-loop control. Dashboard routes are thin adapters.
- [x] Core no longer imports `dashboard` (runtime tests use `runtime::events`).
- [x] Builds checked: minimal, VM-only, dashboard-only and default.
- [x] Host API v1 (`/host/v1`, loopback, per-launch token, declared scopes):
  sessions, messages, graphs, execution, usage, agent control, SSE events, auth.
- [x] Replaceable dashboard packages: `"dashboard"` manifest block,
  `DASHBOARD_PACKAGE` selection, single active dashboard, process supervision,
  no silent fallback. Example package in `examples/dashboard-package`. See
  [dashboard packages](../docs/DASHBOARD_PACKAGES.md).
- [x] Administration services + Host API `admin:read`/`admin:write`: tools,
  templates, workflow files, memory profiles, pairings, cron list, delegations.
- [x] Secrets (masked, `secrets` scope), context profiles, skills, decision
  profiles, router state, context/history writes in services + Host API.
- [x] Media (uploads, avatars, screenshots, message audio, STT) and decision
  probe in services + Host API; dashboard uploads now require auth and media
  names cannot traverse DATA_DIR.
- [x] Feature page slots for dashboard packages: `features` scope, feature
  list, owner-confined HTTP/WebSocket forwarding through the shared
  `runtime::web_proxy` transport.
- [x] Standard dashboard package (`crates/praxis-dashboard`,
  `packages/dashboard`, `scripts/install-dashboard-package.sh`): ships the
  frontend assets and serves the whole UI on Host API v1; verified end to end
  against a `--no-default-features` core.
- [ ] Navigation slots for absent features; remove the built-in dashboard
  feature after one release.

## PR 5 progress — tool packages

- [x] Every native tool has exactly one owning package (`tools::packages`,
  tested against the default catalog). Packages enable/disable independently
  via CLI, dashboard and Host API; catalog, discovery and execution respect
  them; per-tool flags are preserved; startup never changes package state;
  `file_ops` is core. See [tool packages](../docs/TOOL_PACKAGES.md).
- [ ] Move implementations out of the core binary per package, with plugin
  takeover of names whose native package is disabled. Runtime-control and the remaining
feature/tool packages follow; POML/context/SM/IR verification stays in core.

Existing setup guides remain applicable during migration:

- [Reproducible Rust/Snake setup and test prompt](../docs/SNAKE_IR_TEST.md).
- [Verified capabilities in normal coding states](../docs/VERIFIED_CODING_ROLES.md).
- [Language-learning flow](../docs/LANGUAGE_LEARNING_FLOW.md).
- [Current plugin action contracts](../docs/PLUGIN_ACTION_CONTRACTS.md).
