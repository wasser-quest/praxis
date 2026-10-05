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
  flags); the builtin package switch only controls the native code.
- Replacements run through normal plugin dispatch: task cancellation,
  workflow action guards and per-tool flags still apply. They are
  operator-trusted code, not sandboxed.

Disable or uninstall the plugin to return to the native implementation.

Next: move package implementations out of the core binary one at a time
behind the same names.
