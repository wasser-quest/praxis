# Verified plugin capabilities

Plugins can opt into the same execution-evidence policy as named workflow checks.
An optional tool `contract` declares its effect, input schema, independent
preconditions/postconditions, timeout, idempotency and compensation. Existing
plugins without contracts keep their existing behavior.

## First use

After rebuilding Praxis from this revision, update the plugin from the repository root:

```bash
mkdir -p plugins/verified-rust
cp examples/plugins/verified-rust/plugin.json plugins/verified-rust/plugin.json
```

Use your configured `PLUGINS_DIR` if it differs from `plugins`. Restart Praxis
to load the plugin, then select `verified-capabilities` as the `.sm` workflow.
Keep `ROOT_DIR` at the installation assets and set `WORKSPACE_DIR` to the Rust
project being verified (absolute, or relative to `ROOT_DIR`). Without it, verified
actions retain the `ROOT_DIR` default. Restart and begin a new task after a root
change. Adapt
the author-owned commands and resource scopes to that project.

The model calls `build_workspace({"scope":"workspace"})` and
`run_workspace_tests({"scope":"workspace"})`. The native verification adapter
runs the manifest's `cargo build --manifest-path Cargo.toml --locked --workspace` or
`cargo test --manifest-path Cargo.toml --locked --workspace` check exactly once.
Plugin v1.2.1 requires a manifest at the selected root instead of letting Cargo
search parent directories. A zero exit status and fresh
source snapshots produce the corresponding runtime receipt. Both capabilities
must be verified before entering `done` or completing; a successful build alone
cannot authorize completion.

The example permits scoped `apply_patch` edits, with its named `build` check
retained for native patch commit/rollback. It offers semantic build/test actions
instead of raw terminal or `run_check` orchestration. It is opt in and does not
install or run itself. Upgrading from plugin v1.0 only requires replacing the
manifest and restarting; the former request script is no longer used. Start a
new task after changing the pinned workflow or capability definitions.

Plugin v1.2 also provides native transactional `modify_source`. Select
`verified-implementation` to require a source-edit receipt together with build
and test receipts, and optionally call those capabilities through compact IR.
See [Decision IR and native source edits](DECISION_IR.md) for setup and examples.

## Manifest

```json
{
  "name": "filesystem",
  "description": "Trusted filesystem adapter",
  "version": "1.0.0",
  "tools": [{
    "name": "modify_source",
    "description": "Apply the adapter's source modification",
    "parameters": {
      "type": "object",
      "properties": {"content": {"type": "string", "maxLength": 65536}},
      "required": ["content"],
      "additionalProperties": false
    },
    "handler": {"type": "script", "path": "apply.py", "interpreter": "python3"},
    "contract": {
      "effect": "workspace_write",
      "idempotency": "non_idempotent",
      "timeout_secs": 120,
      "preconditions": [{"program": "python3", "args": ["verify_snapshot.py"], "timeout_secs": 10}],
      "postconditions": [{"program": "cargo", "args": ["check", "--locked"], "timeout_secs": 90, "resources": ["src", "Cargo.toml", "Cargo.lock"]}],
      "compensation": {
        "handler": {"type": "script", "path": "restore.py", "interpreter": "python3"},
        "postconditions": [{"program": "python3", "args": ["verify_restore.py"], "timeout_secs": 10}],
        "timeout_secs": 30
      }
    }
  }]
}
```

This illustrates the declaration; adapter scripts and safe snapshot handling
must be supplied by the plugin author. Use Praxis's existing `apply_patch` for
durable transactional host-file edits with expected hashes and crash recovery,
or the native `source_edit` adapter for one existing source file.
Check `cwd` and arguments are relative to the project root, not the plugin
folder; installed handler and compensation script paths resolve from the plugin
folder. No model input interpolates check commands.

| Field | Meaning |
| --- | --- |
| `effect` | `read_only`, `verification`, `workspace_write`, or `external_write`. |
| `idempotency` | `idempotent` or `non_idempotent`, declared semantics; neither enables automatic replay. |
| `timeout_secs` | 1–300 seconds for the complete action lifecycle, including checks. |
| `preconditions` | Optional, at most eight independent checks before effects. |
| `postconditions` | One to eight independent checks, required for success. |
| `compensation` | Optional for mutating effects only; handler plus one to eight independent cleanup checks and its own 1–300 second budget. |

Checks reuse the [named-check contract](EXECUTION_CONTRACTS.md): program, args,
cwd, timeout and optional declared resources. Check exit zero proves only that
configured check passed. A later check changing an earlier check's sampled
scope invalidates the action. Put generated caches outside source scopes.

The initial input schema supports a closed flat object with string, boolean,
integer and number properties, required fields, enums, string length bounds
and numeric bounds. Unsupported nested schemas/keywords fail closed.
Inputs are checked before effects; model-supplied contracts are rejected as
undeclared fields. Script, HTTP GET/POST, native verification and native source-edit handlers are
supported. Other builtin handlers still require a semantic adapter. Contract names use ASCII
letters/digits/underscore/hyphen, with at most 128 bytes per identifier.

Script execution uses the pinned host root, no stdin, and invocation-specific
`PLUGIN_ARGS`, `PLUGIN_CONTEXT` and declared `PLUGIN_SECRETS`. HTTP uses the fixed
author URL and method, without redirects or the context/credential envelopes.
Handler results are bounded to 1 MiB. Handler failures return static categories,
without echoing provider bodies or script diagnostics into errors. Independent
check output remains visible in receipts, capped at 64 KiB per stream; trusted
verifiers should avoid printing credentials.

