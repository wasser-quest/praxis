//! Builtin tool packages: every native tool has exactly one owning package
//! (plan/PLUGINIZATION.md §4). Packages are enabled or disabled as a unit,
//! independently of each other and of per-tool flags, and startup never
//! changes the operator's choice. `file_ops` is the core file contract and
//! cannot be disabled. Disabling a package removes its tools from the model
//! catalog, discovery and execution; workflow guards, receipts and the
//! management CLI/API keep working (they are host-owned, not tools).
use crate::db::tools::Tool;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub struct Package {
    pub id: &'static str,
    pub description: &'static str,
    pub tools: &'static [&'static str],
    /// Core packages are part of the runtime contract.
    pub required: bool,
}

pub const PACKAGES: &[Package] = &[
    Package {
        id: "file_ops",
        description: "Workspace file contract: bounded reads with hashes, writes and transactional patches",
        tools: &["inspect_file", "write_file", "apply_patch"],
        required: true,
    },
    Package {
        id: "runtime_control",
        description: "Model-facing workflow navigation, Decision IR execution, discovery, context and checks",
        tools: &[
            "search_tools", "execute_decision", "read_tool_result", "get_context", "set_context",
            "delete_context", "agent_next", "agent_back", "agent_complete", "agent_set_path",
            "agent_feedback", "run_check",
        ],
        required: false,
    },
    Package {
        id: "legacy_file_ops",
        description: "Raw read_file/edit_file helpers (no hash preconditions)",
        tools: &["read_file", "edit_file"],
        required: false,
    },
    Package {
        id: "shell",
        description: "Terminal commands and background jobs in the workspace",
        tools: &["execute_terminal", "run_background", "background_status"],
        required: false,
    },
    Package {
        id: "delegation",
        description: "Sub-agent delegation",
        tools: &["delegate_task", "list_delegations"],
        required: false,
    },
    Package {
        id: "memory",
        description: "Memory profiles, facts, preferences and topics",
        tools: &[
            "memory_profile_load", "memory_profile_create", "memory_profile_list", "memory_get",
            "memory_set", "learn_fact", "learn_preference", "learn_topic",
        ],
        required: false,
    },
    Package {
        id: "rag",
        description: "Document ingestion and retrieval",
        tools: &["rag_search", "rag_ingest", "rag_list", "rag_delete"],
        required: false,
    },
    Package {
        id: "cron",
        description: "Scheduled jobs",
        tools: &["cron_add", "cron_delete", "cron_list", "cron_toggle", "cron_run"],
        required: false,
    },
    Package {
        id: "discord",
        description: "Discord messages, embeds and uploads",
        tools: &["discord_upload_file", "discord_send_message", "discord_send_embed"],
        required: false,
    },
    Package {
        id: "vision",
        description: "Image understanding",
        tools: &["understand_image"],
        required: false,
    },
    Package {
        id: "interaction",
        description: "Questions to the user and screenshot delivery",
        tools: &["send_screenshot", "ask_questions"],
        required: false,
    },
    Package {
        id: "skills",
        description: "Skill search and activation",
        tools: &["search_skills", "use_skill"],
        required: false,
    },
    Package {
        id: "workflow_authoring",
        description: "Template editing by the model",
        tools: &["update_template"],
        required: false,
    },
];

/// Packages whose names an installed plugin may implement instead. `file_ops`
/// is the core file contract, and a kernel-bundled package (§5.9) drives
/// host-owned semantics (`runtime_control`, `delegation`, …), so none of them
/// is replaceable.
pub fn replaceable(id: &str) -> bool {
    get(id).is_some_and(|p| !p.required && !bundled(p.id))
}

pub fn get(id: &str) -> Option<&'static Package> {
    PACKAGES.iter().find(|p| p.id == id)
}

/// The package that owns a native tool name.
pub fn owner_of(tool: &str) -> Option<&'static Package> {
    PACKAGES.iter().find(|p| p.tools.contains(&tool))
}

/// Kernel-shipped privileged packages. Their tools are declared by a bundled
/// manifest under `packages/`, but the implementations are host-owned
/// operations in the kernel (`tools::builtin_operations`), so the package is
/// never installed, never replaced and never trusted with authority: removing
/// or disabling it removes the tools from the catalog, not the runtime's
/// ability to enforce guards (`docs/PLUGINIZATION_HANDOFF.md` §6C).
pub fn bundled(id: &str) -> bool {
    bundled_manifest(id).is_some()
}

