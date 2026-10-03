//! One parsed graph serves runtime choices, model instructions and dashboard.
use crate::{
    db::{contexts::Context, Database},
    sm::StateMachine,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path};

#[derive(Clone, Serialize)]
pub struct Edge {
    pub id: String,
    pub index: Option<usize>,
    pub from: String,
    pub to: String,
    pub title: String,
    pub description: String,
    pub condition: String,
    pub kind: String,
    pub eligible: bool,
    pub blocked_reason: Option<String>,
}

pub fn edges(sm: &StateMachine, context: &Value, user: &str) -> Vec<Edge> {
    let mut indices = HashMap::<String, usize>::new();
    let mut result = Vec::new();
    for transition in &sm.transitions {
        let index = indices.entry(transition.from.clone()).or_default();
        let id = format!("{}:{}", transition.from, index);
        let metadata = sm.edges.get(&id);
        let condition_met = transition.condition.trim().is_empty()
            || context
                .as_object()
                .is_some_and(|obj| crate::sm::evaluate_condition(&transition.condition, obj));
        let blocked_reason = if !sm.states.contains_key(&transition.to) {
            Some("Destination state is undefined".into())
        } else if !condition_met {
            Some(format!("Condition is false: {}", transition.condition))
        } else {
            super::action_contracts::require_for_workflow(user, sm, &transition.to)
                .err()
                .map(|e| e.to_string())
        };
        result.push(Edge {
            id,
            index: Some(*index),
            from: transition.from.clone(),
            to: transition.to.clone(),
            title: metadata
                .filter(|m| !m.title.is_empty())
                .map(|m| m.title.clone())
                .unwrap_or_else(|| {
                    sm.nodes
                        .get(&transition.to)
                        .filter(|m| !m.title.is_empty())
                        .map(|m| m.title.clone())
                        .unwrap_or_else(|| transition.to.clone())
                }),
            description: metadata
                .filter(|m| !m.description.is_empty())
                .map(|m| m.description.clone())
                .unwrap_or_else(|| {
                    sm.nodes
                        .get(&transition.to)
                        .map(|m| m.description.clone())
                        .unwrap_or_default()
                }),
            condition: transition.condition.clone(),
            kind: "transition".into(),
            eligible: blocked_reason.is_none(),
            blocked_reason,
        });
        *index += 1;
    }
    if !sm.is_graph() {
        for pair in sm.steps.windows(2) {
            if !sm.transitions.iter().any(|t| t.from == pair[0]) {
                let blocked_reason =
                    super::action_contracts::require_for_workflow(user, sm, &pair[1])
                        .err()
                        .map(|e| e.to_string());
                result.push(Edge {
                    id: format!("step:{}", pair[0]),
                    index: None,
                    from: pair[0].clone(),
                    to: pair[1].clone(),
                    title: "Next step".into(),
                    description: String::new(),
                    condition: String::new(),
                    kind: "step".into(),
                    eligible: blocked_reason.is_none(),
                    blocked_reason,
                });
            }
        }
        for (index, rule) in sm.auto_rules.iter().enumerate() {
            for source in sm.states.keys().filter(|s| s.as_str() != "_default") {
                result.push(Edge {
                    id: format!("auto:{index}:{source}"),
                    index: None,
                    from: source.clone(),
                    to: rule.target_state.clone(),
                    title: "Automatic".into(),
                    description: String::new(),
                    condition: rule.condition.clone(),
                    kind: "auto".into(),
                    eligible: false,
                    blocked_reason: Some("Automatic rule; evaluated during prompt routing".into()),
                });
            }
        }
    }
    result
}

pub fn history(user: &str) -> Vec<String> {
    super::task_control::with_verification(user, |ledger| Ok(ledger.navigation_history.clone()))
        .unwrap_or_default()
}