## Native verification adapter

Use `"handler":{"type":"verification"}` with `effect:"verification"` to run an
operation entirely through its configured postcondition checks. The native
adapter has no path, URL, interpreter or command parameters. Unknown handler
fields, missing contracts, other effect classes and use as compensation are
rejected. It requires no Python/helper script.

The same checks, cancellation, timeouts, resource sampling, task ownership and
single-use call IDs apply. The model chooses the semantic action and declared
inputs; it cannot supply a shell command or override verifier arguments. The
acknowledgement under `result` only records the request. The runtime receipt and
condition output determine success. Failed reruns revoke prior action evidence.
Verification programs may write generated artifacts/caches outside declared
logical source scopes. The author remains responsible for choosing appropriate
programs and capturing every relevant build input.

## Native source-edit adapter

Use `"handler":{"type":"source_edit"}` with `workspace_write`,
`non_idempotent`, and exactly `path`, `expected_sha256`, `content` string inputs.
It shares the durable patch journal and publishes a normal action receipt only
after commit. Failed checks, cancellation and a dropped future trigger native
rollback; conflicting external edits remain preserved. Generic compensation is
not permitted. Trusted checks may use an exact `{source_path}` argument for the
validated absolute file path. See the
[source-edit contract and limits](DECISION_IR.md#native-source-edits).

## Receipts and guards

```text
[action_guards]
done = [filesystem/modify_source]
_complete = [filesystem/modify_source]
```

These requirements are ANDed with `[guards]` named checks. The destination must
be a declared state or `_complete`; require at most 16 unique capability keys.
Missing receipts block transitions and report the required capability to the
model. Existing state, context-update, Decision and completion paths apply this
policy. A workflow with checks or action guards and an owned active task is
required to execute a contracted plugin. The legacy plugin execution API rejects
contracted tools; ambiguous names and builtin/VM tool shadowing are rejected.

Returned `receipt` fields include action/task/call IDs, contract SHA-256, effect,
idempotency, attempted, outcome, failure, verified, task/shared workspace revisions,
condition receipts and compensation status. The handler's untrusted output is
separate under `result`; a handler printing `verified:true` cannot publish proof.

| Outcome | Guard authority |
| --- | --- |
| `committed` | Verified postconditions and current evidence may authorize a guard. |
| `precondition_failed` | No handler attempt, no verified action evidence. |
| `failed` | Attempted action failed; no configured compensation. |
| `compensated` | Failed action cleanup independently checked; original action remains unverified. |
| `compensation_failed` | Cleanup failed or its effects remain uncertain; incomplete. |
| `rolled_back` | Native source edit restored; original action remains unverified. |
| `rollback_conflict` | Native restoration conflicts with an external edit; incomplete. |

The live authority ledger belongs to the task, outside editable model/context
data. Contract definitions and the canonical physical root are pinned. Call IDs
are single-use (at most 4096 per task); no automatic retry occurs. Success is
sampled again at guard evaluation, including postcondition resources, task
revision and shared durable workspace revision. Old tasks, copied receipts and
archived `verified:true` cannot authorize a new transition.

Mutating contracts invalidate evidence for all owned tasks on the same root
before preconditions, then advance the durable workspace revision before the
handler. Read-only/verification contracts preserve unrelated evidence. Legacy
and unknown capabilities still conservatively invalidate the requesting task.
Operations share the existing file-operation/advisory workspace locks and patch
recovery gate. `/stop` cancels the action but compensation gets a fresh token and
its own timeout. On Unix, process groups are killed before compensation; other
platforms cannot guarantee descendant cleanup.

## Limits and next work

Effect declarations, adapter/check programs and installed manifests are trusted
configuration. This is an execution protocol, not a sandbox or formal proof.
Checks run on the host and do not verify a different VM filesystem. Authors must
declare all relevant resources; sampling does not freeze files or detect every
transient/external write. Filesystem sampling latency is not bounded by process
timeouts. See existing [freshness limits](EXECUTION_CONTRACTS.md).

Generic compensation is not an atomic transaction and does not provide durable
crash recovery or cleanup after a dropped execution future. An interrupted task
cannot produce passing proof. The native `apply_patch` journal remains the
durable host-file mechanism, also reused by native source edits. External timeout/cancellation/handler errors may
finish remotely after cleanup; those remain `compensation_failed`, even if local
cleanup checks pass. Remote reconciliation needs a future domain adapter.

The initial [Praxis Decision IR](DECISION_IR.md) lowers compact instructions
to the same verified capability execution. Next: richer verified facts,
additional adapters and gradual migration of raw terminal workflows.
The source/build/test example is a first migration; other workflows and capabilities
still need their own adapters.

Validation: `cargo test --locked --lib capability_`. Include ignored tests with a
real `POML_CLI` to exercise full agent execution. Related named-check, patch,
recovery, shared revision and Decision tests remain separate regressions.

For a real, dependency-free Rust project smoke test, run
`cargo test --locked --lib capability_native_bundled_build_and_tests_with_real_cargo -- --ignored`.
It checks passing build/tests, a build that passes while a regression test fails,
and recovery after fixing the source. A Rust toolchain must be on PATH.
