//! POML owns discovery policy; Rust only executes bounded index requests and
//! enforces manifest access flags. The rendered plan must be a JSON object.
use super::{index::SkillSummary, lookup_skill, SkillIndex};
use crate::db::Database;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryPlan {
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub queries: Vec<String>,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    5
}

impl DiscoveryPlan {
    pub fn parse(rendered: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            rendered.len() <= 16384,
            "Skill discovery plan exceeds 16 KiB"
        );
        let plan: Self = serde_json::from_str(rendered)
            .map_err(|e| anyhow::anyhow!("Skill discovery POML must render a JSON plan: {e}"))?;
        anyhow::ensure!(
            plan.instructions.chars().count() <= 4096,
            "Skill discovery instructions exceed 4096 characters"
        );
        anyhow::ensure!(
            (1..=20).contains(&plan.limit),
            "Skill discovery limit must be 1..20"
        );
        anyhow::ensure!(
            plan.queries.len() <= 4 && plan.names.len() <= 20,
            "Too many skill discovery requests"
        );
        for query in &plan.queries {
            anyhow::ensure!(
                query.chars().count() <= 512,
                "Skill discovery query is too long"
            );
        }
        for name in &plan.names {
            super::validate_name(name)?;
        }
        Ok(plan)
    }
}

pub async fn enrich(db: &Database, value: &mut Value, root: &Path) -> anyhow::Result<()> {
    let selected = value
        .pointer("/custom_data/skill_discovery_template")
        .and_then(Value::as_str);
    let name = selected.unwrap_or("discovery/skills");
    // Older/custom asset installations remain usable without silently replacing
    // their templates. Explicit selections, however, must resolve successfully.
    if selected.is_none() && !root.join("templates/discovery/skills.poml").exists() {
        return Ok(());
    }
    let path = crate::gateway::templates::resolve_template(&root.join("templates"), name)?;
    let rendered = crate::runtime::engine::render_strict(&path.to_string_lossy(), value).await?;
    let plan = DiscoveryPlan::parse(&rendered)?;
    value["skill_discovery_instructions"] = json!(plan.instructions);
    // The default empty plan touches neither the index nor the skill directory.
    if plan.names.is_empty() && plan.queries.is_empty() {
        value["skills"] = json!([]);
        return Ok(());
    }
    let db = db.clone();
    let skills_dir = root.join("skills");
    let summaries =
        tokio::task::spawn_blocking(move || candidates(&db, &skills_dir, &plan)).await??;
    value["skills"] = json!(summaries);
    Ok(())
}

fn candidates(
    db: &Database,
    skills_dir: &Path,
    plan: &DiscoveryPlan,
) -> anyhow::Result<Vec<SkillSummary>> {
    let mut summaries: Vec<SkillSummary> = Vec::new();
    for name in &plan.names {
        if summaries.len() == plan.limit {
            break;
        }
        if let Ok(skill) = lookup_skill(db, &skills_dir, name) {
            if !skill.skill_hidden && !summaries.iter().any(|s| s.name == skill.name) {
                summaries.push(skill.into());
            }
        }
    }
    if !plan.queries.is_empty() && summaries.len() < plan.limit && skills_dir.is_dir() {
        let mut index = SkillIndex::open(&db.data_dir(), &skills_dir)?;
        index.ensure_indexed()?;
        for query in &plan.queries {
            if summaries.len() == plan.limit {
                break;
            }
            for skill in index.search(query, plan.limit, false)?.skills {
                if summaries.len() == plan.limit {
                    break;
                }
                if !summaries.iter().any(|s| s.name == skill.name) {
                    summaries.push(skill);
                }
            }
        }
    }
    Ok(summaries)
}