fn bundled_manifest(id: &str) -> Option<&'static str> {
    match id {
        "runtime_control" => Some(include_str!("../../packages/runtime_control/plugin.json")),
        "delegation" => Some(include_str!("../../packages/delegation/plugin.json")),
        "memory" => Some(include_str!("../../packages/memory/plugin.json")),
        "rag" => Some(include_str!("../../packages/rag/plugin.json")),
        "cron" => Some(include_str!("../../packages/cron/plugin.json")),
        "discord" => Some(include_str!("../../packages/discord/plugin.json")),
        _ => None,
    }
}

static BUNDLED_PLUGINS: LazyLock<Vec<crate::plugins::Plugin>> = LazyLock::new(|| {
    PACKAGES
        .iter()
        .filter_map(|package| bundled_manifest(package.id))
        .map(|raw| serde_json::from_str(raw).expect("bundled package manifest must parse"))
        .collect()
});

static BUNDLED_TOOLS: LazyLock<BTreeMap<String, Tool>> = LazyLock::new(|| {
    BUNDLED_PLUGINS
        .iter()
        .flat_map(|plugin| plugin.tools.iter())
        .map(|tool| {
            (
                tool.name.clone(),
                Tool {
                    name: tool.name.clone(),
                    description: Some(tool.description.clone()),
                    parameters: tool.parameters.clone(),
                    is_enabled: true,
                },
            )
        })
        .collect()
});

/// The kernel's bundled package declarations, registered before any installed
/// package so an installed manifest can never take over their tool names.
pub fn bundled_plugins() -> Vec<crate::plugins::Plugin> {
    BUNDLED_PLUGINS.clone()
}

/// Canonical name/description/schema for a tool declared by a bundled package.
/// Bundled declarations are the one source for their parameter contracts.
pub fn bundled_tool(name: &str) -> Option<&'static Tool> {
    BUNDLED_TOOLS.get(name)
}

/// Whether a tool of a kernel-bundled package is switched on. A bundled
/// package has no plugin manifest of its own to disable, so its tool-package
/// switch expresses its absence. Replacement plugins stay controlled by their
/// own flags, not by the native package they replace.
pub fn bundled_tool_enabled(data_dir: &Path, tool: &str) -> anyhow::Result<bool> {
    match owner_of(tool).filter(|package| bundled(package.id)) {
        Some(package) => enabled(data_dir, package.id),
        None => Ok(true),
    }
}

/// Build-time availability is distinct from the persisted operator switch.
/// Retaining names/schemas for migration never installs their implementations.
pub fn native_available(tool: &str) -> bool {
    owner_of(tool).is_none_or(|package| match package.id {
        "legacy_file_ops" => cfg!(feature = "legacy_file_ops"),
        "shell" => cfg!(feature = "shell"),
        "vision" => cfg!(feature = "vision"),
        _ => true,
    })
}

fn state_file(data_dir: &Path) -> PathBuf {
    data_dir.join("tool_packages.json")
}

