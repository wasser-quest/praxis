//! Administration services shared by the built-in dashboard and Host API v1:
//! tools, templates, workflow (.sm/.cl) files, memory profiles, pairings,
//! cron jobs and delegations. Callers authenticate; these functions enforce
//! the same validation regardless of frontend.
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Failure {
    BadRequest(String),
    NotFound,
    Internal(anyhow::Error),
}
impl<E: Into<anyhow::Error>> From<E> for Failure {
    fn from(error: E) -> Self {
        Self::Internal(error.into())
    }
}
pub type Outcome<T> = Result<T, Failure>;

// ── Tools ────────────────────────────────────────────────────────────────

pub fn all_tools(db: &crate::db::Database, plugins: &crate::plugins::PluginRegistry) -> Value {
    let builtin: Vec<_> = crate::tools::registry::all_tool_meta()
        .iter()
        .filter(|m| crate::tools::catalog::native(plugins, m.name))
        .map(|m| json!({
            "name": m.name,
            "description": m.description,
            "category": format!("{:?}", m.category),
            "parameters": m.params_schema,
            "source": "builtin",
            "package": crate::tools::packages::owner_of(m.name).map(|p| p.id),
            "package_enabled": crate::tools::packages::tool_package_enabled(&db.data_dir(), m.name).unwrap_or(true),
            "default_enabled": m.default_enabled,
            "is_enabled": crate::db::tools::get(db, m.name).map(|t| t.is_enabled).unwrap_or(m.default_enabled),
        }))
        .collect();
    let plugin_tools: Vec<_> = plugins
        .enabled_tools()
        .iter()
        .map(|t| json!({
            "name": t.name,
            "description": t.description,
            "category": "Plugin",
            "parameters": t.parameters,
            "source": "plugin",
            "default_enabled": true,
            "is_enabled": plugins.list().into_iter().find(|plugin| plugin.enabled && plugin.tools.iter().any(|tool| tool.name == t.name))
                .is_some_and(|plugin| crate::tools::catalog::plugin_enabled(db, plugin, &t.name).unwrap_or(false)),
        }))
        .collect();
    let all = [builtin, plugin_tools].concat();
    json!({ "tools": all, "total": all.len() })
}

/// Builtin tool packages with their tools and state.
pub fn tool_packages(db: &crate::db::Database, plugins: &crate::plugins::PluginRegistry) -> Outcome<Value> {
    let mut value = crate::tools::packages::list(&db.data_dir())?;
    for package in value["packages"].as_array_mut().into_iter().flatten() {
        let id = package["id"].as_str().unwrap_or_default().to_owned();
        package["replaceable"] = json!(crate::tools::packages::replaceable(&id));
        package["replaced_by"] = json!(plugins
            .list()
            .into_iter()
            .find(|p| p.enabled && p.replaces.iter().any(|r| *r == id))
            .map(|p| p.name.clone()));
    }
    Ok(value)
}

/// Enable or disable a builtin tool package. Per-tool flags are preserved, so
/// re-enabling restores the operator's earlier per-tool choices.
pub fn set_tool_package(db: &crate::db::Database, id: &str, enabled: bool) -> Outcome<Value> {
    let package = crate::tools::packages::get(id).ok_or(Failure::NotFound)?;
    if !enabled && package.required {
        return Err(Failure::BadRequest(format!("Tool package '{id}' is part of the core runtime and cannot be disabled")));
    }
    crate::tools::packages::set(&db.data_dir(), id, enabled)?;
    Ok(json!({ "success": true, "id": id, "enabled": enabled }))
}

/// Stored builtin tool records (dashboard `/tools`).
pub fn tool_records(db: &crate::db::Database) -> Outcome<Value> {
    Ok(json!({ "tools": crate::db::tools::list(db)? }))
}

