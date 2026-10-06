# Praxis pluginization handoff

This is the working handoff for turning Praxis into a small kernel that loads
plugins, everything replaceable, with an optional external package manager
(`xis`). Read this first, then
[the plugin lifecycle and trust model](PLUGIN_LIFECYCLE.md),
[the xis design](XIS_PACKAGE_MANAGER.md) and [the roadmap](../plan/README.md).

Current `main` at handoff: `dc9edbe` (2026-10-05). All work below is merged.

## 1. The idea

Praxis should be **a tiny kernel plus installable packages**:

* The kernel loads plugins, authenticates callers, runs tasks, and owns
  authority and verification.
* Every capability is a package/file: tools, services, providers, channels,
  dashboard, TUI, memory, RAG, cron, skills, workflow assets, and even the
  default POML/state-machine/guard engine.
* Packages are installed, upgraded and removed with lifecycle scripts under an
  operator policy, pinned in a lockfile with hashes.
* A separate executable, **`xis`**, distributes whole setups from self-hosted
  repositories and reports required changes (MOTD).

The one thing that cannot become an ordinary untrusted plugin is verification:
if a plugin could certify its own claims, "verified" becomes text. So the kernel
keeps a **trust root**, an **evidence observer** and **receipt signing**, and
engines own *policy* while the kernel observes *reality*.

## 2. Non-negotiable invariants

Keep these true or the design collapses:

1. **Kernel owns evidence.** It runs checks, observes exit code/timeout, hashes
   resources and the workspace revision, and signs receipts. Plugins define
   which checks/resources matter, not what happened.
2. **Caller identity is host-issued.** No tool argument, context variable or
   plugin claim can change user/session/task/workspace/owner.
3. **Receipts are signed and verified** (`gateway::receipt_sign`); editing
   `verified`/`exit_code`/resource hashes invalidates a guard.
4. **Trust roles are granted by the operator**, never by a package
   (`role` + `praxis plugin trust`).
5. **Missing, disabled or unready code fails closed.** No silent fallback to
   native, no automatic replay of a failed/cancelled effect.
6. **Operator files are never clobbered silently.** Assets/config are
   keep/backup, with an ownership record.
7. **Installing never enables tools, starts services or grants authority.**

## 3. Architecture map

### Crates (`crates/`)

| Crate | Role |
| --- | --- |
| `praxis` (root) | kernel binary + library: registry, dispatch, DB, gateway, dashboard feature, providers/channels/media still compiled in |
| `praxis-plugin-api` | shared host/worker protocol: process protocol v1, executable v1, Host API v1, web descriptors, bounded process capture |
| `praxis-vm`, `praxis-vm-web`, `praxis-vm-worker` | VM engine, web contribution, installed `praxis-vm-service` |
| `praxis-dashboard` | dashboard frontend package on Host API v1 |
| `praxis-legacy-file-ops` | `read_file`/`edit_file` executable |
| `praxis-shell` | `execute_terminal` executable + durable background worker |
| `praxis-vision` | `understand_image` executable |

### Installed packages (`packages/`, `plugins/`)

* `packages/{dashboard,legacy_file_ops,shell,tui,vision}` — first-party optional
  packages with installers in `scripts/`.
* `plugins/*` — shipped v1 tool plugins (vm, comfyui, elevenlabs_tts,
  brave_search, sosse, spotify, system_info, openrouter_image, mimo_understand).

### Cargo features

`compatibility = ["vm", "dashboard", "legacy_file_ops", "shell", "vision"]` is
the default. `--no-default-features` omits the optional crates and their
implementations; `legacy_file_ops`, `shell`, `vision` can be enabled alone.

### Transports

* **Executable v1** (`praxis_plugin_api::executable`): one process per call,
  bounded JSON on stdin/stdout, exact text results. Used by legacy files, shell
  foreground, vision.
* **Process protocol v1** (`praxis_plugin_api::{Client,serve,LaunchSpec}`):
  long-lived worker with handshake, invoke/control/health/cancel/shutdown.
  Used by the VM worker, shell background jobs, declared services, the engine.
