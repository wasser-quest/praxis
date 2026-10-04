# Shared plugin execution boundary

The foundation of the [pluginization roadmap](../plan/PLUGINIZATION.md) provides
shared tool ownership/dispatch and core event/template services. It requires no
new configuration and retains v1 plugin manifests and capability contracts.

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

## Core workflow and background services

POML rendering, contexts and SM/Decision IR interpretation remain in core and
work with an empty feature-plugin registry. Full POML rendering requires Node
and `POML_CLI`; a workflow declaring a missing capability still fails preflight.
Local template/workflow files remain supported when optional asset packages are
introduced. See [plugin-free workflows](CORE_WORKFLOWS.md).

`runtime::events` now owns user event channels. Core DB, gateway, tool and channel
code publish there; `dashboard::stream` is a compatibility re-export of that same
bus. Wire names/payloads remain unchanged. Context notifications still expose
only the public web-speech flag, not private context/settings.

`runtime::templates` owns template resolution and catalog synchronization.
Startup calls it directly without dashboard routes. Synchronization preserves
descriptions/operator flags, leaves disk files untouched, supports nested names
and avoids importing outside-root links or traversing directory cycles. The
gateway template exports and old dashboard synchronization function remain
compatibility adapters.

The native `runtime::services::ServiceHost` API v1 checks versions, service IDs
and duplicate owners before starting periodic callbacks. It owns cancellation,
draining and bounded shutdown; dropped hosts abort their workers. Core tool-output
retention, cron checks and shell job cleanup are separate workers registered after
the gateway binds its listener. A blocked or failing feature worker no longer
holds up the core retention tick. The cron adapter preserves the existing job
check behavior; this change does not implement new scheduled inference.

## Compatibility and remaining work

All current native tools are still present. This change does not yet reduce them
to the three planned builtin file operations or make VM/dashboard installable
packages. The shared dispatcher temporarily contains their native adapters.
Agent VM redirection and chat host-file/terminal behavior remain distinct through
an explicit dispatch mode, ready for the later execution-backend extraction.

The worker API is an in-process migration adapter. Service-backed tool handles,
registry revision pinning and provider-independent management startup are still
tracked in [PR 1](../plan/README.md). Sidecar IPC, installable service manifests,
feature-process lifecycle and dependency removal remain subsequent work.
Contracts/receipts retain their meanings; plugin processes remain operator-trusted.

## Verification

`plugin_dispatch_tests.rs` runs through both real ingress adapters. It covers
VM-prefixed contracted capabilities, duplicate/shadowed owners, live disable,
cancelled writes, shared cron dispatch and resource ownership. Plugin registration
has an atomic-rejection test. Existing capability-dispatch, graph, rollback,
output-archival and tool-loop suites remain the regression checks.

`runtime::tests` covers empty-registry contexts/graphs, a real IR navigation call,
core transition delivery, real POML before/after a state change, missing-plugin
setup errors, template catalog metadata and independent service lifecycle/retention.

Validated without live model inference: 929 library tests, 15 CLI tests and two
real Cargo-based project IR tests passed. The library run requires the real POML
CLI, including the plugin-free render regression and all shipped root templates.
It leaves 48 cases ignored by existing annotations; the two Cargo IR cases were
then selected and run separately. New modules pass rustfmt checks; documentation
links resolve. Homepage source/docs links, metadata and clone commands now use
`https://github.com/wasser-quest/praxis` and were checked in the HTML.
