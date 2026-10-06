//! Shared POML contract for both message paths and dashboard previews.
//! Routing is performed before rendering; builders never consult secrets.
use crate::db::{contexts::Context, Database};
use crate::plugins::PluginRegistry;
use serde_json::{json, Value};
use std::path::Path;

/// Completion belongs to one task, not the persisted conversation. Call only
/// when starting a NEW task under its per-user guard, never on tool follow-ups,
/// retries, injected input or previews. A completed graph restarts at its entry
/// node; legacy workflows keep their state. Preserve user-only settings.
pub fn reset_task_completion(db: &Database, user_id: &str) -> anyhow::Result<()> {
    reset_task_completion_in(db, Path::new("."), user_id)
}
pub fn reset_task_completion_in(db: &Database, root: &Path, user_id: &str) -> anyhow::Result<()> {
    let mut ctx = db.load_context(user_id)?;
    if reset_completion(&mut ctx, root)? {
        db.save_context(&ctx)?;
        tracing::debug!(user_id, "Cleared previous task completion flag");
    }
    Ok(())
}

/// Read-only preparation is also used by inference setup validation. Do not
/// clear persisted completion or emit context events when setup is incomplete.
pub(crate) fn reset_completion(ctx: &mut Context, root: &Path) -> anyhow::Result<bool> {
    if ctx.settings.done {
        ctx.settings.done = false;
        if let Ok(sm) = crate::sm::load_file_in(&root.join("contexts"), workflow_name(&ctx)) {
            if sm.is_graph() {
                let start = sm
                    .entry_state()
                    .ok_or_else(|| anyhow::anyhow!("Completed graph has no entry state"))?;
                let identity = (ctx.user_id.clone(), ctx.session_id.clone());
                let thinking = ctx.settings.show_thinking;
                let skill = ctx.settings.active_skill.clone();
                let mut value = serde_json::to_value(&ctx)?;
                anyhow::ensure!(
                    crate::sm::apply_state(&sm, &mut value, start),
                    "Graph start cannot be applied"
                );
                *ctx = serde_json::from_value(value)?;
                ctx.settings.active_state = ctx.active_state.clone();
                anyhow::ensure!(
                    (ctx.user_id.clone(), ctx.session_id.clone()) == identity,
                    "Workflow must preserve user/session identity"
                );
                anyhow::ensure!(
                    ctx.settings.show_thinking == thinking && ctx.settings.active_skill == skill,
                    "Thinking visibility and persistent skill are user-only"
                );
            }
        }
        return Ok(true);
    }
    Ok(false)
}

pub const OMITTED_HISTORY_IMAGES_NOTICE: &str = "Some earlier inline images are omitted from this request by the configured history-image policy. Their saved text/path references remain. Re-read relevant images with enabled tools before making visual claims; images read by the current task are eligible for the latest-two image window.";

/// Keep the latest two eligible image-bearing history messages, as before.
/// A zero previous-image budget excludes old task attachments but not freshly
/// read tool images/current user attachments. Never mutate the saved history.
pub fn history_image_indices(
    history: &[crate::db::messages::Message],
    current_tool_ids: &std::collections::HashSet<String>,
    previous_limit: usize,
) -> std::collections::HashSet<usize> {
    let last_user = history.iter().rposition(|msg| msg.role == "user");
    let mut selected = std::collections::HashSet::new();
    let mut previous = 0;
    for (index, msg) in history.iter().enumerate().rev() {
        if !msg.content_parts.as_ref().is_some_and(|parts| !parts.is_empty()) {
            continue;
        }
        let current = Some(index) == last_user
            || msg.tool_call_id.as_ref().is_some_and(|id| current_tool_ids.contains(id));
        if !current {
            if previous >= previous_limit { continue; }
            previous += 1;
        }
        selected.insert(index);
        if selected.len() == 2 { break; }
    }
    selected
}

pub fn workflow_name(ctx: &Context) -> &str {
    ctx.settings
        .sm_file
        .as_deref()
        .or(ctx.sm_file.as_deref())
        .unwrap_or("standard")
}

/// Prepare a context without persistence (also used by preview). All persona
/// decisions belong in the selected SM file, never in the message handler.
pub async fn route_context(
    root: &Path,
    ctx: &mut Context,
    input: &str,
    plugins: &PluginRegistry,
    channel_id: Option<&str>,
) -> anyhow::Result<()> {
    route_context_with_workspace(root, root, ctx, input, plugins, channel_id).await
}

