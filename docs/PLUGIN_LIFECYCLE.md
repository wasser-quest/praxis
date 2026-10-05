# Plugin lifecycle, trust and the kernel

Status: agreed architecture; Phase A (lifecycle hooks) is the first
implementation step. This document records the decision so later extraction PRs
follow one model.

## Goal

Praxis should be a small **kernel** that loads **plugins from files**. Every
user-facing capability — tools, providers, channels, dashboard, TUI, memory,
RAG, cron, skills, workflow assets, and even the POML/state-machine/guard
engines — is an installable package selected by a lockfile and an
`--install-default` preset. Installing and removing a package may run
operator-approved lifecycle scripts.

## The one constraint that stays

A package may define *policy*; the kernel owns *evidence*. A guard such as
"tests passed" or "the file hash changed" is only meaningful if something the
kernel trusts observed the process exit code, timeout and resource bytes.
Otherwise every plugin can certify its own claims and "verified" becomes text.

Therefore the kernel keeps:

| Kernel responsibility | Why it cannot be a plugin |
| --- | --- |
| Bootstrap, config, DB/migrations coordinator | entry point |
| Discovery, manifest/lockfile, staged install, hook runner | it decides what runs |
| IPC + Host API + scoped credentials, task/session identity | it issues authority |
| Trust root: signing/approval, capability grants, trust levels | it decides who may do what |
| Evidence observer, receipt signer, workspace authority | it attests reality |
| Cancellation, budgets, event bus | shared runtime |

The **default runtime engine** (POML render, SM/context interpretation, guard
policy, verifier policy) ships as an installed plugin. It is replaceable. The
kernel defines the engine interface and runs the observed side of a check; the
engine plugin decides what to check and whether the observed evidence passes.
Receipts are signed by the kernel and record which engine/verifier decided, so a
third-party engine can be installed deliberately.

## Trust levels

Every plugin requests capabilities; operator policy grants them.

| Level | May do | Examples |
| --- | --- | --- |
| `runtime` | define guard/verifier/SM/POML policy, receive kernel-observed evidence | default runtime preset |
| `authority` | act on the workspace/processes under kernel observation | `file_ops`, `verified_rust` |
| `data` | scoped per-user storage and declared secrets | memory, RAG, cron |
| `tool` | model-facing tools with schemas/contracts | shell, legacy files |
| `ui`/`channel` | routes, UI slots, delivery adapters | dashboard, `praxis-tui`, Discord |

A verifier plugin can be swapped, but the kernel stamps `verified_by: <id>@<version>`
and the grant that allowed it. Downstream guards may require a specific verifier.

## Lifecycle

Install, enable and start are distinct operations. `praxis plugin install` never
starts a service.

```text
stage -> validate -> hooks.install -> publish -> register (disabled) -> enable -> start
                             \---- on failure: drop stage, keep previous revision
```

* `hooks.install` installs dependencies (OS packages, pip/cargo tooling, venvs).
  It is idempotent and runs from the package directory with
  `PRAXIS_PLUGIN_DIR`, `PRAXIS_DATA_DIR`, `PRAXIS_PLUGINS_DIR`, `PATH` and public
  process settings only.
* `hooks.uninstall` removes what install created. Data under `DATA_DIR` is
  preserved unless the operator passes `--purge`.
* Hooks are operator-trusted code, not sandboxed. Safety comes from policy,
  staging/rollback, bounded execution and hashed scripts.

### Hook policy

```toml
# config (or PLUGIN_HOOKS=allow|ask|deny)
[plugins]
hooks = "ask"   # allow | ask | deny
```

* `allow` — run hooks unattended.
* `ask` — prompt on an interactive terminal; non-interactive installs refuse
  unless `--yes`/`--allow-scripts` is given.
* `deny` — register without running hooks and print a warning.

CLI overrides: `--allow-scripts`, `--no-scripts`, `--dry-run`, `--yes`, and for
uninstall `--force`, `--purge`.

Manifest and hook script bytes are SHA-256 hashed into the install record and
registry revision. A changed uninstall script or mismatched hook hash requires
`--force`, so a swapped script cannot silently run.

## Presets and `--install-default`

A preset is a signed list of plugin ids, versions, sources, enabled flags,
secret prompts and the hook policy. `praxis install --install-default` installs
the standard set (runtime engine, `file_ops`, `runtime_control`, providers,
memory, RAG, cron, skills, shell, vision, dashboard, `praxis-tui`, and opt-in VM
and legacy assets), runs each package's hook under the configured policy, and
writes `praxis.lock.json`. A minimal preset is the kernel plus one runtime engine
and `file_ops`.

`praxis.lock.json` pins id/version/source/manifest hash/script hashes/enabled
state. Startup loads exactly the lock; upgrades stage a new revision and keep
the previous for rollback.

## Frontends

`praxis-tui` and the dashboard are ordinary frontend plugins. They talk to the
kernel over Host API v1 (loopback, per-launch token, declared scopes) plus SSE
events, or a binary IPC channel for low-latency streaming. The kernel never
links a frontend. Gaps to close for the TUI: interactive tool/question approval,
raw token streaming, and selection copy.

## Phased delivery

1. **A — lifecycle:** manifest hooks, `allow|ask|deny`, staged install/uninstall,
   script hashing, install record, example plugins, docs/tests.
2. **B — manifest v2 + lockfile:** `requires`/`provides`, ownership across
   tools/routes/UI/assets, immutable revision, rollback.
3. **C — kernel interfaces:** trust levels/capability grants, evidence observer
   and signed receipts, engine interface (POML/SM/guard verifier).
4. **D — extract packages:** default runtime, `file_ops`, `runtime_control`,
   providers, memory, RAG, cron, skills, channels, dashboard, `praxis-tui`, VM,
   workflow assets.
5. **E — distributions:** `--install-default` and minimal presets; remove the
   remaining compiled-in features and dependencies.

POML/context/SM/IR, workspace authority and receipt verification keep working
throughout; they move behind the engine interface in step 3, not before.
