# Praxis Decision IR v1

Decision IR (intermediate representation) is a compact instruction format for
the action the model chooses. A trusted workflow maps an opcode to a capability;
Praxis lowers it into the normal gateway dispatcher. Contracts, checks, rollback
and task-owned receipts still determine whether the action succeeded.

It uses the configured chat model and endpoint, including Ollama. There is no
additional IR endpoint or model setting. State routing and its probability
threshold continue to use the existing Decision-router configuration.

## First use

Rebuild Praxis and update the opt-in plugin manifest:

```bash
mkdir -p plugins/verified-rust
cp examples/plugins/verified-rust/plugin.json plugins/verified-rust/plugin.json
```

Use the configured `PLUGINS_DIR` if different. Restart Praxis and select
`verified-implementation` as `settings.sm_file` for a **new task**. Install the
workflow in the active runtime's `contexts` directory. The pinned `ROOT_DIR` is
the project root; author checks run there. Ensure `cargo` and `rustfmt` are on
Praxis's PATH. Adapt the plugin's edition, commands and resource scopes for the
project. This example assumes Rust edition 2021.

### Select the workflow and its prompt together

The bundled workflow now selects `templates/verified-implementation.poml` in
both `working` and `done` and disables persona Decision routing. The prompt
explains actual tool-call arguments, expected hashes, full-file replacements,
receipts, rollback, and guarded completion. It reports setup mismatches rather
than searching for a substitute workflow. A template alone grants no capability.

Stop the previous task and merge these fields into the **current chat's** context
through the dashboard, keeping the other settings:

```json
{
  "mode": "agent",
  "sm_file": "verified-implementation",
  "active_state": "working",
  "settings": {
    "sm_file": "verified-implementation",
    "system_template": "verified-implementation",
    "active_state": "working",
    "use_decision_router": false,
    "decision_profile": "off",
    "done": false,
    "activated_tools": [
      "execute_decision", "inspect_file", "modify_source",
      "build_workspace", "run_workspace_tests", "agent_complete",
      "get_context", "read_tool_result"
    ]
  }
}
```

`settings.sm_file` takes precedence over the top-level `sm_file`. Set
`settings.system_template` explicitly when switching an existing chat: a retained
manual template such as `states/standard/standard` can keep workflow routing from
applying the new state's settings. Editing only the `default` context does not
update an already-forked chat. `working` exists in several workflows and is not
proof that IR is active.

For an existing separate test project such as `ir-snake`, update **both** its
`ROOT_DIR/contexts/verified-implementation.sm` and
`ROOT_DIR/templates/verified-implementation.poml`, as well as the installed plugin
manifest. The template is self-contained; it has no shared-template includes.
Changing shell environment variables does not update an already-running Praxis
process. Restart after a plugin or root change, then begin a new task.

Only the actual offered tool schemas establish tool availability. The POML
`tools` field is discovery metadata and may omit explicitly activated tools.
The runtime-appended opcode table and contract guards remain authoritative.
Normal calls remain compatible; ask for `execute_decision` explicitly when
testing IR. In the tool archive, expect `execute_decision` calls with canonical
capability names inside the receipts. `agent_complete` need not switch
`active_state` to `done`.

### Diagnose disabled or unavailable IR targets

The runtime's `IR tool ... is disabled or unavailable in the current state`
error covers both global enable flags and the current state's tool selection.
An absent schema alone does not prove that a tool is globally disabled.

Check the active runtime's startup log for `Loaded plugin` with
`name=verified_rust`, `version=1.2.0` and `tools=3`. If the plugin is missing or
failed to load, update its manifest in the actual `PLUGINS_DIR` and restart
Praxis. Copying it into a different project's directory does not install it in
the running process.

In the dashboard's **Tools** tab, enable `execute_decision`, `inspect_file`,
`agent_complete`, `modify_source`, `build_workspace` and `run_workspace_tests`.
Plugin tool switches persist in `DATA_DIR/plugin_tools.json`; builtin switches
persist in `DATA_DIR/tools.json`. A tool flag change takes effect on the next
request without restarting. Plugin installation or manifest changes require a
restart because the gateway holds the loaded plugin registry.

Then inspect the **current chat's** saved context: both `sm_file` fields must
select `verified-implementation`, both state fields must be `working`, and
`settings.activated_tools` must contain the eight names in the setup example.
Use bare tool names in that list; `verified_rust/...` is the qualified identity
for opcode mappings and receipts. In `done`, M/B/T are intentionally unavailable.
Changing an allow-list cannot install a missing plugin or override a globally
disabled tool. After correcting setup, start a new task. Inspecting with
`execute_decision({"ir":"1 R {\"path\":\"src/main.rs\"}"})` can confirm R, but does
not itself verify access to M/B/T or satisfy implementation completion guards.

The workflow enables both normal capability calls and `execute_decision`.
Normal `modify_source`, `build_workspace` and `run_workspace_tests` calls use the
same execution path and can be mixed with IR calls in the same task.

