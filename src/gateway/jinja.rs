//! Native prompt templates: Markdown with Jinja expressions (MiniJinja).
//!
//! A `.md` template is plain Markdown; `{{ … }}`, `{% if %}`, `{% for %}` and
//! `{% include "shared/x.md" %}` add data and structure. Missing variables are
//! empty instead of errors, so templates need no `typeof` guards. Rendering is
//! in-process: no Node, no temporary files, no subprocess per prompt.
//!
//! Templates are trusted local prompt code, but rendering is still bounded: it
//! cannot touch the filesystem beyond `include` inside the template root, has
//! no network or environment access, and runs on a fuel budget so a runaway
//! loop fails instead of hanging the request.

use anyhow::Context as _;
use minijinja::{Environment, Error, ErrorKind, UndefinedBehavior, Value};
use std::path::{Component, Path, PathBuf};

/// Bytes per template file and of rendered output.
const MAX_TEMPLATE_BYTES: u64 = 256 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Roughly one unit per instruction; the shipped prompts use a few thousand.
const FUEL: u64 = 2_000_000;

/// True for files this renderer owns.
pub fn handles(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("md" | "jinja"))
}

/// Render the template at `template_path`. Includes resolve against the
/// nearest ancestor directory named `templates` (or the file's own directory),
/// always as root-relative names such as `shared/runtime.md`.
///
/// For validation, `destination` names the file being replaced: an include of
/// that path reads `template_path` instead, so self-includes are caught too.
pub fn render(template_path: &Path, context: &serde_json::Value, destination: Option<&Path>) -> anyhow::Result<String> {
    let path = template_path
        .canonicalize()
        .with_context(|| format!("Template not found: {}", template_path.display()))?;
    let root = include_root(destination.unwrap_or(&path));
    let entry = match destination {
        // Validation renders the candidate under the destination's name.
        Some(destination) => relative_name(&root, destination)?,
        None => relative_name(&root, &path)?,
    };

    let mut env = environment();
    let loader_root = root.clone();
    let candidate = destination.map(|d| (normalize(d), path.clone()));
    env.set_loader(move |name| load(&loader_root, name, candidate.as_ref()));
    let data_root = root.clone();
    env.add_function("load_json", move |name: &str| -> Result<Value, Error> {
        let text = load(&data_root, name, None)?
            .ok_or_else(|| Error::new(ErrorKind::TemplateNotFound, format!("load_json: {name} not found")))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("load_json {name}: {e}")))?;
        Ok(Value::from_serialize(value))
    });

    let mut render_context = context.clone();
    // Canonical saved JSON is sm_data; keep the old alias for user templates.
    if let (Some(object), Some(data)) = (render_context.as_object_mut(), context.get("sm_data")) {
        object.entry("cl_data").or_insert_with(|| data.clone());
    }
    let template = env.get_template(&entry).map_err(describe)?;
    let rendered = template.render(Value::from_serialize(&render_context)).map_err(describe)?;
    anyhow::ensure!(rendered.len() <= MAX_OUTPUT_BYTES, "Template output exceeds 1 MiB");
    let text = tidy(&rendered);
    anyhow::ensure!(!text.is_empty(), "Template rendering returned empty output");
    Ok(text)
}

/// Render a template given as a string (skill bodies), without includes.
pub fn render_str(name: &str, source: &str, context: &serde_json::Value) -> anyhow::Result<String> {
    let env = environment();
    let rendered = env.render_named_str(name, source, Value::from_serialize(context)).map_err(describe)?;
    anyhow::ensure!(rendered.len() <= MAX_OUTPUT_BYTES, "Template output exceeds 1 MiB");
    Ok(tidy(&rendered))
}

fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    // `a.b.c` on a missing `a` is empty, like an unset variable.
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.set_fuel(Some(FUEL));
    // Prompts read like JSON: true/false, and nothing for none/undefined.
    env.set_formatter(|out, state, value| match value.kind() {
        minijinja::value::ValueKind::Bool => out.write_str(if value.is_true() { "true" } else { "false" }).map_err(Error::from),
        minijinja::value::ValueKind::None | minijinja::value::ValueKind::Undefined => Ok(()),
        _ => minijinja::escape_formatter(out, state, value),
    });
    env.add_filter("keys", keys);
    env.add_filter("merge", merge);
    env.add_filter("timestamp", timestamp);
    env
}

/// `{{ base | merge(override) }}`: a new map with `override`'s keys replacing
/// `base`'s. Anything that is not a map counts as empty.
fn merge(base: Value, extra: Option<Value>) -> Result<Value, Error> {
    let mut merged = std::collections::BTreeMap::new();
    for value in [Some(base), extra].into_iter().flatten() {
        if value.kind() != minijinja::value::ValueKind::Map {
            continue;
        }
        for key in value.try_iter()? {
            let item = value.get_item(&key)?;
            merged.insert(key.to_string(), item);
        }
    }
    Ok(Value::from(merged))
}

