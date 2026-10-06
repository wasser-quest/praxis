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
        .route("/v1/context/exec", axum::routing::post(exec))
        .route("/v1/models", axum::routing::get(models))
        .route("/v1/providers", axum::routing::get(providers))
        .route("/v1/providers/login", axum::routing::post(login))
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
