# Praxis skills: SKILL.md, discovery and lazy loading

Skills live under the installation's `skills/` directory, one folder each, in the
standard `SKILL.md` format that Claude Code and other agents use:

```text
skills/<folder>/SKILL.md          # YAML front matter + Markdown instructions
skills/<folder>/references/...    # optional, read only when needed
skills/<folder>/scripts/...       # optional, never run by the loader
```

```yaml
---
name: my_skill
description: What it does and when to use it (searched; first 320 chars shown).
parameters: [user_request]   # optional required inputs → body rendered with Jinja
user_only: false             # optional
hidden: false                # optional
---
```

Skills from other agents can be copied in unchanged: unknown front matter keys
are ignored and the body is returned **verbatim** unless the skill declares
`parameters` (or `template: true`), so `{{ … }}` in their examples is never
evaluated. Legacy `skill.json` + `skill.poml` folders still load; `SKILL.md` wins
when both exist. Check a skill offline with
`./praxis skill check <folder> --param key=value`. The authoring workflow is the
user-only `skill_creator` skill; template syntax is in `prompt_templates`.

Only front matter is indexed; instructions and references are **never** indexed
or automatically copied into the system prompt.

## Discovery belongs to the template

Edit **`templates/discovery/skills.md`** to define the discovery strategy. For a
per-context variant, set `custom_data.skill_discovery_template` to a template name
without extension, such as `discovery/my_policy`. Ordinary template path/renderer
validation applies. The template receives the normal shared render context,
including `user_prompt`, `settings`, `custom_data` and `sm_data`.

The template renders a **JSON plan**, not a persona or skill body:

```json
{
  "instructions": "Your instructions for the model's skill discovery workflow",
  "names": [],
  "queries": [],
  "limit": 5
}
```

- `instructions` becomes `skill_discovery_instructions`, shown by the shared
  runtime include in agent mode. Change wording, language and search workflow here.
- `names` pins a few preferred registered **metadata** entries.
- `queries` performs a few literal word-prefix searches of names/descriptions.
- `limit` bounds the combined, deduplicated metadata candidates in `skills`.
- Use template expressions, conditions and includes to choose rules based on
  the current request or context. Rust does not contain topic-specific routing.
- The shipped policy uses **empty names and queries**: the normal prompt contains
  no catalog at all. The model calls `search_skills` only when useful, then
  `use_skill` for one selected workflow. A million installed skills do not mean
  a million descriptions in every prompt.

Example custom policy (save under `templates/discovery/my_policy.md`):

```jinja
{% set wanted = ['debug'] if 'debug' in (user_prompt or '') | lower else [] %}
{{ {"instructions": "Use relevant candidates. Otherwise search_skills with a few task keywords. Load only a matching permitted skill with use_skill. Ask the user to activate user_only skills.", "names": [], "queries": wanted, "limit": 5} | tojson }}
```

Templates that want to show candidates render `skills` and
`skill_discovery_instructions` themselves. Missing default discovery assets
in older/custom installations produce no candidates; explicitly selected missing
or invalid templates/plans fail clearly. `repair-assets` adds missing defaults
without replacing custom files.

### Bounds and access controls

A plan may have at most 20 pinned names, 4 queries, a combined result limit of
1–20, and 4096 instruction characters (16 KiB rendered JSON). A search accepts
up to 512 characters / 16 literal words; output descriptions are capped at 320
characters. These are runtime safety bounds, not an invitation to increase the
prompt in proportion to catalog size. Invalid plans fail instead of falling back
to loading every skill. Templates are trusted local prompt code, not a sandbox.

Neither a discovery plan nor model arguments can bypass manifest flags:

| Manifest flag | Meaning |
|---|---|
| `skill_hidden: true` | Omit from model search and template candidates, including pins. An exact-name dependency can still be loaded unless user-only. Human browsing includes it. |
| `user_only: true` | `use_skill` refuses it. The user must select it through `/skill` or authenticated context controls. Discovery may advertise its metadata and tell the model to ask the user. |

Both flags default to false. They are independent. Hidden does not mean secret,
and user-only is an activation boundary, not a sandbox against unrestricted
local file/terminal tools or an operator changing manifests.

## Indexed discovery, not repeated directory scans

`search_skills` uses a separate persistent **`DATA_DIR/skill-index.sqlite`** FTS5
index. The first search for an unindexed root builds it once. Normal prompts with
the default policy never open/scan the skills directory. Searches return at most
20 results; exact canonical-name loads read only that skill folder. Aliased
or grouped/sharded folders are resolved through the index.

Index construction streams metadata from disk instead of collecting all skills
in a Rust HashMap. It supports grouped folders up to 16 levels, skips hidden
staging directories and invalid packages, never traverses symlinked folders, and
aborts on duplicate names rather than overriding a policy. Rebuilds are
transactional. Searches/load calls re-read only selected manifests so deleted
files and newly hidden/user-only flags fail closed.

File additions/description changes are not polled on every turn. Refresh one
folder after editing, or rebuild after bulk imports/removals:

```bash
cd /path/to/installation
./praxis skill index                         # full metadata rebuild
./praxis skill index --folder my_skill       # only one folder
./praxis skill index --folder group/my_skill
./praxis skill search 'memory palace'
./praxis skill list --limit 10
./praxis skill list --after LAST_NAME        # keyset pagination
```

These offline commands do not read `.env`, unlock secrets, initialize the main
Praxis DB, or start services. Storage defaults to `DIRECTORY/data`; if the service
uses another `DATA_DIR`, pass `--data-dir /actual/data`. Use `--directory` when
running from another cwd. For a very large collection, prebuild the index offline
rather than making the first interactive search wait. Searches do not promise
constant CPU time for arbitrarily broad queries; catalog/prompt memory and result
sizes are bounded, while disk/index size and rebuild work grow with the catalog.