pub fn view(sm: &StateMachine, ctx: &Context) -> Value {
    let context = serde_json::to_value(ctx).unwrap_or_default();
    let mut nodes: Vec<_> = sm.states.iter().filter(|(name, _)| name.as_str() != "_default").map(|(name, state)| {
        let metadata = sm.nodes.get(name);
        json!({"id": name, "title": metadata.filter(|m| !m.title.is_empty()).map(|m| m.title.as_str()).unwrap_or(name),
            "description": metadata.map(|m| m.description.as_str()).unwrap_or(""), "variables": state.variables,
            "decision_ir": sm.ir_for(name), "guards": sm.guards.get(name), "action_guards": sm.action_guards.get(name),
            "action_guard_triggers": sm.action_guard_triggers.get(name), "user_reply_guards": sm.user_reply_guards.get(name)})
    }).collect();
    nodes.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    json!({"name": sm.name, "description": sm.description, "routing": if sm.is_graph() {"graph"} else {"linear"},
        "start": sm.entry_state(), "active_state": ctx.active_state, "history": history(&ctx.user_id),
        "nodes": nodes, "edges": edges(sm, &context, &ctx.user_id), "workflow": super::prompt::workflow_name(ctx),
        "reply_facts": super::action_contracts::reply_facts(&ctx.user_id), "completion_guards": sm.action_guards.get("_complete"),
        "completion_triggers": sm.action_guard_triggers.get("_complete")})
}

fn choose<'a>(edges: &'a [Edge], state: &str, index: Option<usize>) -> anyhow::Result<&'a Edge> {
    let outgoing: Vec<_> = edges
        .iter()
        .filter(|edge| edge.from == state && edge.kind == "transition")
        .collect();
    let selected = if let Some(index) = index {
        outgoing
            .iter()
            .find(|edge| edge.index == Some(index))
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid edge {index} from {state}; choices: {}",
                    serde_json::to_string(&outgoing).unwrap_or_default()
                )
            })?
    } else {
        let eligible: Vec<_> = outgoing
            .iter()
            .filter(|edge| edge.eligible)
            .copied()
            .collect();
        anyhow::ensure!(
            eligible.len() == 1,
            "Select an edge explicitly; choices: {}",
            serde_json::to_string(&outgoing)?
        );
        eligible[0]
    };
    anyhow::ensure!(
        selected.eligible,
        "Edge {} is blocked: {}",
        selected.id,
        selected.blocked_reason.as_deref().unwrap_or("unavailable")
    );
    Ok(selected)
}

pub async fn navigate(
    db: &Database,
    root: &Path,
    user: &str,
    back: bool,
    args: &Value,
) -> anyhow::Result<String> {
    navigate_checked(db, root, user, back, args, None).await
}

pub(crate) async fn navigate_checked(
    db: &Database,
    root: &Path,
    user: &str,
    back: bool,
    args: &Value,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> anyhow::Result<String> {
    let before = db.load_context(user)?;
    let sm = crate::sm::load_file_in(
        &root.join("contexts"),
        super::prompt::workflow_name(&before),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    anyhow::ensure!(
        sm.is_graph(),
        "Indexed/back navigation requires @routing graph"
    );
    let obj = args
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Navigation arguments must be an object"))?;
    anyhow::ensure!(
        obj.keys()
            .all(|key| key == "from_state" || (!back && key == "edge")),
        "Unknown navigation argument"
    );
    let source = before.active_state.as_deref().unwrap_or("");
    if let Some(expected) = args.get("from_state") {
        anyhow::ensure!(
            expected.as_str() == Some(source),
            "Stale state; current node is {source}"
        );
    }
    let index = args
        .get("edge")
        .map(|value| {
            value
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| anyhow::anyhow!("edge must be a non-negative integer"))
        })
        .transpose()?;
    let (target, edge_id) = if back {
        (
            history(user)
                .last()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("No previous visited node in this task"))?,
            None,
        )
    } else {
        let context = serde_json::to_value(&before)?;
        let choices = edges(&sm, &context, user);
        let selected = choose(&choices, source, index)?;
        (selected.to.clone(), Some(selected.id.clone()))
    };
    let mut value = serde_json::to_value(&before)?;
    anyhow::ensure!(
        crate::sm::apply_state(&sm, &mut value, &target),
        "Destination cannot be applied"
    );
    let mut after: Context = serde_json::from_value(value)?;
    anyhow::ensure!(
        after.settings.show_thinking == before.settings.show_thinking
            && after.settings.active_skill == before.settings.active_skill,
        "Thinking visibility and persistent skill are user-only"
    );
    after.settings.active_state = after.active_state.clone();
    if let Some(plugins) = plugins {
        super::workflow_preflight::validate(db, plugins, super::prompt::workflow_name(&after), &sm, &after)?;
        super::templates::resolve_template(&root.join("templates"), after.settings.system_template.as_deref().unwrap_or("standard"))?;
    }
    after.settings.llm_turn += 1;
    let mut next_history = history(user);
    if back {
        next_history.pop();
    } else {
        next_history.push(source.to_string());
    }
    let mut graph = view(&sm, &after);
    graph["history"] = json!(next_history);
    // The receipt describes the resulting graph. Every successful navigation
    // consumes the current input event, so reply-guard edges cannot remain
    // eligible based on the ledger's pre-commit facts.
    graph["reply_facts"]["state_changed_since_input"] = json!(true);
    if let Some(edges) = graph["edges"].as_array_mut() {
        for edge in edges {
            if let Some(states) = edge["to"].as_str().and_then(|to| sm.user_reply_guards.get(to)) {
                if edge["eligible"] == true {
                    edge["blocked_reason"] = json!(format!("A new user message while at {states:?} is required after this transition"));
                }
                edge["eligible"] = json!(false);
            }
        }
    }
    let receipt = json!({"kind":"state_transition", "verified":true, "workflow":super::prompt::workflow_name(&after), "from_state":source, "to_state":target, "edge":edge_id, "back":back, "graph":graph});
    super::action_contracts::commit_navigation(db, &before, &after, &sm, back, &receipt)?;
    crate::dashboard::stream::send(user, "state_transition", &receipt.to_string());
    Ok(receipt.to_string())
}

