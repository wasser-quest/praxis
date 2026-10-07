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

For the current state, big ideas and the concrete remaining work, read
[the pluginization handoff](../docs/PLUGINIZATION_HANDOFF.md) first. It links the
trust model, the `xis` design and every implemented milestone.

An agreed follow-on architecture makes the loader itself the kernel and every
capability an installable, replaceable plugin, including the default POML/SM/
guard engine and the TUI. Install/uninstall may run operator-approved lifecycle
hooks under a configured `hooks = allow|ask|deny` policy. See
[plugin lifecycle, trust and the kernel](../docs/PLUGIN_LIFECYCLE.md).

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
- [x] PR 4: optional dashboard with feature-independent runtime APIs/events.
  (Removing the built-in dashboard after one release is tracked in the
  handoff §6G.)
- [x] PR 5: all tool owners are packages. `legacy_file_ops`, `shell` and
  `vision` are optional crates with installers; `runtime_control`, `delegation`,
  `memory`, `rag`, `cron`, `discord`, `interaction`, `skills`,
  `workflow_authoring` and `file_ops` are privileged bundled packages whose
  `plugin.json` declares every tool over `builtin` handlers bound to the
  `tools::builtin_operations` host-operation table. Nothing is seeded into
  `db/tools.rs` anymore: schemas come from the bundled manifests, legacy flags
  migrate, and `execute_decision` lowers one instruction through the dispatch
  loop ("run this resolved call").
- [ ] PR 6: providers, channels, media and workflow/prompt packages.
  Progress: `crates/praxis-provider-api` defines the common interfaces
  (streaming, usage, model list, embedding, typed errors) and the kernel's
  adapters implement them; `plugins/asset_{coding,learning,persona,workflow}`
  ship installable asset packs; `plugins/minimax_image` packages the MiniMax
  image provider; `crates/praxis-comfyui` and `crates/praxis-gpu-router` are
  provider crates with scoped settings; Discord and voice/audio compile behind
  `discord`/`voice` with `serenity` optional. The channel ingress seam and the
  Discord/voice package manifests remain.
- [ ] PR 7: minimal distribution, compatibility preset and dependency cleanup.
- [x] PR 8a: lifecycle hooks (`allow|ask|deny`), staged install/uninstall,
  script hashing, install record and example plugins. See
  [plugin lifecycle](../docs/PLUGIN_LIFECYCLE.md).
- [x] First-class `requires`/`frontend` declarations: validated in the loader
  and included in the registry revision, so a dependency or frontend change
  invalidates task receipts. The raw manifest helpers are gone.
- [x] PR 8b: manifest v2 and `praxis.lock.json` (declared `provides`,
  ownership across tools/routes/UI/assets, immutable revision, rollback).
  Progress: `requires.plugins`/`requires.commands` preflight and uninstall
  dependent protection are implemented, `PLUGINS_DIR/praxis.lock.json` records
  manifest/hook hashes with `praxis plugin verify` reporting drift,
  `install-default` resolves preset dependencies in order with cycle detection,
  and `provides` now declares an external `tools.json` plus route/UI/asset/
  migration namespaces with one-owner checks, and places declared assets into
  `ROOT_DIR` with operator-edit preservation (`plugin_assets.json`). A package
  can bind a service to an authenticated web contribution (`provides.web` +
  worker `web_info`), proxied through the dashboard/Host API feature routes, and
  run namespaced, reversible migrations (`provides.migrations` → `migrations/*.sql`
  against `DATA_DIR/plugin_data/<owner>.db`, `praxis plugin migrate [--down]`).
- [x] PR 8c: kernel interfaces (trust levels, evidence observer and signed
  receipts, replaceable runtime engine interface).
  Progress: the `role` declaration and the operator trust store
  (`praxis plugin trust`, `DATA_DIR/plugin_trust.json`) are implemented; a
  package requesting more than its grant is not activated. Receipts now record
  `verified_by` ("kernel" for native checks, `<package>@<version>` for
  contracted actions) while the kernel observes the evidence, and receipts are
  kernel-signed with `require` verifying the signature. The rendering path is
  behind a `runtime::engine::RuntimeEngine` seam with `KernelEngine` as
  the default, covering rendering and state-machine guard/transition condition
  evaluation. A `runtime` package can declare an `engine` worker that the host
  launches and installs at startup. State-machine evaluation is async, so the
  worker owns guard/transition policy too: `RuntimeEngine::evaluate_condition`
  is a worker call that receives the kernel's observed evidence (signed
  receipts, exit codes, resource/workspace hashes and reply facts) and fails
  closed when the worker is unavailable. Receipt keys persist in
  `DATA_DIR/receipt.key` (or `PRAXIS_RECEIPT_KEY`), so archived receipts verify
  across restarts.
- [ ] PR 8d: extract the remaining packages and ship the `--install-default` and
  minimal presets; make the TUI a separate `praxis-tui` frontend plugin.
  Progress: `praxis-tui` now exists as a standalone executable and the `tui`
  frontend package (`plugins/tui`, `scripts/install-tui-package.sh`), pending
  the crate extraction so the kernel does not link it.
- [ ] Future: **`xis`**, a separate setup/package manager executable that
  installs whole setups (plugins, skills, templates, state machines, config
  profiles) from self-hosted repositories, with keep/backup/overwrite policy
  and a MOTD of required changes. Not part of the kernel; talks to Praxis over
  its CLI and documented files. See
  [the xis design](../docs/XIS_PACKAGE_MANAGER.md).

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