* **Host API v1** (`praxis_plugin_api::host`): loopback HTTP + token + scopes for
  frontend packages (dashboard) and feature pages.
* **Dashboard block** in `plugin.json` selects a dashboard executable.

### Manifest (`plugin.json`)

v1 fields plus additions that are all validated and part of the registry
revision:

```json
{
  "name": "shell", "description": "…", "version": "0.1.0", "enabled": true,
  "replaces": ["shell"],
  "hooks": {"install": "hooks/install.sh", "uninstall": "hooks/uninstall.sh"},
  "requires": {"plugins": ["runtime"], "commands": ["cargo"]},
  "frontend": {"executable": "bin/praxis-tui", "args": []},
  "provides": {
    "tools": "tools.json",
    "routes": ["shell"], "ui": ["shell.panel"],
    "assets": ["templates/foo.poml"], "migrations": ["shell_0001"],
    "web": {"service": "shell", "title": "Shell"}
  },
  "role": "runtime",
  "engine": {"executable": "bin/engine", "args": ["--stdio"]},
  "tools": [ {"name": "…", "handler": {"type": "..."}} ]
}
```

Handlers: `builtin`, `http`, `script`, `executable`, `verification`,
`source_edit`, `service` (native or manifest-declared process worker).

## 4. What is implemented (merged)

### PR 1 / PR 2 / PR 3 — foundation, VM

* One owner catalog (`tools/catalog.rs`, `tools/packages.rs`); discovery,
  preflight and execution share it. Duplicate owners rejected atomically.
* Shared chat/agent/WebSocket/Decision-IR dispatch (`gateway/tool_dispatch.rs`).
* Runtime services: events, templates, retention; `ServiceHost` v1.
* Native service invocation API v1 with host-issued identity, workspace,
  deadline, cancellation, scoped storage and declared secrets.
* VM engine, web contribution (routes/VNC/noVNC/UI), installed worker, CLI and
  screenshots. VM PR3 lifecycle policy remains partly open.

### PR 4 — dashboard

* `dashboard` Cargo feature; without it there is no listener/TLS/assets.
* Host services extracted (`src/services`), Host API v1, feature page slots.
* Standard dashboard package (`crates/praxis-dashboard`,
  `packages/dashboard`, `scripts/install-dashboard-package.sh`).
* Open: navigation slots for absent features; remove the builtin dashboard
  after a release.

### PR 5 — tool packages and lifecycle (the bulk of this session)

* `file_ops` core: `inspect_file`, `write_file`, `apply_patch`, `run_check`.
* Extracted: `legacy_file_ops`, `shell`, `vision` (optional crates + packages +
  installers), and the TUI as a standalone `praxis-tui` executable +
  `packages/tui`.
* **Lifecycle hooks**: `hooks.install`/`hooks.uninstall`, policy
  `PLUGIN_HOOKS=allow|ask|deny`, staged atomic install, rollback, script
  hashing (`PLUGINS_DIR/praxis.lock.json`), `plugin verify`.
* **`plugin upgrade`** with backup and restore on hook failure.
* **`requires`** preflight (plugins/commands) + uninstall dependent protection
  + dependency-ordered `install-default` with cycle detection.
* **`plugin enable/disable`**, `plugin trust/untrust`, `plugin install-default`.
* **Manifest v2 `provides`**: external `tools.json`, `routes`/`ui`/`assets`/
  `migrations` namespaces with one-owner checks; declared **assets are placed
  into `ROOT_DIR`** with operator-edit preservation (`DATA_DIR/plugin_assets.json`);
  a declared **`web`** contribution binds a service to authenticated routes/UI;
  declared **migrations** run against a per-package database
  (`praxis plugin migrate`).
* **Trust roles + operator trust store** (`DATA_DIR/plugin_trust.json`): a
  package requesting more than its grant is not activated.
* **Receipt identity + signing**: `verified_by` (`kernel` or
  `<package>@<version>`), HMAC-SHA256 signature, verified by
  `action_contracts::require` (`PRAXIS_RECEIPT_KEY` for a stable key).