pub async fn route_context_with_workspace(
    root: &Path,
    workspace: &Path,
    ctx: &mut Context,
    input: &str,
    plugins: &PluginRegistry,
    channel_id: Option<&str>,
) -> anyhow::Result<()> {
    let candidate = super::workflow_actions::plan(root,ctx,input,plugins,channel_id).await?;
    let workflow = crate::sm::load_file_in(&root.join("contexts"), workflow_name(&candidate))
        .map_err(|e| anyhow::anyhow!("Workflow routing failed: {e}"))?;
    super::action_contracts::bind(&ctx.user_id, workflow_name(&candidate), &workflow, workspace)?;
    super::action_contracts::validate_context(ctx, &candidate)?;
    *ctx = candidate;
    Ok(())
}

pub(crate) async fn route_context_once(
    root: &Path,
    ctx: &mut Context,
    input: &str,
    plugins: &PluginRegistry,
    channel_id: Option<&str>,
) -> anyhow::Result<()> {
    if ctx.custom_data.is_null() {
        ctx.custom_data = json!({});
    }
    let data = ctx
        .custom_data
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("custom_data must be an object"))?;
    for (key, value) in plugins.context_defaults() {
        data.entry(key).or_insert(value);
    }
    data.insert("user_prompt".into(), json!(input));
    data.entry("user_template".to_string())
        .or_insert(json!("user"));
    if let Some(channel) = channel_id
        .filter(|s| !s.is_empty())
        .or(ctx.settings.feedback_channel_id.as_deref())
    {
        data.entry("channel_id".to_string())
            .or_insert(json!(channel));
    }
    if ctx.active_state.is_none() {
        ctx.active_state = ctx.settings.active_state.clone();
    }
    let workflow = crate::sm::load_file_in(&root.join("contexts"), workflow_name(ctx))
        .map_err(|e| anyhow::anyhow!("Workflow routing failed: {e}"))?;
    let mut value = serde_json::to_value(&*ctx)?;
    // Secret overrides are deliberately not applied to global credentials.
    let _ = crate::sm::apply_to_context(&workflow, &mut value).await;
    crate::db::contexts::normalize_legacy_keys(&mut value);
    let mut routed: Context = serde_json::from_value(value)?;
    anyhow::ensure!(
        routed.user_id == ctx.user_id && routed.session_id == ctx.session_id,
        "Workflow must not change user/session identity"
    );
    if let Some(active) = routed.active_state.as_deref().filter(|s| !s.is_empty()) {
        anyhow::ensure!(
            active != "_default" && workflow.states.contains_key(active),
            "Workflow selected an undefined state: {active}"
        );
    }
    anyhow::ensure!(routed.settings.active_skill == ctx.settings.active_skill,
        "Persistent skill selection is user-only; workflows must use task-local use_skill instead");
    anyhow::ensure!(routed.settings.show_thinking == ctx.settings.show_thinking,
        "Thinking visibility is user-only");
    crate::tags::validate_tag_prefix(&routed.settings.tag_prefix)?;
    routed.settings.active_state = routed.active_state.clone();
    if routed.custom_data.is_null() {
        routed.custom_data = json!({});
    }
    anyhow::ensure!(
        routed.custom_data.is_object(),
        "Workflow custom_data must remain an object"
    );
    routed.custom_data["user_prompt"] = json!(input);
    crate::gateway::templates::resolve_template(
        &root.join("templates"),
        routed
            .settings
            .system_template
            .as_deref()
            .unwrap_or("standard"),
    )?;
    *ctx = routed;
    Ok(())
}

pub async fn prepare_runtime(
    state: &super::GatewayState,
    user_id: &str,
    input: &str,
    turn: Option<i32>,
    channel_id: Option<&str>,
) -> anyhow::Result<Context> {
    super::task_control::check_registry(user_id, &state.plugins)?;
    let mut ctx = state.db.load_context(user_id)?;
    if let Some(turn) = turn {
        ctx.turn = turn;
    }
    let workspace = state.config.workspace_root()?;
    if super::task_control::cancellation(user_id).is_some() {
        super::task_control::pin_workspace(user_id, &workspace)?;
    }
    let root = Path::new(&state.config.root_dir);
    let candidate = super::workflow_actions::plan(root, &ctx, input, &state.plugins, channel_id).await?;
    let workflow = crate::sm::load_file_in(&root.join("contexts"), workflow_name(&candidate))
        .map_err(|e| anyhow::anyhow!("Workflow routing failed: {e}"))?;
    super::workflow_preflight::validate(&state.db, &state.plugins, workflow_name(&candidate), &workflow, &candidate)?;
    super::action_contracts::bind(user_id, workflow_name(&candidate), &workflow, &workspace)?;
    super::action_contracts::validate_context(&ctx, &candidate)?;
    ctx = candidate;
    super::task_control::set_show_thinking(user_id, ctx.settings.show_thinking);
    state.db.save_context(&ctx)?;
    Ok(ctx)
}

