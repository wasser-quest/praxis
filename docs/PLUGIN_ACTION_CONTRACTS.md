# Verified plugin capabilities

Plugins can opt into the same execution-evidence policy as named workflow checks.
An optional tool `contract` declares its effect, input schema, independent
preconditions/postconditions, timeout, idempotency and compensation. Existing
plugins without contracts keep their existing behavior.

## First use

From the Praxis repository root:

```bash
cp -R examples/plugins/verified-rust plugins/verified-rust
```

Use your configured `PLUGINS_DIR` if it differs from `plugins`. Restart Praxis
to load the plugin, then select `verified-capabilities` as the `.sm` workflow.
Run Praxis with `ROOT_DIR` pointing to the Rust project being verified. Adapt
the author-owned commands and resource scopes to that project.

The model calls `run_workspace_tests({"scope":"workspace"})`. The script only
acknowledges the request; Praxis independently executes
`cargo test --locked --workspace` once as the postcondition. A zero exit status
and fresh source snapshots produce the runtime receipt. The example workflow
requires both the named `build` check and `verified_rust/run_workspace_tests`
before entering `done` or completing. It permits scoped `apply_patch` edits and
provides the first semantic alternative to raw terminal orchestration.
The example is opt in, and does not install or run itself.

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
durable transactional host-file edits with expected hashes and crash recovery.
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
undeclared fields. Script and HTTP GET/POST handlers are supported. Builtin
handlers require a future semantic adapter. Contract names use ASCII
letters/digits/underscore/hyphen, with at most 128 bytes per identifier.

Script execution uses the pinned host root, no stdin, and invocation-specific
`PLUGIN_ARGS`, `PLUGIN_CONTEXT` and declared `PLUGIN_SECRETS`. HTTP uses the fixed
author URL and method, without redirects or the context/credential envelopes.
Handler results are bounded to 1 MiB. Handler failures return static categories,
without echoing provider bodies or script diagnostics into errors. Independent
check output remains visible in receipts, capped at 64 KiB per stream; trusted
verifiers should avoid printing credentials.

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
durable host-file mechanism. External timeout/cancellation/handler errors may
finish remotely after cleanup; those remain `compensation_failed`, even if local
cleanup checks pass. Remote reconciliation needs a future domain adapter.

Next: richer verified facts, additional semantic adapters, gradual migration of
raw terminal workflows, then the compact Praxis Decision IR. The initial
workspace-tests example is one capability, not a completed migration.

Validation: `cargo test --locked --lib capability_`. Include ignored tests with a
real `POML_CLI` to exercise full agent execution. Related named-check, patch,
recovery, shared revision and Decision tests remain separate regressions.