* **Engine seam**: `runtime::engine::RuntimeEngine` with `KernelEngine` default;
  rendering and SM guard-condition evaluation route through it. State-machine
  evaluation is async, so a process engine can own guard policy too.
* **Engine host bridge**: a `runtime` package with an `engine` block is launched
  at startup and installs a `RuntimeEngine` that renders and evaluates
  guard/transition conditions through the worker (failing closed on errors),
  receiving the kernel's observed evidence snapshot.
* **Checked `write_file`**: `expected_absent` / `expected_sha256` transactional
  single-file write with the durable journal; legacy raw form unchanged.

### Future design recorded

* `docs/XIS_PACKAGE_MANAGER.md` — separate `xis` executable: repositories,
  setup bundles, keep/backup/overwrite, ownership lock, MOTD.

## 5. Big ideas in detail

### 5.1 Lifecycle hooks and policy

`praxis plugin install` copies to a staging dir, publishes atomically, then runs
the install hook from the plugin dir with `PRAXIS_PLUGIN_DIR/DATA_DIR/PLUGINS_DIR`
and public env only. Failure removes the published dir and writes no record.
Uninstall runs the removal hook first; data is preserved unless `--purge`.
Policy is `allow|ask|deny` (default `ask`); non-interactive `ask` skips. Script
bytes are hashed so a changed uninstall hook needs `--force`.

### 5.2 Everything is a file (`provides`)

Tools can live in an external `tools.json`. Assets mirror their package-relative
path under `ROOT_DIR`; a destination that differs from the last owned revision is
an operator edit and is kept. `routes`/`ui`/`migrations` are ownership-checked
declared namespaces. A `web` block binds a package's service to a descriptor the
worker must match via `web_info`; the host then proxies it through authenticated
dashboard/Host API routes. Declared migrations run against a package-owned
SQLite database under `DATA_DIR/plugin_data/<owner>.db` (namespaced and
reversible), so a migration cannot read core or another package's data.

### 5.3 Trust roles

`tool < data < channel < ui < authority < runtime`. A package declares its
`role`; the operator grants via `praxis plugin trust`. Activation requires
`granted >= declared`. The role is part of the registry revision.

### 5.4 Evidence and receipts

`run_check` and contracted plugin actions run the check in the kernel, which
observes exit code/timeout and resource/workspace hashes and issues a signed
receipt naming the policy owner. Guards compare against kernel state; a plugin
cannot forge or edit a receipt.

### 5.5 Replaceable runtime engine

`RuntimeEngine` owns rendering and guard-condition policy. The default
`KernelEngine` wraps today's POML/SM code. A `runtime`-role package can supply an
engine worker (rendering through process protocol v1). State-machine guard,
auto-rule, override and transition conditions are evaluated asynchronously
through the engine, so the worker owns guard policy as well as rendering; a
crashed or unavailable worker fails closed (condition is false). Every
condition receives a kernel-produced evidence snapshot (receipts with exit
codes/signatures, resource and workspace hashes, reply facts) that the engine
may read but cannot forge. The kernel still parses the state machine and owns
every observed fact and receipt verification.

### 5.6 Checked file writes

`write_file` with `expected_absent` or `expected_sha256` is a single-file
transaction on the durable journal (existing parents, no symlinks/hardlinks,
atomic publish, rollback). Both/neither precondition is an error, so a checked
call never silently falls back to a raw overwrite.

### 5.7 `xis`

Separate executable. Fetches signed repository indexes, resolves a setup
(plugins/skills/templates/contexts/config profile), plans, applies with
keep/backup/overwrite and an ownership lock, delegates plugin installs to the
Praxis CLI, and writes a MOTD of required changes. The kernel does not link it.

### 5.8 Feature navigation slots

