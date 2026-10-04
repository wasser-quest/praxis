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

- [x] PR 1: canonical registry and shared dispatch/host API.
- [ ] PR 2: headless VM adapter extraction, preserving current behavior.
- [ ] PR 3: independently packaged VM service, routes, UI contribution and lifecycle.
- [ ] PR 4: optional dashboard with feature-independent runtime APIs/events.
- [ ] PR 5: runtime-control tools and other tool packages.
- [ ] PR 6: providers, channels, media and workflow/prompt packages.
- [ ] PR 7: minimal distribution, compatibility preset and dependency cleanup.

These are ordered reviewable milestones, not time estimates. Each milestone
includes migration and acceptance checks in the full roadmap. PR 1's native
foundation is implemented and verified; VM is the first feature to move.

## PR 1 progress

- [x] Shared owner catalog for discovery, preflight and execution.
- [x] Atomic rejection of duplicate package/tool owners on activation.
- [x] Shared chat/agent dispatcher, including lowered Decision IR and WebSocket tasks.
- [x] Live enable/cancellation checks and authenticated cron/background ownership.
- [x] Native background-service API v1: owner/version checks, drain and bounded stop.
- [x] Task-owned registry snapshots and revision evidence in action receipts.
- [x] Service-backed feature handles and versioned invocation/context API.
- [x] Runtime events, template synchronization and independent housekeeping services.
- [x] Headless fixture-service lifecycle and plugin-free POML/context/SM coverage.
- [x] Provider-independent management startup and lazy inference clients.

The native worker API and feature invocation API v1 now support service-backed
tool calls without UI or providers. Host-issued scope includes persistent storage,
task identity, workspace, credentials, cancellation and deadlines; the existing
core guard APIs remain the authority for later runtime-control wrappers. See
[the native service guide](../docs/NATIVE_FEATURE_SERVICES.md). The proposed v2
IPC packages are later work. Registry pins cover resolved declarations and native
binding generations, not immutable package/script bytes; atomic package
publication is later work.

See [the current execution boundary](../docs/PLUGIN_RUNTIME.md) for implemented
behavior and compatibility limits. PR 1's full compatibility checks passed:
960 library tests with real POML, 15 CLI tests and seven selected integration
cases, including real Cargo. No live model inference was used. VM is next;
installable feature packages and the minimal distribution remain later work.

Existing setup guides remain applicable during migration:

- [Reproducible Rust/Snake setup and test prompt](../docs/SNAKE_IR_TEST.md).
- [Verified capabilities in normal coding states](../docs/VERIFIED_CODING_ROLES.md).
- [Language-learning flow](../docs/LANGUAGE_LEARNING_FLOW.md).
- [Current plugin action contracts](../docs/PLUGIN_ACTION_CONTRACTS.md).
