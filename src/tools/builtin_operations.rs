//! Host-owned operations reachable from a package's `builtin` handler.
//!
//! `plan/PLUGINIZATION.md` models `runtime_control` as a package that exposes
//! navigation, discovery, context and Decision-IR tools "using host-owned
//! operations": the package declares the tool (name, schema, flags, ownership)
//! with `{"type":"builtin","name":"…"}`, and the kernel runs the operation here
//! under host-issued identity. The operation deliberately stays in the kernel —
//! removing the package removes the tool from the catalog, never the runtime's
//! ability to enforce guards (`docs/PLUGINIZATION_HANDOFF.md` §6C).
//!
//! These are the single implementations: native dispatch and a package's
//! `builtin` handler both land here, so a result format can never drift
//! between the two (§9 step 5).
use crate::{db::Database, plugins::PluginRegistry};
use serde_json::Value;

/// Bound on Decision-IR lowering chains: a workflow may map an opcode to a
/// call that lowers again, and that must terminate.
pub const MAX_LOWER_DEPTH: u8 = 4;

/// What a host-owned operation produced for its caller.
#[derive(Debug)]
pub enum BuiltinStep {
    /// The exact result text.
    Text(String),
    /// A call the `execute_decision` operation lowered. Only the dispatch loop
    /// can run it (as its next iteration, with full checks); a caller without a
    /// dispatch loop must fail closed instead of running it.
    Lower(crate::gateway::llm::provider::ToolCall),
}

/// Host-issued identity for a builtin operation. Built by the dispatcher from
/// the authenticated call only; never reconstructed from tool arguments or a
/// plugin claim (§2 invariant 2).
pub struct BuiltinContext<'a> {
    pub db: &'a Database,
    pub plugins: &'a PluginRegistry,
    pub user: &'a str,
    pub call: &'a str,
    /// The runtime root holding `contexts/`, issued by the dispatcher. Never
    /// taken from a tool argument.
    pub root: &'a std::path::Path,
    /// The dispatcher's execution mode, for operations whose behavior differs
    /// between chat and agent runs (never taken from tool arguments).
    pub mode: crate::gateway::tool_dispatch::DispatchMode,
    /// Decision-IR lowerings already run for this call (bounded).
    pub depth: u8,
}


/// Run a host-owned operation on behalf of a `builtin` handler. Unknown names
/// fail closed: an installed package cannot invent an operation at runtime.
pub async fn execute(
    ctx: &BuiltinContext<'_>,
    name: &str,
    args: &Value,
) -> anyhow::Result<BuiltinStep> {
    // One bounded instruction is lowered through the workflow's pinned opcode
    // table and the resolved call goes back to the dispatch loop (§6C).
    if name == "execute_decision" {
        return lower(ctx, args);
    }
    Ok(BuiltinStep::Text(match name {
        "get_context" => get_context(ctx),
        "set_context" => set_context(ctx, args),
        "delete_context" => delete_context(ctx, args),
        "search_tools" => crate::tools::discovery::search(ctx.db, ctx.plugins, ctx.user, args)
            .unwrap_or_else(|error| format!("Error: {error}")),
        "read_tool_result" => crate::tools::tool_output::run(ctx.db, ctx.user, args)
            .unwrap_or_else(|error| format!("Error: {error}")),
        "run_check" => Box::pin(crate::gateway::action_contracts::run(ctx.user, ctx.call, args))
            .await
            .unwrap_or_else(|error| format!("Error: {error}")),
        "agent_complete" => Box::pin(agent_signal(ctx, "Complete", Value::Null)).await,
        "agent_set_path" => Box::pin(agent_signal(ctx, "Path", args.clone())).await,
        "agent_feedback" => Box::pin(agent_signal(ctx, "Feedback", args.clone())).await,
        // Sub-agent delegation: the child agent loop is host-owned, so the
        // package only declares the tool (§6C).
        "delegate_task" => delegate(ctx, args).await,
        "list_delegations" => list_delegations(ctx),
        // Scheduled jobs over the host-owned job store.
        "cron_add" | "cron_delete" | "cron_list" | "cron_toggle" | "cron_run" => {
            crate::tools::cron::run(ctx.db, ctx.user, name, args)
                .unwrap_or_else(|error| format!("Error: {error}"))
        }
        // Skill discovery/activation and template editing over host-owned
        // files; the packages declare the tools.
        "search_skills" => Box::pin(crate::tools::search_skills::run(ctx.db, args))
            .await
            .unwrap_or_else(|error| format!("Error: {error}")),
        "use_skill" => Box::pin(crate::tools::use_skill::run(ctx.db, args))
            .await
            .unwrap_or_else(|error| format!("Error: {error}")),
        "update_template" => Box::pin(crate::tools::update_template::run(ctx.db, args))
            .await
            .unwrap_or_else(|error| format!("Error: {error}")),
        // Questions and screenshots routed to the user's interface.
        "send_screenshot" | "ask_questions" => {
            Box::pin(crate::tools::interaction::run(
                ctx.db,
                ctx.plugins,
                ctx.user,
                name,
                args,
            ))
            .await
            .unwrap_or_else(|error| format!("Error: {error}"))
        }
        // Discord delivery over the host's authenticated channel bindings.
        "discord_upload_file" | "discord_send_message" | "discord_send_embed" => {
            Box::pin(crate::tools::discord_tools::run(
                ctx.db,
                ctx.user,
                ctx.mode == crate::gateway::tool_dispatch::DispatchMode::Agent,
                name,
                args,
            ))
            .await
            .unwrap_or_else(|error| format!("Error: {error}"))
        }
        // Document ingestion and retrieval over the host-owned store.
        "rag_search" | "rag_ingest" | "rag_list" | "rag_delete" => {
            Box::pin(crate::tools::rag_ingest::run(ctx.db, ctx.user, name, args))
                .await
                .unwrap_or_else(|error| format!("Error: {error}"))
        }
        // Memory profiles, facts, preferences and topics over host-owned
        // storage; one implementation for every declaring package.
        "memory_profile_create" | "memory_profile_load" | "memory_profile_list"
        | "memory_get" | "memory_set" | "learn_fact" | "learn_preference" | "learn_topic" => {
            crate::tools::memory::run(ctx.db, ctx.user, name, args)
                .unwrap_or_else(|error| format!("Error: {error}"))
        }
        // Graph/linear navigation keeps its historical error text (already
        // self-describing), unlike the signal tools above.
        "agent_next" | "agent_back" => Box::pin(navigate(ctx, name == "agent_back", args)).await,
        // Media generation needs no host state, so it takes no context.
        "image_generate" | "image_analyze" => {
            Box::pin(crate::plugins::minimax_image::execute_builtin(name, args)).await?
        }
        other => anyhow::bail!("Unknown host builtin operation: {other}"),
    }))
}