See [process hosting](../docs/PLUGIN_PROCESS_PROTOCOL.md). Guest ownership and
restart recovery are implemented; the full PR 3 checkbox remains open for the
remaining upgrade/lifecycle policy. Generic v2
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
  `plugins/dashboard`, `scripts/install-dashboard-package.sh`): ships the
  frontend assets and serves the whole UI on Host API v1; verified end to end
  against a `--no-default-features` core.
- [x] Navigation slots for absent features: the host reports registered
  `runtime::feature_slots` (live, installed-but-unready and installable) with
  `present`/`hint`, and the dashboard keeps the entry and shows the hint instead
  of hiding it. An absent slot loads no page and executes no package code, and
  listing slots never starts a feature service.
- [ ] Remove the built-in dashboard feature after one release.

## PR 5 progress — tool packages

- [x] Every native tool has exactly one owning package (`tools::packages`,
  tested against the default catalog). Packages enable/disable independently
  via CLI, dashboard and Host API; catalog, discovery and execution respect
  them; per-tool flags are preserved; startup never changes package state;
  `file_ops` is core. See [tool packages](../docs/TOOL_PACKAGES.md).
- [x] Plugin replacement of builtin packages (`"replaces": [...]`), with one
  replacement per package, core packages protected and an example
  (`examples/tool-packages/allowlist_shell`).
- [x] First implementation extraction: `legacy_file_ops` owns `read_file` and
  `edit_file` in an optional crate and independently installed executable.
  Compatibility links the adapter; `--no-default-features` omits its code and
  dependency. Names, schemas, text results and per-tool choices are retained.
- [x] Generic executable transport v1: bounded stdin/stdout, exact text results,
  declared secrets, timeouts and cancellation. Missing native implementations
  cannot reappear via stale rows/static schemas; management reports availability.
- [x] Second implementation extraction: `shell` owns `execute_terminal`,
  `run_background` and `background_status` in `crates/praxis-shell` plus an
  independently installed package and worker. Compatibility links the native
  adapter; `--no-default-features` omits its code and dependency. Foreground
  keeps the exact one-shot result; background jobs use a long-lived worker
  because the registry is process memory. The host issues caller identity and
  drains completion notices.
- [x] Third implementation extraction: `vision` owns `understand_image` in
  `crates/praxis-vision` plus an independently installed executable. The
  compatibility adapter converts the crate's JSON result to the host provider
  types; the installed package returns the same `text`/`content_parts` text.
  `--no-default-features` omits the crate and its dependency.
- [x] Manifest-declared process services: a `service` handler may name an
  installed `executable` and optional `args`. The loader resolves it inside the
  package and the host binds and initializes it at startup, with conflicting
  declarations, missing executables, bad handshakes and crashes failing closed.
  Explicit bindings stay authoritative; models cannot select an executable.
  The installed shell package now declares its worker this way, so
  `SHELL_SERVICE_EXECUTABLE` is only an operator override.
- [ ] Extract the remaining tool owners. Runtime-control wrappers remain scoped
  to host APIs; POML/context/SM/IR verification stays in core.
- [ ] Complete the versioned core `write_file` contract and finish dependency
  cleanup after the corresponding feature packages are independently installed.
  Progress: the checked `expected_absent`/`expected_sha256` single-file
  transactional contract is implemented; the legacy raw form is unchanged.

Verification for the first tool implementation extraction: 1,014 compatibility
and 986 core-only library tests, 17 CLI tests in each build, six file-package
tests, 11 plugin-protocol tests and four dashboard helper scripts passed.
The actual separately built helper passed chat/agent dispatch on both hosts,
including live flags, cancellation and stale verification receipts. Three
selected real-POML workflow integrations covered IR rollback/completion,
capability recovery and graph navigation; two core-only integrations exercised
real Cargo build/tests and source rollback. The core-only dependency tree and
compiled binary exclude the legacy-file implementation. A compiled CLI and
fresh package install passed; reinstall refusal preserved the existing binary
and manifest. No live model inference was used.

Verification for the shell extraction: `crates/praxis-shell` adds foreground
capture plus a durable background-job registry behind the versioned process
protocol. Foreground `execute_terminal` keeps the one-shot exact-text result;
`run_background`/`background_status` require the installed worker bound through
`SHELL_SERVICE_EXECUTABLE`. Host adapter tests use a local protocol fixture and
cover authenticated caller identity, registration without spawning, missing or
disabled workers, bad versions and crash fail-closed without host fallback. The
crate's own tests cover stream/exit/truncation results, completion hooks, owner
scoping, cleanup bounds and the actual installed binary through protocol v1.
The `shell` feature is part of `compatibility`; a `--no-default-features` build
omits the crate and its dependency, and stale rows cannot advertise it. No live
model inference was used.

Verification for the vision extraction: `crates/praxis-vision` reads bounded
PNG/PPM-family files and returns the historical `{"text","content_parts"}`
shape. The native adapter converts that JSON to the host provider types; the
installed executable returns it unchanged so chat/agent content parts still
attach. Crate tests cover error and PNG data-URL output; host tests cover the
installed package through both ingress modes, live flags and cancellation on
both host configurations. The core-only dependency tree excludes
`praxis-vision` and stale rows cannot select it. No live model inference was
used.

Existing setup guides remain applicable during migration:

- [Reproducible Rust/Snake setup and test prompt](../docs/SNAKE_IR_TEST.md).
- [Verified capabilities in normal coding states](../docs/VERIFIED_CODING_ROLES.md).
- [Language-learning flow](../docs/LANGUAGE_LEARNING_FLOW.md).
- [Current plugin action contracts](../docs/PLUGIN_ACTION_CONTRACTS.md).
