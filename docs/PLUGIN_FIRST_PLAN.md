# Plugin-first Praxis: proposed architecture and migration

The direction is useful: a small runtime can start with no feature plugins,
then gain a chat provider, coding tools, teaching workflows or channels through
installed packages. Keep the rules that enforce trust in the runtime. A plugin
declares capabilities and checks; it cannot authorize its own receipts, change
the selected workspace or override a user permission.

This is a migration plan, not a description of a completed plugin-only startup.
The current plugin manifest installs tools; workflows and templates still live
in installation asset directories. This change adds capability preflight and
two opt-in workflows using those existing mechanisms.

## Boundaries

| Trusted runtime | Installed feature packages |
| --- | --- |
| Configuration, authentication and session identity | Chat providers and routing profiles |
| Registry, manifest validation, dependency resolution | Coding/language-specific capabilities |
| State-machine and Decision IR interpretation | Workflows, node descriptions and POML prompts |
| Permission checks and pinned project selection | Author-defined preconditions and verifiers |
| Execution, cancellation, verification, commit and rollback | Media, search, channels and integrations |
| Task-owned receipts and durable recovery/audit | Learning exercises and scoped progress adapters |
| Dashboard shell and authenticated extension APIs | Feature pages with declared routes and assets |

A verifier result establishes the contract's declared fact. It does not prove
general correctness, and language feedback remains model judgment. The runtime
can verify that a progress write committed; it cannot infer that the learner has
mastered a language from a model's claim.

## Migration stages

1. **Setup validation (this batch).** Validate effective state-local IR tables,
   declared contracted owners and required capability guards before the first
   provider request. Setup failures identify the workflow, state, target and
   operator action, and are explicitly non-retryable. Recheck the current state
   on subsequent requests so runtime tool-disable changes still apply.
2. **Feature packaging.** Introduce a versioned manifest API alongside the
   current tool manifest. Packages declare capabilities, templates, workflows,
   bundled assets and optional dashboard extensions. Register namespaced assets
   without silently replacing operator-edited installation files. Retain the
   existing asset paths as compatibility aliases during migration.
3. **Install/enable lifecycle.** Add bounded dependency/version checks and a
   dry-run install plan. Stage a package, validate checksums and declarations,
   then atomically activate its registry revision. Keep the previous revision
   for rollback. An installed package and an enabled feature are separate states.
4. **Provider and channel adapters.** Move current implementations behind
   explicit interfaces, preserving existing configuration and endpoint behavior.
   Provider plugins return usage and stream events; channel plugins receive only
   the authenticated operations they declare. Begin with built-in Rust adapters
   and external process/HTTP plugins; avoid making a dynamically loaded Rust ABI
   a requirement for third-party languages.
5. **Isolated extension host.** Declare filesystem/process/network permissions,
   credential keys, timeouts and output limits. Run external plugins under an
   enforceable host boundary. Contracts alone do not sandbox arbitrary scripts.
   Plugin code and verifier programs remain operator-trusted until that boundary
   is implemented and tested.
6. **Dashboard extensions.** Keep navigation, login, sessions, Graphs and Messages
   in the shell. Feature pages use namespaced authenticated APIs and isolated UI
   surfaces; loading an extension grants no shell-level authority or credentials.
7. **Minimal startup.** Offer an explicit minimal distribution with the runtime
   and plugin manager. With no provider package, show setup rather than attempting
   inference. Ship a default preset for existing users, preserving their behavior.
8. **Measured Decision IR optimization.** Measure tokens, task outcomes and
   recovery behavior on the same fixtures before introducing compact artifact
   references or batch plans. Preserve capability schemas, receipts and limits.

## Proposed package declaration

The following shape is a design example, **not accepted by today's installer**:

```json
{
  "manifest_version": 2,
  "id": "coding.rust",
  "version": "1.0.0",
  "requires": {"runtime_api": "2", "plugins": []},
  "provides": {
    "capabilities": ["inspect_source", "modify_source", "build", "test"],
    "workflows": ["standard-verified", "branching-coding"],
    "templates": ["verified-coding"],
    "dashboard_pages": []
  },
  "permissions": {
    "workspace": "operator_selected",
    "processes": ["cargo", "rustfmt"],
    "credential_keys": []
  }
}
```

Selection, permissions and dependency activation belong to the operator. A
model may discover enabled features and choose an allowed action. It cannot
install a package, enable a disabled tool or edit a pinned contract as a remedy
for a setup failure.

## Acceptance criteria

- Existing installations and current manifests keep working throughout the
  migration, and user-edited assets are preserved unless deliberately replaced.
- A missing dependency fails before paid inference or effects. No retry loop is
  used to repair a missing owner or disabled capability.
- Each active task pins the applicable registry/contract/workflow revision.
  Enable/disable checks remain live; authority cannot increase during a task.
- Failed activation restores the prior revision, and cancelled or interrupted
  actions preserve the durable rollback/recovery guarantees.
- Empty minimal startup, provider-only, verified Rust, language tutoring and
  media presets have offline integration coverage before migration becomes the
  default distribution.

The next implementation after this batch should be feature packaging and a
reviewable install/enable lifecycle. It should reuse the existing contracts and
graph parser rather than introduce another execution path.
