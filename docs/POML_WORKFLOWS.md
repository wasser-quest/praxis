# Semantic templates, skills and statemachines

## What selects the prompt

1. Load the current user's context and plugin defaults.
2. Write the **current raw input** to `custom_data.user_prompt`.
3. Select `settings.sm_file`, then root `sm_file`, otherwise `standard`.
4. Load the workflow, preferring `contexts/NAME.sm` over legacy `.cl`, and apply routing.
5. Strictly render `settings.system_template` (default `standard`) from `templates/`, with the shared runtime variables.
6. Append active skill instructions using this task's input. This does not execute a skill's scripts.

Both runtime paths use this preparation. Agent iterations reload persisted changes before subsequent rendering. Missing/malformed explicit workflows or templates are errors, not permission to use a different prompt silently. Preview shares the context builder and does not save its routing changes. Secrets are not part of the builder.

`sm_file` and `sm_data` are canonical in saved/output JSON. Old `cl_file`, `settings.cl_file`, `cl_data` and dotted `cl_data.*` inputs are normalized before merging without losing workflow data. The renderer retains `cl_data` only as a compatibility alias for existing user-owned POML. A settings-level workflow selection wins over the root-level one.

## Built-in profiles

| Template | Purpose |
|---|---|
| `standard` | General, helpful assistant; does not assume every task is programming |
| `language_instructor` | Configurable, gentle language practice with voice-friendly answers |
| `code_assistant` | Focused implementation, permission checks and verifiable tests |
| `researcher` | Evidence-based research with uncertainty and source awareness |
| `system`, `language_learning` | Compatibility aliases for `standard`, `language_instructor` |
| `roles/*`, `tasks/*` | Compatible role/task names rebuilt with valid POML and shared context |
| `user`, `compaction` | Current-message wrapper and evidence-preserving summary instructions |

The former prompts/workflows were preserved before replacement in `docs/archive/previous-prompts-and-workflows.zip`, outside template discovery. Existing template names remain usable; unsupported `<prompt>` components and unguarded optional variables were removed from active files.

### Switch persona

With the default `standard.sm` workflow, ordinary messages such as these trigger editable rules:

```text
Be a language instructor.
Teach me French.
Be a code assistant.
Act as a researcher.
Switch to standard.
```

A selected persona **sticks** for subsequent messages. A mere quoted mention does not match these anchored rules. This is deterministic routing, not a claim that regexes understand every paraphrase. Edit `contexts/standard.sm` to add variants or custom profiles. No persona regexes live in Rust.

To select explicitly, use the dashboard's dotted context editor or these context commands:

```text
/context set settings.sm_file=standard
/context set settings.system_template=language_instructor
```

In Discord `/context`, enter the text after `/context` in its **command** option. The `standard` router has no unconditional template assignment, so manual selections persist until a matching role-switch rule. Other workflows may deliberately assign a template on every state entry.

### Opt-in real working states and the 20-task experiment

`contexts/20-tasks.sm` and `templates/20-tasks.poml` are separate from the default persona workflow:

```text
/context set settings.sm_file=20-tasks active_state=standard
```

The model selects a declared **real `active_state`** using `set_context`. There are no keyword auto-rules or forced phase queue. State variables and the matching POML instructions are refreshed before subsequent model requests in both chat and agent paths. `sm_data.role` is a derived compatibility label, not the routing input. Invalid model-selected state names are rejected without changing the current state.

The default `custom_data.state_eval_policy` is `continuous`: reevaluate after evidence/tool steps as well as on task entry. `entry`, `fixed` and the separate trusted `decision` router are experimental comparison arms, not claimed optimizations. `tests/fixtures/20-tasks.json` contains twenty natural user tasks and private observer rubrics; user tasks never ask for a switch. The runner defaults to all four arms (80 task runs) and can prepare a frozen snapshot without inference. See the [Decision experiment protocol](DECISION_ROUTER_EXPERIMENT.md), `scripts/bench_state_machine.py` and [the reliability work log](LOCAL_MODEL_RELIABILITY.md) for evidence and limits. An offline scripted integration test is not live acceptance; more transitions alone are not a benefit.

### Configurable response-tag prefix

```text
/context set settings.tag_prefix="!praxis"
```

Templates can interpolate `{{settings.tag_prefix}}`; generated `tag_instructions` use the same setting. With response tags enabled, `!praxis/done` and `!praxis/next` are parsed literally (including regex-special characters in a prefix). Default is `§`; historical `§done` remains compatible. Prefixes must be 1–16 characters without whitespace/control characters. Model-written context updates cannot alter the prefix.

The requested relevance-filter/forget-before-template lifecycle tags are **not implemented yet**. Do not treat their names as an available delete API. The plan is reversible request-context filtering with explicit trusted-template authority, never deletion triggered by interpolated user/tool content.

### Customize a language lesson

Save this object at `custom_data.language_learning` (do not replace unrelated `custom_data`):

```json
{
  "target_language": "Japanese",
  "explanation_language": "German",
  "level": "A1",
  "lesson_goal": "Greetings and introductions",
  "reply_style": "Two short sentences and one useful practice question"
}
```