## Human activation and per-task loading

For a completed Discord pairing:

```text
/skill skillname:skill_creator
/skill skillname:off
/skill
/skill skillname:search query
/skill skillname:list LAST_NAME
```

Omitted argument / `list` browses a bounded page. Selection persists as
`settings.active_skill` for subsequent messages until the user changes it or
switches it off. Raw task input maps to required `user_request`, `code`, `error`;
other inputs must be supplied explicitly. The runtime appends only this selected
skill's rendered instructions; custom system templates cannot accidentally omit
it. Supporting references remain lazy. The model must not reload an already
active skill with `use_skill`.

**Persistent selection is user-controlled for every skill.** Agent `set_context`,
`delete_context` and automated SM transitions cannot change `settings.active_skill`.
Ordinary automatic/task-local loading uses `use_skill` instead; user-only skills
reject that route too. Thus a model cannot disguise activation as a context edit.

All Discord operations (including list, off and unknown names) require completed
pairing plus channel/guild authorization, checked before index/filesystem access
and rechecked before disclosure/mutation. Pending codes and internal context IDs
are insufficient. Replies are ephemeral; disabled `use_skill` stays enforced.
Commands can remain visible to unpaired users but execution is denied.

Example tool arguments:

```json
{"query":"prompt templates","limit":5}
```

Pass this to `search_skills`, then use its required-input contract with `use_skill`:

```json
{"name":"prompt_templates","parameters":{"user_request":"Create a French tutor"}}
```

Only explicit parameters plus the loader's authoritative `skill_dir` are supplied
to the skill body. It does not silently merge stored secrets/context. Loading a
skill returns instructions, not completed actions: no scripts, provider spending,
file changes, or permission changes occur automatically. Relative references and
helpers resolve against the returned absolute skill directory.

## Shipped skills

| Skill | Required string | Notes |
|---|---|---|
| `code_review` | `code` | Review instructions |
| `debug` | `error` | Debugging workflow |
| `tmux` | `user_request` | Host tmux required; scripts never autorun |
| `safe_web_search` | `question` | Loaded automatically before Brave searches |
| `prompt_templates` | `user_request` | Markdown/Jinja template authoring and validation |
| `skill_creator` | `user_request` | **User-only by default**; delegates template syntax to `prompt_templates` with `target_kind: "skill"` |

The creator keeps package/index guidance in a separate reference and does not
duplicate the template guide. `target_kind: "skill"` tells the template skill to
validate staged skill files with `praxis skill check` rather than saving them through `update_template`,
which intentionally writes only `templates/`. Publication/indexing do not activate
anything. See `skills/skill_creator/references/authoring.md` for manifest bounds.
quiz/binding constraints. Cell diagnostics identify the archive path, row,
sheet/key and column. Regression tests include the guide's standalone writer
and malformed native number/boolean/JSON values.

It is **not** the application's complete importer, formula validator or media
decoder. Its PASS explicitly leaves full ZIP/media integrity, reconstructed
document size, formula syntax/runtime and backend import untested. Follow the
guide's section 8: use `decodePackage` and `parseFormula` on final saved bytes
when available, test workbook behavior separately, and report unrun checks.
No local catalogue can guarantee authentication, network or destination state.
Use enabled Praxis `openrouter_image_generate` / `elevenlabs_tts` plugins only for
explicit media requests; no Pi-specific Codex/Kokoro tools are assumed. See
[Media plugins](MEDIA_PLUGINS.md). No real media calls are part of skill tests.

### Updating an installed skill without restarting

On a running version with the existing skill loader, **skill/reference/helper-only
updates need no restart or binary replacement**. Review differences and back up
the installation's existing skill files first; copy only the approved changed
files to `INSTALLATION/skills/<skill-name>/` (references/helpers first,
`SKILL.md` last), using atomic file replacements. Do not overwrite unrelated
customizations. Validate with `praxis skill check <skill-name>`, then refresh
only that folder's metadata if its description changed:

```bash
/path/to/installation/praxis skill --directory /path/to/installation \
  --data-dir /actual/data index --folder <skill-name>
```

`execute_skill` re-reads `SKILL.md` from disk on every load. The new instructions apply
to the next `use_skill` load or next message with that skill already selected;
an in-flight task can still hold older instructions. There is no need to change
persistent selection, credentials, the main DB, VM or service state. A Git push
alone does **not** update an existing installation. The asset allow-list also
bundles the guide in future builds/onboarding/repair; `repair-assets` deliberately
preserves existing skill files and is not an overwrite/update command for them.
New Rust loader behavior would still require a rebuilt binary and restart.

## Validation

Templates and skills render in-process (MiniJinja); no Node or `POML_CLI` is
needed. Legacy `.poml` templates and `skill.poml` skills still render through the
Microsoft CLI when `POML_CLI` is set; convert them instead of extending them.
Rendering fails on syntax errors, missing includes and empty output, with the file
and line in the message; there is no silent fallback.

`update_template` validates a temporary sibling before replacement (including
prospective self-includes), preserves the old file on failure, then mirrors the
saved template to the DB. Saving and activation are separate.

```bash
./praxis skill check <folder> --param user_request="…"     # one skill
./praxis template states/code/code --context ctx.json        # one template
python3 scripts/check_context_docs.py
EMBEDDING_PROVIDER=none cargo test --locked --lib
```

Onboarding and `repair-assets` bundle the shipped skills and the default
discovery policy. Existing custom files/configuration/data are preserved.
