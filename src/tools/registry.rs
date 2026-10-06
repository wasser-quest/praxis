//! Central tool registry with metadata for state-based filtering.
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Tools that execute actions with side effects
    Action,
    /// Discovery tools (search_tools, search_skills)
    Discovery,
    /// Skill loading
    SkillLoader,
    /// Agent loop control
    AgentControl,
    /// Memory operations
    Memory,
    /// Context operations
    Context,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMeta {
    pub name: &'static str,
    pub description: &'static str,
    pub category: ToolCategory,
    pub params_schema: serde_json::Value,
    pub default_enabled: bool,
}

/// Static registry initialized once at startup
static TOOL_REGISTRY: Lazy<Vec<ToolMeta>> = Lazy::new(build_registry);

fn canonical_tool(name: &str) -> Option<&'static crate::db::tools::Tool> {
    static DEFAULTS: Lazy<HashMap<String, crate::db::tools::Tool>> = Lazy::new(|| {
        crate::db::tools::get_default_tools().into_iter().map(|tool| (tool.name.clone(), tool)).collect()
    });
    // Bundled package declarations are the one source for their contracts.
    DEFAULTS.get(name).or_else(|| super::packages::bundled_tool(name))
}

fn build_registry() -> Vec<ToolMeta> {
    let mut tools = vec![
        ToolMeta {
            name: "execute_decision",
            description: "Execute one compact workflow-mapped decision through verified capabilities",
            category: ToolCategory::Action,
            params_schema: crate::gateway::decision_ir::definition().parameters,
            default_enabled: true,
        },
        ToolMeta {
            name: "apply_patch",
            description: "Apply scoped file edits with expected hashes, trusted checks and rollback",
            category: ToolCategory::Action,
            params_schema: crate::tools::apply_patch::definition().parameters,
            default_enabled: true,
        },
        ToolMeta {
            name: "inspect_file",
            description: "Inspect a scoped file and get its content hash for transactional edits",
            category: ToolCategory::Action,
            params_schema: crate::tools::apply_patch::inspect_definition().parameters,
            default_enabled: true,
        },
        ToolMeta {
            name: "run_check",
            description: "Run a named workflow check and record trusted execution evidence",
            category: ToolCategory::Action,
            params_schema: crate::gateway::action_contracts::definition().parameters,
            default_enabled: true,
        },
        // Action tools
        ToolMeta {
            name: "execute_terminal",
            description: "Run shell command on host (or VM if VM_MODE=vm)",
            category: ToolCategory::Action,
            params_schema: terminal_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "read_file",
            description: "Read file contents",
            category: ToolCategory::Action,
            params_schema: read_file_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "write_file",
            description: "Create/overwrite file",
            category: ToolCategory::Action,
            params_schema: write_file_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "edit_file",
            description: "Find/replace in file",
            category: ToolCategory::Action,
            params_schema: edit_file_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "discord_send_message",
            description: "Send Discord message to a channel",
            category: ToolCategory::Action,
            params_schema: discord_send_message_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "discord_send_embed",
            description: "Send Discord rich embed",
            category: ToolCategory::Action,
            params_schema: discord_send_embed_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "discord_upload_file",
            description: "Upload file to Discord channel",
            category: ToolCategory::Action,
            params_schema: discord_upload_file_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_start",
            description: "Start/create a VM",
            category: ToolCategory::Action,
            params_schema: vm_start_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_stop",
            description: "Stop a VM",
            category: ToolCategory::Action,
            params_schema: vm_stop_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_shell",
            description: "Execute shell command in VM",
            category: ToolCategory::Action,
            params_schema: vm_shell_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_keys",
            description: "Send keyboard input to VM",
            category: ToolCategory::Action,
            params_schema: vm_keys_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_mouse",
            description: "Mouse control in VM",
            category: ToolCategory::Action,
            params_schema: vm_mouse_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_screenshot",
            description: "Capture VM display as base64 image",
            category: ToolCategory::Action,
            params_schema: vm_screenshot_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_file_transfer",
            description: "Transfer files to/from VM",
            category: ToolCategory::Action,
            params_schema: vm_file_transfer_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_snapshot",
            description: "Create VM snapshot",
            category: ToolCategory::Action,
            params_schema: vm_snapshot_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "vm_shared_folder",
            description: "Add shared host<->VM folder",
            category: ToolCategory::Action,
            params_schema: vm_shared_folder_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "understand_image",
            description: "Analyze image content",
            category: ToolCategory::Action,
            params_schema: understand_image_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "update_template",
            description: "Strictly validate and save a POML template",
            category: ToolCategory::Action,
            params_schema: update_template_schema(),
            default_enabled: true,
        },
        // Discovery tools
        ToolMeta {
            name: "search_tools",
            description: "Find enabled tools by keyword. Returns name and description only.",
            category: ToolCategory::Discovery,
            params_schema: search_tools_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "search_skills",
            description: "Find skills by keyword. Returns name, description, required_parameters.",
            category: ToolCategory::Discovery,
            params_schema: search_skills_schema(),
            default_enabled: true,
        },
        // Skill Loader
        ToolMeta {
            name: "use_skill",
            description: "Load one registered skill's instructions on demand with explicit parameters",
            category: ToolCategory::SkillLoader,
            params_schema: use_skill_schema(),
            default_enabled: true,
        },
        // Agent Control
        ToolMeta {
            name: "agent_complete",
            description: "Mark current task as done",
            category: ToolCategory::AgentControl,
            params_schema: agent_complete_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "agent_next",
            description: "Advance to next workflow step",
            category: ToolCategory::AgentControl,
            params_schema: agent_next_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "agent_back",
            description: "Return to the previously visited graph node",
            category: ToolCategory::AgentControl,
            params_schema: canonical_tool("agent_back").map(|tool| tool.parameters.clone()).unwrap_or_else(|| json!({"type":"object","properties":{}})),
            default_enabled: true,
        },
        ToolMeta {
            name: "agent_feedback",
            description: "Send progress feedback message",
            category: ToolCategory::AgentControl,
            params_schema: agent_feedback_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "agent_set_path",
            description: "Set working directory for workflow",
            category: ToolCategory::AgentControl,
            params_schema: agent_set_path_schema(),
            default_enabled: true,
        },
        // Context
        ToolMeta {
            name: "read_tool_result",
            description: "Recall saved tool output without re-execution",
            category: ToolCategory::Context,
            params_schema: read_tool_result_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "get_context",
            description: "Read current context variable",
            category: ToolCategory::Context,
            params_schema: get_context_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "set_context",
            description: "Set context variable",
            category: ToolCategory::Context,
            params_schema: set_context_schema(),
            default_enabled: true,
        },
        // Memory
        ToolMeta {
            name: "memory_get",
            description: "Get memory value from profile",
            category: ToolCategory::Memory,
            params_schema: memory_get_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "memory_set",
            description: "Set memory value in profile",
            category: ToolCategory::Memory,
            params_schema: memory_set_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "learn_fact",
            description: "Store a fact in memory",
            category: ToolCategory::Memory,
            params_schema: learn_fact_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "learn_preference",
            description: "Store user preference",
            category: ToolCategory::Memory,
            params_schema: learn_preference_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "learn_topic",
            description: "Track conversation topic",
            category: ToolCategory::Memory,
            params_schema: learn_topic_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "memory_profile_create",
            description: "Create new memory profile",
            category: ToolCategory::Memory,
            params_schema: memory_profile_create_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "memory_profile_load",
            description: "Load memory profile",
            category: ToolCategory::Memory,
            params_schema: memory_profile_load_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "memory_profile_list",
            description: "List available memory profiles",
            category: ToolCategory::Memory,
            params_schema: memory_profile_list_schema(),
            default_enabled: true,
        },
        // Cron
        ToolMeta {
            name: "cron_add",
            description: "Create scheduled cron job",
            category: ToolCategory::Action,
            params_schema: cron_add_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "cron_delete",
            description: "Delete cron job",
            category: ToolCategory::Action,
            params_schema: cron_delete_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "cron_list",
            description: "List cron jobs",
            category: ToolCategory::Action,
            params_schema: cron_list_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "cron_toggle",
            description: "Enable/disable cron job",
            category: ToolCategory::Action,
            params_schema: cron_toggle_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "cron_run",
            description: "Manually trigger cron job",
            category: ToolCategory::Action,
            params_schema: cron_run_schema(),
            default_enabled: true,
        },
        ToolMeta {
            name: "ask_questions",
            description: "Ask interactive questions (Discord/Web)",
            category: ToolCategory::Action,
            params_schema: ask_questions_schema(),
            default_enabled: true,
        },
    ];
    // Category metadata is local, but parameter contracts have one source.
    // Otherwise discovery advertises `path`/active-profile memory while this
    // registry silently requires `image_path`/a nonexistent `profile` argument.
    for tool in &mut tools {
        if let Some(canonical) = canonical_tool(tool.name) {
            tool.params_schema = canonical.parameters.clone();
            tool.default_enabled = canonical.is_enabled;
        }
    }
    tools
}

