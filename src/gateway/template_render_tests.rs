//! Renders every shipped template with the in-process renderer, against a
//! full and an empty context. Broken includes, syntax errors and bad data
//! imports only fail at render time, so this test catches them (bugs.md #8–#11).
use std::path::{Path, PathBuf};

fn shipped_templates(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("templates"), &mut out);
    out.sort();
    out
}

/// Partials under templates/shared and templates/personas are only rendered
/// through their including root templates.
fn is_partial(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root.join("templates")).unwrap();
    rel.starts_with("shared") || rel.starts_with("personas")
}

#[tokio::test]
async fn shipped_templates_render() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let plugins = crate::plugins::PluginRegistry::new();
    let mut ctx = crate::db::contexts::Context { user_id: "render-test".into(), ..Default::default() };
    ctx.sm_data = serde_json::json!({
        "role": "standard",
        "persona_roles": "standard, code, code_architect, senior_dev, expert_programmer, debugger, review, teach, research",
    });
    let base = super::prompt::build_context(&db, &ctx, "Synthetic request: explain the plan.", &plugins, 0, root).await.unwrap();

    let mut failures = Vec::new();
    let mut sizes = Vec::new();
    for path in shipped_templates(root).into_iter().filter(|p| !is_partial(root, p)) {
        let name = path.strip_prefix(root.join("templates")).unwrap().with_extension("");
        let mut context = base.clone();
        context["settings"]["system_template"] = serde_json::json!(name.to_string_lossy());
        context["system_template"] = context["settings"]["system_template"].clone();
        match super::poml::render_strict(&path.to_string_lossy(), &context).await {
            Ok(text) => sizes.push((name.to_string_lossy().into_owned(), text.chars().count())),
            Err(error) => failures.push(format!("{}: {error}", name.display())),
        }
        // A bare render (no context at all) must work too: missing is empty.
        if let Err(error) = super::poml::render_strict(&path.to_string_lossy(), &serde_json::json!({})).await {
            failures.push(format!("{} (empty context): {error}", name.display()));
        }
    }
    assert!(failures.is_empty(), "templates failed to render:\n{}", failures.join("\n"));

    // The compact teach state must still select real due cards and
    // preserve the conflict-safe memory instructions, not claim an empty deck.
    let mut lesson = base.clone();
    lesson["utc_now"] = serde_json::json!("2026-01-10T12:00:00Z");
    lesson["custom_data"] = serde_json::json!({"language_learning":{"target_language":"French"}});
    lesson["memory"]["variables"] = serde_json::json!({"xp":7,"srs_items":[
        {"id":"DUE_FRENCH_CARD","item":"bonjour","translation":"hello","language":"French","due":"2026-01-01T00:00:00Z"},
        {"id":"FUTURE_CARD","language":"French","due":"2099-01-01T00:00:00Z"},
        {"id":"OTHER_LANGUAGE_CARD","language":"Japanese","due":"2026-01-01T00:00:00Z"}
    ]});
    for name in ["states/teach/teach"] {
        let text = super::poml::render_strict(&root.join(format!("templates/{name}.md")).to_string_lossy(), &lesson).await.unwrap();
        assert!(text.contains("DUE_FRENCH_CARD"), "{name}");
        assert!(!text.contains("FUTURE_CARD") && !text.contains("OTHER_LANGUAGE_CARD"), "{name}");
        assert!(text.contains("expected_profile") && text.contains("expected_value") && text.contains("last_review_id"), "{name}");
    }

    // Every state of every shipped workflow must select a renderable template.
    for entry in std::fs::read_dir(root.join("contexts")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "sm") { continue; }
        let sm = crate::sm::load_file_in(&root.join("contexts"), path.file_stem().unwrap().to_str().unwrap()).unwrap();
        for (state, vars) in &sm.states {
            if let Some(template) = vars.variables.get("settings.system_template") {
                assert!(sizes.iter().any(|(n, _)| n == template), "{}:{state} selects unrendered template {template}", path.display());
            }
        }
    }

    // The standard state must stay small enough for a 4096-token context
    // (≈4 chars/token; leaves room for tool schemas, history and the answer).
    for (name, chars) in &sizes {
        eprintln!("{name}: {chars} chars (~{} tokens)", chars / 4);
        let cap = match name.as_str() {
            "states/standard/standard" | "standard" => 2800,
            "states/teach/teach" => 4200,
            _ => continue,
        };
        assert!(*chars <= cap, "{name} renders {chars} chars; budget is {cap}");
    }
}

/// The clock belongs in the current user turn. A system prompt that changes on
/// every request defeats the model server's prefix (KV) cache and makes
/// context proxies treat each turn as a new conversation.
#[tokio::test]
async fn system_prompts_are_byte_stable_across_requests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let at = |time: &str, utc: &str| serde_json::json!({
        "time": time, "utc_now": utc, "mode": "agent", "user_prompt": "hi",
        "sm_data": {"role": "standard"},
    });
    let (first, later) = (at("2026-10-15 08:00:00", "2026-10-15T06:00:00Z"), at("2026-10-15 08:00:07", "2026-10-15T06:00:07Z"));
    for name in ["standard", "states/standard/standard", "states/code/code", "states/teach/teach"] {
        let path = root.join(format!("templates/{name}.md"));
        let a = super::poml::render_strict(&path.to_string_lossy(), &first).await.unwrap();
        let b = super::poml::render_strict(&path.to_string_lossy(), &later).await.unwrap();
        assert_eq!(a, b, "{name} changes with the clock");
    }
    let user = root.join("templates/user.md");
    let text = super::poml::render_strict(&user.to_string_lossy(), &later).await.unwrap();
    assert!(text.contains("2026-10-15 08:00:07") && text.contains("2026-10-15T06:00:07Z"), "{text}");
}