pub fn set_tool_enabled(
    db: &crate::db::Database,
    plugins: &crate::plugins::PluginRegistry,
    name: &str,
    enabled: bool,
) -> Outcome<()> {
    let builtins = crate::db::tools::list(db)?;
    // Builtins own their names, including when a plugin declares the same name.
    if crate::tools::catalog::native(plugins, name) && builtins.iter().any(|tool| tool.name == name) {
        Ok(crate::db::tools::set_enabled(db, name, enabled)?)
    } else if plugins.enabled_tools().iter().any(|tool| tool.name == name) {
        Ok(crate::db::tools::set_plugin_tool_enabled(db, name, enabled)?)
    } else {
        Err(Failure::NotFound)
    }
}

// ── Templates ────────────────────────────────────────────────────────────

const TEMPLATES_DIR: &str = "templates";

/// Template names are `segment(/segment)*` with `[A-Za-z0-9_-]` segments, so
/// a name can never leave the templates directory.
pub fn validate_template_name(name: &str) -> Outcome<()> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && name.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if ok {
        Ok(())
    } else {
        Err(Failure::BadRequest("Invalid template name".into()))
    }
}
fn template_path(root: &Path, name: &str) -> PathBuf {
    root.join(format!("{name}.poml"))
}

pub fn templates(db: &crate::db::Database) -> Outcome<Value> {
    let templates: Vec<Value> = db
        .list_templates()?
        .iter()
        .map(|t| json!({
            "name": t.name,
            "description": t.description,
            "is_system": t.is_system,
            "updated_at": t.updated_at,
        }))
        .collect();
    Ok(json!({ "templates": templates }))
}

pub fn template(db: &crate::db::Database, name: &str) -> Outcome<Value> {
    let template = db
        .list_templates()?
        .into_iter()
        .find(|t| t.name == name)
        .ok_or(Failure::NotFound)?;
    Ok(json!({
        "name": template.name,
        "content": template.content,
        "description": template.description,
    }))
}

pub fn create_template(
    db: &crate::db::Database,
    root: &Path,
    name: &str,
    content: &str,
    description: Option<&str>,
) -> Outcome<Value> {
    validate_template_name(name)?;
    let path = template_path(root, name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, content) {
        return Ok(json!({
            "success": false,
            "error": format!("Failed to write template file: {e}"),
        }));
    }
    let _ = db.save_template(name, content, description, false);
    Ok(json!({ "success": true }))
}

pub fn delete_template(db: &crate::db::Database, root: &Path, name: &str) -> Outcome<Value> {
    validate_template_name(name)?;
    let _ = db
        .conn()
        .execute("DELETE FROM templates WHERE name = ?1", rusqlite::params![name]);
    let _ = std::fs::remove_file(template_path(root, name));
    Ok(json!({ "success": true }))
}

/// Validate a template against a real rendered context before saving it.
pub async fn update_template(
    db: &crate::db::Database,
    name: &str,
    content: &str,
    user_id: Option<&str>,
    user_prompt: Option<&str>,
) -> Outcome<Value> {
    validate_template_name(name)?;
    let result = async {
        let mut ctx = match user_id.filter(|s| !s.is_empty()) {
            Some(uid) => db.load_context(uid)?,
            None => crate::db::contexts::Context::default(),
        };
        let input = crate::gateway::prompt::preview_input(db, &ctx, user_prompt)?;
        let plugins_dir = std::env::var("PLUGINS_DIR").unwrap_or_else(|_| "./plugins".into());
        let plugins = crate::plugins::load_all_plugins(Path::new(&plugins_dir));
        let config = crate::gateway::state_ref()
            .map(|gateway| gateway.config.clone())
            .unwrap_or_else(crate::config::Config::from_env);
        let workspace = config.workspace_root()?;
        crate::gateway::prompt::route_context_with_workspace(
            Path::new("."), &workspace, &mut ctx, &input, &plugins, None,
        )
        .await?;
        let context = crate::gateway::prompt::build_context(
            db, &ctx, &input, &plugins, 0, Path::new("."),
        )
        .await?;
        crate::tools::update_template::save_validated(Path::new(TEMPLATES_DIR), name, content, &context).await
    }
    .await;
    Ok(match result {
        Ok(rendered) => {
            let error = db.save_template(name, content, None, false).err().map(|e| {
                format!("Validated template saved to disk, but database update failed: {e}")
            });
            json!({ "success": true, "error": error, "rendered_preview": rendered })
        }
        Err(e) => json!({
            "success": false,
            "error": format!("Template was not saved: {e}"),
            "rendered_preview": null,
        }),
    })
}

