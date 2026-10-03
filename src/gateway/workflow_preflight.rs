//! Read-only setup validation. Setup errors are operator actions, never
//! authority for a model to invent a different owner, workflow or workspace.
use crate::{
    db::{contexts::Context, Database},
    plugins::PluginRegistry,
    sm::StateMachine,
};
use serde::Serialize;
use std::fmt;

#[derive(Debug, Serialize)]
pub struct SetupError {
    pub code: &'static str,
    pub workflow: String,
    pub state: String,
    pub target: String,
    pub message: String,
    pub operator_action: String,
    pub retryable: bool,
}
impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Workflow setup error [{}] in {} / {} for {}: {}. {}. Do not retry this call or guess different operands; operator setup is required", self.code, self.workflow, self.state, self.target, self.message, self.operator_action)
    }
}
impl std::error::Error for SetupError {}

fn failure(
    code: &'static str,
    workflow: &str,
    state: &str,
    target: &str,
    message: &str,
    action: String,
) -> anyhow::Error {
    SetupError {
        code,
        workflow: workflow.into(),
        state: state.into(),
        target: target.into(),
        message: message.into(),
        operator_action: action,
        retryable: false,
    }
    .into()
}

/// Shared with IR dispatch, which rechecks live enabled flags before lowering.
pub(crate) fn target<'a>(
    db: &Database,
    plugins: &PluginRegistry,
    workflow: &str,
    state: &str,
    target: &'a str,
) -> anyhow::Result<&'a str> {
    let name = if let Some((owner, name)) = target.split_once('/') {
        let plugin = plugins.get(owner).ok_or_else(|| failure("plugin_missing", workflow, state, target, "Declared plugin is not loaded", format!("Install plugin '{owner}' into the runtime's PLUGINS_DIR and restart Praxis; inspect startup load errors if it is already installed")))?;
        if !plugin.enabled {
            return Err(failure(
                "plugin_disabled",
                workflow,
                state,
                target,
                "Declared plugin is disabled",
                format!("Enable plugin '{owner}' in its manifest and restart Praxis"),
            ));
        }
        let tool = plugin.tools.iter().find(|t| t.name == name).ok_or_else(|| failure("capability_missing", workflow, state, target, "The loaded plugin does not declare this capability", format!("Update plugin '{owner}' to the workflow's required version and restart Praxis")))?;
        if tool.contract.is_none() {
            return Err(failure(
                "contract_missing",
                workflow,
                state,
                target,
                "The declared capability has no action contract",
                format!("Install the contracted version of '{target}' and restart Praxis"),
            ));
        }
        if !plugins.manages_contract(name) {
            return Err(failure("owner_conflict", workflow, state, target, "Capability does not have one unambiguous enabled owner, or shadows a builtin", format!("Keep exactly one enabled contracted owner for '{name}', preserve builtin names and restart Praxis")));
        }
        name
    } else {
        if !matches!(
            target,
            "inspect_file" | "agent_complete" | "agent_next" | "agent_back" | "execute_decision"
        ) {
            return Err(failure("invalid_ir_target", workflow, state, target, "Unsupported builtin IR target", "Fix the trusted workflow mapping; use an allowed builtin or contracted plugin/tool identity".into()));
        }
        target
    };
    if !crate::tools::discovery::enabled(db, plugins, name) {
        return Err(failure(
            "tool_disabled",
            workflow,
            state,
            target,
            "Required tool is disabled or unavailable",
            format!(
                "Enable '{name}' in Dashboard → Tools; start a new task after correcting setup"
            ),
        ));
    }
    Ok(name)
}

