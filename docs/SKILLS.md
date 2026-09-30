# Praxis skills: POML discovery and lazy loading

Native packages live under the installation's `skills/` directory. Each contains
`skill.json` and `skill.poml`, optionally `references/`, `scripts/` and examples.
This is not Pi's `SKILL.md` format. Only manifests are indexed; instructions and
references are **never** indexed or automatically copied into the system prompt.

## Discovery belongs to POML

Edit **`templates/discovery/skills.poml`** to define the discovery strategy. For a
per-context variant, set `custom_data.skill_discovery_template` to a template name
without `.poml`, such as `discovery/my_policy`. Ordinary template path/renderer
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
- Use POML expressions, conditions and local includes to choose rules based on
  the current request or context. Rust does not contain topic-specific routing.
- The shipped policy uses **empty names and queries**: the normal prompt contains
  no catalog at all. The model calls `search_skills` only when useful, then
  `use_skill` for one selected workflow. A million installed skills do not mean
  a million descriptions in every prompt.

Example custom policy (save under `templates/discovery/my_policy.poml`):

```xml
<poml>
  <let name="task" value="typeof user_prompt === 'string' ? user_prompt : ''" />
  <let name="wanted" value="/debug|review/i.test(task) ? ['debug'] : []" />
  <p>{{JSON.stringify({instructions:'Use relevant candidates. Otherwise search_skills with a few task keywords. Load only a matching permitted skill with use_skill. Ask the user to activate user_only skills.', names:[], queries:wanted, limit:5})}}</p>
</poml>
```

The shared `templates/shared/runtime.poml` controls presentation of candidates and
instructions. Custom personas should include it, or deliberately render `skills`
and `skill_discovery_instructions` themselves. Missing default discovery assets
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

Neither a POML plan nor model arguments can bypass manifest flags:

| Manifest flag | Meaning |
|---|---|
| `skill_hidden: true` | Omit from model search and POML candidates, including pins. An exact-name dependency can still be loaded unless user-only. Human browsing includes it. |
| `user_only: true` | `use_skill` refuses it. The user must select it through `/skill` or authenticated context controls. Discovery may advertise its metadata and tell the model to ask the user. |

Both flags default to false. They are independent. Hidden does not mean secret,
and user-only is an activation boundary, not a sandbox against unrestricted
local file/terminal tools or an operator changing manifests.

## Indexed discovery, not repeated directory scans

`search_skills` uses a separate persistent **`DATA_DIR/skill-index.sqlite`** FTS5
index. The first search for an unindexed root builds it once. Normal prompts with
the default policy never open/scan the skills directory. Searches return at most
20 results; exact canonical-name loads read only that manifest and POML. Aliased
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
{"query":"POML templates","limit":5}
```

Pass this to `search_skills`, then use its required-input contract with `use_skill`:

```json
{"name":"poml_templates","parameters":{"user_request":"Create a French tutor"}}
```

Only explicit parameters plus the loader's authoritative `skill_dir` are supplied
to per-task POML. It does not silently merge stored secrets/context. Loading a
skill returns instructions, not completed actions: no scripts, provider spending,
file changes, or permission changes occur automatically. Relative references and
helpers resolve against the returned absolute skill directory.

## Shipped skills

| Skill | Required string | Notes |
|---|---|---|
| `code_review` | `code` | Review instructions |
| `debug` | `error` | Debugging workflow |
| `tmux` | `user_request` | Host tmux required; scripts never autorun |
| `poml_templates` | `user_request` | Real Microsoft POML authoring/validation |
| `skill_creator` | `user_request` | **User-only by default**; delegates POML work to `poml_templates` with `target_kind: "skill"` |

The creator keeps package/index guidance in a separate reference and does not
duplicate the POML guide. `target_kind: "skill"` tells the POML skill to validate
staged skill files with its helper rather than saving them through `update_template`,
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
`skill.poml` last), using atomic file replacements. Do not overwrite unrelated
customizations. Validate the installed POML with the real renderer, then refresh
only that folder's metadata if its description changed:

```bash
/path/to/installation/praxis skill --directory /path/to/installation \
  --data-dir /actual/data index --folder <skill-name>
```

`execute_skill` renders the disk POML on every load. The new instructions apply
to the next `use_skill` load or next message with that skill already selected;
an in-flight task can still hold older instructions. There is no need to change
persistent selection, credentials, the main DB, VM or service state. A Git push
alone does **not** update an existing installation. The asset allow-list also
bundles the guide in future builds/onboarding/repair; `repair-assets` deliberately
preserves existing skill files and is not an overwrite/update command for them.
New Rust loader behavior would still require a rebuilt binary and restart.

## POML setup, validation and deployment

Set `POML_CLI` in the service environment to Microsoft's JavaScript CLI; Node is
required. The tested local path is `/workspace/poml/python/poml/js/cli.js`, not a
bundled dependency. Strict skill/discovery/template rendering has no silent
string-substitution fallback and fails on missing input, renderer errors or empty
output. The renderer uses `type="string"` for literal imports in this installed
CLI. References/examples are under `skills/poml_templates/`.

Use the POML skill's `scripts/validate.py` with synthetic contexts, not private
context dumps. For normal templates, `update_template` validates a temporary
sibling before replacement, checks prospective include cycles, preserves old
files on validation failure, then mirrors the saved template to the DB. Saving
and activation are separate. Imports are trusted local code and require review;
the renderer deadline does not make arbitrary template code safe.

```bash
POML_CLI=/path/to/cli.js python3 scripts/test_skills.py
POML_CLI=/path/to/cli.js python3 scripts/test_poml_templates.py
python3 scripts/check_context_docs.py
POML_CLI=/path/to/cli.js EMBEDDING_PROVIDER=none \
  cargo test --locked --lib -- --include-ignored --test-threads=1
cargo check --locked --all-targets --features songbird
```

Onboarding and `repair-assets` bundle both complete new skills and the default
POML discovery policy. Existing custom files/configuration/data are preserved.
Rebuild/deploy the binary and changed public assets together: an old binary does
not gain indexed discovery or access enforcement merely by copying skill folders.
Startup adds the search tool and updates tool metadata while preserving disabled
tools. A running binary needs a normal authorized restart to load new Rust code.
