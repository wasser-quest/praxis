//! Authenticated backend operations for local and remote terminal clients.
//! Transport only: persistence and command semantics stay in existing DB/services.
use super::GatewayState;
use axum::{extract::{Path, State}, http::StatusCode, Json, Router};
use serde_json::{json, Value};

pub fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/v1/sessions", axum::routing::get(sessions))
        .route("/v1/messages/:user", axum::routing::get(messages).delete(clear))
        .route("/v1/context/:user", axum::routing::get(context).delete(delete))
        .route("/v1/context/:user/fork", axum::routing::post(fork))
        .route("/v1/context/:user/events", axum::routing::get(context_events))
        .route("/v1/context/:user/changes", axum::routing::post(change_context))
        .route("/v1/context/exec", axum::routing::post(exec))
        .route("/v1/models", axum::routing::get(models))
        .route("/v1/providers", axum::routing::get(providers))
        .route("/v1/providers/login", axum::routing::post(login))
        .route("/v1/graphs/:user", axum::routing::get(graph))
        .route("/v1/terminal", axum::routing::get(terminal))
        .route("/v1/terminal/:user/status/:plugin", axum::routing::post(terminal_status))
        .route("/v1/terminal/:user/command/:name", axum::routing::post(terminal_command))
}
/// Status lines and `/` commands that enabled packages add to the TUI.
async fn terminal(State(s): State<GatewayState>) -> Json<Value> {
    Json(crate::plugins::terminal::listing(&s.plugins.list()))
}
async fn terminal_status(State(s): State<GatewayState>, Path((user, plugin)): Path<(String, String)>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let plugins = s.plugins.list();
    let tool = crate::plugins::terminal::status_tool(&plugins, &plugin)
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "no status line for this package"}))))?;
    let text = run_terminal_tool(&s, &user, tool, json!({})).await?;
    Ok(Json(json!({"plugin": plugin, "text": crate::plugins::terminal::status_line(&text)})))
}
#[derive(serde::Deserialize, Default)]
struct TerminalInput { #[serde(default)] text: String }
async fn terminal_command(State(s): State<GatewayState>, Path((user, name)): Path<(String, String)>, body: Option<Json<TerminalInput>>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let plugins = s.plugins.list();
    let (plugin, tool) = crate::plugins::terminal::command_tool(&plugins, &name)
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": format!("unknown terminal command '/{name}'")}))))?;
    let input = body.map(|Json(input)| input).unwrap_or_default();
    let text = run_terminal_tool(&s, &user, tool, json!({"text": input.text})).await?;
    Ok(Json(json!({"plugin": plugin, "text": crate::plugins::terminal::sanitize(&text, crate::plugins::terminal::MAX_OUTPUT_BYTES)})))
}
/// Same path as a model-requested call of the package's tool: catalog owner,
/// enable switches, declared secrets and the package's sandbox all apply.
async fn run_terminal_tool(s: &GatewayState, user: &str, tool: &str, args: Value) -> Result<String, (StatusCode, Json<Value>)> {
    let call = format!("terminal-{}", uuid::Uuid::new_v4().simple());
    super::tool_dispatch::invoke_guest(&s.plugins, &s.db, user, &call, tool, &args)
        .await
        .map(|output| crate::plugins::terminal::result_text(&output))
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()}))))
}
/// Models the selected provider can serve. Listing never selects a provider.
async fn models(State(s): State<GatewayState>, axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let provider = query.get("provider").map(String::as_str);
    match s.llm.get().list_models(provider).await {
        Ok(models) => Ok(Json(json!({"provider": provider.unwrap_or(&s.config.use_provider), "models": models}))),
        Err(error) => Err((StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()})))),
    }
}
async fn providers(State(s): State<GatewayState>) -> Json<Value> {
    let statuses = super::providers::statuses(&s);
    Json(json!({"providers": statuses, "default": s.config.use_provider, "text": super::providers::render_status(&statuses, &s.config)}))
}
async fn login(State(s): State<GatewayState>, Json(request): Json<super::providers::LoginRequest>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match super::providers::login(&s, request).await {
        Ok(outcome) => Ok(Json(serde_json::to_value(outcome).unwrap_or_default())),
        // Errors are operator-facing text; credentials are never echoed.
        Err(error) => Err((StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()})))),
    }
}
async fn messages(State(s): State<GatewayState>, Path(user): Path<String>) -> Result<Json<Value>, StatusCode> {
    let messages = s.db.get_chat_messages_with_token_budget(&user, usize::MAX).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?.0;
    Ok(Json(json!({"messages":messages})))
}
async fn context(State(s): State<GatewayState>, Path(user): Path<String>) -> Result<Json<Value>, StatusCode> {
    Ok(Json(serde_json::to_value(s.db.load_context(&user).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?))
}
/// The context's change log after `after` (default 0), oldest first, plus the
/// current seq. Clients poll or reconnect with the last seq they saw.
async fn context_events(
    State(s): State<GatewayState>,
    Path(user): Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let number = |key: &str, default: i64| -> Result<i64, (StatusCode, Json<Value>)> {
        query.get(key).map_or(Ok(default), |v| v.parse().map_err(|_| bad(&format!("{key} must be a number"))))
    };
    let (after, limit) = (number("after", 0)?, number("limit", 100)?);
    let events = s.db.context_events(&user, after, limit.clamp(1, 1000) as usize).map_err(internal)?;
    let seq = s.db.context_seq(&user).map_err(internal)?;
    Ok(Json(json!({"seq": seq, "events": events})))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeRequest {
    /// Dotted paths or nested objects, as accepted by `/context set`.
    set: Value,
    /// Apply only if the context is still at this seq; 409 otherwise.
    #[serde(default)]
    expected_seq: Option<i64>,
}

/// Append a user change. Same validation as `/context set`; with
/// `expected_seq` it never overwrites a change the client has not seen.
async fn change_context(
    State(s): State<GatewayState>,
    Path(user): Path<String>,
    Json(request): Json<ChangeRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match s.db.change_context(&user, request.set, request.expected_seq) {
        Ok(_) => Ok(Json(json!({"success": true, "seq": s.db.context_seq(&user).map_err(internal)?}))),
        Err(error) => match error.downcast_ref::<crate::db::context_events::Conflict>() {
            Some(conflict) => Err((StatusCode::CONFLICT, Json(json!({"error": conflict.to_string(), "seq": conflict.current})))),
            None => Err(bad(&error.to_string())),
        },
    }
}

fn bad(message: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message})))
}