/// Access the static registry
pub fn all_tool_meta() -> &'static [ToolMeta] {
    &TOOL_REGISTRY
}

/// Expand an activation list into a de-duplicated tool list. Each entry is
/// either a group name from `definitions` (expanded recursively; nested
/// groups allowed, cycles tolerated) or a plain tool name (built-in or plugin).
/// There are no built-in groups: every group is defined by a workflow
/// `[tool_groups]` section or by `settings.tool_group_definitions`.
pub fn expand_tool_groups(entries: &[String], definitions: &HashMap<String, Vec<String>>) -> Vec<String> {
    fn walk(name: &str, definitions: &HashMap<String, Vec<String>>, visiting: &mut Vec<String>, out: &mut Vec<String>) {
        if visiting.iter().any(|v| v == name) {
            return; // cycle guard
        }
        match definitions.get(name) {
            Some(members) => {
                visiting.push(name.to_string());
                for member in members {
                    walk(member.trim(), definitions, visiting, out);
                }
                visiting.pop();
            }
            None => {
                if !name.is_empty() && !out.iter().any(|t| t == name) {
                    out.push(name.to_string());
                }
            }
        }
    }
    let mut out = Vec::new();
    for entry in entries {
        walk(entry.trim(), definitions, &mut Vec::new(), &mut out);
    }
    out
}

/// The complete activation list for a context: `activated_tools` (tools and
/// groups mixed) plus the legacy `tool_groups` and `full_tool_schemas` lists.
pub fn activated_tool_names(ctx_settings: &crate::db::contexts::ContextSettings) -> Vec<String> {
    let mut entries = ctx_settings.activated_tools.clone();
    entries.extend(ctx_settings.tool_groups.clone().unwrap_or_default());
    entries.extend(ctx_settings.full_tool_schemas.clone());
    expand_tool_groups(&entries, &ctx_settings.tool_group_definitions)
}

