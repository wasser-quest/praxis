# Praxis prompt template reference

Templates are Markdown files rendered by [MiniJinja](https://docs.rs/minijinja)
(Jinja2 syntax). There is no Node, no POML CLI and no subprocess per render.

## Files and names

- System/user/task templates live in `templates/`, e.g. `templates/tasks/plan.md`.
  The template name is the path without extension: `tasks/plan`.
- `name.md` wins over a legacy `name.poml`. Legacy `.poml` still renders through
  the Microsoft CLI when `POML_CLI` is set; convert it instead of extending it.
- Includes are always relative to `templates/`, whatever the including file's
  folder: `{% include "shared/runtime.md" %}`. `..`, absolute paths and links out
  of `templates/` are refused.
- Skills are `skills/<name>/SKILL.md` (see the skill_creator skill).

## Syntax

```jinja
{{ user_prompt }}                       {# a variable #}
{{ mode or 'chat' }}                    {# default when missing/empty #}
{{ settings.sm_file or 'none' }}        {# missing parents are fine too #}
{% if mode == 'agent' %}…{% elif mode == 'chat' %}…{% else %}…{% endif %}
{% for skill in skills[:5] %}- {{ skill.name }}{% else %}No skills.{% endfor %}
{{ loop.index }}                        {# 1-based inside for; loop.index0 is 0-based #}
{% set role = sm_data.role or 'standard' %}
{{ facts | tojson }}                    {# JSON for lists/objects #}
{{ names | join(', ') }}  {{ text | upper }}  {{ items | length }}
{{ memory.variables | keys(30) }}       {# Praxis: first 30 keys, sorted #}
{{ 'yes' if flag else 'no' }}
{% include "shared/output_format.md" %}
{% raw %}{{ shown literally }}{% endraw %}
```

- Missing variables and attributes render as empty and are false in `if`.
- Block tags strip their own line (`trim_blocks`, `lstrip_blocks`), and runs of
  blank lines collapse, so `{% if %}` on its own line leaves no gap. Put block
  tags on their own lines: a tag at the end of a text line also removes that
  line's newline and joins it to the next one. Inline `{% if %}…{% endif %}`
  inside a sentence is fine.
- Rendering is bounded: 256 KiB per file, 1 MiB output, and a fuel budget that
  stops runaway loops. Errors name the file and line.

## Variables Praxis supplies

System and user templates receive: `user_prompt` (also `user_message`), `mode`
(`chat`/`agent`), `username`, `turn`, `time`, `path`, `settings` (all context
settings), `sm_file`, `active_state`, `system_template`, `sm_data` (workflow
data), `custom_data`, `memory` (`profile`, `profiles`, `shared`, `facts`,
`preferences`, `topics`, `variables`), `skills` (a few preloaded candidates),
`skill_discovery_instructions`, `tools` (currently callable tools), `blueprint`
data where a state sets it. See `docs/CONTEXT_VARIABLES.md` for the full list.
Compaction templates receive `conversation_text`.

Skill bodies receive only the parameters passed to `use_skill` plus `skill_dir`.

## Validation

- Templates: `update_template` validates before saving. Pass a `context` object
  to test a specific case (missing, empty and populated values, both branches).
- Skills: `./praxis skill check <folder> --param user_request="…"` prints the
  rendered body without starting Praxis or calling a model.

Inspect the rendered text. A saved file or an index refresh is not proof that a
template renders.
