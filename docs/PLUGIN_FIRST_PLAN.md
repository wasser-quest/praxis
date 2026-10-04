# Plugin-first Praxis: architecture overview

The detailed [app-wide migration roadmap](../plan/PLUGINIZATION.md) and
[delivery checklist](../plan/README.md) now live in `plan/`. They map the current
modules and all 73 default tools to target owners, starting with VM support,
then the dashboard, then the remaining feature packages.

The [first implementation slice](PLUGIN_RUNTIME.md) now shares ownership and
execution across ingress paths. Current manifests install tools with existing handler
types and contracts; they do not yet install service, route, UI, provider or
workflow packages. The new manifest and host API examples in the roadmap are
designs, not supported installer syntax.

## Boundaries

| Trusted runtime | Installed feature packages |
| --- | --- |
| Authentication, configuration, sessions and plugin management | Dashboard UI, optional clients and onboarding |
| Registry, dependency validation and lifecycle | VM, shell, media, channels and integrations |
| POML/context runtime, state-machine/Decision IR interpretation and guards | Additional workflow/template/persona packs and routing profiles |
| Workspace permissions, journals and receipt verification | Language-specific capabilities and verifier definitions |
| Budgets, cancellation, compaction, events and scoped storage | Providers, memory, RAG, learning, cron and skills |
| Small builtin `file_ops` set | Runtime-control wrappers and other model-facing tools |

The dashboard, including its shell, Graphs, Messages and chat UI, becomes an
optional plugin. Reusable administration APIs, graph parsing and event delivery
remain runtime services. The intended builtin tools are `inspect_file`,
`write_file` and `apply_patch`; the roadmap explicitly accounts for the
contract/schema changes needed by today's raw `write_file`.

A plugin can declare a contract and provide verifier evidence. The runtime
determines whether its task/resource-bound receipt is valid and current.
Contracts do not sandbox arbitrary scripts. A confirmed learning-progress write
proves persistence, not linguistic mastery.

## Starting order

1. Introduce one owner catalog, dispatcher and small host/service interface.
2. Extract VM tools/service, then package its lifecycle, routes and noVNC assets.
3. Extract the whole dashboard without making other features depend on it.
4. Move remaining tools, providers, channels and workflow/prompt assets.
5. Ship minimal and standard presets; remove obsolete feature dependencies.

Retain current manifests, identifiers, receipts, disabled flags and
operator-edited assets through explicit migration adapters. Preserve
`0.0.0.0:1337` for the installed dashboard and the separate gateway port.
Missing dependencies fail before inference; package installation/enabling and
workspace selection remain operator actions.

POML rendering, contexts and state machines remain available without feature
plugins. Node and `POML_CLI` are still required for full POML syntax. Local
`templates/` and `contexts/` work without an asset package; a workflow declaring
an unavailable plugin capability fails setup. Core events/template sync and
independent retention now support the headless path. Management starts without
an inference provider, clients initialize lazily and tasks pin registry
declarations. Service-backed feature handles and native invocation API v1 are
implemented. The first feature extraction is the optional native VM crate;
independent IPC hosting and VM UI packaging are next. See
[VM migration](VM_PLUGIN.md), [compatibility installation](INSTALLATION_PRESETS.md)
and [plugin-free workflows](CORE_WORKFLOWS.md) for the behavior and commands.

Existing [verified coding roles](VERIFIED_CODING_ROLES.md),
[Decision IR](DECISION_IR.md), and [language-learning flow](LANGUAGE_LEARNING_FLOW.md)
remain the behavior fixtures for this migration. The full roadmap provides
per-PR acceptance criteria and a minimal/VM/dashboard/provider/workflow matrix.