/// Persisted operator choices. Missing entries mean enabled (compatibility
/// distribution); unknown entries are kept but ignored.
pub fn load(data_dir: &Path) -> anyhow::Result<BTreeMap<String, bool>> {
    match std::fs::read_to_string(state_file(data_dir)) {
        Ok(data) => Ok(serde_json::from_str(&data)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn enabled(data_dir: &Path, id: &str) -> anyhow::Result<bool> {
    let package = get(id).ok_or_else(|| anyhow::anyhow!("Unknown tool package '{id}'"))?;
    Ok(package.required || load(data_dir)?.get(id).copied().unwrap_or(true))
}

/// Whether the owning package of a native tool is enabled. Tools without a
/// package entry are not native and are unaffected.
pub fn tool_package_enabled(data_dir: &Path, tool: &str) -> anyhow::Result<bool> {
    match owner_of(tool) {
        Some(package) => enabled(data_dir, package.id),
        None => Ok(true),
    }
}

pub fn set(data_dir: &Path, id: &str, on: bool) -> anyhow::Result<()> {
    let package = get(id).ok_or_else(|| anyhow::anyhow!("Unknown tool package '{id}'"))?;
    anyhow::ensure!(on || !package.required, "Tool package '{id}' is part of the core runtime and cannot be disabled");
    let mut state = load(data_dir)?;
    state.insert(id.into(), on);
    std::fs::create_dir_all(data_dir)?;
    let path = state_file(data_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&state)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn list(data_dir: &Path) -> anyhow::Result<Value> {
    let state = load(data_dir)?;
    let packages: Vec<_> = PACKAGES
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "description": p.description,
                "tools": p.tools,
                "required": p.required,
                "native_available": p.tools.iter().all(|tool| native_available(tool)),
                "enabled": p.required || state.get(p.id).copied().unwrap_or(true),
            })
        })
        .collect();
    Ok(json!({ "packages": packages }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "legacy_file_ops"))]
    #[test]
    fn unlinked_legacy_package_is_absent_even_with_stale_enabled_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        set(dir.path(), "legacy_file_ops", true).unwrap();
        let plugins = crate::plugins::PluginRegistry::new();
        let catalog = crate::tools::catalog::definitions(&db, &plugins).unwrap();
        for name in ["read_file", "edit_file"] {
            assert!(!catalog.iter().any(|t| t.function.name == name), "unlinked implementation advertised: {name}");
            assert!(crate::tools::catalog::owner(&plugins, name).is_err(), "stale schema must not select unavailable code");
        }
        let packages = list(dir.path()).unwrap();
        let legacy = packages["packages"].as_array().unwrap().iter().find(|p| p["id"] == "legacy_file_ops").unwrap();
        assert_eq!(legacy["native_available"], false);
        let mut settings = crate::db::contexts::ContextSettings::default();
        settings.activated_tools = vec!["read_file".into(), "edit_file".into()];
        assert!(crate::tools::registry::build_tool_definitions(&settings, None, None).is_empty());
    }

    #[cfg(not(feature = "shell"))]
    #[test]
    fn unlinked_shell_package_is_absent_even_with_stale_enabled_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        set(dir.path(), "shell", true).unwrap();
        let plugins = crate::plugins::PluginRegistry::new();
        let catalog = crate::tools::catalog::definitions(&db, &plugins).unwrap();
        for name in ["execute_terminal", "run_background", "background_status"] {
            assert!(!catalog.iter().any(|t| t.function.name == name), "unlinked shell implementation advertised: {name}");
            assert!(crate::tools::catalog::owner(&plugins, name).is_err(), "stale schema must not select unavailable code");
        }
        let packages = list(dir.path()).unwrap();
        let shell = packages["packages"].as_array().unwrap().iter().find(|p| p["id"] == "shell").unwrap();
        assert_eq!(shell["native_available"], false);
        let mut settings = crate::db::contexts::ContextSettings::default();
        settings.activated_tools = vec!["execute_terminal".into(), "run_background".into()];
        assert!(crate::tools::registry::build_tool_definitions(&settings, None, None).is_empty());
    }

    #[cfg(not(feature = "vision"))]
    #[test]
    fn unlinked_vision_package_is_absent_even_with_stale_enabled_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        set(dir.path(), "vision", true).unwrap();
        let plugins = crate::plugins::PluginRegistry::new();
        let catalog = crate::tools::catalog::definitions(&db, &plugins).unwrap();
        assert!(!catalog.iter().any(|t| t.function.name == "understand_image"), "unlinked vision implementation advertised");
        assert!(crate::tools::catalog::owner(&plugins, "understand_image").is_err(), "stale schema must not select unavailable code");
        let packages = list(dir.path()).unwrap();
        let vision = packages["packages"].as_array().unwrap().iter().find(|p| p["id"] == "vision").unwrap();
        assert_eq!(vision["native_available"], false);
        let mut settings = crate::db::contexts::ContextSettings::default();
        settings.activated_tools = vec!["understand_image".into()];
        assert!(crate::tools::registry::build_tool_definitions(&settings, None, None).is_empty());
    }

    #[test]
    fn every_native_tool_has_exactly_one_package() {
        let natives: Vec<String> = crate::db::tools::get_default_tools()
            .into_iter()
            .map(|t| t.name)
            .filter(|name| !crate::runtime::vm::is_vm_tool(name))
            .collect();
        for name in &natives {
            let owners = PACKAGES.iter().filter(|p| p.tools.contains(&name.as_str())).count();
            assert_eq!(owners, 1, "{name} must have exactly one owning package");
        }
        for package in PACKAGES {
            for tool in package.tools {
                assert!(
                    natives.iter().any(|n| n == tool)
                        || bundled(package.id)
                            && bundled_tool(tool).is_some_and(|declared| declared.name == *tool),
                    "{} lists unknown tool {tool}",
                    package.id
                );
            }
        }
    }

    #[test]
    fn bundled_manifests_declare_exactly_their_package_tools() {
        for package in PACKAGES.iter().filter(|p| bundled(p.id)) {
            let plugin = BUNDLED_PLUGINS
                .iter()
                .find(|plugin| plugin.name == package.id)
                .expect("bundled package manifest is registered");
            let mut declared: Vec<&str> = plugin.tools.iter().map(|t| t.name.as_str()).collect();
            let mut owned: Vec<&str> = package.tools.to_vec();
            declared.sort_unstable();
            owned.sort_unstable();
            assert_eq!(declared, owned, "{} declares a different tool set", package.id);
            for tool in &plugin.tools {
                assert!(
                    matches!(&tool.handler, crate::plugins::PluginHandler::Builtin { .. }),
                    "{} tool {} must run a host-owned operation",
                    package.id,
                    tool.name
                );
            }
        }
    }

    /// Core-only exclusion (§9 step 6): without its package, `runtime_control`
    /// tools leave the catalog, discovery and execution, while host-owned
    /// workflow authority keeps enforcing kernel evidence.
    #[tokio::test]
    async fn absent_runtime_control_removes_tools_but_not_host_authority() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let plugins = crate::plugins::PluginRegistry::new();
        let user = format!("pkg-absent-{}", uuid::Uuid::new_v4());
        let _task = crate::gateway::task_control::begin(&user).unwrap();
        crate::gateway::task_control::pin_registry(&user, &plugins).unwrap();
        let call = crate::gateway::llm::provider::ToolCall {
            id: "call-1".into(),
            function: crate::gateway::llm::provider::FunctionCall {
                name: "agent_feedback".into(),
                arguments: json!({"message": "hi"}).to_string(),
            },
        };
        let dispatch = crate::gateway::tool_dispatch::DispatchContext::new(
            dir.path(),
            &db,
            &user,
            &plugins,
            crate::gateway::tool_dispatch::DispatchMode::Chat,
        );
        assert_eq!(dispatch.execute(&call).await, "Feedback sent: hi");

        set(&db.data_dir(), "runtime_control", false).unwrap();
        let catalog: Vec<String> = crate::tools::catalog::definitions(&db, &plugins)
            .unwrap()
            .into_iter()
            .map(|tool| tool.function.name)
            .collect();
        for tool in get("runtime_control").unwrap().tools {
            assert!(!catalog.contains(&tool.to_string()), "{tool} leaked into the catalog");
        }
        let denied = dispatch.execute(&call).await;
        assert!(denied.contains("disabled or unavailable"), "{denied}");
        let mut settings = crate::db::contexts::ContextSettings::default();
        settings.activated_tools = vec!["agent_feedback".into(), "execute_decision".into()];
        assert!(
            crate::tools::registry::build_tool_definitions(
                &settings,
                Some(&plugins.tool_definitions()),
                Some(&db),
            )
            .is_empty(),
            "a disabled package leaked into the model request"
        );
        // The runtime's authority is intact: guards still require verified,
        // kernel-signed evidence and cannot be satisfied by a claim.
        let sm = crate::sm::parse(
            "[state standard]\n[action_guards]\n_complete = [runtime_control/agent_complete]",
        )
        .unwrap();
        crate::gateway::action_contracts::bind(&user, "absent", &sm, dir.path()).unwrap();
        let error = crate::gateway::action_contracts::require(&user, "_complete").unwrap_err();
        assert!(error.to_string().contains("verified capabilities"), "{error}");
    }

    #[test]
    fn bundled_declarations_match_the_kernel_definitions() {
        // The manifest is the one source for these contracts; where a kernel
        // definition still exists it must not drift from the declaration.
        for definition in [
            crate::gateway::decision_ir::definition(),
            crate::gateway::action_contracts::definition(),
            crate::tools::tool_output::definition(),
        ] {
            let declared = bundled_tool(&definition.name).expect("kernel definition is bundled");
            assert_eq!(
                declared.description.as_deref(),
                definition.description.as_deref(),
                "{}",
                definition.name
            );
            assert_eq!(declared.parameters, definition.parameters, "{}", definition.name);
        }
    }

    #[test]
    fn bundled_tools_keep_the_operator_choice_when_the_package_switch_flips() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        // An older install stored the flags on the builtin rows.
        crate::db::tools::save(
            &db,
            &Tool {
                name: "agent_feedback".into(),
                description: Some("Send progress update".into()),
                parameters: json!({"type": "object"}),
                is_enabled: false,
            },
        )
        .unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        // The rows are gone; the choice moved to the plugin flag store.
        assert!(crate::db::tools::get(&db, "agent_feedback").is_err());
        assert!(!crate::db::tools::tool_enabled(&db, "agent_feedback").unwrap());
        assert!(crate::db::tools::tool_enabled(&db, "agent_next").unwrap());
        // The package switch removes the tools; the per-tool flag survives it.
        set(dir.path(), "runtime_control", false).unwrap();
        assert!(!crate::db::tools::tool_enabled(&db, "agent_next").unwrap());
        assert!(!crate::db::tools::tool_enabled(&db, "agent_feedback").unwrap());
        set(dir.path(), "runtime_control", true).unwrap();
        assert!(crate::db::tools::tool_enabled(&db, "agent_next").unwrap());
        assert!(!crate::db::tools::tool_enabled(&db, "agent_feedback").unwrap());
    }

    #[test]
    fn packages_toggle_independently_and_core_stays_on() {
        let dir = tempfile::tempdir().unwrap();
        assert!(tool_package_enabled(dir.path(), "execute_terminal").unwrap());
        set(dir.path(), "shell", false).unwrap();
        assert!(!tool_package_enabled(dir.path(), "execute_terminal").unwrap());
        assert!(tool_package_enabled(dir.path(), "agent_next").unwrap());
        assert!(set(dir.path(), "file_ops", false).is_err());
        assert!(set(dir.path(), "nope", false).is_err());
        assert!(tool_package_enabled(dir.path(), "brave_search").unwrap());
        set(dir.path(), "shell", true).unwrap();
        assert!(enabled(dir.path(), "shell").unwrap());
    }

    #[test]
    fn disabled_package_leaves_catalog_and_execution_but_keeps_tool_flags() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        crate::db::tools::set_enabled(&db, "run_background", false).unwrap();
        let plugins = crate::plugins::PluginRegistry::new();
        let names = |db: &crate::db::Database| -> Vec<String> {
            crate::tools::catalog::definitions(db, &plugins).unwrap().into_iter().map(|t| t.function.name).collect()
        };
        assert_eq!(names(&db).contains(&"execute_terminal".to_string()), cfg!(feature = "shell"));
        set(&db.data_dir(), "shell", false).unwrap();
        let without = names(&db);
        assert!(!without.iter().any(|n| PACKAGES[3].tools.contains(&n.as_str())), "{without:?}");
        let mut settings = crate::db::contexts::ContextSettings::default();
        settings.activated_tools = vec!["execute_terminal".into()];
        assert!(crate::tools::registry::build_tool_definitions(&settings, None, Some(&db)).is_empty(), "disabled native package leaked into the model request");
        assert!(without.contains(&"agent_next".to_string()) && without.contains(&"apply_patch".to_string()));
        if cfg!(feature = "shell") {
            let owner = crate::tools::catalog::owner(&plugins, "execute_terminal").unwrap();
            assert!(crate::tools::catalog::require_enabled(&db, &owner, "execute_terminal").is_err());
        } else {
            // Omitted implementation: the stale name must not select any code.
            assert!(crate::tools::catalog::owner(&plugins, "execute_terminal").is_err());
        }
        // Startup does not undo the operator's choice.
        crate::db::tools::init_default_tools(&db).unwrap();
        assert_eq!(names(&db).contains(&"execute_terminal".to_string()), false);
        set(&db.data_dir(), "shell", true).unwrap();
        let back = names(&db);
        assert_eq!(back.contains(&"execute_terminal".to_string()), cfg!(feature = "shell"));
        assert!(!back.contains(&"run_background".to_string()), "per-tool flag preserved");
    }
}
