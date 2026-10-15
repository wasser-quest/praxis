# Praxis skill authoring

## Folder layout

```text
skills/<folder>/SKILL.md          # front matter + instructions
skills/<folder>/references/...    # optional, read only when needed
skills/<folder>/scripts/...       # optional, never run by the loader
```

This is the same `SKILL.md` layout Claude Code and other agents use, so their
skills can be copied into `skills/` as they are. Legacy Praxis skills
(`skill.json` + `skill.poml`) still load; when a folder has both, `SKILL.md` wins.

`use_skill` prints the skill's directory; resolve relative references and scripts
against it, not against the shell's working directory.

## Front matter

```yaml
---
name: my_skill                 # 1–64 letters, digits, _ or -; unique
description: >                 # 1–1024 chars; search shows the first 320
  What it does and when to use it, with trigger words (also in the user's language).
parameters: [user_request]     # optional: required non-empty string inputs
hidden: false                  # optional: omit from model discovery
user_only: false               # optional: only the user may activate it
version: "1.0.0"               # optional
template: false                # optional: render the body with Jinja without parameters
---
```

- Unknown keys from other agents (`license`, `allowed-tools`, `metadata`, …) are
  accepted and ignored. `metadata.version` is used when `version` is missing.
- The body is returned **verbatim** unless `parameters` is non-empty or
  `template: true`. Then it is rendered with Jinja (see the prompt_templates
  skill), receiving only the passed parameters and `skill_dir`. Missing values
  are empty, so validate inputs through `parameters`, not template guards.
- Avoid the names `list`, `search` and `off` (Discord `/skill` controls).
- Front matter ≤ 16 KiB, file ≤ 256 KiB, at most 16 parameters. `SKILL.md` must
  be a regular file, not a symlink. Grouped folders work up to 16 levels deep;
  do not nest a skill inside another skill's references or scripts.
- Human activation maps the latest task to `user_request`, `code` and `error`;
  other required inputs must be passed explicitly.

## Discovery

The prompt never contains the whole catalog. `templates/discovery/skills.*`
picks a few candidates, `search_skills` searches the metadata index on demand, and
only `use_skill` or human activation loads a body. Keep references out of the body.

## Validate and publish

```bash
./praxis skill check my_skill --param user_request="Validate this skill"
./praxis skill index --folder my_skill            # refresh one folder
./praxis skill index --folder documents/my_skill  # grouped folder
./praxis skill index                              # full rebuild after bulk imports
```

These commands do not load `.env`, unlock secrets or start services. The index
lives in `./data` by default; pass `--data-dir` if the service uses another
`DATA_DIR`, and `--directory /installation` from another working directory.
Duplicate names fail instead of overriding an existing activation policy. An
index refresh is metadata only; it does not prove the body renders.

Human activation: `/skill skillname:my_skill`, `/skill skillname:off`.
Agent loading (not for `user_only`): `use_skill` with
`{"name":"my_skill","parameters":{"user_request":"The actual task"}}`.

Writing or indexing files never activates a skill. Local file and terminal access
is not a sandbox; do not use it to bypass manifest flags or human activation.