/// Undefined groups resolve as tool names for late-installed plugins. Keep
/// that compatibility, but expose a diagnostic instead of silently losing tools.
fn unavailable_activated_tools(settings: &crate::db::contexts::ContextSettings, plugins: Option<&[crate::gateway::llm::provider::ToolDefinition]>) -> Vec<String> {
    activated_tool_names(settings).into_iter().filter(|name| {
        canonical_tool(name).is_none() && get_tool_meta(name).is_none()
            && !plugins.unwrap_or_default().iter().any(|tool| tool.function.name == *name)
    }).collect()
}

fn has_tool_allowlist(settings: &crate::db::contexts::ContextSettings) -> bool {
    !settings.activated_tools.is_empty() || !settings.full_tool_schemas.is_empty()
        || settings.tool_groups.as_ref().is_some_and(|groups| !groups.is_empty())
        || !settings.full_tool_categories.is_empty()
}

/// Get tool metadata by name
pub fn get_tool_meta(name: &str) -> Option<&'static ToolMeta> {
    TOOL_REGISTRY.iter().find(|m| m.name == name)
}

/// Get all tool names for a category
pub fn tool_names_by_category(category: ToolCategory) -> Vec<&'static str> {
    TOOL_REGISTRY
        .iter()
        .filter(|m| m.category == category)
        .map(|m| m.name)
        .collect()
}

/// Get tool names for multiple categories
pub fn tool_names_by_categories(categories: &[ToolCategory]) -> Vec<&'static str> {
    TOOL_REGISTRY
        .iter()
        .filter(|m| categories.contains(&m.category))
        .map(|m| m.name)
        .collect()
}

/// Filter tools for LLM request based on context settings
pub fn filter_tools_for_request(
    allowed_names: Option<&[String]>,
    allowed_categories: Option<&[ToolCategory]>,
    full_schema_names: Option<&[String]>,
    full_schema_categories: Option<&[ToolCategory]>,
    discovery_mode: ToolDiscoveryMode,
) -> Vec<crate::gateway::llm::provider::ToolDefinition> {
    use crate::gateway::llm::provider::{FunctionDefinition, ToolDefinition};

    let full_names: HashSet<&str> = full_schema_names.map(|v| v.iter().map(|s| s.as_str()).collect()).unwrap_or_default();
    let full_cats: HashSet<ToolCategory> = full_schema_categories.map(|v| v.iter().cloned().collect()).unwrap_or_default();

    TOOL_REGISTRY
        .iter()
        .filter(|meta| {
            // Filter by allowed names (if specified)
            if let Some(names) = allowed_names {
                if !names.iter().any(|n| n == meta.name) {
                    return false;
                }
            }
            // Filter by allowed categories (if specified)
            if let Some(cats) = allowed_categories {
                if !cats.contains(&meta.category) {
                    return false;
                }
            }
            true
        })
        .filter_map(|meta| {
            // Determine if this tool gets full schema or empty
            let has_full_schema = full_names.contains(meta.name) || full_cats.contains(&meta.category);
            // None: only explicitly activated tools are offered at all.
            if discovery_mode == ToolDiscoveryMode::None && !has_full_schema {
                return None;
            }

            let params = if has_full_schema {
                meta.params_schema.clone()
            } else if discovery_mode == ToolDiscoveryMode::Full {
                meta.params_schema.clone()
            } else {
                // DescriptionOnly or None: empty schema for search_tools to work
                json!({"type": "object", "properties": {}})
            };

            let mut tool = ToolDefinition {
                tool_type: "function".into(),
                function: FunctionDefinition {
                    name: meta.name.into(),
                    description: meta.description.into(),
                    parameters: params,
                },
            };
            if has_full_schema || discovery_mode == ToolDiscoveryMode::Full {
                super::tool_output::augment_definition(&mut tool);
            }
            Some(tool)
        })
        .collect()
}

/// Parse comma-separated category strings into ToolCategory vec
pub fn parse_tool_categories(cats: &[String]) -> Vec<ToolCategory> {
    cats.iter().filter_map(|s| match s.as_str() {
        "Action" => Some(ToolCategory::Action),
        "Discovery" => Some(ToolCategory::Discovery),
        "SkillLoader" => Some(ToolCategory::SkillLoader),
        "AgentControl" => Some(ToolCategory::AgentControl),
        "Memory" => Some(ToolCategory::Memory),
        "Context" => Some(ToolCategory::Context),
        _ => None,
    }).collect()
}

/// Tool definitions for one user's request: the state's activated tools plus
/// whatever `search_tools` selected for the current task (task-local, bounded).
pub fn build_tool_definitions_for_user(
    ctx_settings: &crate::db::contexts::ContextSettings,
    plugin_tools: Option<&[crate::gateway::llm::provider::ToolDefinition]>,
    db: Option<&crate::db::Database>,
    user_id: &str,
) -> Vec<crate::gateway::llm::provider::ToolDefinition> {
    let mut selected: Vec<_> = crate::gateway::task_control::selected_tools(user_id).into_iter().collect();
    selected.sort();
    build_tool_definitions_with_selection(ctx_settings, plugin_tools, db, &selected)
}

/// Build filtered tool definitions from context settings, merging with plugin tools
pub fn build_tool_definitions(
    ctx_settings: &crate::db::contexts::ContextSettings,
    plugin_tools: Option<&[crate::gateway::llm::provider::ToolDefinition]>, // dynamic plugin tools
    db: Option<&crate::db::Database>, // for checking plugin tool enabled state
) -> Vec<crate::gateway::llm::provider::ToolDefinition> {
    build_tool_definitions_with_selection(ctx_settings, plugin_tools, db, &[])
}

