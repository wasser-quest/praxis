# Builtin tool packages

Every native tool belongs to exactly one package (ownership table in
[plan/PLUGINIZATION.md](../plan/PLUGINIZATION.md) §4). A package is enabled or
disabled as a unit:

```bash
praxis plugin builtins                     # list packages, state and tools
praxis plugin disable-builtin shell        # execute_terminal, run_background, background_status
praxis plugin enable-builtin shell
```

The same switch is on the dashboard Tools page, in the built-in and packaged
dashboard (`/api/tool-packages`), and in Host API v1
(`GET /admin/tool-packages`, `POST /admin/tool-packages/:id {"enabled": bool}`).
State lives in `DATA_DIR/tool_packages.json`; running instances apply it on
the next model turn.

| Package | Tools |
| --- | --- |
| `file_ops` (core, always on) | `inspect_file`, `write_file`, `apply_patch` |
| `runtime_control` | `search_tools`, `execute_decision`, `read_tool_result`, `get_context`, `set_context`, `delete_context`, `agent_next`, `agent_back`, `agent_complete`, `agent_set_path`, `agent_feedback`, `run_check` |
| `legacy_file_ops` | `read_file`, `edit_file` |
| `shell` | `execute_terminal`, `run_background`, `background_status` |
| `delegation` | `delegate_task`, `list_delegations` |
| `memory` | `memory_profile_*`, `memory_get`, `memory_set`, `learn_*` |
| `rag` | `rag_search`, `rag_ingest`, `rag_list`, `rag_delete` |
| `cron` | `cron_add`, `cron_delete`, `cron_list`, `cron_toggle`, `cron_run` |
| `discord` | `discord_upload_file`, `discord_send_message`, `discord_send_embed` |
| `vision` | `understand_image` |
| `interaction` | `send_screenshot`, `ask_questions` |
| `skills` | `search_skills`, `use_skill` |
| `workflow_authoring` | `update_template` |

VM tools are owned by the installed `vm` plugin.

Rules:

- A disabled package's tools leave the model catalog, `search_tools`
  results and execution (rechecked at the effect boundary, so stale schemas
  cannot call them).
- Per-tool flags are separate and preserved: re-enabling a package restores
  the earlier per-tool choices. Startup never changes package state.
- `native_available` describes build-time availability. Enabling a package
  switch cannot install code omitted from the binary. The CLI and dashboard
  report that distinction; install the separate package or select its Cargo
  feature explicitly.
- Disabling `runtime_control` removes the model-facing navigation and
  Decision IR tools, not the runtime: SM guards, IR enforcement, receipts and
  the management CLI/API keep working.
- `legacy_file_ops` and `shell` are raw operations; they are not verified
  language capabilities (those are contracts such as `verified_rust`).

## Replacing a package with your own plugin

A plugin can implement a builtin package's tool names instead of the native
code by declaring `"replaces"` in its `plugin.json`:

```json
{ "name": "allowlist_shell", "replaces": ["shell"],
  "tools": [{ "name": "execute_terminal", "handler": { "type": "script", "path": "run.py", "interpreter": "python3" }, ... }] }
```

```bash
praxis plugin install examples/tool-packages/allowlist_shell
```

- The plugin owns every name of that package it declares; undeclared names
  (here `run_background`, `background_status`) stay native.
- Only one enabled plugin may replace a package, and it may not declare
  native names from other packages.
- `file_ops` and `runtime_control` cannot be replaced: they carry the core
  file contract and host-owned workflow semantics.
- The replacement is controlled by the plugin (enabled flag and per-tool
  flags); the builtin package switch only controls the native code. A
  replacement inherits an existing native per-tool flag until an explicit
  plugin flag overrides it. Installing a package never reenables a tool.
- Replacements run through normal plugin dispatch: task cancellation,
  workflow action guards and per-tool flags still apply. They are
  operator-trusted code, not sandboxed.

Disable or uninstall the plugin to return to an enabled native implementation
if one was linked. A core-only host cannot fall back to omitted code.

## Independently installed legacy file operations

`read_file` and `edit_file` now live in `crates/praxis-legacy-file-ops`, not
`src/tools`. The `legacy_file_ops` Cargo feature links the compatibility
adapter and is included in the default `compatibility` build. Without that
feature, neither its implementation nor its crate dependency ships in the
host. Stale tool rows and static schemas cannot advertise or execute it.

Build a host without the native package, then install its executable separately:

```bash
cargo build --release --locked -p praxis --no-default-features
./scripts/install-legacy-file-ops-package.sh /absolute/path/to/plugins
# Keep PLUGINS_DIR pointed to that directory and restart Praxis.
```

The installer builds only the package and installs its `plugin.json` and
`bin/praxis-legacy-file-ops`. It honors `CARGO_TARGET_DIR`, `PLUGINS_DIR` and
`PROFILE=debug`; it refuses to overwrite an existing package. For an upgrade,
stop Praxis and explicitly replace the binary from `target/release` (or your
custom target directory), preserving any edited manifest. No provider, VM,
dashboard, Node or Python is needed for these two file operations.

The installed package declares `replaces: ["legacy_file_ops"]` and uses the
same tool names, public schemas and text results. Reads retain whitespace and
the full text up to the existing 8 MiB bound; oversized or non-UTF-8 files fail
explicitly. Edits replace the first match and accept the legacy
`old_text`/`new_text` aliases as well as `old_string`/`new_string`. Relative
paths keep the host process's working directory. These remain raw operations,
not transactional or language-verified source changes.

This separately installed package performs **host** file operations, as other
replacement plugins do. The native compatibility adapter retains agent-mode VM
aliases; use `vm_file_read`/`vm_shell` explicitly for guest operations in a host
without that adapter. Verified host-workspace contracts are unaffected.

## Executable tool transport v1

A raw tool can declare a handler such as:

```json
{"type":"executable","path":"bin/praxis-legacy-file-ops","timeout_secs":30}
```

The loader resolves a regular file inside the package. Each call starts one
process and sends a JSON request on stdin containing `protocol_version: 1`,
`tool`, `arguments`, `context` and declared `secrets`. The helper returns
`{"protocol_version":1,"result":"exact result text"}`. The host preserves
empty strings and whitespace and rejects incompatible, malformed, oversized
or nonzero-exit responses. JSON travels over pipes rather than environment
variables; only public process settings are inherited.

Requests are bounded to 8 MiB and decoded result text to 8 MiB. The response
capture allows JSON escaping overhead. Timeouts are 1–600 seconds; cancellation
or timeout kills the owned helper/process group. Existing task guards and live
tool flags apply before spawning. This raw protocol supplies no verification
authority: contracted actions continue using service, verification, source-edit
or existing script/HTTP adapters. Installed executables remain operator-trusted.

Next: extract `shell` and its background-job service, then the remaining owners
listed in the plan. Their code still ships in the host until each extraction.