/// `{{ "2026-10-15T08:00:00Z" | timestamp }}`: seconds since the epoch for an
/// RFC 3339 date, or none for anything else, so invalid dates never compare true.
fn timestamp(value: Value) -> Value {
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|date| Value::from(date.timestamp()))
        .unwrap_or(Value::from(()))
}

/// `{{ memory.variables | keys(30) }}`: the first `limit` keys of a map, sorted.
fn keys(value: Value, limit: Option<usize>) -> Result<Value, Error> {
    let mut names: Vec<String> = match value.try_iter() {
        Ok(iter) if value.kind() == minijinja::value::ValueKind::Map => iter.map(|k| k.to_string()).collect(),
        _ => Vec::new(),
    };
    names.sort();
    names.truncate(limit.unwrap_or(usize::MAX));
    Ok(Value::from(names))
}

fn load(root: &Path, name: &str, candidate: Option<&(PathBuf, PathBuf)>) -> Result<Option<String>, Error> {
    let invalid = |detail: &str| Error::new(ErrorKind::InvalidOperation, format!("include \"{name}\": {detail}"));
    let relative = Path::new(name);
    if name.is_empty()
        || name.contains('\\')
        || !relative.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(invalid("use a path relative to the templates folder, without '..'"));
    }
    let wanted = root.join(relative);
    let path = match candidate {
        Some((destination, candidate)) if normalize(&wanted) == *destination => candidate.clone(),
        _ => wanted,
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return Ok(None);
    };
    let real = path.canonicalize().map_err(|e| invalid(&e.to_string()))?;
    if !real.starts_with(root) || !meta.is_file() {
        return Err(invalid("must be a file inside the templates folder"));
    }
    if meta.len() > MAX_TEMPLATE_BYTES {
        return Err(invalid("file exceeds 256 KiB"));
    }
    std::fs::read_to_string(&real).map(Some).map_err(|e| invalid(&e.to_string()))
}

fn include_root(path: &Path) -> PathBuf {
    let path = normalize(path);
    path.ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|n| n == "templates"))
        .or_else(|| path.parent())
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn relative_name(root: &Path, path: &Path) -> anyhow::Result<String> {
    let path = normalize(path);
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("Template {} is outside {}", path.display(), root.display()))?;
    relative
        .to_str()
        .map(|s| s.replace(std::path::MAIN_SEPARATOR, "/"))
        .context("Template path is not UTF-8")
}

/// Canonicalize when the file exists; otherwise canonicalize its directory.
fn normalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf()).join(name),
        _ => path.to_path_buf(),
    })
}

/// Trim line ends and collapse runs of blank lines left by skipped blocks.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    crate::gateway::poml::strip_think_tags(out.trim())
}