pub fn instructions(sm: &StateMachine, ctx: &Context) -> String {
    if !sm.is_graph() {
        return String::new();
    }
    let graph = view(sm, ctx);
    let current = ctx.active_state.as_deref().unwrap_or("");
    let outgoing: Vec<_> = graph["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|edge| edge["from"].as_str() == Some(current))
        .collect();
    let nodes: Vec<_> = graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|node| json!({"id":node["id"], "title":node["title"], "purpose":node["description"]}))
        .collect();
    format!("\n[WORKFLOW GRAPH]\nWorkflow: {}. Purpose: {}. Current node: {current}. Nodes: {}.\nOutgoing edges (stable zero-based indices, including blocked choices): {}. Visited nodes: {}.\nChoose a declared eligible edge with agent_next {{\"edge\":INDEX,\"from_state\":\"{current}\"}} or its active IR opcode. Without edge, exactly one eligible edge is required. agent_back (or its IR opcode) returns to the last visited node in this task; it does not undo source changes. Conditions and current verified receipts are enforced by Praxis. Never change active_state with set_context. IR tables replace the global table in states that declare one. Read the new table and choices after every transition.\n", sm.name, sm.description, serde_json::to_string(&nodes).unwrap_or_default(), serde_json::to_string(&outgoing).unwrap_or_default(), graph["history"])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_graph_branch_indices_do_not_shift_when_blocked() {
        let sm = crate::sm::parse("@routing graph\n[state a]\n[state b]\n[state c]\n[transitions]\na -> b : when flag == true\na -> c").unwrap();
        let edges = edges(&sm, &json!({"flag":false}), "graph-index-test");
        assert!(!edges[0].eligible);
        assert_eq!(choose(&edges, "a", None).unwrap().to, "c");
        assert!(choose(&edges, "a", Some(0)).is_err());
        assert_eq!(choose(&edges, "a", Some(1)).unwrap().id, "a:1");
        assert!(choose(&edges, "a", Some(2)).is_err());
    }
    #[tokio::test]
    async fn state_graph_navigation_history_ir_and_no_model_state_bypass() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("contexts")).unwrap();
        let text = "@routing graph\n@start a\n[state a]\n[state b]\n[state c]\n[transitions]\na -> b\na -> c\n[decision_ir a]\nN = agent_next\n[decision_ir b]\nK = agent_back";
        std::fs::write(dir.path().join("contexts/branch.sm"), text).unwrap();
        let sm = crate::sm::parse(text).unwrap();
        let db = Database::new(&dir.path().join("data")).unwrap();
        let user = "graph-history-test";
        let mut ctx = db.load_context(user).unwrap();
        ctx.settings.sm_file = Some("branch".into());
        ctx.active_state = Some("a".into());
        ctx.settings.active_state = ctx.active_state.clone();
        db.save_context(&ctx).unwrap();
        let _task = super::super::task_control::begin(user).unwrap();
        super::super::action_contracts::bind(user, "branch", &sm, dir.path()).unwrap();
        super::super::action_contracts::validate_context(&ctx, &ctx).unwrap();
        assert!(navigate(&db, dir.path(), user, false, &json!({}))
            .await
            .is_err());
        assert!(db
            .merge_context_from_agent(user, json!({"active_state":"b"}))
            .is_err());
        assert!(navigate(
            &db,
            dir.path(),
            user,
            false,
            &json!({"edge":1,"from_state":"stale"})
        )
        .await
        .is_err());
        navigate(&db, dir.path(), user, false, &json!({"edge":0}))
            .await
            .unwrap();
        assert_eq!(history(user), vec!["a"]);
        assert!(
            super::super::action_contracts::decision_ir_mapping_for(user, "b")
                .unwrap()
                .contains_key("K")
        );
        navigate(&db, dir.path(), user, true, &json!({}))
            .await
            .unwrap();
        assert_eq!(
            db.load_context(user).unwrap().active_state.as_deref(),
            Some("a")
        );
        assert!(history(user).is_empty());
        assert!(navigate(&db, dir.path(), user, true, &json!({}))
            .await
            .is_err());
    }

    fn graph_fixture(
        user: &str,
        text: &str,
    ) -> (
        tempfile::TempDir,
        Database,
        Context,
        super::super::task_control::TaskGuard,
    ) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("contexts")).unwrap();
        std::fs::write(dir.path().join("contexts/branch.sm"), text).unwrap();
        let db = Database::new(&dir.path().join("data")).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let mut ctx = db.load_context(user).unwrap();
        ctx.settings.sm_file = Some("branch".into());
        ctx.active_state = Some("a".into());
        ctx.settings.active_state = ctx.active_state.clone();
        db.save_context(&ctx).unwrap();
        let task = super::super::task_control::begin(user).unwrap();
        let sm = crate::sm::parse(text).unwrap();
        super::super::action_contracts::bind(user, "branch", &sm, dir.path()).unwrap();
        super::super::action_contracts::validate_context(&ctx, &ctx).unwrap();
        (dir, db, ctx, task)
    }

    #[tokio::test]
    async fn state_graph_transition_and_audit_commit_atomically() {
        let user = "graph-atomic-audit";
        let (dir, db, before, _task) = graph_fixture(
            user,
            "@routing graph\n@start a\n[state a]\n[state b]\n[transitions]\na -> b",
        );
        // A real SQLite write failure must not leave a successful state change.
        db.conn().execute_batch("CREATE TRIGGER reject_graph_audit BEFORE INSERT ON execution_events BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END;").unwrap();
        assert!(navigate(&db, dir.path(), user, false, &json!({}))
            .await
            .is_err());
        assert_eq!(
            serde_json::to_value(db.load_context(user).unwrap()).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        assert!(history(user).is_empty());
        db.conn()
            .execute_batch("DROP TRIGGER reject_graph_audit")
            .unwrap();
        navigate(&db, dir.path(), user, false, &json!({}))
            .await
            .unwrap();
        let events = db.execution_events(user, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["payload"]["graph"]["history"], json!(["a"]));
    }

    #[tokio::test]
    async fn state_graph_guards_back_cancel_and_pinned_policy_fail_without_changes() {
        let user = "graph-guard-failures";
        let text = "@routing graph\n@start a\n[state a]\n[state b]\n[state c]\n[transitions]\na -> b\nb -> c\n[checks]\nproof = {\"program\":\"sh\",\"args\":[\"-c\",\"exit 0\"],\"resources\":[\"proof.txt\"]}\n[guards]\na = [proof]\nc = [proof]";
        let (dir, db, _ctx, _task) = graph_fixture(user, text);
        std::fs::write(dir.path().join("proof.txt"), "before").unwrap();
        navigate(&db, dir.path(), user, false, &json!({}))
            .await
            .unwrap();
        let in_b = serde_json::to_value(db.load_context(user).unwrap()).unwrap();
        assert!(navigate(&db, dir.path(), user, false, &json!({}))
            .await
            .is_err());
        assert!(navigate(&db, dir.path(), user, true, &json!({}))
            .await
            .is_err());
        assert_eq!(history(user), vec!["a"]);
        assert_eq!(
            serde_json::to_value(db.load_context(user).unwrap()).unwrap(),
            in_b
        );
        super::super::action_contracts::run(user, "proof-call", &json!({"name":"proof"}))
            .await
            .unwrap();
        // External edits invalidate the real receipt, including back navigation.
        std::fs::write(dir.path().join("proof.txt"), "changed").unwrap();
        assert!(navigate(&db, dir.path(), user, true, &json!({}))
            .await
            .is_err());
        super::super::action_contracts::run(user, "fresh-proof", &json!({"name":"proof"}))
            .await
            .unwrap();
        std::fs::write(
            dir.path().join("contexts/branch.sm"),
            format!("{text}\n[node b]\ntitle = \"Edited policy\""),
        )
        .unwrap();
        assert!(navigate(&db, dir.path(), user, true, &json!({}))
            .await
            .is_err());
        std::fs::write(dir.path().join("contexts/branch.sm"), text).unwrap();
        super::super::task_control::cancel(user);
        assert!(navigate(&db, dir.path(), user, true, &json!({}))
            .await
            .is_err());
        assert_eq!(
            serde_json::to_value(db.load_context(user).unwrap()).unwrap(),
            in_b
        );
        assert_eq!(history(user), vec!["a"]);
        assert_eq!(db.execution_events(user, 10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn state_graph_ir_reselects_table_after_navigation_and_keeps_permissions() {
        use super::super::llm::provider::{FunctionCall, ToolCall};
        let user = "graph-local-ir-execution";
        let text = "@routing graph\n@start a\n[state a]\nsettings.activated_tools = [\"execute_decision\",\"agent_next\",\"agent_back\"]\n[state b]\nsettings.activated_tools = [\"execute_decision\",\"inspect_file\",\"agent_back\"]\n[transitions]\na -> b\n[decision_ir]\nN = agent_next\nK = agent_back\n[decision_ir b]\nR = inspect_file\nK = agent_back";
        let (dir, db, mut ctx, _task) = graph_fixture(user, text);
        ctx.settings.activated_tools = vec![
            "execute_decision".into(),
            "agent_next".into(),
            "agent_back".into(),
        ];
        db.save_context(&ctx).unwrap();
        let plugins = crate::plugins::PluginRegistry::new();
        let instruction = |ir: &str| ToolCall {
            id: "ir-call".into(),
            function: FunctionCall {
                name: "execute_decision".into(),
                arguments: json!({"ir":ir}).to_string(),
            },
        };
        assert!(super::super::decision_ir::resolve(
            &db,
            user,
            &instruction("1 R {\"path\":\"src/main.rs\"}"),
            &plugins
        )
        .is_err());
        let lowered =
            super::super::decision_ir::resolve(&db, user, &instruction("1 N"), &plugins).unwrap();
        assert_eq!(lowered.function.name, "agent_next");
        navigate(&db, dir.path(), user, false, &json!({}))
            .await
            .unwrap();
        assert!(
            super::super::decision_ir::resolve(&db, user, &instruction("1 N"), &plugins).is_err()
        );
        assert_eq!(
            super::super::decision_ir::resolve(
                &db,
                user,
                &instruction("1 R {\"path\":\"src/main.rs\"}"),
                &plugins
            )
            .unwrap()
            .function
            .name,
            "inspect_file"
        );
        let mut ctx = db.load_context(user).unwrap();
        ctx.settings
            .activated_tools
            .retain(|name| name != "inspect_file");
        db.save_context(&ctx).unwrap();
        assert!(super::super::decision_ir::resolve(
            &db,
            user,
            &instruction("1 R {\"path\":\"src/main.rs\"}"),
            &plugins
        )
        .is_err());
    }
}