// ── Workflow files ───────────────────────────────────────────────────────

pub fn sm_files(dir: &Path) -> Value {
    let mut files: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if ext == Some("sm") || ext == Some("cl") {
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                files.push(json!({ "name": name, "path": path.to_string_lossy() }));
            }
        }
    }
    files.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    files.dedup_by(|a, b| a["name"] == b["name"]);
    json!({ "sm_files": files })
}

pub fn sm_file(dir: &Path, name: &str) -> Outcome<String> {
    let path = crate::sm::resolve_file_in(dir, name).map_err(|_| Failure::NotFound)?;
    Ok(std::fs::read_to_string(path)?)
}

/// Parses and validates before writing (same rule as the built-in editor).
pub fn save_sm_file(dir: &Path, name: &str, content: &str) -> Outcome<()> {
    crate::sm::save_file_in(dir, name, content).map_err(|e| Failure::BadRequest(e.to_string()))
}

// ── Memory profiles ──────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MemoryUpdate {
    pub profile: Option<String>,
    pub reason: Option<String>,
    pub user_preferences: Option<HashMap<String, Value>>,
    pub custom_variables: Option<HashMap<String, Value>>,
    pub learned_facts: Option<Vec<String>>,
    pub last_topics: Option<Vec<String>>,
}

pub fn memory(db: &crate::db::Database, user_id: &str, profile: Option<&str>) -> Outcome<Value> {
    use crate::db::memory_profiles as profiles;
    let ctx = db.load_context(user_id)?;
    let mut view = profiles::snapshot(db, &ctx)?;
    let active_profile = view.profile.clone();
    if let Some(name) = profile {
        profiles::validate_name(name).map_err(|e| Failure::BadRequest(e.to_string()))?;
        let memory = profiles::read_named(db, user_id, name)?;
        view.profile = name.into();
        view.exists = memory.is_some();
        view.memory = memory.unwrap_or_default();
    }
    Ok(json!({
        "user_id": user_id,
        "active_profile": active_profile,
        "profile": view.profile,
        "profile_exists": view.exists,
        "profiles": view.profiles,
        "shared": view.shared,
        "user_preferences": view.memory.user_preferences,
        "custom_variables": view.memory.custom_variables,
        "learned_facts": view.memory.learned_facts,
        "last_topics": view.memory.last_topics,
    }))
}

/// Shared-profile writes need a short reason (audit), as in the dashboard.
pub fn update_memory(db: &crate::db::Database, user_id: &str, update: MemoryUpdate) -> Outcome<()> {
    use crate::db::memory_profiles as profiles;
    let profile = match update.profile {
        Some(name) => name,
        None => profiles::snapshot(db, &db.load_context(user_id)?)?.profile,
    };
    profiles::validate_name(&profile).map_err(|e| Failure::BadRequest(e.to_string()))?;
    if profile == profiles::SHARED
        && !update
            .reason
            .as_deref()
            .is_some_and(|r| !r.trim().is_empty() && r.len() <= 512)
    {
        return Err(Failure::BadRequest("Shared memory updates need a reason".into()));
    }
    if profiles::read_named(db, user_id, &profile)?.is_none() {
        return Err(Failure::NotFound);
    }
    profiles::update_named(db, user_id, &profile, |memory| {
        if let Some(vars) = update.custom_variables {
            memory.custom_variables = vars;
        }
        if let Some(facts) = update.learned_facts {
            memory.learned_facts = facts;
        }
        if let Some(topics) = update.last_topics {
            memory.last_topics = topics;
        }
        if let Some(preferences) = update.user_preferences {
            memory.user_preferences = preferences;
        }
        Ok(())
    })
    .map(|_| ())
    .map_err(|e| Failure::BadRequest(e.to_string()))
}