A feature that can contribute a dashboard page has a **registered slot**
(`runtime::feature_slots`). A slot is only a title plus a hint: it grants no
authority and never starts a service. The extension list reports every slot as
`present` or `hint`ed, covering three sources — a live web binding, an installed
package whose service is not ready (hint names the service), and a first-party
feature that is not installed (hint says how to install it). The dashboard then
keeps the navigation entry instead of hiding the feature and shows the hint
panel; no page, script or stylesheet is fetched for an absent slot. So “absent”
means *explained*, never *silently missing*, and drawing the UI never boots a
VM/media service.

## 6. What needs to be done (prioritized)

### A. Finish the kernel seam — done

Async SM evaluation (A.1), observed evidence to the engine (A.2) and a stable
receipt-key store (A.3) are implemented. See §5.4, §5.5 and §7. The remaining
kernel-seam work is in §B (routes/UI loading and migrations).

### B. Manifest v2 loading — done

A declared `provides.web` binds a package's service to an authenticated web
contribution: the host derives the descriptor, requires the worker's `web_info`
to match it, and proxies it through the dashboard/Host API feature routes
(one owner, authenticated). Declared `provides.migrations` resolve to
`migrations/<id>.sql` scripts that run against a per-package database
(`DATA_DIR/plugin_data/<owner>.db`) with recorded hashes and optional `.down.sql`
reversal (`praxis plugin migrate [--down]`). See §5.2.

### C. Package extraction (one PR per owner)

Extract, in roughly this order: `runtime_control`, `delegation`, `memory`,
`rag`, `cron` (+ its scheduler worker), `discord`, `interaction`, `skills`
(+ `skills/` assets), `workflow_authoring`. Each needs the engine/evidence seam
plus a host bridge (render/storage/delivery) where it touches core services.
Then make `file_ops` a privileged bundled plugin.

**Blocker found:** none of these owners has a separable implementation today.
`runtime_control` alone dispatches to `db::load_context`,
`action_contracts::run`, `tools::{discovery,tool_output,agent_control}` and
`gateway::prompt`; `memory`/`rag`/`cron` own core tables; `skills` renders
through the engine and reads the skills index. So the recipe's “implementation
crate with no host internals” cannot be satisfied without first defining the
**host bridge** trait surface (context read/write, navigation, check runner,
scoped storage, delivery). That seam is a prerequisite PR; extraction is
mechanical after it. `runtime_control` is still the right first owner because
its 12 tools are pure wrappers over host-owned operations and nothing else in C
depends on it.

### D. Providers, channels, media, assets (PR 6)

Move providers behind common streaming/usage/error interfaces; move Discord,
voice/audio, ComfyUI, GPU routing, image providers; add coding/learning/persona/
workflow asset packs.

### E. TUI crate extraction

Move `src/tui` into an unlinked `praxis-tui` crate; complete the remote Host API
path so it no longer reads the local DB or links the kernel.

### F. Minimal distribution and dependency cleanup (PR 7)

Minimal kernel + `file_ops` + plugin management; a standard `--install-default`
assembled from packages; remove now-unconditional heavy deps (`serenity`,
`ratatui`/`crossterm`, `image`, audio, TLS) after their packages move out.

### G. VM/dashboard loose ends

Dashboard navigation slots for absent features are implemented: the host
reports registered `runtime::feature_slots` (live, installed-but-unready and
first-party installable features) with `present`/`hint`, and the dashboard
renders an absent feature as an install/enable hint panel that loads no page
and executes no package code. Listing never starts a feature service.

Still open: VM upgrade/drain/lifecycle policy for active guests, and removing
the builtin dashboard after a release.

## 7. Known limitations and caveats

* Route/UI/migration loading is implemented: `provides.web` binds a service
  contribution proxied through authenticated routes, and `provides.migrations`
  run against a package-owned database with recorded hashes and optional down
  scripts. Assets are placed under `ROOT_DIR`.
* Guard-condition policy is replaceable by a process engine: state-machine
  evaluation is async, `RuntimeEngine::evaluate_condition` is a worker call that
  receives the kernel's observed evidence snapshot, and worker failure fails
  closed. Receipt keys persist in `DATA_DIR/receipt.key` (or
  `PRAXIS_RECEIPT_KEY`), so archived receipts verify across restarts.
