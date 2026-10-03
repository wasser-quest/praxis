//! Bounded, task-local tool discovery. Searching never enables disabled tools
//! or executes a plugin. Provider schemas and POML use the same selection.
use crate::{db::Database, gateway::{llm::provider::ToolDefinition, task_control}, plugins::PluginRegistry};
use serde_json::{json, Value};

pub const CORE: &[&str] = &[
    "search_tools", "search_skills", "use_skill", "get_context", "set_context",
    "read_file", "write_file", "edit_file", "execute_terminal", "read_tool_result",
    "agent_complete", "agent_feedback", "agent_next", "agent_back",
];

pub fn catalog(db: &Database, plugins: &PluginRegistry) -> anyhow::Result<Vec<ToolDefinition>> {
    super::catalog::definitions(db, plugins)
}

pub fn definitions(db: &Database, plugins: &PluginRegistry, user: &str) -> anyhow::Result<Vec<ToolDefinition>> {
    let selected = task_control::selected_tools(user);
    Ok(catalog(db, plugins)?.into_iter().filter(|t| CORE.contains(&t.function.name.as_str()) || selected.contains(&t.function.name)).collect())
}

pub fn enabled(db: &Database, plugins: &PluginRegistry, name: &str) -> bool {
    catalog(db, plugins).is_ok_and(|tools| tools.iter().any(|t| t.function.name == name))
}

pub fn search(db: &Database, plugins: &PluginRegistry, user: &str, args: &Value) -> anyhow::Result<String> {
    anyhow::ensure!(crate::db::tools::get(db, "search_tools")?.is_enabled, "search_tools is disabled");
    let query = args["query"].as_str().filter(|q| !q.trim().is_empty() && q.chars().count() <= 256)
        .ok_or_else(|| anyhow::anyhow!("query must contain 1..256 characters"))?.trim().to_lowercase();
    let limit = match args.get("limit") {
        None => 5,
        Some(v) => v.as_u64().filter(|n| (1..=8).contains(n)).ok_or_else(|| anyhow::anyhow!("limit must be 1..8"))? as usize,
    };
    let replace = match args.get("replace") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| anyhow::anyhow!("replace must be boolean"))?,
    };
    let terms: Vec<_> = query.split_whitespace().collect();
    let mut matches: Vec<_> = catalog(db, plugins)?.into_iter().filter_map(|tool| {
        let name = tool.function.name.to_lowercase();
        let description = tool.function.description.to_lowercase();
        let score = if name == query { 1000 } else {
            terms.iter().map(|t| if name.contains(t) { 10 } else if description.contains(t) { 1 } else { 0 }).sum()
        };
        (score > 0).then_some((score, tool))
    }).collect();
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.function.name.cmp(&b.1.function.name)));
    let has_more = matches.len() > limit;
    let tools: Vec<_> = matches.into_iter().take(limit).map(|(_, t)| t).collect();
    task_control::select_tools(user, tools.iter().filter(|t| !CORE.contains(&t.function.name.as_str())).map(|t| t.function.name.clone()).collect(), replace)?;
    Ok(json!({
        // Full schemas are already attached to the very next request by
        // definitions(). Duplicating them here bloats every subsequent turn.
        "tools": tools.iter().map(|t| json!({"name":t.function.name, "description":t.function.description})).collect::<Vec<_>>(),
        "has_more": has_more,
        "scope": "Activated for the next model turn in this task; full parameter schemas are in that request's tools. Disabled tools and permissions remain enforced. Refine the query rather than enumerate the catalog.",
    }).to_string())
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