fn build_tool_definitions_with_selection(
    ctx_settings: &crate::db::contexts::ContextSettings,
    plugin_tools: Option<&[crate::gateway::llm::provider::ToolDefinition]>,
    db: Option<&crate::db::Database>,
    selected: &[String],
) -> Vec<crate::gateway::llm::provider::ToolDefinition> {
    let unavailable = unavailable_activated_tools(ctx_settings, plugin_tools);
    if !unavailable.is_empty() {
        let names: Vec<_> = unavailable.iter().take(32).map(|s| s.chars().take(80).collect::<String>()).collect();
        tracing::warn!(tools = ?names, "Activated tools are unknown or unavailable; check tool-group definitions and installed/enabled plugins");
    }
    // Discovery adds full schemas and extends an existing state allow-list,
    // but must not create a new allow-list when the state has none. Selected
    // entries are actual tool names, not groups to expand a second time.
    let mut full_schemas = activated_tool_names(ctx_settings);
    for name in selected {
        if !full_schemas.contains(name) { full_schemas.push(name.clone()); }
    }
    let full_names = if full_schemas.is_empty() {
        None
    } else {
        Some(full_schemas.as_slice())
    };
    let full_cats = if ctx_settings.full_tool_categories.is_empty() {
        None
    } else {
        Some(parse_tool_categories(&ctx_settings.full_tool_categories))
    };
    let discovery_mode = match ctx_settings.tool_discovery_mode.as_str() {
        "Full" => ToolDiscoveryMode::Full,
        "None" => ToolDiscoveryMode::None,
        _ => ToolDiscoveryMode::DescriptionOnly,
    };
    // Determine allowed tools from state config
    // If state explicitly configures tools, use as allow-list; otherwise allow all
    let has_explicit_config = has_tool_allowlist(ctx_settings);
    if has_explicit_config && full_schemas.is_empty() && full_cats.is_none() {
        tracing::warn!("Tool activation resolved to no tools; check empty/cyclic groups (allow-list remains closed)");
    }
    
    // A tool is allowed when named (directly, via a group or discovery) OR its
    // category is listed. An explicitly configured empty resolution stays closed.
    let allowed_names = if has_explicit_config {
        Some(full_schemas.as_slice())
    } else {
        None
    };
    let allowed_cats = if has_explicit_config { full_cats.clone() } else { None };

    let mut tools = if has_explicit_config && allowed_names.is_some() && allowed_cats.is_some() {
        // Union of both selectors (filter_tools_for_request intersects them).
        let mut by_name = filter_tools_for_request(allowed_names, None, full_names, full_cats.as_deref(), discovery_mode);
        for tool in filter_tools_for_request(None, allowed_cats.as_deref(), full_names, full_cats.as_deref(), discovery_mode) {
            if !by_name.iter().any(|t| t.function.name == tool.function.name) {
                by_name.push(tool);
            }
        }
        by_name
    } else {
        filter_tools_for_request(allowed_names, allowed_cats.as_deref(), full_names, full_cats.as_deref(), discovery_mode)
    };
    let package_switches = match db.map(|db| super::packages::load(&db.data_dir())).transpose() {
        Ok(switches) => switches.unwrap_or_default(),
        Err(_) => {
            tracing::warn!("Cannot load tool package state; no tools offered");
            return Vec::new();
        }
    };
    let native_on = |name: &str| {
        super::packages::native_available(name)
            && super::packages::owner_of(name).is_none_or(|p| p.required || package_switches.get(p.id).copied().unwrap_or(true))
    };
    tools.retain(|tool| native_on(&tool.function.name));
    
    // Use the same enabled contracts as discovery, including built-ins that
    // have no category metadata (background/delegation tools, etc.). Never let
    // the static table re-enable a tool disabled in the dashboard.
    let native = match db.map(crate::db::tools::list).transpose() {
        Ok(native) => native.unwrap_or_default(),
        Err(_) => {
            tracing::warn!("Cannot load enabled tool contracts; no tools offered");
            return Vec::new();
        }
    };
    if db.is_some() {
        tools.retain(|tool| native.iter().any(|n| n.name == tool.function.name && n.is_enabled));
        for tool in native.iter().filter(|tool| tool.is_enabled && !crate::runtime::vm::is_vm_tool(&tool.name) && native_on(&tool.name)) {
            let category = get_tool_meta(&tool.name).map(|meta| meta.category).unwrap_or(ToolCategory::Action);
            let activated = full_schemas.contains(&tool.name) || full_cats.as_ref().is_some_and(|cats| cats.contains(&category));
            if (has_explicit_config || discovery_mode == ToolDiscoveryMode::None) && !activated { continue; }
            let full = activated || discovery_mode == ToolDiscoveryMode::Full;
            let mut definition = crate::gateway::llm::provider::ToolDefinition {
                tool_type: "function".into(),
                function: crate::gateway::llm::provider::FunctionDefinition {
                    name: tool.name.clone(), description: tool.description.clone().unwrap_or_default(),
                    parameters: if full { tool.parameters.clone() } else { json!({"type":"object","properties":{}}) },
                },
            };
            if full { super::tool_output::augment_definition(&mut definition); }
            if let Some(existing) = tools.iter_mut().find(|t| t.function.name == tool.name) {
                *existing = definition;
            } else {
                tools.push(definition);
            }
        }
    }

    // VM schemas are provided only by the explicitly bound feature package.
    tools.retain(|tool| !crate::runtime::vm::is_vm_tool(&tool.function.name));

    // Plugin registration validates replacement ownership. Use the installed
    // owner's schema rather than a stale native schema for replaceable names.
    // With no DB/native row, keep the historical closed static fallback.
    let replacements: HashSet<&str> = plugin_tools.into_iter().flatten()
        .filter(|tool| super::packages::owner_of(&tool.function.name)
            .is_some_and(|package| super::packages::replaceable(package.id)))
        .filter(|tool| !super::packages::native_available(&tool.function.name)
            || native.iter().any(|row| row.name == tool.function.name))
        .map(|tool| tool.function.name.as_str()).collect();
    tools.retain(|tool| !replacements.contains(tool.function.name.as_str()));

    // Merge package-declared tools - also filter them by state settings. A
    // native builtin keeps its own definition; a name owned by a package (an
    // installed replacement or a bundled package like `runtime_control`) takes
    // the owner's declaration, never a stale static schema.
    if let Some(plugins) = plugin_tools {
        for plugin_tool in plugins {
            if super::catalog::is_builtin(&plugin_tool.function.name) && super::packages::native_available(&plugin_tool.function.name) && !replacements.contains(plugin_tool.function.name.as_str()) && !crate::runtime::vm::is_vm_tool(&plugin_tool.function.name) {
                continue;
            }
            // Check if plugin tool is enabled in DB (per-tool enable/disable)
            // Plugin tools default to enabled unless explicitly disabled in plugin_tools.json
            let plugin_enabled = db
                .map(|db| crate::db::tools::get_plugin_tool_enabled(db, &plugin_tool.function.name))
                .unwrap_or(true);
            if !plugin_enabled {
                continue; // Skip disabled plugin tools
            }
            // When an extracted implementation is supplied by a package,
            // honor legacy per-tool flags unless explicitly overridden.
            if let Some(db) = db {
                if replacements.contains(plugin_tool.function.name.as_str())
                    && !crate::db::tools::list_plugin_tools(db).ok().is_some_and(|flags| flags.contains_key(&plugin_tool.function.name))
                    && native.iter().any(|tool| tool.name == plugin_tool.function.name && !tool.is_enabled) {
                    continue;
                }
            }
            // Plugin tools obey the same allow-list as built-ins: when the state
            // configures tools explicitly, a plugin tool must be named directly in
            // full_tool_schemas, via a tool group, or by its category; otherwise it
            // is not sent.
            let category_listed = get_tool_meta(&plugin_tool.function.name)
                .is_some_and(|meta| full_cats.as_ref().is_some_and(|cats| cats.contains(&meta.category)));
            let listed = full_schemas.iter().any(|s| s == &plugin_tool.function.name)
                || category_listed;
            if has_explicit_config && !listed {
                continue;
            }
            if discovery_mode == ToolDiscoveryMode::None && !listed {
                continue;
            }
            let params = if listed || discovery_mode == ToolDiscoveryMode::Full {
                plugin_tool.function.parameters.clone()
            } else {
                json!({"type": "object", "properties": {}})
            };
            let mut definition = crate::gateway::llm::provider::ToolDefinition {
                tool_type: "function".into(),
                function: crate::gateway::llm::provider::FunctionDefinition {
                    name: plugin_tool.function.name.clone(),
                    description: plugin_tool.function.description.clone(),
                    parameters: params,
                },
            };
            if listed || discovery_mode == ToolDiscoveryMode::Full {
                super::tool_output::augment_definition(&mut definition);
            }
            // The owner's declaration replaces any stale entry for this name.
            tools.retain(|t| t.function.name != plugin_tool.function.name);
            tools.push(definition);
        }
    }
    tools
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDiscoveryMode {
    /// Full parameters for all allowed tools
    Full,
    /// Only name + description in schema (minimal payload)
    DescriptionOnly,
    /// Only explicitly activated/discovered tools, with full schemas
    None,
}

impl Default for ToolDiscoveryMode {
    fn default() -> Self {
        ToolDiscoveryMode::DescriptionOnly
    }
}

// Schema functions for each tool
fn terminal_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "description": "Shell command to execute"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["command"]
    })
}

