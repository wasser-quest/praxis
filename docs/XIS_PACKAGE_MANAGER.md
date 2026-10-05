# xis — a separate setup and package manager for Praxis

Status: future design. Nothing here is implemented. This records the agreed
direction so the current plugin/manifest work stays compatible with it.

## Decision

The package manager is a **separate executable/project called `xis`**, not a
Praxis module. Praxis stays the kernel that loads plugins and owns authority;
`xis` is an installer/orchestrator that fetches a whole setup, applies it, and
reports what the operator must change.

That split keeps the trust boundary intact:

| | Praxis (kernel) | xis |
| --- | --- | --- |
| Owns | registry, lifecycle hooks, receipts, guards, workspace authority | repositories, resolution, download, file placement, change report |
| Links | nothing of xis | nothing of Praxis internals |
| Talks via | CLI + documented files | the same CLI + files |

`xis` must not be able to bypass guards or grant itself a trust role. It is a
glorified, signed `curl | tar` with a plan, a diff and a lockfile.

## What a "setup" is

A setup bundles any mix of:

* **plugins** — `plugin.json` v2 packages (tools, services, routes, UI, hooks).
* **skills** — `skills/<name>/skill.json` + `skill.poml`.
* **templates** — `templates/**/*.poml`.
* **state machines** — `contexts/**/*.sm` (and legacy `.cl`).
* **config profile** — non-secret settings (provider choice, `sm_file`, tool
  groups, UI prefs) plus a list of required environment changes.

Examples: a `comfyui` setup (media plugin + provider profile + models note), a
`vim-states` setup (contexts/templates + key settings), a `verified-rust` setup
(capability plugin + workflow assets + `WORKSPACE_DIR` requirement).

## Repository format

Anyone can host a repository as a static directory, HTTP(S) site or git repo.
A small index plus content-addressed artifacts:

```json
{
  "schema": 1,
  "name": "getpraxis-standard",
  "generated_at": "…",
  "signing_key": "ed25519:…",
  "setups": [
    {
      "name": "vim-states",
      "version": "1.2.0",
      "description": "Vim-like routing, states and templates",
      "runtime_api": "2",
      "items": [
        {"type": "templates", "path": "templates/", "sha256": "…"},
        {"type": "contexts", "path": "contexts/", "sha256": "…"},
        {"type": "skills", "path": "skills/", "sha256": "…"},
        {"type": "plugin", "name": "runtime_control", "version": "1.0.0", "sha256": "…"}
      ],
      "config_changes": [
        {"key": "USE_PROVIDER", "required": false, "reason": "…"},
        {"key": "WORKSPACE_DIR", "required": true, "reason": "verified actions need a project"}
      ]
    }
  ]
}
```

* Every artifact is addressed by `sha256`; a version is immutable (republishing
  the same version with different bytes is a verify failure).
* The index is signed; `xis` refuses unsigned repos unless the operator passes
  `--allow-unsigned`. Repository keys are pinned by the operator
  (`~/.config/xis/repositories.json`), never by the repository itself.
* Multiple repositories resolve with normal version constraints; conflicts fail
  instead of last-write-wins.

## Commands (sketch)

```bash
xis repo add standard https://getpraxis.boo/repo --key <pubkey>
xis repo list | refresh | remove
xis search <query>
xis plan    standard/vim-states@1.2.0     # show files, overwrites, config changes
xis install standard/vim-states@1.2.0
xis upgrade vim-states
xis remove  vim-states --keep-data
xis motd [--ack]
```

`xis install`:

1. Resolve the setup and its dependencies; verify signatures and hashes.
2. Compute a **plan**: every file to write, every overwrite, every backup,
   every config/secret the operator must supply. Print it before touching disk.
3. Apply with `keep | backup | overwrite` policy (below).
4. Delegate plugin packages to the Praxis CLI so the kernel still runs the
   lifecycle hooks, writes `praxis.lock.json`, and enforces `PLUGIN_HOOKS` and
   the trust store.
5. Write `xis.lock.json` (item versions, file hashes, ownership) and a MOTD.

## Overwrite and ownership policy

Operator files are never silently destroyed:

* **keep** (default) — existing file wins; report the conflict.
* **backup** — move the existing file to `.xis-backups/<timestamp>/…`, then
  write; the MOTD lists what was moved.
* **overwrite** — only with `--force`; still backed up.

`xis.lock.json` records which files each setup owns, so `remove`/`upgrade`
touch only owned files, never operator-created ones. A file whose hash changed
since install is reported as an operator edit and left alone unless forced.

## MOTD / change report

After install (and until acknowledged), `xis` writes a report and Praxis shows
it (`praxis run`, dashboard, `praxis motd`):

```text
xis: vim-states 1.2.0 installed

Required changes
  WORKSPACE_DIR      set the prepared project root            (docs link)
  OPENAI_API_KEY     provider credential for USE_PROVIDER=openai

Backed up (3)
  templates/standard.poml -> .xis-backups/2026-10-05T…/

Notes
  Restart Praxis to load 1 plugin. No tools were enabled automatically.
```

Rules: the report never contains secret values, never enables tools or trust
roles on the model's behalf, and never claims a change succeeded when a step
was skipped. A setup that needs a `runtime`/`authority` plugin lists the exact
`praxis plugin trust …` command for the operator.

## Configuration application

Config profiles touch only documented, non-secret keys (provider/model choice,
`sm_file`, tool groups, UI preferences, `POML_CLI`, ports). Application is:

* diffable (`xis plan` shows old → new),
* backed up,
* explicit about secrets (they are listed, never written).

A "ComfyUI setup" is therefore: install the media package + a provider profile
+ templates/workflows + a MOTD that points at the model files and GPU notes. A
"vim-like" setup is contexts/templates/skills + routing settings, no code.

## Security

* Repositories and plugins are operator-trusted code, like today's installed
  executables; signatures and hashes make them tamper-evident, not sandboxed.
* `xis` inherits the Praxis hooks policy for plugin hooks and adds a
  repo-level policy; it must not silently run hooks the operator denied.
* No secret values in bundles, indexes, logs or MOTDs.
* Trust roles (`praxis plugin trust`) are always an explicit operator action.

## Relationship to what exists

Already in place and reused: manifest v2 `provides`/`requires`/`replaces`/
hooks/frontend/role, `PLUGIN_HOOKS`, `praxis.lock.json` + `plugin verify`,
`praxis plugin install/upgrade/uninstall/enable/disable/trust`, dependency
ordered presets (`install-default`), and `install-preset compatibility` with
backups for dashboard assets.

Missing (this design): a repository index client, content-addressed bundles and
signature verification, a setup bundle format, a file-ownership lock for
templates/contexts/skills, a MOTD service, and a safe config-profile writer.

## Phasing

1. **Read-only repositories**: `xis repo add/list/refresh`, `xis search`,
   `xis plan` over an index with hashes (no writes).
2. **Setup install**: bundles + `keep/backup/overwrite` + `xis.lock.json`
   ownership + plugin delegation to the Praxis CLI.
3. **MOTD**: change report service, `praxis motd`, shown on `praxis run`.
4. **Trust**: index signatures, pinned repo keys, immutable-version enforcement.
