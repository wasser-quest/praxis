# Praxis pluginization plan

This folder contains the implementation roadmap for turning Praxis into a small
runtime with optional feature plugins. It describes planned work, not features
already available in the installer.

Start with [the full app roadmap](PLUGINIZATION.md). The order is:

1. Add the shared registry, dispatcher and host interfaces needed by VM plugins.
2. Extract VM support, including its service, tools, routes and noVNC assets.
3. Extract the whole dashboard UI and its listener into an optional plugin.
4. Move the remaining tools, providers, channels and workflow assets into packages.
5. Ship a minimal runtime and a compatibility preset with independent packages.

The intended builtin tool set is `inspect_file`, `write_file` and `apply_patch`
under `file_ops`. State-machine execution, workspace authority, permissions and
receipt verification remain trusted runtime services. Their model-facing tools
can be supplied by a `runtime_control` plugin.

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
change should be PR 1; VM is the first feature to move.

Existing setup guides remain applicable during migration:

- [Reproducible Rust/Snake setup and test prompt](../docs/SNAKE_IR_TEST.md).
- [Verified capabilities in normal coding states](../docs/VERIFIED_CODING_ROLES.md).
- [Language-learning flow](../docs/LANGUAGE_LEARNING_FLOW.md).
- [Current plugin action contracts](../docs/PLUGIN_ACTION_CONTRACTS.md).