fn read_file_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "File path to read"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["path"]
    })
}

fn write_file_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "File path to write"},
            "content": {"type": "string", "description": "Content to write"},
            "expected_absent": {"type": "boolean", "description": "Require that the file does not exist (checked write)"},
            "expected_sha256": {"type": "string", "description": "Require this lowercase SHA-256 before replacing"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["path", "content"]
    })
}

fn edit_file_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "File path to edit"},
            "old_string": {"type": "string", "description": "Text to find and replace"},
            "new_string": {"type": "string", "description": "Replacement text"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["path", "old_string", "new_string"]
    })
}

fn discord_send_message_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "channel_id": {"type": "string", "description": "Discord channel ID"},
            "content": {"type": "string", "description": "Message content"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["channel_id", "content"]
    })
}

fn discord_send_embed_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "channel_id": {"type": "string", "description": "Discord channel ID"},
            "title": {"type": "string"},
            "description": {"type": "string"},
            "color": {"type": "integer"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["channel_id"]
    })
}

fn discord_upload_file_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "channel_id": {"type": "string", "description": "Discord channel ID"},
            "file_path": {"type": "string", "description": "Local file path to upload"},
            "filename": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["channel_id", "file_path"]
    })
}

fn vm_start_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "cpu_cores": {"type": "integer"},
            "ram_mb": {"type": "integer"},
            "disk_size": {"type": "string"},
            "arch": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn vm_stop_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn vm_shell_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "command": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name", "command"]
    })
}