/// Validate effective tables in all declared states. Unused global mappings
/// overridden by every state do not create fictitious requirements. Discovery
/// cannot make an invalid trusted allow-list pass setup.
pub fn validate(
    db: &Database,
    plugins: &PluginRegistry,
    workflow: &str,
    sm: &StateMachine,
    ctx: &Context,
) -> anyhow::Result<()> {
    crate::tools::catalog::validate(plugins).map_err(|error| failure(
        "owner_conflict", workflow, ctx.active_state.as_deref().unwrap_or(""), "tool_catalog",
        "Enabled tool declarations have conflicting owners", error.to_string(),
    ))?;
    let mut states: Vec<_> = sm
        .states
        .keys()
        .filter(|s| s.as_str() != "_default")
        .collect();
    states.sort();
    let plugin_tools = plugins.tool_definitions();
    for state in states {
        let mapping = sm.ir_for(state);
        if mapping.is_empty() {
            continue;
        }
        let mut value = serde_json::to_value(ctx)?;
        anyhow::ensure!(
            crate::sm::apply_state(sm, &mut value, state),
            "Preflight cannot apply declared state"
        );
        let mut candidate: Context = serde_json::from_value(value)?;
        candidate
            .settings
            .tool_group_definitions
            .extend(sm.tool_groups.clone());
        let definitions = crate::tools::registry::build_tool_definitions(
            &candidate.settings,
            Some(&plugin_tools),
            Some(db),
        );
        target(db, plugins, workflow, state, "execute_decision")?;
        if !definitions
            .iter()
            .any(|d| d.function.name == "execute_decision")
        {
            return Err(failure(
                "ir_target_not_allowed",
                workflow,
                state,
                "execute_decision",
                "State has an IR table but does not allow its dispatcher",
                "Add execute_decision to this state's trusted tool selection".into(),
            ));
        }
        let mut mappings: Vec<_> = mapping.iter().collect();
        mappings.sort_by_key(|(op, _)| *op);
        for (_, identity) in mappings {
            let name = target(db, plugins, workflow, state, identity)?;
            if !definitions.iter().any(|d| d.function.name == name) {
                return Err(failure("ir_target_not_allowed", workflow, state, identity, "IR target is absent from this state's trusted tool selection", format!("Add '{name}' to this state's activated tools or remove its opcode; changing operands cannot grant authority")));
            }
        }
    }
    let mut guards: Vec<_> = sm.action_guards.iter().collect();
    guards.sort_by_key(|(state, _)| *state);
    for (state, identities) in guards {
        for identity in identities {
            target(db, plugins, workflow, state, identity)?;
        }
    }
    Ok(())
}

