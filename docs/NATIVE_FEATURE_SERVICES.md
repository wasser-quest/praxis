# Native feature services: invocation API v1

This is the in-process boundary for extracting VM and other native features.
It does not by itself make a Rust adapter independently installable. The VM now
uses this host seam for an optional [installed process worker](PLUGIN_PROCESS_PROTOCOL.md).
Script and HTTP plugins still work as before; generic process manifests and
immutable installation remain later milestones in the [pluginization plan](../plan/PLUGINIZATION.md).

## Declaration and binding

A tool owned by plugin `fixture` can declare:

```json
{
  "name": "feature_echo",
  "description": "Invoke the fixture service",
  "parameters": {
    "type": "object",
    "properties": { "message": { "type": "string" } },
    "required": ["message"],
    "additionalProperties": false
  },
  "handler": {
    "type": "service",
    "service": "backend",
    "operation": "echo",
    "api_version": 1,
    "timeout_secs": 30
  }
}
```

The host explicitly binds a trusted implementation after loading the owner:

```rust
use praxis::runtime::features::{InvocationContext, NativeService, INVOCATION_API_VERSION};
use serde_json::{json, Value};
use std::sync::Arc;

struct Echo;

#[async_trait::async_trait]
impl NativeService for Echo {
    async fn invoke(
        &self,
        context: InvocationContext,
        operation: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(operation == "echo", "Unsupported operation");
        Ok(json!({"message": args["message"], "user": context.user()}))
    }
}

// In native host setup, with an already-loaded `fixture` plugin:
// registry.register_service("fixture", "backend", INVOCATION_API_VERSION,
//     &["echo"], Arc::new(Echo))?;
```

Registration rejects unknown owners, duplicate bindings, incompatible versions
and missing declared operations without invoking the implementation. Supported
operations are host declarations, never model-selected command strings. A JSON
manifest alone cannot create a native service: the runtime must bind its adapter.
Service IDs and operation IDs accept ASCII letters, digits, `.`, `_` and `-`, up
to 128 bytes. Unknown handler fields and timeouts outside 1–300 seconds fail
activation. API v1 here is separate from the proposed v2 IPC package format.

Unbound or disabled services are absent from discovery. A workflow explicitly
selecting their tools or IR targets fails setup before inference, including in
future graph states. Merely loading an unused optional declaration does not
block a core workflow. Correct the host binding and start a new task; the model
cannot install or guess around a missing service.

## Host-issued invocation context

Chat, agent and lowered Decision IR use the same host-aware dispatcher. It
supplies the authenticated user/session, task ID, actual call ID, plugin owner,
registry revision, active state, pinned workspace, deadline and cancellation.
The workspace comes from runtime configuration, including on legacy workflows;
model operands and `settings.path` cannot retarget it. Changing the root during a
task requires a new task. Receipt task IDs exist from task admission and stay
identical when a verified workflow is subsequently bound.

The context has no public constructor, deserialization, raw database connection,
mutable settings or global secret store. `secret(name)` exposes only credential
keys declared by the owning plugin. Navigation and context changes continue
through the core guard-aware APIs; this adapter does not add a bypass. Moving
their model-facing wrappers into `runtime_control` remains PR 5.

`cancellation()` returns an invocation child token; cancelling it cannot stop the
parent task. Task stop, forced service stop and the deadline cancel or reject the
invocation. Storage checks also reject changed sessions, restarted tasks and
expired contexts. Retaining a context clone does not retain execution authority
after the callback finishes. Native adapters are operator-trusted Rust code:
these interfaces are a migration boundary, not a process sandbox.

## Scoped storage

`storage_get`, `storage_put` and `storage_compare_exchange` use persistent SQLite
storage under `(plugin owner, authenticated user, key)`. Two services of the same
owner can share state; different owners/users cannot access one another through
this API. Sessions of the same user intentionally share feature state, useful for
VM ownership across chats. Keys are bounded identifiers and each JSON value is
limited to 64 KiB. Compare/exchange is atomic in SQLite and reports a conflict
instead of overwriting a concurrent value.

Read-only and verification contracts cannot write this storage. Mutating
contracts and legacy uncontracted calls can. Stored values survive service disable
and database reopen; uninstall does not implicitly delete them. Migration 016
adds a generic core table, without a feature-specific schema.

## Contracts, receipts and lifecycle

A service tool may use the existing [action contract](PLUGIN_ACTION_CONTRACTS.md).
The core validates input, runs preconditions, rechecks live availability before
the callback, executes it and verifies postconditions. Only the core ledger can
publish the resulting receipt. The service's result is always nested under
`result`; printing a receipt does not satisfy a transition guard. Legacy calls
remain unverified and conservatively invalidate prior evidence.

Native input/result objects are bounded to 1 MiB of serialized JSON. Legacy
calls retain the existing required-field validation; contracted calls use the
strict contract schema. Native implementation error details are replaced with
controlled failure messages. Deadline/cancellation failures remain failures in
contract receipts. The invocation budget starts at host admission and uses the
smaller of the handler and contract budgets, including waiting for the shared
contract lock. Host postcondition/compensation checks keep their contract budgets.

Generic script/HTTP compensation remains available for a mutating service
contract. A service handler cannot itself be a compensation handler in v1.
Features must explicitly define their recovery; this change does not invent VM
rollback or turn guest evidence into verified host-file evidence.

`service_handle(owner, id).disable(grace)` blocks new calls immediately, drains
current calls, then cancels them if grace expires. Adapter `shutdown()` runs once
under the remaining deadline. A `false` result means forced stop or incomplete
cleanup, and does not prove that external processes stopped. Adapters must own
and safely clean up their QEMU/process/file resources, including on future drop.
Gateway shutdown closes all feature handles before draining them under one
feature deadline. Registry snapshots share the live handles, so an old snapshot
cannot revive a disabled service. A fresh binding has a fresh generation in the
registry revision; it requires a new task even with an unchanged manifest.

Registration validates declarations without requiring an already running worker.
Host startup calls `initialize()` before inference; `available()` controls live
discovery/preflight. `force_stop()` signals owned resources synchronously when
the shutdown budget expires. Initialization is one-shot per binding; a failed
initialization or dead process requires a new binding, without lazy restart.

## Verification

`runtime::feature_tests` covers registration, discovery/preflight, both ingress
modes and IR lowering, host identity/workspace/credential scope, task/session
changes, durable owner/user storage and compare/exchange conflicts, cancellation,
deadlines including a queued contract, receipt guards, declared compensation,
output bounds, error redaction/display, live disable and drain/forced shutdown.
These 20 tests use fixture
services and local commands, without QEMU, a dashboard or model inference.

```bash
cargo test --lib native_feature_
```

The existing plugin-free POML/context/SM and real Cargo IR regressions remain the
compatibility checks before the first VM extraction.