fn vm_keys_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "keys": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name", "keys"]
    })
}

fn vm_mouse_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "action": {"type": "string"},
            "x": {"type": "integer"},
            "y": {"type": "integer"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name", "action"]
    })
}

fn vm_screenshot_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn vm_file_transfer_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "direction": {"type": "string", "enum": ["to_vm", "from_vm"]},
            "src": {"type": "string"},
            "dst": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name", "direction", "src", "dst"]
    })
}

fn vm_snapshot_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "snapshot_name": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn vm_shared_folder_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "host_path": {"type": "string"},
            "vm_path": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn understand_image_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "image_path": {"type": "string"},
            "prompt": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["image_path", "prompt"]
    })
}

fn update_template_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "content": {"type": "string"},
            "user_id": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name", "content"]
    })
}

fn search_tools_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Keywords to search tools"},
            "limit": {"type": "integer", "default": 5, "minimum": 1, "maximum": 20},
            "replace": {"type": "boolean", "default": false},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["query"]
    })
}

fn search_skills_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Keywords to search skills"},
            "limit": {"type": "integer", "default": 5, "minimum": 1, "maximum": 20},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["query"]
    })
}

fn use_skill_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string", "description": "Exact registered skill name", "maxLength": 64},
            "parameters": {"type": "object", "description": "Required non-empty string inputs advertised by discovery"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name", "parameters"]
    })
}

fn agent_complete_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn agent_next_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "edge": {"type": "integer", "minimum": 0, "description": "Declared outgoing edge index in graph mode"},
            "from_state": {"type": "string", "description": "Reject navigation if the current state has changed"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn agent_feedback_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "message": {"type": "string", "description": "Feedback message"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["message"]
    })
}

fn agent_set_path_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Working directory path"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["path"]
    })
}

fn read_tool_result_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "output_id": {"type": "string", "description": "out_... reference from a previous tool response"},
            "view": {"type": "string", "enum": ["full", "lines", "tail", "search"], "default": "full"},
            "start_line": {"type": "integer", "default": 1, "minimum": 1},
            "line_count": {"type": "integer", "default": 100, "minimum": 1, "maximum": 2000},
            "max_chars": {"type": "integer", "minimum": 1, "maximum": 32000},
            "query": {"type": "string", "minLength": 1, "maxLength": 256},
            "json_pointer": {"type": "string", "maxLength": 1024},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["output_id"]
    })
}

fn get_context_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "key": {"type": "string", "description": "Context key to read"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["key"]
    })
}

fn set_context_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "key": {"type": "string", "description": "Context key to set"},
            "value": {"description": "Value to set"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["key", "value"]
    })
}

fn memory_get_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "profile": {"type": "string", "description": "Memory profile name"},
            "key": {"type": "string", "description": "Memory key"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["profile", "key"]
    })
}

fn memory_set_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "profile": {"type": "string", "description": "Memory profile name"},
            "key": {"type": "string", "description": "Memory key"},
            "value": {"description": "Value to set"},
            "expected_profile": {"type": "string", "description": "Expected profile for conflict detection"},
            "expected_value": {"description": "Expected current value for conflict detection"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["profile", "key", "value"]
    })
}

fn learn_fact_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "fact": {"type": "string", "description": "Fact to remember"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["fact"]
    })
}

fn learn_preference_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "key": {"type": "string", "description": "Preference key"},
            "value": {"description": "Preference value"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["key"]
    })
}

fn learn_topic_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "topic": {"type": "string", "description": "Topic to track"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["topic"]
    })
}

fn memory_profile_create_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string", "description": "Profile name"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name"]
    })
}

fn memory_profile_load_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string", "description": "Profile name"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name"]
    })
}

fn memory_profile_list_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn cron_add_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "schedule": {"type": "string"},
            "prompt": {"type": "string"},
            "template": {"type": "string"},
            "timezone": {"type": "string"},
            "description": {"type": "string"},
            "enabled": {"type": "boolean"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["name", "schedule", "prompt"]
    })
}

fn cron_delete_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "job_id": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["job_id"]
    })
}

fn cron_list_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        }
    })
}

fn cron_toggle_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "job_id": {"type": "string"},
            "enabled": {"type": "boolean"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["job_id", "enabled"]
    })
}

fn cron_run_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "job_id": {"type": "string"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["job_id"]
    })
}

