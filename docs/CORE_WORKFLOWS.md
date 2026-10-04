# POML, context and state machines without feature plugins

POML rendering, context storage/routing and the state-machine/Decision IR
interpreter remain core runtime services. They do not require the dashboard, VM
or another feature plugin. Optional workflow/template packs supply additional
assets; they do not replace the core interpreter or disable local `templates/`
and `contexts/` files.

## Requirements

| Operation | Requirement |
| --- | --- |
| Load/save context and apply an SM state | Core runtime and local workflow |
| Render POML with that context | Node and `POML_CLI`, set to the installed Microsoft JavaScript CLI |
| Navigate a graph and enforce state guards | Core graph/task runtime; model-facing navigation tools are currently native |
| Generate an answer | Configured inference provider |
| Execute a capability such as `verified_rust/modify_source` | Installed, enabled owner with the declared contract |
| View Graphs, Messages or chat in a browser | Dashboard frontend |

Rendering and local graph operations can be tested without contacting an
inference provider. The running application currently validates the selected
provider at startup; provider-independent management startup remains in
[milestone PR 1](../plan/README.md). A missing feature plugin does not prevent a
plain POML/SM workflow, but it does prevent a workflow that declares that plugin's
capabilities. That case produces a non-retryable setup error; the model cannot
replace the owner, invent a receipt or change the workspace to bypass it.

The future `runtime_control` package can expose model-facing control tools using
these host operations. Its absence does not remove the POML/context/SM engines;
it does affect which tools a model can call. Current manifests and builtin
navigation tools remain compatible during migration.

## Core services

`runtime::events` owns the per-user event channels used by CLI/API tasks and
frontends. `dashboard::stream` re-exports the same bus for compatibility; it does
not create a second registry. Event names/payloads remain compatible. Context
save notifications publish only the public speech flag, never the full context.

`runtime::templates` resolves templates and synchronizes their disk contents to
the catalog using an explicit root. Synchronization leaves disk files untouched,
retains existing descriptions/operator flags and does not delete catalog-only
entries. It ignores symlinks outside that root and detects directory cycles.
The old gateway/template and dashboard-sync entry points remain adapters.

`runtime::services::ServiceHost` owns periodic workers. Its native adapter API v1
checks versions, IDs and duplicate ownership before starting a callback. Disable
blocks new ticks and drains the current tick within a deadline; timeout or host
drop aborts the worker. Core output retention, cron checks and shell job cleanup
are separate services. Retention remains usable with cron/shell absent or blocked.
This worker adapter does not implement sidecar IPC, feature-process shutdown,
package revision pinning or installable v2 service manifests.

## Reproducible checks

From the repository root, using the already installed POML CLI:

```bash
export POML_CLI=/absolute/path/to/pomljs/dist/cli.cjs
PRAXIS_REQUIRE_POML=1 cargo test --lib runtime::tests -- --nocapture
PRAXIS_REQUIRE_POML=1 cargo test --lib shipped_templates_render_with_the_real_poml_cli -- --nocapture
```

The first suite creates temporary local workflows/templates and an empty plugin
registry. It verifies context interpolation, an actual `execute_decision`
navigation call, durable state/context changes, core transition events, back
navigation, real POML output before/after the transition, missing-capability setup
errors, template synchronization and service disable/drain/retention behavior.
The second renders every shipped root template with synthetic context data.
Neither test sends live inference requests or starts a dashboard/VM.

Without `POML_CLI`, non-renderer checks still run; renderer checks report a skip.
`PRAXIS_REQUIRE_POML=1` makes a missing CLI a failure, so CI cannot silently omit
the real-renderer regression. These checks prove the core runtime behavior;
removing compiled feature dependencies is a later distribution milestone.