* `file_ops`, `runtime_control`, memory, RAG, cron, Discord, interaction,
  skills, workflow authoring, providers, channels and media still compile into
  the kernel; only the crates/packages in §3 are independently installable.
* `praxis-tui` is a `src/bin` inside the kernel crate, not yet an unlinked crate.
* The engine package requires `role: "runtime"` **and** an operator grant; a
  package cannot promote itself.
* **Test environment note:** on the small handoff box, compiling the lib test
  with full debug info was OOM-killed (`SIGKILL`); verification used
  `CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1`. On a
  memory-tight builder the *link* of that single large test binary can be
  OOM-killed too (it needs more RAM than a ~2 GiB budget); free memory and
  retry, or set `PRAXIS_LOW_MEMORY_LINK=1` to link through BFD in low-memory
  mode (see `build.rs`). Two ComfyUI tests are timing-flaky under parallel load
  and pass in isolation.
* Hooks and installed executables are operator-trusted code, not sandboxed.

## 8. Verification commands

```bash
cargo build --locked --no-default-features -p praxis
cargo build --locked -p praxis                      # default compatibility
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 \
  cargo test --locked --lib
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 \
  cargo test --locked --no-default-features --lib
cargo test --locked --bin praxis
cargo test --locked --no-default-features --bin praxis
cargo test -p praxis-shell -p praxis-vision -p praxis-plugin-api
cargo tree --locked --no-default-features -p praxis   # no optional packages
node scripts/test_ui_static.js
node scripts/test_workflow_dashboard.js
node tests/test_dashboard_extensions.js
node tests/test_chat_commands.js
git diff --check
```

Lifecycle smoke (temp dirs; note `ROOT_DIR` receives declared assets):

```bash
TMP=$(mktemp -d); mkdir -p "$TMP/root/templates"
PLUGINS_DIR="$TMP/plugins" DATA_DIR="$TMP/data" ROOT_DIR="$TMP/root" PLUGIN_HOOKS=allow \
  ./target/debug/praxis plugin install ./examples/plugins/file_tools --no-scripts
PLUGINS_DIR="$TMP/plugins" DATA_DIR="$TMP/data" ROOT_DIR="$TMP/root" \
  ./target/debug/praxis plugin verify
```

## 9. Recipe: add a package

1. Create the implementation crate under `crates/` (no host internals) or the
   installed executable + `plugin.json` under `packages/`.
2. Add an optional `[features]` entry and dependency; include it in
   `compatibility` only if it is a former default. Gate host call sites with
   `#[cfg(feature = "…")]`.
3. Add `packages/<id>/plugin.json` and `scripts/install-<id>-package.sh`
   (honors `CARGO_TARGET_DIR`, `PLUGINS_DIR`, `PROFILE`; refuses overwrite).
4. Add the tool/owner to `tools/packages.rs`; extend `native_available` if the
   implementation is optional.
5. Keep schemas, result formats, flags and allowlists; route any changed
   rendering through `runtime::engine`.
6. Add crate tests, host adapter/installed-worker tests, and a core-only
   exclusion test; update docs and `plan/README.md`.

## 10. Key files

| Area | File |
| --- | --- |
| Owner catalog / packages | `src/tools/catalog.rs`, `src/tools/packages.rs` |
| Dispatch | `src/gateway/tool_dispatch.rs` |
| Plugin manifest/loader | `src/plugins/mod.rs` |
| Lifecycle/hooks/lockfile/assets | `src/plugins/lifecycle.rs` |
| Trust store | `src/plugins/trust.rs` |
| Receipts/signing | `src/gateway/action_contracts.rs`, `src/gateway/receipt_sign.rs` |
| Engine seam/bridge | `src/runtime/engine.rs`, `src/runtime/engine_bridge.rs` |
| Process host API | `crates/praxis-plugin-api/` |
| Docs | `docs/PLUGIN_LIFECYCLE.md`, `docs/XIS_PACKAGE_MANAGER.md`, `plan/README.md`, `plan/PLUGINIZATION.md` |