fn internal(_: anyhow::Error) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal error"})))
}

async fn clear(State(s): State<GatewayState>, Path(user): Path<String>) -> Result<Json<Value>, StatusCode> {
    if super::task_control::cancellation(&user).is_some() { return Err(StatusCode::CONFLICT); }
    s.db.clear_messages(&user).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"success":true})))
}
async fn delete(State(s): State<GatewayState>, Path(user): Path<String>) -> Result<Json<Value>, StatusCode> {
    if super::task_control::cancellation(&user).is_some() { return Err(StatusCode::CONFLICT); }
    s.db.delete_context(&user).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"success":true})))
}
async fn sessions(State(s): State<GatewayState>) -> Result<Json<Value>, StatusCode> {
    let conn=s.db.conn();
    let mut stmt=conn.prepare("SELECT user_id, data FROM contexts ORDER BY updated_at DESC LIMIT 500").map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows=stmt.query_map([], |row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut sessions=Vec::new();
    for row in rows {
        let (id,data)=row.map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
        let ctx: Value=serde_json::from_str(&data).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
        sessions.push(json!({"id":id,"name":ctx.pointer("/custom_data/session_title").and_then(Value::as_str).or_else(||ctx["username"].as_str()).unwrap_or(&id),"username":ctx["username"].as_str().unwrap_or("User")}));
    }
    Ok(Json(json!({"sessions":sessions})))
}
/// The workflow graph for one session, read through the shared SM parser. A
/// terminal client can explain a workflow without opening the dashboard.
async fn graph(State(s): State<GatewayState>, Path(user): Path<String>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    crate::services::graphs::graph_in(&s.db, &std::path::PathBuf::from(&s.config.root_dir), &user, None)
        .await
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()}))))
}
#[derive(serde::Deserialize)]
struct Fork { new_user_id:String, username:Option<String> }
async fn fork(State(s): State<GatewayState>, Path(user): Path<String>, Json(body):Json<Fork>) -> Result<Json<Value>, StatusCode> {
    let ctx=s.db.fork_context(&user,&body.new_user_id,body.username.as_deref()).map_err(|_|StatusCode::BAD_REQUEST)?;
    Ok(Json(json!({"user_id":ctx.user_id})))
}
#[derive(serde::Deserialize)]
struct Command { user_id:String, line:String }
async fn exec(State(s):State<GatewayState>, Json(command):Json<Command>) -> Result<Json<Value>,StatusCode> {
    let op=crate::context_cmd::parse(&command.line).map_err(|_|StatusCode::BAD_REQUEST)?;
    Ok(Json(json!({"result":crate::context_cmd::apply(&s.db,&command.user_id,&op)})))
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod terminal_tests;
