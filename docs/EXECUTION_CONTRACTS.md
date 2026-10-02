# Execution contracts

Praxis can now require **runtime-produced evidence** before entering a workflow
state or completing a task. This is opt-in; existing workflows have no guards.
Select `verified-coding` as the workflow to try the bundled example when running
Praxis from its Rust project root. Adapt commands and `cwd` for your workspace.
For scoped file edits with expected hashes and automatic compensation, see
[transactional patches](TRANSACTIONAL_PATCHES.md).

```text
[checks]
build = {"program":"cargo","args":["check","--locked"],"cwd":".","timeout_secs":300,"resources":["src","Cargo.toml","Cargo.lock"]}
tests = {"program":"cargo","args":["test","--locked","--lib"],"cwd":".","timeout_secs":300,"resources":["src","Cargo.toml","Cargo.lock"]}

[guards]
done = [build, tests]
_complete = [build, tests]
```

The destination `done` must be a declared state. `_complete` is reserved for
completion; it covers `agent_complete`, `settings.done` updates, agent completion
signals, completion tags, and finishing a direct agent run with plain text.
Declaring a state guard alone does **not** implicitly guard task completion.

The model calls `run_check({"name":"build"})`. Praxis chooses the executable,
arguments, working directory and timeout from the workflow. There is no implicit
shell or interpolation. Include `run_check` in the state's tool allowance, or
allow discovery. Only `name` is accepted, plus the normal gateway `_output` view.
Check programs are trusted workflow-author configuration and run on the **host**,
even when raw terminal tools are redirected to a VM. Do not use host checks as
evidence about a different VM filesystem.

`cwd` is relative to `ROOT_DIR`, defaults to `.`, and must resolve to a directory
inside that root (including symlink checks). Timeouts are 1–300 seconds. Standard
output and error are drained with the existing bounded terminal capture. On
timeout/cancellation, no passing receipt is produced; on Unix the verifier's
process group is killed as well as the child. On other platforms descendant
cleanup is not guaranteed.

The returned receipt includes task/call/check IDs, a workspace revision, outcome,
exit status and `verified`. A receipt is verified only when the configured process
exits with status zero, without cancellation, in the current revision, and any
declared resources remain unchanged. It proves **that check passed**, not that the implementation meets every requirement.
The normal tool-output archive retains the returned receipt and captured output.

## Declared resources

Optional `resources` lists files or directories relative to the pinned `ROOT_DIR`,
independently of the check's `cwd`. The runtime samples them before and after the
check, before receipt publication, and whenever a guard needs that evidence.
An external edit or another task's write therefore blocks a guard without needing
an observed tool revision change. Rerun the affected check to restore evidence.
A process that changes its own declared resources returns `resources_changed`,
even if its exit status is zero. An unreadable or unsupported scope fails closed;
if the initial sample fails, the check is not started.

Scoped receipts include `resources: {sha256, paths, entries, bytes}`. The digest
binds the canonical workspace location, normalized paths, contents, file types,
directory membership (including empty directories), missing declared paths, and
permissions. On Unix permissions include the mode bits; other platforms bind the
read-only flag. Contents are hashed, not included in the receipt. There are no
implicit exclusions or dependency discovery: authors must declare every relevant
input. Keep generated build output outside source scopes when checks write it.
`resources:["."]` includes the entire root and often exceeds these bounds.

Each check permits up to 16 unique normalized paths, 4096 inventory entries,
16 MiB of file bytes, and 64 directory levels below a scope root. Absolute paths,
`..`, aliases such as `./src`, backslashes, symlinks, special files and non-UTF-8
names are rejected. Missing paths are supported so later creation invalidates
evidence. Omitted or empty `resources` preserves revision-only behavior and omits
the resource object from receipts. The bundled example scopes Rust source and
manifests; extend it for any additional build inputs in your project.

## Authority and freshness

- Live receipts live in the owned task, outside editable context JSON. A model
  writing `tests.verified=true`, copying a receipt, or printing passing output
  cannot satisfy a guard. Old task receipts cannot be replayed.
- Check definitions and guards are pinned when a guarded workflow is selected.
  Changing workflow or editing its contract definitions during the task fails
  closed. Start a new task to adopt author changes.
- Each potentially mutating tool invalidates all evidence **before execution**,
  even if it fails. Unknown/plugin tools are conservative. Read/discovery/context
  and control tools preserve evidence; model context updates still cannot clear
  the guarded state, change workflow, or bypass completion requirements.
- Rerunning a check revokes its prior receipt first. Failed, missing, timed-out,
  cancelled and stale results cannot authorize a transition.
- Force transitions, `set_context` state selection, auto routing and Decision
  routing enforce the destination guard. `_complete` is also checked again after
  a tool batch, so completion followed by a write cannot reuse earlier evidence.
- The agent receives missing-check feedback and can recover within its existing
  turn/tool budgets. A one-turn chat reports missing verification as incomplete.

This is a runtime protocol, not a sandbox or formal proof. Trusted check programs,
plugins, workflow files and the runtime remain in the trusted computing base.
Raw shell/file tools can affect resources outside this protocol; external writers
and already-running background jobs can change files without advancing the
ledger. Declared scopes detect differences at the sampling boundaries, but do
not freeze files. Transient writes restored between samples and writes after the
last sample can escape detection; metadata stability checks cannot provide an
atomic multi-file snapshot. Resources outside declared scopes are not covered.
Use controlled foreground capabilities and an isolated workspace for guarded
work. Inventory size is bounded, but filesystem I/O latency is not governed by
the check-process timeout. Archived `verified:true` records a past successful
sample; guards always evaluate current evidence again.

## Next implementation slices

1. Durable recovery of interrupted file transactions and stronger filesystem
   isolation for concurrent writers.
2. Plugin effect metadata and typed preconditions/postconditions, with conservative
   defaults and explicit compensation semantics for non-transactional APIs.

Run `cargo test --locked --lib action_contract` for the offline contract tests.
The handler regression `contract_agent_rejects_unverified_completion_then_recovers`
also exercises a scripted model with the real POML renderer (requires POML_CLI).

Resource regressions: `cargo test --locked --lib resource_`. The ignored
`resource_contract_agent_rejects_external_mutation_then_rechecks` test uses the
real POML renderer and an offline external-writer fixture.