Durable learning state lives separately in **Memory → language_instructor → Custom Data**: `learning_profile`, `srs_items` and `xp`. Load/create the relevant [memory profile](MEMORY_PROFILES.md) first; other modes do not receive the legacy standard bucket. The instructor/daily quiz use `memory_get` / `memory_set`, discovered through `search_tools`, with typed JSON and conflict-aware writes. Do not set `memory.variables.*` using context tools. Due cards are filtered using the shared `utc_now` timestamp; other languages/future cards stay stored. See [progressive discovery and SRS memory](TOOL_DISCOVERY.md) for the schema and review policy.

Without overrides, the tutor supports French/Japanese practice with German explanations. The learner's explicit request wins over defaults. Text transcripts are not proof of pronunciation; the template does not claim that audio played or promise medical benefits.

## Semantic blueprints

Each primary persona imports a JSON blueprint from `templates/blueprints/` and renders it through `templates/shared/blueprint.poml`. The contract defines **what happens to whom**, not just a vague role label:

```json
{
  "scene_goal": "Increase tension by showing defiance",
  "participants": [
    {"name": "Onyx", "role": "Agent", "description": "black cat"},
    {"name": "Red Ball", "role": "Patient", "description": "mysterious"},
    {"name": "Grandfather Clock", "role": "Source_of_Threat", "description": "ancient, looming"}
  ],
  "action_to_complete": {"predicate": "play with", "agent": "Onyx", "patient": "Red Ball"},
  "constraints": ["Preserve the stated participants and action"],
  "success_criteria": ["Defiance increases the scene's tension"],
  "output_format": "One suspenseful sentence"
}
```

Save overrides at `custom_data.semantic_blueprint`, or use `sm_data.semantic_blueprint` for a workflow-provided contract. The custom-data object takes precedence over the workflow object; the selected override merges with the persona defaults. `action_to_complete` merges by field; other arrays/fields replace their defaults. The blueprint is task data, not authority to bypass permissions. It is not printed to the user unless requested.

Templates ask for private planning/checking and concise public plans, assumptions, evidence and results—not exposed chain-of-thought. Simple questions still receive direct answers. This is a structured prompt/task contract, not a modification of the model's underlying inference algorithm or a guarantee of correctness.