pub(crate) fn tool_error(error: &anyhow::Error) -> String {
    match error.downcast_ref::<SetupError>() {
        Some(setup) => {
            serde_json::json!({"error":setup,"executed":false,"verified":false}).to_string()
        }
        None => format!("Error: {error}; decision not executed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (
        tempfile::TempDir,
        Database,
        StateMachine,
        Context,
        PluginRegistry,
    ) {
        let root = tempfile::tempdir().unwrap();
        let db = Database::new(root.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let sm = crate::sm::parse("@routing graph\n@start route\n[state route]\nsettings.activated_tools = [\"execute_decision\",\"agent_next\"]\n[state edit]\nsettings.activated_tools = [\"execute_decision\",\"modify_source\",\"agent_back\"]\n[transitions]\nroute -> edit\n[decision_ir]\nN = agent_next\n[decision_ir edit]\nM = native/modify_source\nK = agent_back\n[action_guards]\n_complete = [native/modify_source]").unwrap();
        let mut ctx = db.load_context("preflight-test").unwrap();
        ctx.active_state = Some("route".into());
        ctx.settings.active_state = ctx.active_state.clone();
        let mut registry = PluginRegistry::new();
        registry.register(serde_json::from_value(json!({"name":"native","version":"1","description":"fixture","tools":[{
            "name":"modify_source","description":"edit","handler":{"type":"source_edit"},
            "parameters":{"type":"object","properties":{"path":{"type":"string"},"expected_sha256":{"type":"string"},"content":{"type":"string"}},"required":["path","expected_sha256","content"],"additionalProperties":false},
            "contract":{"effect":"workspace_write","idempotency":"non_idempotent","timeout_secs":5,"postconditions":[{"program":"/bin/true"}]}
        }]})).unwrap());
        (root, db, sm, ctx, registry)
    }

    #[test]
    fn workflow_preflight_validates_future_state_and_keeps_context_unchanged() {
        let (_root, db, sm, ctx, registry) = fixture();
        let before = json!(ctx);
        validate(&db, &registry, "fixture", &sm, &ctx).unwrap();
        assert_eq!(json!(ctx), before);
        assert!(crate::gateway::action_contracts::action_root(&ctx.user_id).is_err());
    }

    #[test]
    fn workflow_preflight_missing_plugin_is_actionable_and_non_retryable() {
        let (_root, db, sm, ctx, _registry) = fixture();
        let error = validate(&db, &PluginRegistry::new(), "fixture", &sm, &ctx).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("plugin_missing") && text.contains("native") && text.contains("restart"),
            "{text}"
        );
        assert!(text.contains("Do not retry"), "{text}");
        let result: serde_json::Value = serde_json::from_str(&tool_error(&error)).unwrap();
        assert_eq!(result["error"]["code"], "plugin_missing");
        assert_eq!(result["error"]["retryable"], false);
        assert_eq!(result["executed"], false);
        assert_eq!(result["verified"], false);
    }

    #[test]
    fn workflow_preflight_ignores_global_table_overridden_by_every_state() {
        let (_root, db, _sm, ctx, _registry) = fixture();
        let source = "@routing graph\n[state a]\nsettings.activated_tools = [\"execute_decision\",\"agent_next\"]\n[state b]\nsettings.activated_tools = [\"execute_decision\",\"agent_back\"]\n[transitions]\na -> b\n[decision_ir]\nM = missing/modify_source\n[decision_ir a]\nN = agent_next\n[decision_ir b]\nK = agent_back";
        let sm = crate::sm::parse(source).unwrap();
        validate(&db, &PluginRegistry::new(), "overridden", &sm, &ctx).unwrap();
    }

    #[tokio::test]
    async fn workflow_preflight_live_disable_keeps_graph_state_history_and_audit_unchanged() {
        let (root, db, sm, mut ctx, registry) = fixture();
        std::fs::create_dir(root.path().join("contexts")).unwrap();
        std::fs::create_dir(root.path().join("templates")).unwrap();
        std::fs::write(root.path().join("contexts/fixture.sm"), "@routing graph\n@start route\n[state route]\nsettings.activated_tools = [\"execute_decision\",\"agent_next\"]\n[state edit]\nsettings.activated_tools = [\"execute_decision\",\"modify_source\",\"agent_back\"]\n[transitions]\nroute -> edit\n[decision_ir]\nN = agent_next\n[decision_ir edit]\nM = native/modify_source\nK = agent_back\n[action_guards]\n_complete = [native/modify_source]").unwrap();
        std::fs::write(
            root.path().join("templates/standard.poml"),
            "<poml><role>Fixture</role></poml>",
        )
        .unwrap();
        ctx.user_id = "preflight-live-navigation".into();
        ctx.settings.sm_file = Some("fixture".into());
        db.save_context(&ctx).unwrap();
        let _task = crate::gateway::task_control::begin(&ctx.user_id).unwrap();
        validate(&db, &registry, "fixture", &sm, &ctx).unwrap();
        crate::gateway::action_contracts::bind(&ctx.user_id, "fixture", &sm, root.path()).unwrap();
        crate::gateway::action_contracts::validate_context(&ctx, &ctx).unwrap();
        crate::db::tools::set_plugin_tool_enabled(&db, "modify_source", false).unwrap();
        let result = crate::tools::agent_control::navigate_with_plugins(
            &db,
            root.path(),
            &registry,
            &ctx.user_id,
            false,
            &json!({"edge":0}),
        )
        .await
        .unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(error["error"]["code"], "tool_disabled");
        assert_eq!(json!(db.load_context(&ctx.user_id).unwrap()), json!(ctx));
        assert!(crate::gateway::workflow_graph::history(&ctx.user_id).is_empty());
        assert!(db.get_messages(&ctx.user_id, 10).unwrap().is_empty());
    }

    #[test]
    fn workflow_preflight_rejects_disabled_uncontracted_and_conflicting_owners() {
        let (_root, db, sm, ctx, registry) = fixture();
        for issue in ["plugin_disabled", "contract_missing", "owner_conflict"] {
            let mut plugin = registry.get("native").unwrap().clone();
            if issue == "plugin_disabled" {
                plugin.enabled = false;
            }
            if issue == "contract_missing" {
                plugin.tools[0].contract = None;
            }
            let mut bad = PluginRegistry::new();
            bad.register(plugin.clone());
            if issue == "owner_conflict" {
                plugin.name = "other".into();
                bad.register(plugin);
            }
            let error = validate(&db, &bad, "fixture", &sm, &ctx).unwrap_err();
            assert!(error.to_string().contains(issue), "{error}");
        }
    }

    #[test]
    fn workflow_preflight_rejects_disabled_tool_and_unallowed_ir_target() {
        let (_root, db, sm, ctx, registry) = fixture();
        crate::db::tools::set_plugin_tool_enabled(&db, "modify_source", false).unwrap();
        let error = validate(&db, &registry, "fixture", &sm, &ctx).unwrap_err();
        assert!(error.to_string().contains("tool_disabled"), "{error}");
        crate::db::tools::set_plugin_tool_enabled(&db, "modify_source", true).unwrap();
        let mut bad = sm.clone();
        bad.states.get_mut("edit").unwrap().variables.insert(
            "settings.activated_tools".into(),
            "[\"execute_decision\",\"agent_back\"]".into(),
        );
        let error = validate(&db, &registry, "fixture", &bad, &ctx).unwrap_err();
        assert!(
            error.to_string().contains("ir_target_not_allowed"),
            "{error}"
        );
        crate::db::tools::set_enabled(&db, "execute_decision", false).unwrap();
        let error = validate(&db, &registry, "fixture", &sm, &ctx).unwrap_err();
        assert!(error.to_string().contains("tool_disabled"), "{error}");
    }
}
