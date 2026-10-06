use crate::db::Database;
use crate::skills::{execute_skill, lookup_skill};
use serde_json::Value;
use std::path::Path;

/// Load instructions only. The agent performs any follow-up actions via normal
/// tools and their permission controls; loading a skill never runs its scripts.
pub async fn run(db: &Database, args: &Value) -> anyhow::Result<String> {
    run_in(db, args, Path::new("skills")).await
}

pub async fn run_in(db: &Database, args: &Value, directory: &Path) -> anyhow::Result<String> {
    anyhow::ensure!(
        crate::db::tools::tool_enabled(db, "use_skill")?,
        "use_skill is disabled"
    );
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("use_skill requires a non-empty string 'name'"))?;
    let empty = serde_json::json!({});
    let parameters = args.get("parameters").unwrap_or(&empty);
    anyhow::ensure!(
        parameters.is_object(),
        "Skill parameters must be a JSON object"
    );

    // Look up a registered name, never construct a path from model arguments.
    let skill = lookup_skill(db, directory, name)?;
    anyhow::ensure!(!skill.user_only, "Skill '{name}' is user-only. Ask the user to select it via /skill or authenticated context controls; automated activation is not allowed");
    let instructions = execute_skill(&skill, parameters).await?;
    Ok(format!(
        "Loaded skill '{name}'. These are instructions, not a completed action. Follow them using the available tools and existing permissions.\nSkill directory: {}\nResolve relative references/scripts against this directory and load them only when needed.\n\n{instructions}", skill.folder
    ))
}