`shared/runtime.poml` advertises the current input, selected workflow/template/skill, the bounded metadata candidates chosen by the [POML discovery policy](SKILLS.md#discovery-belongs-to-poml), the small current task's tool selection, and **this user's selected profile's** facts, preferences, topics and memory-variable names, plus separate rare shared identity facts. Callable schemas are loaded progressively through `search_tools`, not duplicated as a complete catalog in each persona. The user/compaction wrappers stay purpose-specific; all conversational persona/role/task includes share this discovery policy. Only successful memory tool results justify saying information was saved. Tool descriptions and old memories are not new authorization.

## Actual SM grammar

```ini
@name "Example coding phases"
@version "1.0"
@steps [plan, implement, verify, done]

[state plan]
settings.system_template = "tasks/plan"

[state implement]
settings.system_template = "tasks/code"

[state verify]
settings.system_template = "tasks/test"

[state done]
settings.system_template = "tasks/done"

[overrides]
if custom_data.user_prompt =~ "(?i)^reset role$" -> settings.system_template = "standard"
```

`@steps` supplies the ordered states for `agent_next`. State assignments currently store string-compatible values; use normal typed context tools for JSON booleans/objects. `[auto]` accepts ordered `condition -> use STATE` rules (first match wins). `[transitions]` uses `FROM -> TO : when CONDITION`; `[overrides]` uses `if CONDITION -> dotted.key = value`. The old inline `transition -> ... on next` / `auto_rule:` snippets are not the supported parser grammar.

### Tool groups and activated tools

There are no built-in tool groups. A `[tool_groups]` section defines them per workflow; members may be built-in tool names, plugin tool names (e.g. `brave_web_search`), or other group names. Defining a group does **not** activate it; each state lists what it activates in `settings.activated_tools`, mixing groups and single tools freely:

```
[tool_groups]
core_files = [read_file, write_file, edit_file]
coding = [core_files, execute_terminal, search_tools]
role_switch = [set_context, get_context]

[state research]
settings.activated_tools = ["coding", "brave_web_search", "role_switch"]
settings.tool_discovery_mode = "Full"
```

Workflow groups are merged into `settings.tool_group_definitions` on every routing pass (same-named context entries are replaced so `.sm` edits propagate; groups the user added under other names are kept). Users can also define groups in the context, e.g. `set_context settings.tool_group_definitions.my_kit = ["execute_terminal", "coding"]`. When a state activates tools, only those tools (built-in *and* plugin) are sent to the model, each with its full schema; nothing else is appended. Keep `set_context` activated in every state, otherwise the model cannot switch roles again.

### Model-driven state switching

The model switches state by writing a context variable — the shipped `standard.sm` uses `set_context("sm_data.role", "<role>")` — and `[auto]` rules map that value to a state on the next routing pass (every LLM request, including tool-call follow-ups). The state then sets `settings.system_template` and `settings.activated_tools`, so instructions and tool allowance change together. `[transitions]` are only evaluated by `agent_next`; use `[auto]` for model-driven switching. A `settings.system_template` that no state of the active workflow produces is treated as a manual choice and disables routing until it is cleared.

State variables persist in the context until another state overwrites them, so every state should declare the variables it depends on (`system_template`, `activated_tools`, `tool_discovery_mode`).

### Template includes

POML resolves `<include src>` and `<let src>` relative to the **file containing the tag**, including nested includes. `templates/shared/state_base.poml` therefore includes `blueprint.poml`, not `../../shared/blueprint.poml`. Raw `&&` is not allowed in text content; put such expressions in a `<let>`.

Conditions support dotted paths, comparisons, `=~` / `!~` regexes and quote-aware `&&` / `||`. Tool history lives at `custom_data.used_tools` and `custom_data.tool_history`, not an automatically populated root `used_tools`. Regex backslashes are literal in `.sm` files: use `\b` or `\d`, not JSON's doubled-backslash encoding. Do not use turn counts as proof a task finished. The shipped `coding.sm` and `self_learning.sm` use explicit phases instead.

## Repair an incomplete onboarding installation

Older onboarding code installed only the previous template list and created the obsolete `contextlanguage/` directory; it omitted the new includes, `contexts/standard.sm`, native skills and bitmap icons. This caused the first paired message to fail with `Workflow routing failed: SM IO Error: No such file or directory` and the logo to return 404. Changing `sm_file` does not create the missing file.

Use the **updated binary**, not interactive onboarding, to repair the working directory used by `praxis run`:

```bash
/path/to/updated/praxis repair-assets --directory /path/to/installation --update-dashboard
```

- Adds all missing bundled workflows, templates/imports, skills and logo/favicon assets.
- Preserves existing templates, workflows, skills, `.env`, secrets and databases; does not re-pair users or change activation.
- `--update-dashboard` explicitly replaces differing bundled static UI files and saves their previous bytes under `.praxis-asset-backups/<unique-id>/`. Omit the flag to preserve every existing file.
- Refuses symlinked asset destinations. No providers, databases or services are opened/started; even `.env` is not loaded by this command.
- Refresh/hard-refresh the browser after updating JS/CSS. A running binary with the new routing/asset endpoints reads these repaired files on subsequent requests; installing a newer binary itself still requires your normal service update.

Fresh `onboard --interactive` now uses this same complete asset bundle. **Do not rerun onboarding just to repair files**: onboarding is a configuration wizard and can rewrite settings/credentials. Node and a working `POML_CLI` remain separately required; asset repair does not install the renderer.

## Validation and deployment

From the source root:

```bash
export POML_CLI=/absolute/path/to/poml/js/cli.js
python3 scripts/test_poml_templates.py
python3 scripts/test_language_learning_poml.py
python3 scripts/test_prompt_discovery.py
python3 scripts/test_brave_search.py
node scripts/test_memory_ui.js
python3 scripts/test_skills.py
python3 scripts/check_context_docs.py
node scripts/test_ui_static.js
CARGO_BIN=cargo bash scripts/test_workflow_backend.sh
# Requires Playwright + its Chromium, installed separately:
PLAYWRIGHT_BROWSERS_PATH=/tmp/praxis-ui-browser-cache node scripts/test_ui_browser.js
```

[`examples/poml-test-context.json`](../examples/poml-test-context.json) contains synthetic identity, current input, settings, workflow state, skills/tools, active-skill metadata, memory, token/history metadata, tutor settings, semantic roles and optional legacy task inputs. The fixture-coverage check also requires every typed `Context` and `ContextSettings` field. The all-template check renders **every `.poml` file recursively**, from a different cwd, against full, sparse, null and chat cases. It verifies sentinel content as well as strict exit status. No live context dump or database is needed.

Strict rendering requires Node and the actual Microsoft JavaScript CLI in the service environment. It does not fall back to brace substitution. Templates are trusted local code, not a sandbox; imports need authorization. See [Skills](SKILLS.md) for safe template saves and targeted Rust test commands.

Old absolute workflow paths and the former `contextlanguage/` / `data/contexts/` search roots must be relocated/reselected under the canonical `contexts/` directory. Explicit session helpers now read/write one scoped row without double-appending a session ID or silently changing activation. Durable profile contents remain shared across one user's sessions, not across different user IDs. Profile selection is session/persona-scoped; see [memory profiles](MEMORY_PROFILES.md).

Changes are **source-only** until rebuilt and deployed. Ship the updated binary and complete `templates/`, `contexts/`, `skills/` and `static/` directories, including blueprint/include dependencies. The cactus/dune `logo.png`, favicon and touch icon all derive from `static/logo-source.png`; `scripts/build_logo.mjs` reproduces the sizes using `sharp`. Normal bot startup registers `/skill`. Do not restart an active service or change user selections merely to run tests.