| Opcode | Capability | Example instruction |
| --- | --- | --- |
| `R` | `inspect_file` | `1 R {"path":"src/lib.rs"}` |
| `M` | `verified_rust/modify_source` | `1 M {"path":"src/lib.rs","expected_sha256":"<hash from R>","content":"<replacement source>"}` |
| `B` | `verified_rust/build_workspace` | `1 B {"scope":"workspace"}` |
| `T` | `verified_rust/run_workspace_tests` | `1 T {"scope":"workspace"}` |
| `C` | `agent_complete` | `1 C` |

Send **one instruction per tool call**, using the tool name `execute_decision`:

```json
{"ir":"1 T {\"scope\":\"workspace\"}"}
```

Encode source newlines as JSON string escapes. Use the actual 64-character hash
returned by inspection. The model receives the canonical action receipt and
check output; the archive retains the outer tool name and original call ID.

Completion and `done` require current receipts for `modify_source`,
`build_workspace`, and `run_workspace_tests`. Rolled-back edits, failed tests
and model claims cannot satisfy a guard. Edits invalidate prior build/test
evidence, so run those after the last edit. Verification capabilities preserve
the source receipt. For verification tasks requiring no edit, continue using
`verified-capabilities`.

## Native source edits

`"handler":{"type":"source_edit"}` accepts exactly required string fields
`path`, `expected_sha256` and `content`, under a closed schema. It requires
`workspace_write`, `non_idempotent`, and no generic compensation handler. The
contract owns pre/post checks and the total timeout.

Praxis validates an existing regular UTF-8 file, a normalized path inside the
pinned physical root, and the expected bytes/mode. Symlinks, hardlinks, missing
files, unchanged content and model-supplied commands are rejected. Originals
are saved in the **same durable journal as `apply_patch`** before replacement.
V1 edits one existing file, bounded to 1 MiB; creation/deletion remains available
through `apply_patch`. The complete serialized capability input also has the
existing 1 MiB bound, including JSON overhead and escaping.
Durable native source edits currently require Unix.

An exact check argument `{source_path}` becomes the validated absolute file
path as one argv operand. Programs, cwd and embedded text are never expanded.
Use it in programs expecting a file operand; trusted authors must not use it as
interpreted shell/program text. The bundled contract runs
`rustfmt --check --edition 2021 <source_path>`, then
`cargo check --locked --workspace`. Formatting is checked, not rewritten.
Separate build/test capabilities produce their own receipts.

Only fresh passing postconditions plus durable commit publish action authority.
The edited file is automatically included in the first postcondition's resource
scopes even if the author declared none (the 16-scope bound still applies).
External edits therefore stale the source receipt. Guards resample resources
and the shared workspace revision.

Failed checks, timeout or cancellation restore original bytes and permissions.
Dropping the execution future uses the same native rollback; process death
leaves a journal for recovery. Conflicting external edits are preserved and
reported as `rollback_conflict`, with completion blocked. `rolled_back` also has
`verified:false`. Rollback covers the edited file; verifier programs and their
other effects remain trusted author policy. Existing journal/sampling limits
apply.

## Workflow declarations and limits

```text
[decision_ir]
R = inspect_file
M = verified_rust/modify_source
B = verified_rust/build_workspace
T = verified_rust/run_workspace_tests
C = agent_complete
```

Mappings use unique uppercase single-letter opcodes, pinned with the workflow
and root. Targets are `inspect_file`, `agent_complete`, or a uniquely owned
enabled **contracted** `plugin/tool`. The target and `execute_decision` must also
be allowed in the current state and enabled in the tool database. An opcode
grants no extra permission. Legacy tools, raw shell calls, arbitrary builtins
and recursive IR cannot be targets.

Unknown versions/opcodes, batches, duplicate operand keys, nested operands and
trailing instructions fail closed. Flat scalar operands use the existing
capability schema. IR text is bounded to 2 MiB and 32 operand keys; individual
capability limits still apply. Put `_output` on the outer call, never inside IR.
Each instruction consumes one tool-call budget slot. Original call IDs reach
the capability engine, preserving replay protection. Both gateway dispatchers
support IR; the standalone router/plugin API does not execute IR or contracts.

This is the first executable representation. Patch/artifact references, batch
plans, broader facts and measured token/behavior improvements remain future
work. V1 keeps original capability schemas available and makes no token-saving
or Decision-router quality claim.

## Validation

Strictly render the new prompt against the existing full, sparse, null and chat
fixtures without contacting a model:

```bash
python3 scripts/test_poml_templates.py --template verified-implementation.poml
```

Set `POML_CLI` to the installed Microsoft JavaScript CLI, or pass its path through
`--cli`. This verifies rendering; it does not establish live model behavior.

```bash
cargo test --locked --lib source_capability
cargo test --locked --lib decision_ir
cargo test --locked --lib source_capability_bundled_rust -- --ignored
```

The real Cargo test rolls back a formatted but type-invalid edit, then verifies
a valid edit, build and regression tests permit completion. With a real Microsoft
POML CLI configured through `POML_CLI`, include the full gateway tests:

```bash
cargo test --locked --lib decision_ir -- --include-ignored --test-threads=1
```

These use an offline scripted model and real local processes. They cover chat
and agent loops, permissions, receipts, rollback, call IDs, normal/IR equivalence
and tool budgets without paid inference.