// ── Skills ───────────────────────────────────────────────────────────────

pub fn skills(db: &crate::db::Database, dir: &Path) -> Outcome<Value> {
    let mut registry = crate::skills::SkillRegistry::new();
    if dir.exists() {
        registry.load_from_dir(dir)?;
    }
    let skills: Vec<Value> = registry
        .list()
        .iter()
        .map(|s| json!({"name": s.name, "description": s.description, "user_only": s.user_only}))
        .collect();
    let active = db
        .load_context("default")
        .ok()
        .and_then(|c| c.settings.active_skill.clone());
    Ok(json!({ "skills": skills, "active_skill": active }))
}

// ── Decision profiles (files under decisions/) ───────────────────────────

const DECISIONS: &str = "decisions";

pub fn decision_profiles() -> Outcome<Value> {
    let list = crate::gateway::decision_profiles::list(Path::new(DECISIONS))
        .map_err(|e| Failure::BadRequest(e.to_string()))?;
    Ok(json!({ "profiles": list }))
}
pub fn decision_profile(name: &str) -> Outcome<Value> {
    let path = crate::gateway::decision_profiles::resolve(Path::new(DECISIONS), name)
        .map_err(|_| Failure::NotFound)?;
    Ok(json!({ "name": name, "content": std::fs::read_to_string(path)? }))
}
/// Validated by the decision-profile parser before an atomic write.
pub fn save_decision_profile(name: &str, content: &str) -> Outcome<()> {
    crate::gateway::decision_profiles::save(Path::new(DECISIONS), name, content)
        .map(|_| ())
        .map_err(|e| Failure::BadRequest(e.to_string()))
}