/// The general "run this resolved call" capability
/// (`docs/PLUGINIZATION_HANDOFF.md` §6C): lower one bounded Decision IR
/// instruction through the workflow's pinned opcode table and hand the
/// resolved call to the dispatch loop, which runs it with the same state
/// permissions, contracts, rollback and receipts as a direct call. Lowering
/// stays bounded so a mapped target can never recurse without end.
fn lower(ctx: &BuiltinContext<'_>, args: &Value) -> anyhow::Result<BuiltinStep> {
    anyhow::ensure!(
        ctx.depth < MAX_LOWER_DEPTH,
        "Decision IR lowering is nested too deeply"
    );
    let call = crate::gateway::llm::provider::ToolCall {
        id: ctx.call.to_string(),
        function: crate::gateway::llm::provider::FunctionCall {
            name: "execute_decision".into(),
            arguments: args.to_string(),
        },
    };
    Ok(match crate::gateway::decision_ir::resolve(ctx.db, ctx.user, &call, ctx.plugins) {
        Ok(resolved) => BuiltinStep::Lower(resolved),
        // A rejected instruction keeps its historical error text.
        Err(error) => BuiltinStep::Text(crate::gateway::workflow_preflight::tool_error(&error)),
    })
}

/// The runtime root (holding `contexts/`) for callers that have no dispatcher
/// root of their own, e.g. a package's `builtin` handler.
pub fn root() -> std::path::PathBuf {
    crate::gateway::state_ref()
        .map(|state| std::path::PathBuf::from(&state.config.root_dir))
        .or_else(|| std::env::var_os("ROOT_DIR").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

async fn agent_signal(ctx: &BuiltinContext<'_>, kind: &str, args: Value) -> String {
    use crate::tools::agent_control::AgentControlSignal;
    let signal = match kind {
        "Path" => AgentControlSignal::Path(args["path"].as_str().unwrap_or("").to_string()),
        "Feedback" => {
            AgentControlSignal::Feedback(args["message"].as_str().unwrap_or("").to_string())
        }
        _ => AgentControlSignal::Complete,
    };
    crate::tools::agent_control::run(ctx.db, ctx.user, signal)
        .await
        .unwrap_or_else(|error| format!("Error: {error}"))
}

async fn navigate(ctx: &BuiltinContext<'_>, back: bool, args: &Value) -> String {
    crate::tools::agent_control::navigate_with_plugins(
        ctx.db,
        ctx.root,
        ctx.plugins,
        ctx.user,
        back,
        args,
    )
    .await
    .unwrap_or_else(|error| error)
}

/// Needs GatewayState for the child agent loop; fetch it from the global
/// gateway state accessor used by the message handler. Boxed to break the
/// async recursion (loop -> tool -> child loop).
async fn delegate(ctx: &BuiltinContext<'_>, args: &Value) -> String {
    match crate::gateway::state_ref() {
        Some(state) => {
            let state = state.clone();
            let user = ctx.user.to_string();
            let args = args.clone();
            Box::pin(async move {
                crate::gateway::delegation::delegate_task(&state, &user, &args).await
            })
            .await
        }
        None => "Error: gateway state unavailable for delegation.".to_string(),
    }
}

fn list_delegations(ctx: &BuiltinContext<'_>) -> String {
    match crate::gateway::delegation::list_delegations(ctx.db, ctx.user) {
        Ok(list) if list.is_empty() => "No delegations yet.".to_string(),
        Ok(list) => serde_json::to_string_pretty(&list)
            .unwrap_or_else(|_| format!("{} delegations", list.len())),
        Err(error) => format!("Error: {}", error),
    }
}

/// The whole context namespace, so the kernel's own authority checks keep
/// running (`action_contracts::validate_context`) regardless of who exposes the
/// tool. Result formats are the historical ones.
pub fn get_context(ctx: &BuiltinContext<'_>) -> String {
    match ctx.db.load_context(ctx.user) {
        Ok(context) => serde_json::to_string_pretty(&context)
            .unwrap_or_else(|_| "Failed to serialize".to_string()),
        Err(error) => format!("Error: {error}"),
    }
}

pub fn set_context(ctx: &BuiltinContext<'_>, args: &Value) -> String {
    let key = args["key"].as_str().unwrap_or("");
    let value = args.get("value").cloned().unwrap_or(Value::Null);
    match ctx
        .db
        .merge_context_from_agent(ctx.user, serde_json::json!({key: value}))
    {
        Ok(_) => format!("Context key '{key}' set"),
        Err(error) => format!("Error: {error}"),
    }
}

pub fn delete_context(ctx: &BuiltinContext<'_>, args: &Value) -> String {
    let key = args["key"].as_str().unwrap_or("");
    match ctx
        .db
        .merge_context_from_agent(ctx.user, serde_json::json!({key: null}))
    {
        Ok(_) => format!("Context key '{key}' deleted"),
        Err(error) => format!("Error: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::plugins::Plugin;
    use serde_json::json;

    fn context<'a>(db: &'a Database, plugins: &'a PluginRegistry) -> BuiltinContext<'a> {
        BuiltinContext {
            db,
            plugins,
            user: "builtin-op-user",
            call: "call-1",
            root: std::path::Path::new("."),
            mode: crate::gateway::tool_dispatch::DispatchMode::Chat,
            depth: 0,
        }
    }

    /// The result text of an operation, or a panic when it lowers unexpectedly.
    async fn text(ctx: &BuiltinContext<'_>, name: &str, args: &Value) -> String {
        match execute(ctx, name, args).await.unwrap() {
            BuiltinStep::Text(text) => text,
            BuiltinStep::Lower(_) => panic!("unexpected lowering from {name}"),
        }
    }

    fn setup() -> (tempfile::TempDir, Database, PluginRegistry) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(&dir.path().join("data")).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        (dir, db, PluginRegistry::new())
    }

    #[tokio::test]
    async fn context_operations_round_trip_through_host_identity() {
        let (_dir, db, plugins) = setup();
        let ctx = context(&db, &plugins);

        assert_eq!(
            text(
                &ctx,
                "set_context",
                &json!({"key":"custom_data.nickname","value":"Ada"})
            )
            .await,
            "Context key 'custom_data.nickname' set"
        );
        let stored = text(&ctx, "get_context", &json!({})).await;
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["custom_data"]["nickname"], json!("Ada"));

        assert_eq!(
            text(&ctx, "delete_context", &json!({"key":"custom_data.nickname"}))
                .await,
            "Context key 'custom_data.nickname' deleted"
        );
        let stored = text(&ctx, "get_context", &json!({})).await;
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert!(parsed["custom_data"]["nickname"].is_null());
    }

    #[tokio::test]
    async fn unknown_operations_fail_closed() {
        let (_dir, db, plugins) = setup();
        let ctx = context(&db, &plugins);
        // A package cannot invent an operation the kernel does not own.
        let error = execute(&ctx, "invented", &json!({})).await.unwrap_err();
        assert!(error.to_string().contains("Unknown host builtin operation"));
    }

    #[tokio::test]
    async fn host_operations_keep_their_historical_result_formats() {
        let (_dir, db, plugins) = setup();
        let ctx = context(&db, &plugins);

        // Agent feedback keeps its exact historical text.
        assert_eq!(
            text(&ctx, "agent_feedback", &json!({"message":"hi"})).await,
            "Feedback sent: hi"
        );
        // Discovery and check running degrade to result strings, never panics.
        assert!(!text(&ctx, "search_tools", &json!({"query":""}))
            .await
            .is_empty());
        assert!(!text(&ctx, "read_tool_result", &json!({"id":"missing"}))
            .await
            .is_empty());
    }

    /// A package tool backed by a host-owned operation. The name is the
    /// package's own; `builtin` names the kernel operation it runs.
    fn register_builtin_tool(plugins: &mut PluginRegistry, tool: &str, operation: &str) {
        let plugin: Plugin = serde_json::from_value(json!({
            "name": "pkg", "description": "x", "version": "1",
            "tools": [{
                "name": tool,
                "description": "x",
                "parameters": {"type": "object"},
                "handler": {"type": "builtin", "name": operation}
            }]
        }))
        .unwrap();
        plugins.try_register(plugin).unwrap();
    }

    #[tokio::test]
    async fn a_builtin_handler_package_reaches_host_operations() {
        let (_dir, db, _empty) = setup();
        let mut plugins = PluginRegistry::new();
        register_builtin_tool(&mut plugins, "pkg_feedback", "agent_feedback");
        let user = "builtin-pkg-user";
        let _guard = crate::gateway::task_control::begin(user).unwrap();
        crate::gateway::task_control::pin_registry(user, &plugins).unwrap();
        // The package supplies the tool; the kernel supplies the operation and
        // the host-issued identity, and the result format is the historical one.
        let out = plugins
            .execute_tool_with_host(
                &db,
                user,
                "call-1",
                "pkg_feedback",
                &json!({"message":"hi"}),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(out, "Feedback sent: hi");
    }

    #[tokio::test]
    async fn a_missing_package_fails_closed_instead_of_running_a_builtin() {
        let (_dir, db, plugins) = setup();
        let user = "builtin-absent-user";
        let _guard = crate::gateway::task_control::begin(user).unwrap();
        crate::gateway::task_control::pin_registry(user, &plugins).unwrap();
        // No package declares the tool: nothing may run, native or otherwise.
        let error = plugins
            .execute_tool_with_host(
                &db,
                user,
                "call-1",
                "pkg_feedback",
                &json!({"message":"hi"}),
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unavailable"), "{error}");
    }

    #[tokio::test]
    async fn a_package_cannot_invent_a_host_operation() {
        let (_dir, db, _empty) = setup();
        let mut plugins = PluginRegistry::new();
        register_builtin_tool(&mut plugins, "pkg_evil", "invented");
        let user = "builtin-evil-user";
        let _guard = crate::gateway::task_control::begin(user).unwrap();
        crate::gateway::task_control::pin_registry(user, &plugins).unwrap();
        let error = plugins
            .execute_tool_with_host(&db, user, "call-1", "pkg_evil", &json!({}), None, None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Unknown host builtin operation"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn nested_decision_lowering_is_bounded() {
        let (_dir, db, plugins) = setup();
        let mut ctx = context(&db, &plugins);
        ctx.depth = MAX_LOWER_DEPTH;
        // A workflow mapping can chain lowerings; the stack stays bounded.
        let error = execute(&ctx, "execute_decision", &json!({"ir": "1 A"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("nested too deeply"), "{error}");
    }

    #[tokio::test]
    async fn bundled_packages_only_name_host_owned_operations() {
        let (_dir, db, plugins) = setup();
        let ctx = context(&db, &plugins);
        for plugin in crate::tools::packages::bundled_plugins() {
            for tool in &plugin.tools {
                let crate::plugins::PluginHandler::Builtin { name: operation } = &tool.handler else {
                    panic!("{} must run a host-owned operation", tool.name);
                };
                // Unknown operations fail closed instead of running anything.
                let out = text(&ctx, operation, &json!({})).await;
                assert!(
                    !out.contains("Unknown host builtin operation"),
                    "{} names an operation the kernel does not own",
                    tool.name
                );
            }
        }
    }

    #[tokio::test]
    async fn run_check_never_invents_evidence() {
        let (_dir, db, plugins) = setup();
        let ctx = context(&db, &plugins);
        // No task/policy is bound here, so no receipt can be produced.
        let out = text(&ctx, "run_check", &json!({"name":"tests"})).await;
        assert!(out.starts_with("Error:"), "{out}");
    }

    #[tokio::test]
    async fn identity_is_host_issued_not_argument_supplied() {
        let (_dir, db, plugins) = setup();
        let ctx = context(&db, &plugins);
        // A hostile argument naming another user is ignored: the operation
        // only ever reads the dispatcher's user.
        db.merge_context_from_agent(
            "someone-else",
            json!({"custom_data.nickname":"mallory"}),
        )
        .unwrap();
        let stored = text(&ctx, "get_context", &json!({"user_id":"someone-else"})).await;
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["user_id"], json!("builtin-op-user"));
        assert!(parsed["custom_data"]["nickname"].is_null());
    }
}
