# Execution contracts: first slice

Praxis can now require **runtime-produced evidence** before entering a workflow
state or completing a task. This is opt-in; existing workflows have no guards.
Select `verified-coding` as the workflow to try the bundled example when running
Praxis from its Rust project root. Adapt commands and `cwd` for your workspace.

```text
[checks]
build = {"program":"cargo","args":["check","--locked"],"cwd":".","timeout_secs":300}
tests = {"program":"cargo","args":["test","--locked","--lib"],"cwd":".","timeout_secs":300}

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
exits with status zero, without cancellation, in the current revision. It proves
**that check passed**, not that the implementation meets every requirement.
The normal tool-output archive retains the returned receipt and captured output.

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
ledger. Use controlled foreground capabilities for guarded work. Freshness is
relative to observed tool execution, not a filesystem snapshot or content hash.

## Next implementation slices

1. Transactional `apply_patch` capability: scoped paths, expected content hashes,
   snapshot, atomic apply and restoration on failed postconditions.
2. Resource-bound receipts: bind checks to an immutable tree hash and reject
   external/concurrent edits, including background work.
3. Plugin effect metadata and typed preconditions/postconditions, with conservative
   defaults and explicit compensation semantics for non-transactional APIs.

Run `cargo test --locked --lib action_contract` for the offline contract tests.
The handler regression `contract_agent_rejects_unverified_completion_then_recovers`
also exercises a scripted model with the real POML renderer (requires POML_CLI).