/// Classification-only playground: runs the decision model, changes nothing.
pub async fn decision_probe(
    profile: &crate::gateway::decision_profiles::DecisionProfile,
    contexts: Vec<String>,
) -> Outcome<Value> {
    let start = std::time::Instant::now();
    let results = crate::gateway::decision_client::decide(
        profile,
        contexts,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(|e| Failure::BadRequest(format!("Decision error: {e}")))?;
    Ok(json!({
        "results": results.iter().map(|d| json!({
            "label": d.label,
            "state": profile.state_map.get(&d.label),
            "probability": d.probability,
            "confidence": d.confidence,
            "usage": d.usage,
            "meets_threshold": d.probability >= profile.minimum_probability,
        })).collect::<Vec<_>>(),
        "elapsed_ms": start.elapsed().as_millis(),
        "notice": "Classification only: no context, state, history or permissions changed. Probabilities are not calibrated certainty.",
    }))
}

// ── Pairings, cron, delegations ──────────────────────────────────────────

fn query(db: &crate::db::Database, sql: &str, row: impl Fn(&rusqlite::Row) -> rusqlite::Result<Value>) -> Outcome<Vec<Value>> {
    let conn = db.conn();
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| row(r))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn pairings(db: &crate::db::Database) -> Outcome<Value> {
    let pairings = query(
        db,
        "SELECT user_id, discord_user_id, discord_guild_id, paired_at, last_seen_at FROM pairings ORDER BY paired_at DESC",
        |row| Ok(json!({
            "user_id": row.get::<_, String>(0)?,
            "discord_user_id": row.get::<_, String>(1)?,
            "discord_guild_id": row.get::<_, Option<String>>(2)?,
            "paired_at": row.get::<_, Option<String>>(3)?,
            "last_seen_at": row.get::<_, Option<String>>(4)?,
        })),
    )?;
    Ok(json!({ "pairings": pairings }))
}

pub fn pending_pairings(db: &crate::db::Database) -> Outcome<Value> {
    let pending = query(
        db,
        "SELECT code, discord_user_id, expires_at, created_at FROM pending_pairings ORDER BY created_at DESC",
        |row| Ok(json!({
            "code": row.get::<_, String>(0)?,
            "discord_user_id": row.get::<_, String>(1)?,
            "expires_at": row.get::<_, String>(2)?,
            "created_at": row.get::<_, Option<String>>(3)?,
        })),
    )?;
    Ok(json!({ "pending_pairings": pending }))
}

pub fn delete_pairing(db: &crate::db::Database, user_id: &str) -> Outcome<()> {
    db.conn()
        .execute("DELETE FROM pairings WHERE user_id = ?1", rusqlite::params![user_id])?;
    Ok(())
}

/// Returns the paired Discord user id.
pub fn approve_pairing(db: &crate::db::Database, code: &str) -> Outcome<String> {
    let pending = db.get_pending_pairing(code)?.ok_or(Failure::NotFound)?;
    let user_id = uuid::Uuid::new_v4().to_string();
    db.create_pairing(&user_id, &pending.discord_user_id, None)?;
    db.delete_pending_pairing(code)?;
    Ok(pending.discord_user_id)
}

pub fn delete_pending_pairing(db: &crate::db::Database, code: &str) -> Outcome<()> {
    Ok(db.delete_pending_pairing(code)?)
}

pub fn cron_jobs(db: &crate::db::Database) -> Outcome<Value> {
    let jobs = query(
        db,
        "SELECT id, name, schedule, enabled, last_run, run_count FROM cron_jobs ORDER BY name",
        |row| Ok(json!({
            "id": row.get::<_, String>(0)?,
            "name": row.get::<_, String>(1)?,
            "schedule": row.get::<_, String>(2)?,
            "enabled": row.get::<_, i32>(3)? != 0,
            "last_run": row.get::<_, Option<String>>(4)?,
            "run_count": row.get::<_, i64>(5)?,
        })),
    )?;
    Ok(json!({ "cron_jobs": jobs }))
}

pub fn delegations(db: &crate::db::Database, user_id: &str) -> Outcome<Value> {
    Ok(json!({ "delegations": crate::gateway::delegation::list_delegations(db, user_id)? }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_names_cannot_escape_the_templates_directory() {
        for ok in ["standard", "personas/code", "a-b_c"] {
            assert!(validate_template_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "../x", "a/../../b", "/etc/passwd", "a//b", "a.b", "a\\b", "x/."] {
            assert!(validate_template_name(bad).is_err(), "{bad}");
        }
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(&dir.path().join("db")).unwrap();
        let root = dir.path().join("templates");
        assert!(matches!(
            create_template(&db, &root, "../escape", "x", None),
            Err(Failure::BadRequest(_))
        ));
        assert!(!dir.path().join("escape.poml").exists());
        assert_eq!(create_template(&db, &root, "personas/new", "<poml/>", None).unwrap()["success"], true);
        assert!(root.join("personas/new.poml").is_file());
        assert_eq!(template(&db, "personas/new").unwrap()["content"], "<poml/>");
        delete_template(&db, &root, "personas/new").unwrap();
        assert!(!root.join("personas/new.poml").exists());
    }

    #[test]
    fn pairings_and_cron_lists_are_readable_on_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        assert_eq!(pairings(&db).unwrap()["pairings"], json!([]));
        assert_eq!(pending_pairings(&db).unwrap()["pending_pairings"], json!([]));
        assert_eq!(cron_jobs(&db).unwrap()["cron_jobs"], json!([]));
        assert!(matches!(approve_pairing(&db, "nope"), Err(Failure::NotFound)));
    }
}