/// Template name, line and the failing expression, without a Rust backtrace.
fn describe(error: Error) -> anyhow::Error {
    // Name each failing include down to the root cause, then show the source
    // excerpt of the innermost template (where the mistake actually is).
    let mut message = error.to_string();
    let mut innermost = &error;
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        message.push_str(&format!("\n  caused by: {cause}"));
        if let Some(inner) = cause.downcast_ref::<Error>() {
            innermost = inner;
        }
        source = cause.source();
    }
    if innermost.name().is_some() {
        // `{:#}` = message line + source excerpt; keep only the excerpt.
        let detail = format!("{innermost:#}");
        message.push('\n');
        message.push_str(&detail.lines().skip(1).take(12).collect::<Vec<_>>().join("\n"));
    }
    anyhow::anyhow!("Template rendering failed: {}", crate::util::truncate_chars(&message, 3000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(root: &Path, name: &str, content: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn missing_variables_are_empty_and_chain_safely() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir.path().join("templates"), "t.md", "Mode: {{ mode or 'chat' }}. Role: {{ sm_data.role or 'standard' }}.{% if memory.profile %} X{% endif %}");
        assert_eq!(render(&path, &json!({}), None).unwrap(), "Mode: chat. Role: standard.");
        let shown = write(&dir.path().join("templates"), "v.md", "[{{ flag }}|{{ off }}|{{ nothing }}|{{ missing }}]");
        assert_eq!(render(&shown, &json!({"flag": true, "off": false, "nothing": null}), None).unwrap(), "[true|false||]");
        assert_eq!(render(&path, &json!({"mode": "agent", "sm_data": {"role": "code"}}), None).unwrap(), "Mode: agent. Role: code.");
    }

    #[test]
    fn includes_resolve_from_the_templates_root_and_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("templates");
        write(&root, "shared/role.md", "Role: {{ role }}");
        let state = write(&root, "states/code/code.md", "# Code\n{% include \"shared/role.md\" %}");
        assert_eq!(render(&state, &json!({"role": "code"}), None).unwrap(), "# Code\nRole: code");
        write(dir.path(), "secret.md", "secret");
        let escape = write(&root, "escape.md", "{% include \"../secret.md\" %}");
        assert!(render(&escape, &json!({}), None).unwrap_err().to_string().contains("without '..'"));
        let missing = write(&root, "missing.md", "{% include \"nope.md\" %}");
        assert!(render(&missing, &json!({}), None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_includes_outside_the_root_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("templates");
        write(dir.path(), "outside.md", "secret");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside.md"), root.join("link.md")).unwrap();
        let path = write(&root, "t.md", "{% include \"link.md\" %}");
        assert!(render(&path, &json!({}), None).is_err());
    }

    #[test]
    fn filters_cover_what_the_poml_templates_did_with_javascript() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir.path().join("templates"),
            "t.md",
            "{{ facts | tojson }} {{ vars | keys(2) | tojson }} {{ (missing or []) | tojson }} {% for s in skills[:1] %}{{ s.name }}: {{ s.required_parameters | join(', ') }}{% else %}none{% endfor %}",
        );
        let context = json!({"facts": ["a"], "vars": {"z": 1, "b": 2, "c": 3}, "skills": [{"name": "debug", "required_parameters": ["error", "code"]}, {"name": "x"}]});
        assert_eq!(render(&path, &context, None).unwrap(), r#"["a"] ["b","c"] [] debug: error, code"#);
        assert!(render(&path, &json!({"skills": []}), None).unwrap().ends_with("none"));
    }

    #[test]
    fn blank_lines_from_skipped_blocks_collapse() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir.path().join("templates"), "t.md", "A\n\n{% if no %}\nB\n{% endif %}\n\n\nC\n");
        assert_eq!(render(&path, &json!({}), None).unwrap(), "A\n\nC");
    }

    #[test]
    fn runaway_loops_run_out_of_fuel() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir.path().join("templates"), "t.md", "{% for a in range(10000) %}{% for b in range(10000) %}x{% endfor %}{% endfor %}");
        assert!(render(&path, &json!({}), None).is_err());
    }

    #[test]
    fn errors_name_the_template_and_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir.path().join("templates"), "broken.md", "ok\n{% if %}\n");
        let error = render(&path, &json!({}), None).unwrap_err().to_string();
        assert!(error.contains("broken.md:2") && error.contains("syntax error"), "{error}");
    }

    #[test]
    fn validation_renders_the_candidate_in_place_of_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("templates");
        write(&root, "shared/part.md", "old part");
        let destination = root.join("shared/part.md");
        let candidate = write(&root, "shared/.validate-x.md", "new {{ v }}");
        assert_eq!(render(&candidate, &json!({"v": 1}), Some(&destination)).unwrap(), "new 1");
        let host = write(&root, "host.md", "{% include \"shared/part.md\" %}");
        let self_include = write(&root, "shared/.validate-y.md", "{% include \"shared/part.md\" %}");
        assert!(render(&self_include, &json!({}), Some(&destination)).is_err());
        assert_eq!(render(&host, &json!({}), None).unwrap(), "old part");
    }

    #[test]
    fn json_data_merge_and_dates_replace_poml_data_imports() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("templates");
        write(&root, "blueprints/base.json", r#"{"goal": "base", "keep": 1}"#);
        let path = write(
            &root,
            "t.md",
            "{% set bp = load_json('blueprints/base.json') | merge(custom.override) %}{{ bp.goal }} {{ bp.keep }} \
             {{ 'due' if ('2026-01-01T00:00:00Z' | timestamp) <= (now | timestamp) else 'later' }} {{ 'x' | timestamp is none }}",
        );
        let context = json!({"custom": {"override": {"goal": "custom"}}, "now": "2026-10-15T08:00:00+00:00"});
        assert_eq!(render(&path, &context, None).unwrap(), "custom 1 due true");
        let escape = write(&root, "e.md", "{{ load_json('../secret.json') }}");
        assert!(render(&escape, &json!({}), None).is_err());
    }

    #[test]
    fn included_templates_see_variables_set_before_the_include() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("templates");
        write(&root, "shared/part.md", "{{ blueprint.goal }}");
        let path = write(&root, "t.md", "{% set blueprint = {'goal': 'set'} %}{% include 'shared/part.md' %}");
        assert_eq!(render(&path, &json!({}), None).unwrap(), "set");
    }

    #[test]
    fn string_templates_render_skill_bodies() {
        assert_eq!(render_str("debug", "Error: {{ error }}", &json!({"error": "E1"})).unwrap(), "Error: E1");
    }
}
