//! Shared ownership for discovery, preflight and execution. Native ownership
//! is explicit; a prefix or persisted tool row cannot reserve plugin names.
use crate::{
    db::Database,
    gateway::llm::provider::ToolDefinition,
    plugins::{Plugin, PluginRegistry, PluginTool},
};
use std::{collections::HashSet, sync::LazyLock};

static BUILTINS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    crate::db::tools::get_default_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect()
});

pub(crate) enum ToolOwner<'a> {
    Builtin,
    Plugin {
        plugin: &'a Plugin,
        tool: &'a PluginTool,
    },
}

pub(crate) fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(name) && !crate::runtime::vm::is_vm_tool(name)
}

pub(crate) fn owner<'a>(plugins: &'a PluginRegistry, name: &str) -> anyhow::Result<ToolOwner<'a>> {
    let owners: Vec<_> = plugins
        .list()
        .into_iter()
        .filter(|plugin| plugin.enabled)
        .flat_map(|plugin| {
            plugin
                .tools
                .iter()
                .filter(move |tool| tool.name == name)
                .map(move |tool| (plugin, tool))
        })
        .collect();
    if is_builtin(name) {
        anyhow::ensure!(
            owners.is_empty(),
            "Tool owner_conflict: '{name}' is owned by a builtin adapter"
        );
        return Ok(ToolOwner::Builtin);
    }
    anyhow::ensure!(
        owners.len() <= 1,
        "Tool owner_conflict: '{name}' must have exactly one enabled owner"
    );
    let (plugin, tool) = owners
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Tool '{name}' is disabled or unavailable"))?;
    Ok(ToolOwner::Plugin { plugin, tool })
}

pub(crate) fn validate(plugins: &PluginRegistry) -> anyhow::Result<()> {
    let mut names: Vec<_> = plugins
        .enabled_tools()
        .into_iter()
        .map(|tool| tool.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        owner(plugins, name)?;
    }
    Ok(())
}

/// Recheck persisted flags at the effect boundary, even if the model's schema
/// snapshot was produced before an operator disabled the tool. Empty legacy
/// builtin configuration retains the native defaults for internal callers.
pub(crate) fn require_enabled(
    db: &Database,
    owner: &ToolOwner<'_>,
    name: &str,
) -> anyhow::Result<()> {
    let enabled = match owner {
        ToolOwner::Builtin => {
            super::packages::tool_package_enabled(&db.data_dir(), name)?
                && crate::db::tools::list(db)?
                    .into_iter()
                    .find(|tool| tool.name == name)
                    .map_or(true, |tool| tool.is_enabled)
        }
        ToolOwner::Plugin { .. } => crate::db::tools::plugin_tool_enabled(db, name)?,
    };
    anyhow::ensure!(enabled, "Tool '{name}' is disabled or unavailable");
    Ok(())
}

pub fn definitions(db: &Database, plugins: &PluginRegistry) -> anyhow::Result<Vec<ToolDefinition>> {
    validate(plugins)?;
    // Persisted rows describe flags/schemas, not arbitrary new native handlers.
    let packages = super::packages::load(&db.data_dir())?;
    let package_on = |name: &str| {
        super::packages::owner_of(name).map_or(true, |p| p.required || packages.get(p.id).copied().unwrap_or(true))
    };
    let mut tools: Vec<_> = crate::db::tools::to_tool_definitions(db)?
        .into_iter()
        .filter(|tool| is_builtin(&tool.function.name) && package_on(&tool.function.name))
        .collect();
    let flags = crate::db::tools::list_plugin_tools(db)?;
    let legacy = crate::db::tools::list(db)?;
    tools.extend(
        plugins
            .tool_definitions()
            .into_iter()
            .filter(|tool| flags.get(&tool.function.name).copied().unwrap_or_else(|| {
                if crate::runtime::vm::is_vm_tool(&tool.function.name) {
                    legacy.iter().find(|row| row.name == tool.function.name).is_some_and(|row| row.is_enabled)
                } else { true }
            })),
    );
    tools.sort_by(|a, b| a.function.name.cmp(&b.function.name));
    let mut names = HashSet::new();
    for tool in &mut tools {
        anyhow::ensure!(
            names.insert(tool.function.name.clone()),
            "Tool owner_conflict: duplicate catalog entry '{}'",
            tool.function.name
        );
        super::tool_output::augment_definition(tool);
    }
    Ok(tools)
}