fn ask_questions_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "label": {"type": "string"},
                        "question": {"type": "string"},
                        "suggestions": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["label", "question"]
                }
            },
            "channel_id": {"type": "string"},
            "timeout_secs": {"type": "integer"},
            "_output": {"$ref": "#/components/schemas/OutputSelection"}
        },
        "required": ["questions"]
    })
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::db::contexts::ContextSettings;
    use crate::gateway::llm::provider::{FunctionDefinition, ToolDefinition};

    fn plugin(name: &str) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: name.into(),
                description: format!("{name} plugin"),
                parameters: json!({"type":"object","properties":{"query":{"type":"string"}}}),
            },
        }
    }

    fn names(tools: &[ToolDefinition]) -> Vec<&str> {
        tools.iter().map(|t| t.function.name.as_str()).collect()
    }

    fn settings_with_groups() -> ContextSettings {
        let mut settings = ContextSettings::default();
        settings.tool_group_definitions.insert("core_files".into(), vec!["read_file".into(), "write_file".into(), "edit_file".into()]);
        settings.tool_group_definitions.insert("agent_basic".into(), vec!["agent_complete".into(), "agent_next".into()]);
        settings.tool_discovery_mode = "DescriptionOnly".into();
        settings
    }

    #[test]
    fn activation_diagnostics_distinguish_known_tools_and_unknown_groups() {
        let mut settings = crate::db::contexts::ContextSettings::default();
        settings.activated_tools = vec!["read_file".into(), "missing_group".into(), "late_plugin".into()];
        assert_eq!(unavailable_activated_tools(&settings, None), vec!["missing_group", "late_plugin"]);
        let plugins = vec![crate::gateway::llm::provider::ToolDefinition { tool_type: "function".into(), function: crate::gateway::llm::provider::FunctionDefinition {
            name: "late_plugin".into(), description: String::new(), parameters: serde_json::json!({}) } }];
        assert_eq!(unavailable_activated_tools(&settings, Some(&plugins)), vec!["missing_group"]);
    }

    #[test]
    fn activated_tools_mix_groups_and_single_tools_and_gate_plugins() {
        let mut settings = settings_with_groups();
        settings.activated_tools = vec!["core_files".into(), "execute_terminal".into()];
        let plugins = [plugin("brave_web_search")];
        let tools = build_tool_definitions(&settings, Some(&plugins), None);
        let got = names(&tools);
        assert!(!got.contains(&"brave_web_search"), "unlisted plugin tool must not leak");
        assert_eq!(got.contains(&"read_file"), cfg!(feature = "legacy_file_ops"));
        assert_eq!(got.contains(&"execute_terminal"), cfg!(feature = "shell"));
        assert!(!got.contains(&"agent_complete") && !got.contains(&"memory_get"));
        // Activated tools carry full schemas even in DescriptionOnly mode.
        if cfg!(feature = "shell") {
            let term = tools.iter().find(|t| t.function.name == "execute_terminal").unwrap();
            assert!(term.function.parameters["properties"].get("command").is_some());
        }

        // Activating the plugin tool directly admits it with a full schema.
        settings.activated_tools.push("brave_web_search".into());
        let tools = build_tool_definitions(&settings, Some(&plugins), None);
        let brave = tools.iter().find(|t| t.function.name == "brave_web_search").expect("listed plugin tool");
        assert!(brave.function.parameters["properties"].get("query").is_some());
    }

    #[test]
    fn legacy_tool_groups_and_full_tool_schemas_still_activate() {
        let mut settings = settings_with_groups();
        settings.tool_groups = Some(vec!["agent_basic".into()]);
        settings.full_tool_schemas = vec!["read_file".into()];
        let tools = build_tool_definitions(&settings, None, None);
        let got = names(&tools);
        assert_eq!(got.len(), 2 + usize::from(cfg!(feature = "legacy_file_ops")));
        assert!(got.contains(&"agent_complete") && got.contains(&"agent_next"));
        assert_eq!(got.contains(&"read_file"), cfg!(feature = "legacy_file_ops"));
    }

    #[test]
    fn discovered_tools_join_the_state_allow_list_for_the_task() {
        let mut settings = settings_with_groups();
        settings.activated_tools = vec!["core_files".into()];
        let user = format!("registry-{}", uuid::Uuid::new_v4());
        let plugins = [plugin("brave_web_search")];
        let _task = crate::gateway::task_control::begin(&user).unwrap();
        crate::gateway::task_control::select_tools(&user, vec!["memory_get".into(), "brave_web_search".into()], false).unwrap();
        let tools = build_tool_definitions_for_user(&settings, Some(&plugins), None, &user);
        let got = names(&tools);
        assert!(got.contains(&"memory_get") && got.contains(&"brave_web_search"));
        assert_eq!(got.contains(&"read_file"), cfg!(feature = "legacy_file_ops"));
        assert!(!got.contains(&"execute_terminal"));
        assert_eq!(build_tool_definitions_for_user(&settings, Some(&plugins), None, "someone-else").len(), if cfg!(feature = "legacy_file_ops") { 3 } else { 1 });
    }

    #[test]
    fn discovery_without_allowlist_loads_schemas_without_hiding_other_tools() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let plugins = [plugin("brave_web_search")];
        let user = format!("discovery-unrestricted-{}", uuid::Uuid::new_v4());
        for mode in ["Full", "DescriptionOnly", "None"] {
            let mut settings = ContextSettings::default();
            settings.tool_discovery_mode = mode.into();
            let baseline = build_tool_definitions(&settings, Some(&plugins), Some(&db));
            let task = crate::gateway::task_control::begin(&user).unwrap();
            let mut selected = ["memory_get", "brave_web_search"].map(str::to_string).to_vec();
            if cfg!(feature = "shell") {
                selected.push("run_background".into());
            }
            crate::gateway::task_control::select_tools(&user, selected.clone(), false).unwrap();
            let got = build_tool_definitions_for_user(&settings, Some(&plugins), Some(&db), &user);
            for name in &selected {
                let tool = got.iter().find(|t| &t.function.name == name)
                    .unwrap_or_else(|| panic!("{mode}: discovered {name} must be offered"));
                let params = &tool.function.parameters;
                assert_eq!(params["properties"]["_output"]["type"], "object", "{mode}:{name}");
                if name == "memory_get" { assert_eq!(params["required"], json!(["key"])); }
                if name == "run_background" { assert_eq!(params["required"], json!(["command"])); }
            }
            if mode == "None" {
                assert!(baseline.is_empty());
                assert_eq!(got.len(), selected.len());
            } else {
                assert_eq!(names(&got), names(&baseline), "discovery must not create an allow-list");
            }
            drop(task);
            let fresh = build_tool_definitions_for_user(&settings, Some(&plugins), Some(&db), &user);
            assert_eq!(serde_json::to_value(fresh).unwrap(), serde_json::to_value(baseline).unwrap());
        }
    }

    #[test]
    fn empty_native_catalog_does_not_fall_back_to_static_contracts() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let mut settings = ContextSettings::default();
        settings.tool_discovery_mode = "Full".into();
        let plugins = [plugin("memory_set"), plugin("brave_web_search")];
        assert_eq!(names(&build_tool_definitions(&settings, Some(&plugins), Some(&db))), vec!["brave_web_search"]);
    }

    #[test]
    fn routed_contracts_match_discovery_and_respect_disabled_builtins() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let mut settings = ContextSettings::default();
        let mut activated = ["memory_get", "memory_set"].map(str::to_string).to_vec();
        let mut expected = 2;
        if cfg!(feature = "vision") {
            activated.push("understand_image".into());
            expected += 1;
        }
        if cfg!(feature = "shell") {
            activated.push("run_background".into());
            expected += 1;
        }
        settings.activated_tools = activated;
        let tools = build_tool_definitions(&settings, None, Some(&db));
        assert_eq!(tools.len(), expected, "built-ins without category metadata must be callable too");
        for tool in &tools {
            let mut expected = crate::tools::discovery::catalog(&db, &crate::plugins::PluginRegistry::new()).unwrap()
                .into_iter().find(|t| t.function.name == tool.function.name).unwrap();
            super::super::tool_output::augment_definition(&mut expected);
            assert_eq!(tool.function.parameters, expected.function.parameters, "{}", tool.function.name);
        }
        let memory = tools.iter().find(|t| t.function.name == "memory_set").unwrap();
        assert_eq!(memory.function.parameters["required"], json!(["key", "value"]));
        if cfg!(feature = "vision") {
            let image = tools.iter().find(|t| t.function.name == "understand_image").unwrap();
            assert_eq!(image.function.parameters["required"], json!(["path", "prompt"]));
            assert_eq!(image.function.parameters["properties"]["_output"]["type"], "object");
        }
        crate::db::tools::disable(&db, "memory_set").unwrap();
        let shadow = [plugin("memory_set")];
        assert!(!names(&build_tool_definitions(&settings, Some(&shadow), Some(&db))).contains(&"memory_set"));
    }

    #[test]
    fn registry_has_no_duplicate_tool_names() {
        let mut seen = HashSet::new();
        for meta in all_tool_meta() {
            assert!(seen.insert(meta.name), "duplicate tool {}", meta.name);
        }
    }

    #[test]
    fn discovery_mode_none_sends_only_activated_tools() {
        let mut settings = settings_with_groups();
        settings.tool_discovery_mode = "None".into();
        assert!(build_tool_definitions(&settings, None, None).is_empty());
        settings.activated_tools = vec!["core_files".into()];
        assert_eq!(build_tool_definitions(&settings, None, None).len(), if cfg!(feature = "legacy_file_ops") { 3 } else { 1 });
    }

    #[test]
    fn empty_or_cyclic_groups_do_not_turn_into_an_unrestricted_allow_list() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        for mode in ["Full", "DescriptionOnly", "None"] {
            let mut settings = ContextSettings::default();
            settings.tool_discovery_mode = mode.into();
            settings.tool_group_definitions.insert("empty".into(), vec![]);
            settings.tool_group_definitions.insert("cycle".into(), vec!["cycle".into()]);
            for name in ["empty", "cycle"] {
                settings.activated_tools = vec![name.into()];
                for db in [None, Some(&db)] {
                    assert!(build_tool_definitions(&settings, Some(&[plugin("brave_web_search")]), db).is_empty(), "{mode}:{name}");
                }
            }
        }
    }

    #[test]
    fn no_builtin_groups_exist() {
        let mut settings = ContextSettings::default();
        settings.activated_tools = vec!["coding".into()];
        // "coding" is not defined anywhere -> treated as an (unknown) tool name -> nothing matches.
        assert!(build_tool_definitions(&settings, None, None).is_empty());
        assert_eq!(expand_tool_groups(&["coding".into()], &HashMap::new()), vec!["coding".to_string()]);
    }

    #[test]
    fn plugin_tools_without_explicit_config_follow_discovery_mode() {
        let mut settings = ContextSettings::default();
        let plugins = [plugin("brave_web_search")];
        settings.tool_discovery_mode = "DescriptionOnly".into();
        let tools = build_tool_definitions(&settings, Some(&plugins), None);
        let brave = tools.iter().find(|t| t.function.name == "brave_web_search").unwrap();
        assert!(brave.function.parameters["properties"].as_object().unwrap().is_empty());
        settings.tool_discovery_mode = "None".into();
        assert!(!names(&build_tool_definitions(&settings, Some(&plugins), None)).contains(&"brave_web_search"));
    }

    #[test]
    fn expand_tool_groups_handles_nesting_cycles_and_unknown_names() {
        let mut defs: HashMap<String, Vec<String>> = HashMap::new();
        defs.insert("a".into(), vec!["b".into(), "read_file".into()]);
        defs.insert("b".into(), vec!["a".into(), "write_file".into()]);
        let expanded = expand_tool_groups(&["a".into(), "not_a_group".into(), " read_file ".into()], &defs);
        assert_eq!(expanded, vec!["write_file", "read_file", "not_a_group"]);
    }
}
