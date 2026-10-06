use crate::db::Database;
use crate::skills::SkillIndex;
use serde_json::{json, Value};
use std::path::Path;

/// No instructions, hidden entries, arbitrary SQL, or renderer execution.
pub async fn run(db: &Database, args: &Value) -> anyhow::Result<String> {
    let db = db.clone();
    let args = args.clone();
    // A first-time large index build must not block an async gateway worker.
    tokio::task::spawn_blocking(move || run_in(&db, &args, Path::new("skills"))).await?
}

pub fn run_in(db: &Database, args: &Value, directory: &Path) -> anyhow::Result<String> {
    anyhow::ensure!(
        crate::db::tools::tool_enabled(db, "search_skills")?,
        "search_skills is disabled"
    );
    anyhow::ensure!(
        crate::db::tools::tool_enabled(db, "use_skill")?,
        "use_skill is disabled"
    );
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("search_skills requires a string query"))?;
    let limit = match args.get("limit") {
        None => 5,
        Some(value) => value
            .as_u64()
            .filter(|v| (1..=20).contains(v))
            .ok_or_else(|| anyhow::anyhow!("Skill result limit must be 1..20"))?
            as usize,
    };
    anyhow::ensure!(query.chars().count() <= 512, "Skill query is too long");
    if !directory.exists() {
        return Ok(json!({"skills":[],"has_more":false,"next_after":null}).to_string());
    }
    let mut index = SkillIndex::open(&db.data_dir(), directory)?;
    index.ensure_indexed()?;
    Ok(serde_json::to_string(&index.search(query, limit, false)?)?)
}
