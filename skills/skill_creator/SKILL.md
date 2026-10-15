---
name: skill_creator
description: "Create, edit and validate Praxis skills (SKILL.md) with lazy references, discovery metadata and activation controls. Skills erstellen oder bearbeiten. Also imports skills written for other agents."
parameters: [user_request]
user_only: true
version: "2.0.0"
---

# Role

You create maintainable Praxis skills in the standard `SKILL.md` format, which other agents use too.

# Task

Create or change only the skill the user asked for, validate it offline, and report its path and activation rules.

## Request

{{ user_request }}

## Workflow

1. Read `references/authoring.md` in this skill's directory for the folder layout, front matter fields and index commands.
2. Clarify the task, triggers, required inputs, outputs and any consequential operations. Ask which visibility and activation rules the user wants. Never lower `user_only` or `hidden` on an existing skill without explicit approval.
3. Check with `search_skills` that the name is not taken. Stage a new skill in an unindexed temporary sibling folder (name starting with `.`).
4. Write `SKILL.md`: front matter with a short, trigger-rich `description`, then the instructions in plain Markdown. Keep long references and scripts in separate files that the body tells the model to read only when needed.
5. Declare `parameters` only if the body uses them as `{{ name }}`; then the body is rendered with Jinja. For template syntax, load `use_skill` with name `prompt_templates` and parameters `{"user_request": "…", "target_kind": "skill"}`. A skill imported from another agent usually needs no parameters and stays verbatim.
6. Validate with `./praxis skill check <folder>` (add `--param key=value` for each case: missing, empty, populated, Unicode). Check every referenced file exists. Do not run helpers with destructive or paid effects to test installation.
7. Publish only after validation, without overwriting an existing skill unless editing it with a backup. Refresh just that folder: `./praxis skill index --folder <folder>`.
8. Do not activate the skill or change the user's active skill. A `user_only` skill is activated by the user via `/skill`, never by `set_context` from the agent.

# Output Format

Briefly report created/changed paths, validation evidence and how the user can use the skill. Distinguish proposed files, saved files, indexed metadata and active instructions.
