# Shared plugin execution boundary

The first implementation slice of the [pluginization roadmap](../plan/PLUGINIZATION.md)
introduces shared tool ownership and dispatch. It requires no new configuration
and retains existing v1 plugin manifests and verified capability contracts.

## Implemented behavior

`src/tools/catalog.rs` resolves each name to an explicit native adapter or one
enabled plugin. Discovery, workflow preflight and execution use this ownership
rule. Native names remain reserved when their tools are disabled. Other names,
including a `vm_` prefix, can belong to plugins; a prefix grants no VM authority.
Conflicting enabled declarations fail with `owner_conflict` before execution or
workflow inference, including workflows with no Decision IR table.

The startup loader uses `PluginRegistry::try_register`: it validates declarations
and the candidate owner catalog before activation. Duplicate package IDs or
tool owners reject that candidate and preserve the previously accepted registry.
Rejected candidates are reported at startup; workflows requiring them fail setup.
The low-level `register` builder remains compatible, but does not activate effects
or bypass preflight/execution owner checks.

`src/gateway/tool_dispatch.rs` executes tools for both chat and agent paths;
WebSocket tasks use those same paths. Decision IR lowers one instruction into
the same dispatcher, retaining the call ID and task-owned receipt/guard checks.
Advertised native tools previously missing from chat dispatch, such as cron and
background tools, now reach the common adapters.

The host constructs `DispatchContext` from the authenticated caller and
installation root. Tool arguments cannot supply that identity or the pinned
workspace. Immediately before execution, the dispatcher rechecks persisted
enable flags and task cancellation. Invalid flag files fail closed. Ingress
continues to own charging, schema/state selection and output archival; dispatch
does not add retries or charge a second time for an IR instruction.

Cron operations and background-job reads use the authenticated owner. Cron
delete/toggle include user ownership in the SQL mutation; run/status responses
cannot reveal another user's job. Supplying a different `user_id` in tool
arguments does not change the caller.

## Compatibility and remaining work

All current native tools are still present. This change does not yet reduce them
to the three planned builtin file operations or make VM/dashboard installable
packages. The shared dispatcher temporarily contains their native adapters.
Agent VM redirection and chat host-file/terminal behavior remain distinct through
an explicit dispatch mode, ready for the later execution-backend extraction.

Versioned service hosting, registry revision pinning, runtime event/template
services, provider-independent startup and feature dependency removal are still
tracked in [PR 1 and subsequent milestones](../plan/README.md). Contracts and
receipts retain their existing meanings; plugin processes remain operator-trusted.

## Verification

`plugin_dispatch_tests.rs` runs through both real ingress adapters. It covers
VM-prefixed contracted capabilities, duplicate/shadowed owners, live disable,
cancelled writes, shared cron dispatch and resource ownership. Plugin registration
has an atomic-rejection test. Existing capability-dispatch, graph, rollback,
output-archival and tool-loop suites remain the regression checks.

Validated without live model inference: 918 library tests, 15 CLI tests and two
real Cargo-based project IR tests passed. The full library run leaves 48 cases
ignored by their existing annotations; the two Cargo IR cases were then selected
and run separately. New modules pass rustfmt checks and documentation links resolve.