/// Prefer explicit preview input, then the last raw runtime input. On legacy
/// contexts fall back to the most recent USER record, never an assistant/tool.
pub fn preview_input(
    db: &Database,
    ctx: &Context,
    explicit: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(input) = explicit {
        return Ok(input.to_string());
    }
    if let Some(input) = ctx.custom_data.get("user_prompt").and_then(Value::as_str) {
        return Ok(input.to_string());
    }
    let (messages, _) = db.get_messages_with_token_budget(&ctx.user_id, usize::MAX)?;
    Ok(messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_else(|| "Preview message".into()))
}

/// Complete non-sensitive defaults, shared by runtime/preview enrichment and
/// synthetic template validation. No database, provider or secret lookup.
pub fn base_context(ctx: &Context, input: &str) -> anyhow::Result<Value> {
    let mut value = serde_json::to_value(ctx)?;
    value["username"] = json!(ctx.username.as_deref().unwrap_or("User"));
    value["settings"]["system_template"] = json!(ctx
        .settings
        .system_template
        .as_deref()
        .unwrap_or("standard"));
    value["system_template"] = value["settings"]["system_template"].clone();
    value["sm_file"] = json!(workflow_name(ctx));
    value["active_state"] = json!(ctx
        .active_state
        .as_deref()
        .or(ctx.settings.active_state.as_deref())
        .unwrap_or(""));
    value["active_templates"] = json!(if ctx.active_templates.is_empty() {
        &ctx.settings.active_templates
    } else {
        &ctx.active_templates
    });
    value["user_prompt"] = json!(input);
    value["user_message"] = json!(input);
    value["system_info"] = json!(format!("Praxis v{}", env!("CARGO_PKG_VERSION")));
    value["path"] = json!(if ctx.settings.path.is_empty() {
        std::env::current_dir()?.to_string_lossy().to_string()
    } else {
        ctx.settings.path.clone()
    });
    value["time"] = json!(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
    value["utc_now"] = json!(chrono::Utc::now().to_rfc3339());
    if !value["custom_data"].is_object() {
        value["custom_data"] = json!({});
    }
    value["custom_data"]["user_prompt"] = json!(input);
    if value["sm_data"].is_null() {
        value["sm_data"] = json!({});
    }
    value["used_tools_history_size"] = json!(ctx.settings.tool_history_limit);
    value["tag_instructions"] = json!(if ctx.settings.tags_enabled {
        crate::tags::get_tag_instructions_with_prefix(&ctx.settings.tag_prefix)?
    } else {
        String::new()
    });
    value["user_template"] = json!(ctx
        .custom_data
        .get("user_template")
        .and_then(Value::as_str)
        .unwrap_or("user"));
    value["active_skill"] = json!(ctx.settings.active_skill.as_deref().unwrap_or(""));
    value["active_skill_instructions"] = json!("");
    value["skills"] = json!([]);
    value["skill_discovery_instructions"] = json!("");
    value["tools"] = json!([]);
    value["memory"] = json!({"profile": crate::db::memory_profiles::mode(ctx)?, "profile_exists": false, "profile_loaded": false,
        "profiles": [], "shared": {}, "facts": [], "topics": [], "preferences": {}, "variables": {}});
    value["paired_users"] = json!([]);
    value["paired_users_count"] = json!(0);
    value["uptime_secs"] = json!(0);
    value["uptime"] = json!("0m 0s");
    value["conversation_text"] = json!(format!("user: {input}"));
    value["tokens_used"] = json!(0);
    value["tokens_limit"] = json!(ctx.settings.history_token_limit.unwrap_or(32000));
    value["tokens_percentage"] = json!("0.0");
    value["compaction_token_limit"] = json!(super::compaction::threshold(&ctx.settings));
    value["compaction_percentage"] = json!("0.0");
    value["message_count"] = json!(0);
    Ok(value)
}

pub async fn build_context(
    db: &Database,
    ctx: &Context,
    input: &str,
    plugins: &PluginRegistry,
    uptime_secs: u64,
    root: &Path,
) -> anyhow::Result<Value> {
    let memory_view = crate::db::memory_profiles::snapshot(db, ctx)?;
    let memory = &memory_view.memory;
    let tools = crate::tools::discovery::definitions(db, plugins, &ctx.user_id)?;
    let tools: Vec<_> = tools.iter().map(|t| json!({"name": t.function.name, "description": t.function.description, "parameters": t.function.parameters})).collect();
    let (messages, tokens_used) = db.get_messages_with_token_budget(&ctx.user_id, usize::MAX)?;
    let token_limit = ctx.settings.history_token_limit.unwrap_or(32000);
    let compaction_limit = super::compaction::threshold(&ctx.settings);
    let percentage = |limit: usize| {
        if limit == 0 {
            "0.0".to_string()
        } else {
            format!(
                "{:.1}",
                (tokens_used as f64 / limit as f64 * 100.0).min(100.0)
            )
        }
    };
    // Keep compatibility metadata scoped to this user, not other users' IDs.
    let paired: Vec<_> = db.list_all_pairings()?.into_iter().filter(|p| p.user_id == ctx.user_id).map(|p| json!({"user_id": p.user_id, "discord_user_id": p.discord_user_id, "paired_at": p.paired_at})).collect();
    let mut value = base_context(ctx, input)?;
    value["uptime_secs"] = json!(uptime_secs);
    value["uptime"] = json!(format!("{}m {}s", uptime_secs / 60, uptime_secs % 60));
    value["paired_users_count"] = json!(paired.len());
    value["paired_users"] = json!(paired);
    value["tools"] = json!(tools);
    value["memory"] = json!({"profile": memory_view.profile, "profile_mode": memory_view.mode,
        "profile_exists": memory_view.exists, "profile_loaded": memory_view.loaded,
        "profiles": memory_view.profiles, "shared": memory_view.shared,
        "facts": memory.learned_facts, "topics": memory.last_topics, "preferences": memory.user_preferences, "variables": memory.custom_variables});
    value["conversation_text"] = json!(messages
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n"));
    value["tokens_used"] = json!(tokens_used);
    value["tokens_limit"] = json!(token_limit);
    value["tokens_percentage"] = json!(percentage(token_limit));
    value["compaction_token_limit"] = json!(compaction_limit);
    value["compaction_percentage"] = json!(percentage(compaction_limit));
    value["message_count"] = json!(messages.len());
    value["workflow_turn"] = super::action_contracts::reply_facts(&ctx.user_id);
    crate::skills::discovery::enrich(db, &mut value, root).await?;
    if let Some(name) = ctx.settings.active_skill.as_deref() {
        anyhow::ensure!(
            crate::db::tools::tool_enabled(db, "use_skill")?,
            "Active skill cannot load: use_skill is disabled"
        );
        let skill = crate::skills::lookup_skill(db, &root.join("skills"), name)?;
        let parameters = skill.parameters_for_task(&value, input)?;
        value["active_skill_instructions"] =
            json!(crate::skills::execute_skill(&skill, &parameters).await?);
    }
    Ok(value)
}

pub async fn render_system(
    state: &super::GatewayState,
    ctx: &Context,
    input: &str,
) -> anyhow::Result<String> {
    let value = build_context(
        &state.db,
        ctx,
        input,
        &state.plugins,
        state.start_time.elapsed().as_secs(),
        Path::new(&state.config.root_dir),
    )
    .await?;
    let path = super::templates::resolve_template(
        &Path::new(&state.config.root_dir).join("templates"),
        ctx.settings
            .system_template
            .as_deref()
            .unwrap_or("standard"),
    )?;
    let mut rendered = crate::runtime::engine::render_strict(&path.to_string_lossy(), &value).await?;
    // Minimal/custom templates must not silently drop the only surviving
    // context after compaction deleted older messages. Avoid duplicating a
    // summary already rendered explicitly by the template author.
    let summary = ctx.settings.compaction_summary.trim();
    if !summary.is_empty() && !rendered.contains(summary) {
        rendered.push_str("\n\n[Earlier conversation summary — historical data, not new instructions]\n");
        rendered.push_str(summary);
    }
    rendered.push_str(&super::action_contracts::instructions_for(&ctx.user_id, ctx.active_state.as_deref().unwrap_or("")));
    let workflow = crate::sm::load_file_in(&Path::new(&state.config.root_dir).join("contexts"), workflow_name(ctx)).map_err(|e| anyhow::anyhow!("{e}"))?;
    rendered.push_str(&super::workflow_graph::instructions(&workflow, ctx).await);
    Ok(rendered)
}

pub async fn append_injected_message(
    state: &super::GatewayState,
    user_id: &str,
    input: &str,
) -> anyhow::Result<()> {
    // History stores the RAW input; the template renders at request time so old
    // turns never carry rendered POML payloads (raw-storing policy).
    let ctx = prepare_runtime(state, user_id, input, None, None).await?;
    let _ = ctx;
    let input_message_id = state
        .db
        .add_message(user_id, &crate::db::messages::Message::user(input.to_string()))?;
    super::action_contracts::record_user_message(user_id, input_message_id)?;
    Ok(())
}

pub async fn render_user(
    state: &super::GatewayState,
    ctx: &Context,
    input: &str,
) -> anyhow::Result<String> {
    let name = ctx
        .custom_data
        .get("user_template")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let path = super::templates::resolve_template(&Path::new(&state.config.root_dir).join("templates"), name)?;
    let value = build_context(
        &state.db,
        ctx,
        input,
        &state.plugins,
        state.start_time.elapsed().as_secs(),
        Path::new(&state.config.root_dir),
    )
    .await?;
    crate::runtime::engine::render_strict(&path.to_string_lossy(), &value).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn state_graph_bundled_example_selects_its_prompt_over_a_previous_workflow() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut ctx = Context {
            user_id: "graph-example-prompt".into(),
            ..Default::default()
        };
        ctx.settings.sm_file = Some("branching-coding".into());
        ctx.active_state = Some("route".into());
        ctx.settings.active_state = ctx.active_state.clone();
        ctx.settings.system_template = Some("states/standard/standard".into());
        ctx.settings.use_decision_router = true;
        route_context(
            root,
            &mut ctx,
            "Implement the game",
            &PluginRegistry::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            ctx.settings.system_template.as_deref(),
            Some("graph-coding")
        );
        assert_eq!(ctx.settings.max_llm_turns, Some(40));
        assert_eq!(ctx.settings.max_tool_calls, Some(80));
        assert!(!ctx.settings.use_decision_router);
        let sm = crate::sm::load_file_in(&root.join("contexts"), "branching-coding").unwrap();
        assert_eq!(
            super::super::workflow_graph::view(&sm, &ctx).await["edges"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["from"] == "route")
                .count(),
            5
        );
        assert!(sm
            .states
            .iter()
            .filter(|(name, _)| name.as_str() != "_default")
            .all(|(_, state)| state
                .variables
                .get("settings.system_template")
                .is_some_and(|v| v == "graph-coding")));
    }

    #[test]
    fn state_graph_completed_task_restarts_at_entry_and_preserves_user_settings() {
        let root = fixture();
        std::fs::write(root.path().join("contexts/graph.sm"), "@routing graph\n@start a\n[state a]\nsettings.system_template = \"selected\"\n[state done]").unwrap();
        let db = Database::new(&root.path().join("data")).unwrap();
        let mut ctx = db.load_context("graph-restart").unwrap();
        ctx.settings.sm_file = Some("graph".into());
        ctx.active_state = Some("done".into());
        ctx.settings.active_state = ctx.active_state.clone();
        ctx.settings.done = true;
        ctx.settings.show_thinking = true;
        ctx.settings.model = Some("my-model".into());
        ctx.settings.active_skill = Some("my-skill".into());
        db.save_context(&ctx).unwrap();
        reset_task_completion_in(&db, root.path(), &ctx.user_id).unwrap();
        let next = db.load_context(&ctx.user_id).unwrap();
        assert_eq!(next.active_state.as_deref(), Some("a"));
        assert_eq!(next.settings.active_state, next.active_state);
        assert!(!next.settings.done);
        assert_eq!(next.settings.system_template.as_deref(), Some("selected"));
        assert!(next.settings.show_thinking);
        assert_eq!(next.settings.model, ctx.settings.model);
        assert_eq!(next.settings.active_skill, ctx.settings.active_skill);
        assert_eq!(next.user_id, ctx.user_id);
        assert_eq!(next.session_id, ctx.session_id);
    }

    #[tokio::test]
    async fn state_graph_poml_protocol_renders_with_real_cli() {
        if std::env::var_os("POML_CLI").is_none() {
            return;
        }
        let text = crate::runtime::engine::render_strict(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("templates/graph-coding.poml")
                .to_string_lossy(),
            &json!({}),
        )
        .await
        .unwrap();
        assert!(text.contains("runtime appends [WORKFLOW GRAPH]"));
        assert!(
            text.contains(r#"{"ir":"1 N {\"edge\":0,\"from_state\":\"route\"}"}"#),
            "{text}"
        );
        assert!(text.contains("current verified modification, build and test receipts"));
    }

    #[test]
    fn history_image_policy_keeps_current_tools_and_never_deletes_saved_images() {
        use crate::db::messages::Message;
        let image = |id: &str| Message::tool_with_image(
            format!("Saved image path: /synthetic/{id}.png"), id.into(),
            vec![json!({"type":"image_url","image_url":{"url":"data:image/png;base64,SYNTHETIC"}})],
        );
        let history = vec![image("old-a"), image("old-b"), Message::user("new task".into()), image("current")];
        let before = serde_json::to_value(&history).unwrap();
        let current = std::collections::HashSet::from(["current".into()]);
        assert_eq!(history_image_indices(&history, &current, 0), std::collections::HashSet::from([3]));
        for limit in [1, 2] {
            assert_eq!(history_image_indices(&history, &current, limit), std::collections::HashSet::from([1, 3]));
        }
        assert!(history_image_indices(&history[..3], &current, 0).is_empty());
        assert_eq!(history_image_indices(&history[..3], &current, 2), std::collections::HashSet::from([0, 1]));
        assert_eq!(serde_json::to_value(&history).unwrap(), before);
        let mut attached = Message::user("new attached image".into());
        attached.content_parts = history[0].content_parts.clone();
        assert_eq!(history_image_indices(&[attached], &Default::default(), 0), std::collections::HashSet::from([0]));
    }

    // Exercise the shipped workflows/templates without changing the working
    // directory, editing fixtures in the repository, or invoking POML/LLMs.
    async fn route_shipped(ctx: &mut Context, input: &str) {
        route_context(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            ctx,
            input,
            &PluginRegistry::new(),
            None,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn backend_repo_explicit_role_requests_route_through_the_model() {
        for input in [
            "Be my language instructor.",
            "Teach me Japanese.",
            "Sei bitte mein Sprachtrainer.",
        ] {
            let mut ctx = Context { user_id: "alice".into(), ..Default::default() };
            route_shipped(&mut ctx, input).await;
            // Routing is model-driven: the shipped regex rules no longer switch templates.
        // None is the default-standard state; the model routes via set_context.
        assert!(matches!(ctx.settings.system_template.as_deref(), None | Some("states/standard/standard")), "{input}");
            assert_eq!(ctx.active_state.as_deref(), Some("routing"));
            assert_eq!(ctx.custom_data["user_prompt"], input);
        }
    }

    #[tokio::test]
    async fn backend_repo_model_role_switch_changes_state_template_and_tools() {
        let mut ctx = Context { user_id: "alice".into(), ..Default::default() };
        route_shipped(&mut ctx, "hi").await;
        assert_eq!(ctx.active_state.as_deref(), Some("routing"));
        let before = crate::tools::registry::build_tool_definitions(&ctx.settings, None, None);
        assert!(!before.iter().any(|t| t.function.name == "execute_terminal"));
        assert!(before.iter().any(|t| t.function.name == "set_context"), "every state must keep set_context to switch roles");

        // What set_context("sm_data.role", "code") persists; the next routing pass must react.
        ctx.sm_data["role"] = serde_json::json!("code");
        route_shipped(&mut ctx, "please fix the build").await;
        assert_eq!(ctx.active_state.as_deref(), Some("code"));
        assert_eq!(ctx.settings.system_template.as_deref(), Some("states/code/code"));
        let after = crate::tools::registry::build_tool_definitions(&ctx.settings, None, None);
        assert_eq!(after.iter().any(|t| t.function.name == "execute_terminal"), cfg!(feature = "shell"));
        assert!(!ctx.settings.tool_group_definitions.is_empty(), "workflow [tool_groups] land in the context");

        ctx.sm_data["role"] = serde_json::json!("standard");
        route_shipped(&mut ctx, "thanks").await;
        assert_eq!(ctx.active_state.as_deref(), Some("standard"));
        assert!(!crate::tools::registry::build_tool_definitions(&ctx.settings, None, None).iter().any(|t| t.function.name == "execute_terminal"));
    }

    #[tokio::test]
    async fn backend_repo_language_instructor_sticks_on_next_ordinary_task() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).unwrap();
        let mut ctx = Context { user_id: "alice".into(), ..Default::default() };
        route_shipped(&mut ctx, "Be my language instructor.").await;
        db.save_context(&ctx).unwrap();
        let mut next = db.load_context("alice").unwrap();
        let input = "Explain why this sentence uses the past tense.";
        route_shipped(&mut next, input).await;
        assert!(matches!(next.settings.system_template.as_deref(), None | Some("states/standard/standard")));
        assert_eq!(next.custom_data["user_prompt"], input);
    }

    #[tokio::test]
    async fn backend_repo_manual_template_is_preserved() {
        let mut ctx = Context { user_id: "alice".into(), ..Default::default() };
        ctx.settings.system_template = Some("researcher".into());
        route_shipped(&mut ctx, "Compare the evidence for these two claims.").await;
        assert_eq!(ctx.settings.system_template.as_deref(), Some("researcher"));
    }

    #[tokio::test]
    async fn backend_repo_quoted_role_request_does_not_switch_template() {
        for input in [
            r#""Be my language instructor" is a quoted example; explain its wording."#,
            r#"Translate "Be my language instructor" into French."#,
            "> Be my language instructor",
        ] {
            let mut ctx = Context { user_id: "alice".into(), ..Default::default() };
            ctx.settings.system_template = Some("researcher".into());
            route_shipped(&mut ctx, input).await;
            assert_eq!(ctx.settings.system_template.as_deref(), Some("researcher"), "{input}");
        }
    }

    #[tokio::test]
    async fn backend_repo_explicit_role_reset_keeps_standard() {
        for reset in ["reset role.", "return to standard"] {
            let mut ctx = Context { user_id: "alice".into(), ..Default::default() };
            route_shipped(&mut ctx, "Be my language instructor.").await;
            assert!(matches!(ctx.settings.system_template.as_deref(), None | Some("states/standard/standard")));
            route_shipped(&mut ctx, reset).await;
            assert!(matches!(ctx.settings.system_template.as_deref(), None | Some("states/standard/standard")), "{reset}");
            let stored = base_context(&ctx, reset).unwrap()["system_template"].clone();
            assert!(stored.is_null() || stored == "states/standard/standard", "{stored}");
        }
    }

    #[tokio::test]
    async fn backend_repo_all_shipped_sm_states_resolve_templates() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let contexts = root.join("contexts");
        let mut names: Vec<_> = std::fs::read_dir(&contexts)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sm"))
            .map(|path| path.file_stem().unwrap().to_str().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["20-tasks", "branching-coding", "language-learning", "standard", "standard-verified", "verified-capabilities", "verified-coding", "verified-implementation"]);
        let mut explicit_selections = 0;
        for name in names {
            let sm = crate::sm::load_file_in(&contexts, &name).unwrap();
            assert!(!sm.states.is_empty(), "{name}");
            if name == "standard" {
                assert!(sm.overrides.iter().any(|rule| rule.key == "sm_data.persona_roles"), "{name}");
                let mut stale = Context { user_id: "stale-catalog".into(),
                    sm_data: serde_json::json!({"persona_roles":"obsolete", "role":"code"}), ..Default::default() };
                route_shipped(&mut stale, "Continue coding.").await;
                assert!(stale.sm_data["persona_roles"].as_str().unwrap().contains("research"));
                assert!(!stale.sm_data["persona_roles"].as_str().unwrap().contains("obsolete"));
            } else if name == "20-tasks" {
                assert!(sm.auto_rules.is_empty(), "the experiment must not classify tasks deterministically");
                assert!(sm.states.values().all(|state| state.variables.contains_key("sm_data.role")));
            }
            for (state_name, state) in sm.states.iter().filter(|(name,_)| name.as_str() != "_default") {
                let mut ctx = Context {
                    user_id: "alice".into(),
                    sm_file: Some(name.clone()),
                    active_state: Some(state_name.clone()),
                    ..Default::default()
                };
                route_shipped(&mut ctx, "Ordinary task with no role change.").await;
                if let Some(selected) = state.variables.get("settings.system_template") {
                    explicit_selections += 1;
                    assert_eq!(ctx.settings.system_template.as_deref(), Some(selected.as_str()), "{name}:{state_name}");
                }
                let selected = ctx.settings.system_template.as_deref().unwrap_or("standard");
                crate::gateway::templates::resolve_template(&root.join("templates"), selected)
                    .unwrap_or_else(|error| panic!("{name}:{state_name} -> {selected}: {error}"));
            }
        }
        assert_eq!(explicit_selections, 44, "all shipped state template contracts must be exercised");
    }

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("contexts")).unwrap();
        std::fs::create_dir(root.path().join("templates")).unwrap();
        for name in ["standard", "selected"] {
            std::fs::write(
                root.path().join(format!("templates/{name}.poml")),
                "<poml><p>test</p></poml>",
            )
            .unwrap();
        }
        std::fs::write(root.path().join("contexts/standard.sm"), "@steps [base]\n[state base]\n[overrides]\nif custom_data.user_prompt =~ \"route-me\" -> settings.system_template = \"selected\"\n").unwrap();
        root
    }

    #[tokio::test]
    async fn backend_routing_sees_current_input_before_template_selection() {
        let root = fixture();
        let mut ctx = Context {
            user_id: "alice".into(),
            custom_data: json!({"user_prompt":"stale"}),
            ..Default::default()
        };
        route_context(
            root.path(),
            &mut ctx,
            "route-me now",
            &PluginRegistry::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(ctx.settings.system_template.as_deref(), Some("selected"));
        assert_eq!(ctx.custom_data["user_prompt"], "route-me now");
        assert_eq!(ctx.active_state, ctx.settings.active_state);
        ctx.settings.system_template = None;
        route_context(
            root.path(),
            &mut ctx,
            "no match",
            &PluginRegistry::new(),
            None,
        )
        .await
        .unwrap();
        assert!(ctx.settings.system_template.is_none());
    }

    #[tokio::test]
    async fn backend_changed_workflow_does_not_keep_an_unknown_old_state() {
        let root = fixture();
        std::fs::write(
            root.path().join("contexts/alternate.sm"),
            "@steps [new]\n[state new]\nsettings.system_template = \"selected\"\n",
        )
        .unwrap();
        let db = Database::new(&root.path().join("db")).unwrap();
        let mut ctx = Context {
            user_id: "alice".into(),
            ..Default::default()
        };
        let plugins = PluginRegistry::new();
        route_context(root.path(), &mut ctx, "input", &plugins, None).await.unwrap();
        db.save_context(&ctx).unwrap();
        db.merge_context("alice", json!({"settings.sm_file":"alternate"}))
            .unwrap();
        let mut fresh = db.load_context("alice").unwrap();
        route_context(root.path(), &mut fresh, "current input", &plugins, None).await.unwrap();
        assert_eq!(fresh.active_state.as_deref(), Some("new"));
        assert_eq!(fresh.settings.system_template.as_deref(), Some("selected"));
        assert_eq!(fresh.custom_data["user_prompt"], "current input");
    }

    #[test]
    fn backend_preview_never_uses_assistant_or_tool_as_current_input() {
        let root = fixture();
        let db = Database::new(&root.path().join("db")).unwrap();
        let mut ctx = Context {
            user_id: "alice".into(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();
        db.add_message(
            "alice",
            &crate::db::messages::Message::user("user input".into()),
        )
        .unwrap();
        db.add_message(
            "alice",
            &crate::db::messages::Message::assistant("assistant output".into()),
        )
        .unwrap();
        assert_eq!(preview_input(&db, &ctx, None).unwrap(), "user input");
        ctx.custom_data = json!({"user_prompt":"raw runtime input"});
        assert_eq!(preview_input(&db, &ctx, None).unwrap(), "raw runtime input");
        assert_eq!(
            preview_input(&db, &ctx, Some("explicit")).unwrap(),
            "explicit"
        );
    }

    #[tokio::test]
    async fn backend_active_skill_rejects_unknown_disabled_or_unmapped_inputs() {
        let root = fixture();
        let db = Database::new(&root.path().join("db")).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let mut ctx = Context {
            user_id: "alice".into(),
            ..Default::default()
        };
        ctx.settings.active_skill = Some("unregistered".into());
        let plugins = PluginRegistry::new();
        assert!(build_context(&db, &ctx, "task", &plugins, 0, root.path())
            .await
            .unwrap_err()
            .to_string()
            .contains("not registered"));
        let skill = root.path().join("skills/test");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("skill.json"),
            r#"{"name":"test","description":"Test", "required_parameters":["unmapped"]}"#,
        )
        .unwrap();
        std::fs::write(skill.join("skill.poml"), "<poml><p>{{unmapped}}</p></poml>").unwrap();
        ctx.settings.active_skill = Some("test".into());
        assert!(build_context(&db, &ctx, "task", &plugins, 0, root.path())
            .await
            .unwrap_err()
            .to_string()
            .contains("unmapped"));
        crate::db::tools::set_plugin_tool_enabled(&db, "use_skill", false).unwrap();
        assert!(build_context(&db, &ctx, "task", &plugins, 0, root.path())
            .await
            .unwrap_err()
            .to_string()
            .contains("disabled"));
    }

    #[tokio::test]
    async fn backend_shared_context_contract_preserves_memory_and_input() {
        let root = fixture();
        let db = Database::new(&root.path().join("db")).unwrap();
        db.add_memory("alice", "only alice", Some("fact")).unwrap();
        db.add_memory("bob", "only bob", Some("fact")).unwrap();
        crate::db::memory::update_memory(&db, "alice", |m| {
            m.user_preferences.insert("theme".into(), json!("current"));
        })
        .unwrap();
        let mut ctx = Context {
            user_id: "alice".into(),
            custom_data: json!({"pref_theme":"stale"}),
            ..Default::default()
        };
        let plugins = PluginRegistry::new();
        route_context(root.path(), &mut ctx, "raw input", &plugins, None).await.unwrap();
        let value = build_context(&db, &ctx, "raw input", &plugins, 0, root.path())
            .await
            .unwrap();
        for key in ["user_prompt", "user_message"] {
            assert_eq!(value[key], "raw input");
        }
        assert_eq!(value["custom_data"]["user_prompt"], "raw input");
        assert_eq!(value["memory"]["facts"], json!(["only alice"]));
        assert_eq!(value["memory"]["preferences"]["theme"], "current");
        assert_eq!(value["settings"]["system_template"], "standard");
        for key in [
            "skills",
            "tools",
            "active_skill",
            "active_skill_instructions",
            "settings",
            "sm_data",
            "sm_file",
            "active_state",
            "tokens_used",
            "user_template",
            "conversation_text",
            "tag_instructions",
        ] {
            assert!(value.get(key).is_some(), "{key}");
        }
        assert!(value.get("cl_file").is_none());
    }
}
