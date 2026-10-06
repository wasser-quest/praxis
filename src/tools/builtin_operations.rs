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

/// Host-issued identity for a builtin operation. Built by the dispatcher from
/// the authenticated call only; never reconstructed from tool arguments or a
/// plugin claim (§2 invariant 2).
pub struct BuiltinContext<'a> {
    pub db: &'a Database,
    pub plugins: &'a PluginRegistry,
    pub user: &'a str,
    pub call: &'a str,
}

/// Run a host-owned operation on behalf of a `builtin` handler. Unknown names
/// fail closed: an installed package cannot invent an operation at runtime.
pub async fn execute(
    ctx: &BuiltinContext<'_>,
    name: &str,
    args: &Value,
) -> anyhow::Result<String> {
    Ok(match name {
        "get_context" => get_context(ctx),
        "set_context" => set_context(ctx, args),
        "delete_context" => delete_context(ctx, args),
        // Media generation needs no host state, so it takes no context.
        "image_generate" | "image_analyze" => {
            crate::plugins::minimax_image::execute_builtin(name, args).await?
        }
        other => anyhow::bail!("Unknown host builtin operation: {other}"),
    })
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
    use serde_json::json;

    fn context<'a>(db: &'a Database, plugins: &'a PluginRegistry) -> BuiltinContext<'a> {
        BuiltinContext {
            db,
            plugins,
            user: "builtin-op-user",
            call: "call-1",
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
            execute(
                &ctx,
                "set_context",
                &json!({"key":"custom_data.nickname","value":"Ada"})
            )
            .await
            .unwrap(),
            "Context key 'custom_data.nickname' set"
        );
        let stored = execute(&ctx, "get_context", &json!({})).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["custom_data"]["nickname"], json!("Ada"));

        assert_eq!(
            execute(&ctx, "delete_context", &json!({"key":"custom_data.nickname"}))
                .await
                .unwrap(),
            "Context key 'custom_data.nickname' deleted"
        );
        let stored = execute(&ctx, "get_context", &json!({})).await.unwrap();
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
        let stored = execute(&ctx, "get_context", &json!({"user_id":"someone-else"}))
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["user_id"], json!("builtin-op-user"));
        assert!(parsed["custom_data"]["nickname"].is_null());
    }
}
