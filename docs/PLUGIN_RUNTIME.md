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

Native feature tools can now bind an owner-scoped `NativeService` using
invocation API v1. The shared dispatcher supplies host-issued identity, workspace,
deadline, cancellation, declared credentials and persistent scoped storage.
Contracted calls use the same core verification/receipt lifecycle. Missing
required bindings fail workflow setup; disabled bindings disappear from discovery.
See [native feature services](NATIVE_FEATURE_SERVICES.md) for the manifest,
binding API, lifecycle and limits.

## Task registry revisions

Before task routing, discovery or inference, Praxis captures an owned immutable
registry snapshot and its `praxis.registry.v1` SHA-256. Package order and JSON map
insertion order do not change the hash. It covers resolved declarations: package
identity/version, schemas, handlers, contracts, defaults, credential-grant names
and manifest enable flags, with the native API/application version as a domain.
Native service descriptors and host-issued binding generations are included;
cloned registries share a generation, while replacement bindings require a new
task. Live service enable flags do not change the revision.
Context default collisions now resolve in package-name order rather than hash-map
iteration order. Secret-store credential values are excluded.

The shared dispatcher checks that revision before lowering Decision IR or
looking up a handler. Direct task-owned plugin execution and receipt creation
also check pinned declarations. A changed registry is rejected before execution;
a new task can pin the new declarations. Action receipts carry
`registry_revision`. Snapshot teardown follows task teardown, including failure
and cancellation. Persisted tool enable flags are checked live at dispatch, so
pinning does not grant permission to use a disabled tool.

This identifies declarations, not script contents or native executable bytes.
The running gateway still loads its registry at startup; this is not hot reload
or immutable package installation. Explicit VM worker bindings now support the
[process protocol](PLUGIN_PROCESS_PROTOCOL.md).

## Management and inference readiness

Praxis starts with incomplete inference-provider setup. Host authentication,
resilience configuration, workspace validity and owner-catalog checks still
apply. `/health` reports process health. Authenticated `/api/status` adds
`inference.ready` and a non-retryable setup diagnostic; context/session APIs and
provider administration remain usable. These operations read metadata without
constructing inference clients or sending a provider request. Clients initialize
once per router generation on inference use; explicit provider login can rebuild
the router as before.

Chat, WebSocket messages and direct agent entry first plan the trusted workflow
on an in-memory context, including the next entry node of a completed graph.
They check workspace/capability setup and the planned provider plus explicit
fallbacks. An unavailable provider returns `provider_not_configured` without
clearing persisted completion, changing graph state, saving input, creating
verification tickets or registering an agent input queue. REST chat returns
HTTP 503 with the existing `success/response/error` body; WebSocket reports the
setup error and stays usable. Missing default-provider setup also skips the
WebSocket session GPU prewarm. A configured session/workflow override can run
even when the installation default is missing.

Readiness means a provider is configured/registered, not that its server or model
is reachable. Ollama and llama.cpp remain key-optional endpoint adapters.
`/login` and `/v1/providers/login` retain their explicit endpoint/login checks and
hot-swap behavior; correcting setup takes effect without restarting the gateway.

For example, with normal host credentials and local assets configured:

```bash
USE_PROVIDER=openai cargo run -- run --no-discord --no-dashboard
# No OpenAI key is needed to start the management listener.
curl http://localhost:3537/health
curl http://localhost:3537/api/status \
  -H "Authorization: Bearer $GATEWAY_API_KEY"
```

To enable inference, use `/login ollama <api_base> <model>` and select
`settings.provider=ollama` / `settings.model=<model>` in context, or log into the
selected default provider. The management test suite uses temporary local assets,
empty plugin registries and synthetic providers; it makes no paid inference calls.

## Compatibility and remaining work

Most current native tools remain. The shared boundary does not yet reduce them
to the three planned builtin file operations. VM supports an installed headless
worker with package-owned administration/VNC/assets and a contributed dashboard
page. The whole dashboard listener/UI is still mandatory; its optional extraction
is PR 4. Independent VM CLI/ownership remain. See [web contributions](PLUGIN_WEB_CONTRIBUTIONS.md).
Agent VM redirection and chat host-file/terminal behavior remain distinct through
an explicit dispatch mode, ready for the later execution-backend extraction.

The worker and feature invocation APIs are in-process migration adapters for
[PR 1](../plan/README.md). Core guard-aware navigation/context operations remain
the authority behind future model-facing runtime-control wrappers.
The VM now has versioned IPC and process lifecycle. Generic process manifests,
route/UI registration and the remaining dependency removal are subsequent work.
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

`gateway::foundation_tests` verifies stable declaration hashing, snapshot lifetime,
live disable with pinned IR dispatch, receipt revisions, rejection of changed
registries through native/IR/direct execution, lazy client inventory, authenticated
management HTTP routes, WebSocket setup recovery, preserved completed graphs,
workflow provider overrides, router swaps and invalid host settings.

`runtime::feature_tests` adds service binding/version checks, both ingress modes
and IR lowering, scoped identity/credentials/storage, task/session invalidation,
deadline/cancellation handling, receipt guards, output/error bounds and service
drain/forced shutdown. See the [invocation guide](NATIVE_FEATURE_SERVICES.md).

Validated without live model inference: 960 library tests, 15 CLI tests and seven
selected integration tests passed. Three integration cases use real Cargo for
project-root isolation and chat/agent IR execution; four use scripted models to
check retry/history retention, WebSocket stop/retry, graph restart/navigation and
IR source rollback/completion. The library run requires the real POML
CLI, including the plugin-free render regression and all shipped root templates.
It leaves 48 cases ignored by existing annotations; those seven cases were
selected and run separately. New modules pass rustfmt checks; documentation
links resolve. Homepage source/docs links, metadata and clone commands now use
`https://github.com/wasser-quest/praxis` and were checked in the HTML.
