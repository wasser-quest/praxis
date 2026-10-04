# Praxis pluginization plan

This folder contains the implementation roadmap for turning Praxis into a small
runtime with optional feature plugins. Feature packaging is planned; the PR 1
progress section records the implemented execution boundary.

Start with [the full app roadmap](PLUGINIZATION.md). The order is:

1. Add the shared registry, dispatcher and host interfaces needed by VM plugins.
2. Extract VM support, including its service, tools, routes and noVNC assets.
3. Extract the whole dashboard UI and its listener into an optional plugin.
4. Move the remaining tools, providers, channels and workflow assets into packages.
5. Ship a minimal runtime and a compatibility preset with independent packages.

The intended builtin tool set is `inspect_file`, `write_file` and `apply_patch`
under `file_ops`. POML rendering, context handling, state-machine execution,
workspace authority, permissions and receipt verification remain trusted runtime
services. Feature plugins and the dashboard are not required for local POML/SM
workflows. Model-facing runtime tools can be supplied by `runtime_control`;
optional workflow packs add assets rather than replacing the core interpreter.

## Delivery checklist

- [ ] PR 1: canonical registry and shared dispatch/host API.
- [ ] PR 2: headless VM adapter extraction, preserving current behavior.
- [ ] PR 3: independently packaged VM service, routes, UI contribution and lifecycle.
- [ ] PR 4: optional dashboard with feature-independent runtime APIs/events.
- [ ] PR 5: runtime-control tools and other tool packages.
- [ ] PR 6: providers, channels, media and workflow/prompt packages.
- [ ] PR 7: minimal distribution, compatibility preset and dependency cleanup.

These are ordered reviewable milestones, not time estimates. Each milestone
includes migration and acceptance checks in the full roadmap. The first code
changes implement part of PR 1; VM is the first feature to move.

## PR 1 progress

- [x] Shared owner catalog for discovery, preflight and execution.
- [x] Atomic rejection of duplicate package/tool owners on activation.
- [x] Shared chat/agent dispatcher, including lowered Decision IR and WebSocket tasks.
- [x] Live enable/cancellation checks and authenticated cron/background ownership.
- [x] Native background-service API v1: owner/version checks, drain and bounded stop.
- [x] Task-owned registry snapshots and revision evidence in action receipts.
- [ ] Service-backed feature handles and versioned invocation/context API.
- [x] Runtime events, template synchronization and independent housekeeping services.
- [x] Headless fixture-service lifecycle and plugin-free POML/context/SM coverage.
- [x] Provider-independent management startup and lazy inference clients.

The native worker API does not yet host service-backed tool calls or the proposed
v2 IPC packages. Service-backed feature handles and the invocation/context API
remain before the first VM extraction. Registry pins cover resolved declarations,
not immutable package/script bytes; atomic package publication is later work.

See [the current execution boundary](../docs/PLUGIN_RUNTIME.md) for implemented
behavior and compatibility limits. Milestone PR 1 remains incomplete until its
remaining items pass.

Existing setup guides remain applicable during migration:

- [Reproducible Rust/Snake setup and test prompt](../docs/SNAKE_IR_TEST.md).
- [Verified capabilities in normal coding states](../docs/VERIFIED_CODING_ROLES.md).
- [Language-learning flow](../docs/LANGUAGE_LEARNING_FLOW.md).
- [Current plugin action contracts](../docs/PLUGIN_ACTION_CONTRACTS.md).
