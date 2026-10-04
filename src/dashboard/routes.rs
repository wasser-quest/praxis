use axum::response::sse::{Event, KeepAlive, Sse};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct DashboardState {
    pub plugins: Arc<crate::plugins::PluginRegistry>,
    pub db: crate::db::Database,
    pub gateway_api_key: String,
    pub admin_password: String,
}

#[cfg(all(test, feature = "vm"))]
#[path = "vm_tests.rs"]
mod vm_tests;

#[derive(Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
}

#[derive(Deserialize)]
pub struct ToolUpdate {
    pub is_enabled: bool,
}

#[derive(Deserialize)]
pub struct MemoryUpdate {
    pub profile: Option<String>,
    pub reason: Option<String>,
    pub user_preferences: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub custom_variables: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub learned_facts: Option<Vec<String>>,
    pub last_topics: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct MemoryInfo {
    pub user_id: String,
    pub profile: String,
    pub active_profile: String,
    pub profile_exists: bool,
    pub profiles: Vec<String>,
    pub shared: HashMap<String, serde_json::Value>,
    pub user_preferences: std::collections::HashMap<String, serde_json::Value>,
    pub custom_variables: std::collections::HashMap<String, serde_json::Value>,
    pub learned_facts: Vec<String>,
    pub last_topics: Vec<String>,
}

#[derive(Deserialize)]
pub struct TemplateUpdate {
    pub content: String,
    pub user_id: Option<String>,
    pub user_prompt: Option<String>,
}

#[derive(Deserialize)]
pub struct TemplateCreate {
    pub name: String,
    pub content: String,
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct TemplateSaveResult {
    pub success: bool,
    pub error: Option<String>,
    pub rendered_preview: Option<String>,
}

#[derive(Serialize)]
pub struct SecretsInfo {
    /// Write-only auth.json import; never expose token suffixes from JSON.
    pub codex_auth: String,
    pub discord_bot_token: String,
    pub openai_api_key: String,
    pub anthropic_api_key: String,
    pub ollama_api_key: String,
    pub llamacpp_api_key: String,
    pub minimax_api_key: String,
    pub mimo_api_key: String,
    pub elevenlabs_api_key: String,
    pub gateway_api_key: String,
    pub dashboard_admin_password: String,
    #[serde(flatten)]
    pub custom: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
pub struct SecretsUpdate {
    /// CLI auth.json or normalized CodexAuth JSON. Empty removes, *** preserves.
    pub codex_auth: Option<String>,
    pub discord_bot_token: Option<String>,
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub ollama_api_key: Option<String>,
    pub llamacpp_api_key: Option<String>,
    pub minimax_api_key: Option<String>,
    pub mimo_api_key: Option<String>,
    pub elevenlabs_api_key: Option<String>,
    pub gateway_api_key: Option<String>,
    pub dashboard_admin_password: Option<String>,
    pub master_password: Option<String>,
    #[serde(flatten)]
    pub custom: std::collections::HashMap<String, String>,
}

#[derive(Serialize)]
pub struct SmFileInfo {
    pub name: String,
    pub path: String,
}

#[derive(Deserialize)]
pub struct SmFileUpdate {
    pub content: String,
}

#[derive(Deserialize)]
pub struct DashboardLoginRequest {
    pub password: String,
}

#[derive(Serialize)]
pub struct DashboardLoginResponse {
    pub token: String,
}

/// Compatibility entry point; synchronization belongs to the core runtime.
pub fn sync_templates_from_disk(db: &crate::db::Database) {
    if let Err(error) = crate::runtime::templates::sync_from_disk(db, std::path::Path::new("templates")) {
        tracing::warn!(%error, "Template catalog synchronization failed");
    }
}

pub fn routes(db: crate::db::Database) -> Router {
    routes_with_plugins(db, Arc::new(crate::plugins::PluginRegistry::new()))
}

pub fn routes_with_plugins(db: crate::db::Database, plugins: Arc<crate::plugins::PluginRegistry>) -> Router {
    let secrets_src = crate::db::secrets::get_secrets();
    let config = crate::config::Config::from_env();
    // Transparenz: Der Store ÜBERSCHREIBT die Env-Werte (Feature: Passwort-
    // Rotation ohne Redeploy) — aber ein versehentlich ins Secrets-Formular
    // geschriebener Wert führt sonst zu einem Rätsel-Login (21.09. live
    // passiert). Beim Start deutlich loggen:
    if secrets_src.dashboard_admin_password.is_some() {
        tracing::warn!("Dashboard-Passwort kommt aus dem Secret-Store (überschreibt DASHBOARD_ADMIN_PASSWORD aus der Env)");
    }
    if secrets_src.gateway_api_key.is_some() {
        tracing::warn!("Gateway-API-Key kommt aus dem Secret-Store (überschreibt GATEWAY_API_KEY aus der Env)");
    }
    let state = Arc::new(DashboardState {
        plugins,
        db,
        gateway_api_key: secrets_src.gateway_api_key.unwrap_or(config.gateway_api_key),
        admin_password: secrets_src
            .dashboard_admin_password
            .unwrap_or(config.dashboard_admin_password),
    });

    router_with_state(state)
}

pub(crate) fn router_with_state(state: Arc<DashboardState>) -> Router {
    // Protected API routes with auth middleware
    let protected = Router::new()
        .route("/contexts", axum::routing::get(list_contexts))
        .route("/chat-sessions", axum::routing::get(list_chat_sessions))
        .route("/contexts/:user_id", axum::routing::get(get_context))
        .route("/contexts/:user_id", axum::routing::put(update_context))
        .route("/contexts/:user_id", axum::routing::delete(delete_context))
        .route("/contexts/:user_id/fork", axum::routing::post(fork_context_route))
        .route("/messages/:user_id", axum::routing::get(get_messages))
        .route("/graphs/:user_id", axum::routing::get(super::graphs::graph))
        .route("/execution/:user_id", axum::routing::get(super::graphs::execution))
        .route("/usage/:user_id", axum::routing::get(super::graphs::usage))
        .route("/messages/:user_id", axum::routing::delete(clear_messages))
        .route("/messages/:user_id/clear-chat", axum::routing::post(clear_chat_view))
        .route("/messages/:user_id/compact", axum::routing::post(compact_messages))
        .route("/skills", axum::routing::get(list_skills))
        .route("/router-state", axum::routing::get(router_state))
        .route("/delegations/:user_id", axum::routing::get(list_delegations_route))
        .route("/decision-profiles", axum::routing::get(super::decision_profiles::list))
        .route("/decision-profiles/:name", axum::routing::get(super::decision_profiles::get).put(super::decision_profiles::save))
        .route("/decision-probe", axum::routing::post(super::decision_profiles::probe))
        .route("/templates", axum::routing::get(list_templates))
        .route("/templates", axum::routing::post(create_template))
        .route("/templates/:name", axum::routing::get(get_template))
        .route("/templates/:name", axum::routing::put(update_template))
        .route("/templates/:name", axum::routing::delete(delete_template))
        .route("/tools", axum::routing::get(list_tools))
        .route("/tools/all", axum::routing::get(list_all_tools))
        .route("/tools/:name", axum::routing::put(update_tool))
        .route("/memory/:user_id", axum::routing::get(get_memory))
        .route("/memory/:user_id", axum::routing::put(update_memory))
        .route("/secrets", axum::routing::get(get_secrets))
        .route("/secrets", axum::routing::put(update_secrets))
        .route("/pairings", axum::routing::get(list_pairings))
        .route("/pairings/:user_id", axum::routing::delete(delete_pairing))
        .route(
            "/pairings/pending",
            axum::routing::get(list_pending_pairings),
        )
        .route(
            "/pairings/pending/:code/approve",
            axum::routing::post(approve_pending_pairing),
        )
        .route(
            "/pairings/pending/:code",
            axum::routing::delete(delete_pending_pairing),
        )
        .route("/sm-files", axum::routing::get(list_sm_files))
        .route("/sm-files/:name", axum::routing::get(get_sm_file))
        .route("/sm-files/:name", axum::routing::put(save_sm_file))
        .route("/cron-jobs", axum::routing::get(list_cron_jobs))
        .route("/tool-activity", axum::routing::get(tool_activity))
        .route("/agent/input", axum::routing::post(send_agent_input))
        .route("/agent/begin", axum::routing::post(begin_agent))
        .route("/agent/status/:user_id", axum::routing::get(get_agent_status))
        .route("/agent/stop/:user_id", axum::routing::post(stop_agent))
        .route("/sm/:user_id", axum::routing::get(get_sm_info))
        .route("/cl/:user_id", axum::routing::get(get_sm_info)) // legacy route alias
        .route("/chat/send", axum::routing::post(chat_query))
        .route("/chat/audio/:user_id/:message_id", axum::routing::get(get_chat_audio))
        .route("/context/exec", axum::routing::post(context_exec))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            dashboard_auth_middleware,
        ));

    // Static file service. A small middleware adds Cache-Control: no-store to
    // UI files (app.js/index) so browser caches (Brave is aggressive) never
    // keep stale UI code after an update.
    let static_service = tower_http::services::ServeDir::new("static");

    // Authenticated routes that the commit notes flagged as unprotected.
    let protected_extra = Router::new()
        .route("/stt", axum::routing::post(dashboard_stt))
        .route("/profiles", axum::routing::get(list_profiles))
        .route("/profiles", axum::routing::post(save_profile))
        .route("/profiles/:name/apply/:user_id", axum::routing::post(apply_profile))
        .route("/profiles/:name", axum::routing::delete(delete_profile))
        .route("/media", axum::routing::get(list_media))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            dashboard_auth_middleware,
        ));

    Router::new()
        .route("/", axum::routing::get(index))
        .route("/logo.svg", axum::routing::get(logo_svg))
        .route_service("/logo.png", tower_http::services::ServeFile::new("static/logo.png"))
        .route_service("/favicon.ico", tower_http::services::ServeFile::new("static/favicon.ico"))
        .route_service("/apple-touch-icon.png", tower_http::services::ServeFile::new("static/apple-touch-icon.png"))
        .route("/api/status", axum::routing::get(status))
        .route("/api/auth/login", axum::routing::post(login_handler))
        .route("/api/chat/stream/:user_id", axum::routing::get(chat_stream_auth))
        .route("/api/chat/stream/:user_id/tts", axum::routing::get(chat_stream_tts_only))
        .route("/api/avatar/:name", axum::routing::get(get_avatar))
        .route("/api/files/:name", axum::routing::get(get_chat_file))
        .route("/api/screenshots/*path", axum::routing::get(get_screenshot))
        .route("/api/upload-avatar", axum::routing::post(upload_avatar))
        .route("/api/upload-file", axum::routing::post(upload_chat_file))
        .nest("/api", protected_extra)
        .nest_service("/static", static_service)
        .layer(middleware::from_fn(static_no_cache_middleware))
        .nest("/api", protected)
        .with_state(state.clone())
        .merge(super::extensions::router(state.clone()))
}

/// Serve static UI files with Cache-Control: no-store so browser caches
/// (Brave caches aggressively) never keep stale app.js/index after updates.
async fn static_no_cache_middleware(
    req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store, must-revalidate"),
    );
    res
}

async fn dashboard_auth_middleware(
    State(state): State<Arc<DashboardState>>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header) => match header.strip_prefix("Bearer ") {
            Some(t) => t,
            None => return Err(StatusCode::UNAUTHORIZED),
        },
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    if dashboard_token_valid(&state, token) { return Ok(next.run(req).await); }
    Err(StatusCode::UNAUTHORIZED)
}

pub(crate) fn dashboard_token_valid(state: &DashboardState, token: &str) -> bool {
    !state.gateway_api_key.is_empty() && !token.is_empty() && (token == state.gateway_api_key || jsonwebtoken::decode::<crate::gateway::auth::Claims>(token, &jsonwebtoken::DecodingKey::from_secret(state.gateway_api_key.as_bytes()), &jsonwebtoken::Validation::default()).is_ok())
}

async fn login_handler(
    State(state): State<Arc<DashboardState>>,
    Json(payload): Json<DashboardLoginRequest>,
) -> Result<Json<DashboardLoginResponse>, StatusCode> {
    if payload.password != state.admin_password {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let claims = crate::gateway::auth::Claims {
        sub: "admin".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
        iat: chrono::Utc::now().timestamp() as usize,
    };

    use jsonwebtoken::{encode, EncodingKey, Header};
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.gateway_api_key.as_bytes()),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(DashboardLoginResponse { token }))
}

async fn index() -> axum::response::Response {
    // The dashboard HTML embeds the app.js URL with a version query. Always
    // serve it with no-store so browsers pick up UI updates immediately
    // instead of keeping a stale cached page (Brave caches aggressively).
    (
        [
            (axum::http::header::CACHE_CONTROL, "no-store, must-revalidate"),
        ],
        axum::response::Html(include_str!("../../static/index.html")),
    )
        .into_response()
}

async fn logo_svg() -> impl axum::response::IntoResponse {
    (
        axum::http::header::HeaderMap::from_iter([(
            axum::http::header::CONTENT_TYPE,
            "image/svg+xml".parse().unwrap(),
        )]),
        include_str!("../../static/logo.svg"),
    )
}

async fn status(State(_state): State<Arc<DashboardState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// GPU-Router-State als Proxy (Badge im Overview): Slot-States + Budget
/// („heute X $"). Auth läuft über die Dashboard-Session — der Router-Token
/// bleibt serverseitig in gpu_router (nie im Browser).
async fn router_state() -> Json<serde_json::Value> {
    match crate::gpu_router::state().await {
        Some(s) => Json(serde_json::to_value(&s).unwrap_or_else(|_| serde_json::json!({"configured": false}))),
        None => Json(serde_json::json!({"configured": false})),
    }
}

// ── Contexts ─────────────────────────────────────────────────────────────────

/// All chat sessions known to the server, independent of the browser's
/// localStorage cache. The dashboard previously rendered only the sessions it
/// happened to have cached, so chats created in another browser or on another
/// device (and Discord-paired histories) stayed invisible until manually
/// reconstructed. Context rows are authoritative; message stats and a preview
/// are resolved from the messages table, where forked sessions store rows
/// under `user_id:::session_id` while the session itself is `user_id`.
async fn list_chat_sessions(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT user_id, updated_at FROM contexts ORDER BY updated_at DESC LIMIT 500",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    drop(stmt);

    let mut sessions = Vec::new();
    for (user_id, updated_at) in rows {
        let username: Option<String> = conn
            .query_row(
                "SELECT data FROM contexts WHERE user_id = ?1 LIMIT 1",
                rusqlite::params![user_id],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|data| {
                serde_json::from_str::<crate::db::contexts::Context>(&data).ok()
            })
            .and_then(|ctx| ctx.username.filter(|u| !u.trim().is_empty()));
        // Messages live under the session key or its forked `:::` sub-keys.
        let prefix = format!("{user_id}:::");
        let message_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE user_id = ?1 OR substr(user_id, 1, length(?2)) = ?2",
                rusqlite::params![user_id, prefix],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let preview: Option<String> = conn
            .query_row(
                "SELECT content FROM messages WHERE user_id = ?1 AND role = 'user' AND content <> '' ORDER BY id ASC LIMIT 1",
                rusqlite::params![user_id],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .or_else(|| {
                conn.query_row(
                    "SELECT content FROM messages WHERE substr(user_id, 1, length(?1)) = ?1 AND role = 'user' AND content <> '' ORDER BY id ASC LIMIT 1",
                    rusqlite::params![prefix],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            })
            .map(|text| {
                let flat: String = text.trim().chars().take(120).collect();
                flat
            });
        let title: Option<String> = conn.query_row(
            "SELECT json_extract(data, '$.custom_data.session_title') FROM contexts WHERE user_id=?1",
            rusqlite::params![user_id], |row| row.get(0),
        ).ok().flatten();
        sessions.push(serde_json::json!({
            "user_id": user_id,
            "session_title": title,
            "username": username,
            "updated_at": updated_at,
            "message_count": message_count,
            "preview": preview,
        }));
    }
    Ok(Json(serde_json::json!({ "sessions": sessions })))
}

async fn list_contexts(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let mut stmt = conn
        .prepare("SELECT user_id, data, updated_at FROM contexts ORDER BY updated_at DESC")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let contexts: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            let user_id: String = row.get(0)?;
            let data: String = row.get(1)?;
            let updated_at: String = row.get(2)?;
            let mut data: serde_json::Value = serde_json::from_str(&data).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
            crate::db::contexts::normalize_legacy_keys(&mut data);
            Ok(serde_json::json!({
                "user_id": user_id,
                "data": data,
                "updated_at": updated_at,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "contexts": contexts })))
}

async fn get_context(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = state.db.load_context(&user_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(ctx).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

async fn update_context(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
    Json(update): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = state.db.merge_context(&user_id, update).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(serde_json::to_value(ctx).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

async fn delete_context(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    state
        .db
        .delete_context(&user_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// Fork a context: create a new context under `new_user_id` by cloning the
/// context for the user_id in the path. Each chat session gets its own
/// fully-independent user_id; this lets the frontend create a new session
/// that inherits settings/custom data from a parent without sharing messages.
async fn fork_context_route(
    State(state): State<Arc<DashboardState>>,
    Path(parent_user_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let new_user_id = body
        .get("new_user_id")
        .and_then(|v| v.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();
    if new_user_id.is_empty() || new_user_id == parent_user_id {
        return Err(StatusCode::BAD_REQUEST);
    }
    let username = body.get("username").and_then(|v| v.as_str());

    let ctx = state
        .db
        .fork_context(&parent_user_id, &new_user_id, username)
        .map_err(|e| {
            tracing::error!(error = %e, "fork_context failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "user_id": ctx.user_id,
        "username": ctx.username,
        "parent_user_id": parent_user_id,
    })))
}

/// Execute a `/context …` slash command. The body is `{user_id, line}`. The
/// shared parser at `crate::context_cmd` is the single source of truth for
/// the syntax — same parser drives the TUI and Discord.
async fn context_exec(
    State(state): State<Arc<DashboardState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user_id = body
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();
    let line = body
        .get("line")
        .and_then(|v| v.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    match crate::context_cmd::parse(line) {
        Ok(op) => {
            let response = crate::context_cmd::apply(&state.db, &user_id, &op);
            Ok(Json(serde_json::json!({ "response": response })))
        }
        Err(e) => Ok(Json(serde_json::json!({ "error": e.to_string() }))),
    }
}

// ── Messages ─────────────────────────────────────────────────────────────────

async fn get_messages(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let budget = 500000usize;
    let chat_only = params.get("chat_only").map(|v| v == "1" || v == "true").unwrap_or(false);
    tracing::debug!(user_id = %user_id, chat_only, "[MESSAGES] fetching messages");
    let result = if chat_only {
        // Chat view: hide rows up to the clear marker (kept in Messages tab).
        let marker = state
            .db
            .load_context(&user_id)
            .ok()
            .and_then(|ctx| {
                ctx.custom_data
                    .get("chat_cleared_message_id")
                    .and_then(|v| v.as_i64())
            })
            .unwrap_or(0);
        state.db.get_chat_messages_after(&user_id, budget, marker)
    } else {
        state.db.get_chat_messages_with_token_budget(&user_id, budget)
    };
    match result {
        Ok((messages, total_tokens)) => {
            let msgs: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| {
                    let mut val = serde_json::json!({
                        "id": m.id,
                        "audio_mime": m.audio_mime,
                        "role": m.role,
                        "content": m.content,
                        "tool_call_id": m.tool_call_id,
                        "tool_name": m.tool_name,
                        "prompt_tokens": m.prompt_tokens,
                        "completion_tokens": m.completion_tokens,
                        "total_tokens": m.total_tokens,
                        "generation_ms": m.generation_ms,
                    });
                    if let Some(meta) = m.discord_meta.as_ref() {
                        val["discord_meta"] = meta.clone();
                    }
                    if let Some(ref tool_calls) = m.tool_calls {
                        val["tool_calls"] = serde_json::json!(tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                })
                            })
                            .collect::<Vec<_>>());
                    }
                    val
                })
                .collect::<Vec<_>>();
            Ok(Json(serde_json::json!({
                "messages": msgs,
                "total_tokens": total_tokens,
                "message_count": messages.len(),
            })))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Authenticated, on-demand replay. Never embed credentials or audio blobs in history.
async fn get_chat_audio(
    State(state): State<Arc<DashboardState>>,
    Path((user_id, message_id)): Path<(String, i64)>,
) -> Result<axum::response::Response, StatusCode> {
    let (mime, audio) = state.db.get_message_audio(&user_id, message_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(([
        (axum::http::header::CONTENT_TYPE, mime),
        (axum::http::header::CACHE_CONTROL, "private, no-store".to_string()),
    ], audio).into_response())
}

async fn clear_messages(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    tracing::debug!(user_id = %user_id, "[MESSAGES] clearing messages");
    match state.db.clear_messages(&user_id) {
        Ok(()) => Ok(Json(serde_json::json!({ "success": true }))),
        Err(e) => {
            tracing::error!(user_id = %user_id, error = %e, "[MESSAGES] clear failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Chat-only clear: hides everything up to the newest message id from the
/// CHAT view (custom_data.chat_cleared_message_id marker) without deleting
/// any rows. The Messages tab keeps showing the full history; new messages
/// (and new Discord mirrors) appear in the chat again after the marker.
async fn clear_chat_view(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let max_id = state
        .db
        .max_message_id(&user_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = state.db.merge_context(
        &user_id,
        serde_json::json!({ "custom_data": { "chat_cleared_message_id": max_id } }),
    );
    tracing::info!(user_id = %user_id, marker = max_id, "[MESSAGES] chat view cleared (rows kept)");
    Ok(Json(serde_json::json!({ "success": true, "cleared_before_id": max_id })))
}

/// Manual compaction (dashboard /compact): generate a summary, then replace
/// the message history with the recent kept rows. Same logic as the
/// WebSocket /compact command.
async fn compact_messages(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let gw = crate::gateway::state_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    tracing::info!(user_id = %user_id, "[MESSAGES] compacting history");
    match crate::gateway::ws_handler::compact_history(gw, &user_id).await {
        Ok(summary) => Ok(Json(serde_json::json!({
            "success": true,
            "summary": summary,
        }))),
        Err(e) => {
            tracing::error!(user_id = %user_id, error = %e, "[MESSAGES] compact failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// List registered skills (dashboard /skill with no argument).
async fn list_skills(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut registry = crate::skills::SkillRegistry::new();
    let dir = std::path::Path::new("skills");
    if dir.exists() {
        registry
            .load_from_dir(dir)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let skills: Vec<serde_json::Value> = registry
        .list()
        .iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "user_only": s.user_only,
            })
        })
        .collect();
    let active = state
        .db
        .load_context("default")
        .ok()
        .and_then(|c| c.settings.active_skill.clone());
    Ok(Json(serde_json::json!({ "skills": skills, "active_skill": active })))
}

/// Delegation records for a user (dashboard /delegations).
async fn list_delegations_route(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match crate::gateway::delegation::list_delegations(&state.db, &user_id) {
        Ok(list) => Ok(Json(serde_json::json!({ "delegations": list }))),
        Err(e) => {
            tracing::error!(user_id = %user_id, error = %e, "[DELEGATIONS] list failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ── Templates ────────────────────────────────────────────────────────────────

async fn list_templates(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.db.list_templates() {
        Ok(templates) => {
            let tpls: Vec<serde_json::Value> = templates
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "is_system": t.is_system,
                        "updated_at": t.updated_at,
                    })
                })
                .collect();
            Ok(Json(serde_json::json!({ "templates": tpls })))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn get_template(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let templates = state
        .db
        .list_templates()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let template = templates
        .into_iter()
        .find(|t| t.name == name)
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(serde_json::json!({
        "name": template.name,
        "content": template.content,
        "description": template.description,
    })))
}

async fn update_template(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
    Json(update): Json<TemplateUpdate>,
) -> Result<Json<TemplateSaveResult>, StatusCode> {
    let result = async {
        let mut ctx = if let Some(uid) = update.user_id.as_deref().filter(|s| !s.is_empty()) {
            state.db.load_context(uid)?
        } else { crate::db::contexts::Context::default() };
        let input = crate::gateway::prompt::preview_input(&state.db, &ctx, update.user_prompt.as_deref())?;
        let plugins_dir = std::env::var("PLUGINS_DIR").unwrap_or_else(|_| "./plugins".into());
        let plugins = crate::plugins::load_all_plugins(std::path::Path::new(&plugins_dir));
        let config = crate::gateway::state_ref().map(|gateway| gateway.config.clone()).unwrap_or_else(crate::config::Config::from_env);
        let workspace = config.workspace_root()?;
        crate::gateway::prompt::route_context_with_workspace(std::path::Path::new("."), &workspace, &mut ctx, &input, &plugins, None)?;
        let context = crate::gateway::prompt::build_context(&state.db, &ctx, &input, &plugins, 0, std::path::Path::new(".")).await?;
        crate::tools::update_template::save_validated(std::path::Path::new("templates"), &name, &update.content, &context).await
    }.await;
    match result {
        Ok(rendered) => {
            let error = state.db.save_template(&name, &update.content, None, false).err().map(|e| format!("Validated template saved to disk, but database update failed: {e}"));
            Ok(Json(TemplateSaveResult { success: true, error, rendered_preview: Some(rendered) }))
        }
        Err(e) => Ok(Json(TemplateSaveResult {
            success: false, error: Some(format!("Template was not saved: {e}")), rendered_preview: None,
        })),
    }
}

async fn create_template(
    State(state): State<Arc<DashboardState>>,
    Json(create): Json<TemplateCreate>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let file_path = format!("templates/{}.poml", create.name);
    if let Some(parent) = std::path::Path::new(&file_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&file_path, &create.content) {
        return Ok(Json(serde_json::json!({
            "success": false,
            "error": format!("Failed to write template file: {}", e)
        })));
    }
    let _ = state.db.save_template(
        &create.name,
        &create.content,
        create.description.as_deref(),
        false,
    );
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn delete_template(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let _ = conn.execute(
        "DELETE FROM templates WHERE name = ?1",
        rusqlite::params![name],
    );
    let file_path = format!("templates/{}.poml", name);
    let _ = std::fs::remove_file(&file_path);
    Ok(Json(serde_json::json!({ "success": true })))
}

// ── Tools ────────────────────────────────────────────────────────────────────

async fn list_tools(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let tools = crate::db::tools::list(&state.db).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "tools": tools })))
}

async fn list_all_tools(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Built-in tools from registry
    let builtin: Vec<_> = crate::tools::registry::all_tool_meta()
        .iter()
        .filter(|m| crate::tools::catalog::is_builtin(m.name))
        .map(|m| serde_json::json!({
            "name": m.name,
            "description": m.description,
            "category": format!("{:?}", m.category),
            "parameters": m.params_schema,
            "source": "builtin",
            "default_enabled": m.default_enabled,
            "is_enabled": crate::db::tools::get(&state.db, m.name).map(|t| t.is_enabled).unwrap_or(m.default_enabled),
        }))
        .collect();

    // Plugin tools
    let plugin_tools: Vec<_> = state.plugins
        .enabled_tools()
        .iter()
        .map(|t| serde_json::json!({
            "name": t.name,
            "description": t.description,
            "category": "Plugin",
            "parameters": t.parameters,
            "source": "plugin",
            "default_enabled": true,
            "is_enabled": crate::db::tools::get_plugin_tool_enabled(&state.db, &t.name),
        }))
        .collect();

    let all = [builtin, plugin_tools].concat();
    Ok(Json(serde_json::json!({ "tools": all, "total": all.len() })))
}

async fn update_tool(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
    Json(update): Json<ToolUpdate>,
) -> Result<String, StatusCode> {
    set_dashboard_tool_enabled(&state.db, &state.plugins, &name, update.is_enabled)?;
    Ok("Tool updated".to_string())
}

fn set_dashboard_tool_enabled(
    db: &crate::db::Database,
    plugins: &crate::plugins::PluginRegistry,
    name: &str,
    enabled: bool,
) -> Result<(), StatusCode> {
    let builtins = crate::db::tools::list(db).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Builtins own their names, including when a plugin declares the same name.
    if crate::tools::catalog::is_builtin(name) && builtins.iter().any(|tool| tool.name == name) {
        crate::db::tools::set_enabled(db, name, enabled)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    } else if plugins.enabled_tools().iter().any(|tool| tool.name == name) {
        crate::db::tools::set_plugin_tool_enabled(db, name, enabled)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

// ── Memory ───────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct MemoryQuery {
    profile: Option<String>,
}

async fn get_memory(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<MemoryInfo>, StatusCode> {
    use crate::db::memory_profiles as profiles;
    let ctx = state
        .db
        .load_context(&user_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut view =
        profiles::snapshot(&state.db, &ctx).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let active_profile = view.profile.clone();
    if let Some(name) = query.profile {
        profiles::validate_name(&name).map_err(|_| StatusCode::BAD_REQUEST)?;
        let memory = profiles::read_named(&state.db, &user_id, &name)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        view.profile = name;
        view.exists = memory.is_some();
        view.memory = memory.unwrap_or_default();
    }
    Ok(Json(MemoryInfo {
        user_id,
        active_profile,
        profile: view.profile,
        profile_exists: view.exists,
        profiles: view.profiles,
        shared: view.shared,
        user_preferences: view.memory.user_preferences,
        custom_variables: view.memory.custom_variables,
        learned_facts: view.memory.learned_facts,
        last_topics: view.memory.last_topics,
    }))
}

async fn update_memory(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
    Json(update): Json<MemoryUpdate>,
) -> Result<String, StatusCode> {
    use crate::db::memory_profiles as profiles;
    let profile = match update.profile {
        Some(name) => name,
        None => {
            let ctx = state
                .db
                .load_context(&user_id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            profiles::snapshot(&state.db, &ctx)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .profile
        }
    };
    profiles::validate_name(&profile).map_err(|_| StatusCode::BAD_REQUEST)?;
    if profile == profiles::SHARED
        && !update
            .reason
            .as_deref()
            .is_some_and(|r| !r.trim().is_empty() && r.len() <= 512)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    if profiles::read_named(&state.db, &user_id, &profile)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    profiles::update_named(&state.db, &user_id, &profile, |memory| {
        if let Some(vars) = update.custom_variables {
            memory.custom_variables = vars;
        }
        if let Some(facts) = update.learned_facts {
            memory.learned_facts = facts;
        }
        if let Some(topics) = update.last_topics {
            memory.last_topics = topics;
        }
        if let Some(preferences) = update.user_preferences {
            memory.user_preferences = preferences;
        }
        Ok(())
    })
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok("Memory updated".to_string())
}

// ── Secrets ──────────────────────────────────────────────────────────────────

async fn get_secrets() -> Result<Json<SecretsInfo>, StatusCode> {
    let secrets = crate::db::secrets::get_secrets();
    let custom_masked: std::collections::HashMap<String, String> = secrets
        .custom
        .iter()
        .filter(|(k, _)| k.as_str() != crate::gateway::llm::codex::SECRET_KEY)
        .map(|(k, v)| (k.clone(), crate::db::secrets::mask_secret(&Some(v.clone()))))
        .collect();
    Ok(Json(SecretsInfo {
        codex_auth: if crate::gateway::llm::codex::CodexAuth::from_secrets(&secrets).is_some() { "***".into() } else { String::new() },
        discord_bot_token: crate::db::secrets::mask_secret(&secrets.discord_bot_token),
        openai_api_key: crate::db::secrets::mask_secret(&secrets.openai_api_key),
        anthropic_api_key: crate::db::secrets::mask_secret(&secrets.anthropic_api_key),
        ollama_api_key: crate::db::secrets::mask_secret(&secrets.ollama_api_key),
        llamacpp_api_key: crate::db::secrets::mask_secret(&secrets.llamacpp_api_key),
        minimax_api_key: crate::db::secrets::mask_secret(&secrets.minimax_api_key),
        mimo_api_key: crate::db::secrets::mask_secret(&secrets.mimo_api_key),
        elevenlabs_api_key: crate::db::secrets::mask_secret(&secrets.elevenlabs_api_key),
        gateway_api_key: crate::db::secrets::mask_secret(&secrets.gateway_api_key),
        dashboard_admin_password: crate::db::secrets::mask_secret(
            &secrets.dashboard_admin_password,
        ),
        custom: custom_masked,
    }))
}

async fn update_secrets(Json(update): Json<SecretsUpdate>) -> Result<String, StatusCode> {
    let mut secrets = crate::db::secrets::get_secrets();
    if let Some(auth) = update.codex_auth.as_deref() {
        apply_codex_secret(&mut secrets, auth).map_err(|_| StatusCode::BAD_REQUEST)?;
    }
    // Felder, die wegen Leer-Werten übersprungen wurden (Selbst-Zugangs-
    // daten dürfen nie mit "" in den Store — sonst Login/Gateway-401).
    let mut skipped: Vec<&str> = Vec::new();

    if let Some(v) = update.discord_bot_token {
        secrets.discord_bot_token = Some(v);
    }
    if let Some(v) = update.openai_api_key {
        secrets.openai_api_key = Some(v);
    }
    if let Some(v) = update.anthropic_api_key {
        secrets.anthropic_api_key = Some(v);
    }
    if let Some(v) = update.ollama_api_key {
        secrets.ollama_api_key = Some(v);
    }
    if let Some(v) = update.llamacpp_api_key {
        secrets.llamacpp_api_key = Some(v);
    }
    if let Some(v) = update.minimax_api_key {
        secrets.minimax_api_key = Some(v);
    }
    if let Some(v) = update.mimo_api_key {
        secrets.mimo_api_key = Some(v);
    }
    if let Some(v) = update.elevenlabs_api_key {
        secrets.elevenlabs_api_key = Some(v);
    }
    if let Some(v) = update.gateway_api_key {
        // Selbst-Zugangsdaten: Leere Werte würden Login/Gateway still mit ""
        // in den Store schreiben (Store > Env) → Lockout/401 bis Store-Reset.
        // Leere = überspringen (Nichts senden = unverändert).
        if v.trim().is_empty() {
            skipped.push("gateway_api_key");
        } else {
            secrets.gateway_api_key = Some(v);
        }
    }
    if let Some(v) = update.dashboard_admin_password {
        if v.trim().is_empty() {
            skipped.push("dashboard_admin_password");
        } else {
            secrets.dashboard_admin_password = Some(v);
        }
    }

    for (k, v) in update.custom {
        if v.is_empty() {
            secrets.custom.remove(&k);
        } else {
            secrets.custom.insert(k, v);
        }
    }

    let skipped_note = if skipped.is_empty() {
        String::new()
    } else {
        format!(" (leere Werte ignoriert: {})", skipped.join(", "))
    };

    // Persist to enc2 if master password provided
    if let Some(ref password) = update.master_password {
        // Trim wie beim Start (MASTER_KEY_FILE wird beim Lesen getrimmt):
        // Copy-Paste-Zeilenümbrüche dürfen kein Re-Keying auslösen.
        let password = password.trim();
        // Guard: Bei vorhandenem Store MUSS das Feld den AKTUELLEN Master-Key
        // öffnen (echter Decrypt-Test). Ohne Check verschlüsselt save_secrets
        // den Store still mit einem evtl. falschen Wert NEU → nächster Start
        // „Invalid MASTER_KEY (hash mismatch)" (21.09. live passiert: Secret
        // im Dashboard geändert, Restart brickte).
        if crate::db::secrets::has_secrets() && !crate::db::enc2::verify_password(password) {
            return Ok("Falsches Master-Passwort — NICHTS gespeichert, Store unverändert. (Feld = exakter Inhalt von vps/master_key)".to_string());
        }
        if let Err(_e) = crate::db::secrets::save_secrets(&secrets, password) {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        crate::db::secrets::init_secrets(secrets.clone());
        reload_llm_router(&secrets);
        Ok(format!("Secrets saved and encrypted.{}", skipped_note))
    } else {
        crate::db::secrets::init_secrets(secrets.clone());
        reload_llm_router(&secrets);
        Ok(format!(
            "Secrets updated in memory. Provide master_password to persist to disk.{}",
            skipped_note
        ))
    }
}

/// Provider keys changed in the dashboard take effect without a restart.
fn reload_llm_router(secrets: &crate::db::secrets::Secrets) {
    if let Some(state) = crate::gateway::state_ref() {
        let config = crate::gateway::providers::effective_config(&state.config, secrets);
        state.llm.swap(crate::gateway::llm::LLMRouter::new(&config, secrets));
    }
}

fn apply_codex_secret(secrets: &mut crate::db::secrets::Secrets, value: &str) -> anyhow::Result<()> {
    use crate::gateway::llm::codex::{CodexAuth, SECRET_KEY};
    if value.starts_with("***") { return Ok(()); }
    if value.trim().is_empty() {
        secrets.custom.remove(SECRET_KEY);
    } else {
        CodexAuth::from_json(value)?.store(secrets);
    }
    Ok(())
}

#[cfg(test)]
mod codex_secret_tests {
    use super::*;

    #[tokio::test]
    async fn dashboard_codex_auth_is_validated_normalized_and_masked() {
        use crate::gateway::llm::codex::{CodexAuth, SECRET_KEY};
        let _lock = crate::db::secrets::test_lock();
        let original = crate::db::secrets::get_secrets();
        let mut secrets = crate::db::secrets::Secrets::default();
        apply_codex_secret(&mut secrets, r#"{"tokens":{"access_token":"private-access","refresh_token":"private-refresh"}}"#).unwrap();
        assert!(CodexAuth::from_secrets(&secrets).is_some());
        let saved = secrets.custom[SECRET_KEY].clone();
        for invalid in ["invalid", "{}", r#"{"tokens":{"access_token":"bad token"}}"#] {
            assert!(apply_codex_secret(&mut secrets, invalid).is_err());
            assert_eq!(secrets.custom[SECRET_KEY], saved);
        }
        apply_codex_secret(&mut secrets, "***").unwrap();
        assert_eq!(secrets.custom[SECRET_KEY], saved);
        crate::db::secrets::init_secrets(secrets.clone());
        let info = serde_json::to_value(get_secrets().await.unwrap().0).unwrap();
        assert_eq!(info["codex_auth"], "***");
        assert!(!info.to_string().contains("private"));
        apply_codex_secret(&mut secrets, "").unwrap();
        assert!(CodexAuth::from_secrets(&secrets).is_none());
        crate::db::secrets::init_secrets(original);
    }
}

// ── Pairings ─────────────────────────────────────────────────────────────────

async fn list_pairings(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let mut stmt = conn
        .prepare("SELECT user_id, discord_user_id, discord_guild_id, paired_at, last_seen_at FROM pairings ORDER BY paired_at DESC")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let pairings: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "user_id": row.get::<_, String>(0)?,
                "discord_user_id": row.get::<_, String>(1)?,
                "discord_guild_id": row.get::<_, Option<String>>(2)?,
                "paired_at": row.get::<_, Option<String>>(3)?,
                "last_seen_at": row.get::<_, Option<String>>(4)?,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    Ok(Json(serde_json::json!({ "pairings": pairings })))
}

async fn delete_pairing(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<String, StatusCode> {
    let conn = state.db.conn();
    conn.execute(
        "DELETE FROM pairings WHERE user_id = ?1",
        rusqlite::params![user_id],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok("Pairing deleted".to_string())
}

async fn list_pending_pairings(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let mut stmt = conn
        .prepare("SELECT code, discord_user_id, expires_at, created_at FROM pending_pairings ORDER BY created_at DESC")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let pending: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "code": row.get::<_, String>(0)?,
                "discord_user_id": row.get::<_, String>(1)?,
                "expires_at": row.get::<_, String>(2)?,
                "created_at": row.get::<_, Option<String>>(3)?,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    Ok(Json(serde_json::json!({ "pending_pairings": pending })))
}

async fn approve_pending_pairing(
    State(state): State<Arc<DashboardState>>,
    Path(code): Path<String>,
) -> Result<String, StatusCode> {
    let pending = state
        .db
        .get_pending_pairing(&code)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let pending = match pending {
        Some(p) => p,
        None => return Err(StatusCode::NOT_FOUND),
    };

    let user_id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .create_pairing(&user_id, &pending.discord_user_id, None)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    state
        .db
        .delete_pending_pairing(&code)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(format!(
        "Pairing approved for Discord user {}",
        pending.discord_user_id
    ))
}

async fn delete_pending_pairing(
    State(state): State<Arc<DashboardState>>,
    Path(code): Path<String>,
) -> Result<String, StatusCode> {
    state
        .db
        .delete_pending_pairing(&code)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok("Pending pairing deleted".to_string())
}

// ── Statemachine Files ────────────────────────────────────────────────

async fn list_sm_files(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = state; // Workflows use the same canonical root as runtime routing.
    let sm_dir = std::path::Path::new("contexts");
    let mut files = Vec::new();

    for dir in [sm_dir] {
        if dir.exists() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|e| e.to_str());
                    if ext == Some("sm") || ext == Some("cl") {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        if !files
                            .iter()
                            .any(|f: &serde_json::Value| f["name"].as_str() == Some(&name))
                        {
                            files.push(serde_json::json!({
                                "name": name,
                                "path": path.to_string_lossy(),
                            }));
                        }
                    }
                }
            }
        }
    }
    files.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    Ok(Json(serde_json::json!({ "sm_files": files })))
}

async fn get_sm_file(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<String, StatusCode> {
    let _ = state;
    let path = crate::sm::resolve_file_in(std::path::Path::new("contexts"), &name).map_err(|_| StatusCode::NOT_FOUND)?;
    std::fs::read_to_string(path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn save_sm_file(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
    Json(update): Json<SmFileUpdate>,
) -> Result<String, StatusCode> {
    let _ = state;
    crate::sm::save_file_in(std::path::Path::new("contexts"), &name, &update.content).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok("SM file saved".to_string())
}

// ── Cron Jobs ────────────────────────────────────────────────────────────────

async fn list_cron_jobs(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, schedule, enabled, last_run, run_count FROM cron_jobs ORDER BY name",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let jobs: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "schedule": row.get::<_, String>(2)?,
                "enabled": row.get::<_, i32>(3)? != 0,
                "last_run": row.get::<_, Option<String>>(4)?,
                "run_count": row.get::<_, i64>(5)?,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    Ok(Json(serde_json::json!({ "cron_jobs": jobs })))
}

pub(crate) async fn tool_activity(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let limit = params.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(50).clamp(1, 500);
    let mut stmt = conn
        .prepare("SELECT vm_id, action, input, output, created_at FROM vm_activity_log ORDER BY id DESC LIMIT ?1")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let activities: Vec<serde_json::Value> = stmt
        .query_map([limit], |row| {
            Ok(serde_json::json!({
                "vm_id": row.get::<_, String>(0)?,
                "action": row.get::<_, String>(1)?,
                "input": row.get::<_, Option<String>>(2)?,
                "output": row.get::<_, Option<String>>(3)?,
                "created_at": row.get::<_, Option<String>>(4)?,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    Ok(Json(serde_json::json!({ "activities": activities })))
}

#[derive(Deserialize)]
struct AgentInputRequest {
    user_id: String,
    message: String,
    attachments: Option<Vec<String>>,
}

async fn send_agent_input(
    Json(req): Json<AgentInputRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut full_message = req.message.clone();
    if let Some(ref atts) = req.attachments {
        for a in atts {
            full_message.push_str(&format!("\n[Attachment: {}]", a));
        }
    }
    match crate::gateway::agent_loop::get_user_input_sender(&req.user_id).await {
        Some(sender) => {
            if let Err(e) = sender.send(full_message) {
                return Ok(Json(serde_json::json!({
                    "error": format!("Failed to send message: {}", e)
                })));
            }
            Ok(Json(serde_json::json!({
                "success": true,
                "message": "Message sent"
            })))
        }
        None => Ok(Json(serde_json::json!({
            "error": "No active agent loop found for this user"
        }))),
    }
}

async fn begin_agent(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user_id = req["user_id"].as_str().unwrap_or("default").to_string();
    let message = req["message"].as_str().unwrap_or("").to_string();
    let gateway_key = state.gateway_api_key.clone();

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let url = "http://127.0.0.1:3537/v1/chat";
        let body = serde_json::json!({
            "user_id": user_id,
            "message": message,
        });
        let _ = client
            .post(url)
            .bearer_auth(&gateway_key)
            .json(&body)
            .send()
            .await;
    });

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Agent dispatched to gateway"
    })))
}

async fn get_agent_status(
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let has_loop = crate::gateway::agent_loop::get_user_input_sender(&user_id).await.is_some();
    Ok(Json(serde_json::json!({ "active": has_loop })))
}

async fn stop_agent(
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::gateway::agent_loop::stop_agent_loop(&user_id).await;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn upload_avatar(
    State(_state): State<Arc<DashboardState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    std::fs::create_dir_all(format!("{}/avatars", data_dir)).ok();

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("unknown").to_string();
        let mut data = Vec::new();
        while let Ok(Some(chunk)) = field.chunk().await {
            data.extend_from_slice(&chunk);
        }
        if data.len() > 2_000_000 {
            return Ok(Json(serde_json::json!({"error": "File too large (max 2MB)"})));
        }
        let ext = if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) { "png" }
            else if data.starts_with(&[0xFF, 0xD8, 0xFF]) { "jpg" }
            else if data.starts_with(b"GIF8") { "gif" }
            else if data.starts_with(b"RIFF") && data.len() > 8 && &data[8..12] == b"WEBP" { "webp" }
            else { return Ok(Json(serde_json::json!({"error": "Unsupported format. Use PNG, JPG, GIF, or WebP."}))); };

        let path = format!("{}/avatars/{}.{}", data_dir, name, ext);
        std::fs::write(&path, &data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(Json(serde_json::json!({"success": true, "url": format!("/api/avatar/{}", name)})));
    }
    Ok(Json(serde_json::json!({"error": "No file uploaded"})))
}

async fn get_avatar(
    Path(name): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    for ext in &["png", "jpg", "jpeg", "gif", "webp"] {
        let path = format!("{}/avatars/{}.{}", data_dir, name, ext);
        if let Ok(data) = std::fs::read(&path) {
            let ct = match *ext {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "application/octet-stream",
            };
            return Ok(([(axum::http::header::CONTENT_TYPE, ct)], data));
        }
    }
    Err(StatusCode::NOT_FOUND)
}

async fn upload_chat_file(
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    std::fs::create_dir_all(format!("{}/uploads", data_dir)).ok();
    let mut files = Vec::new();

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let filename = field.file_name().unwrap_or("file").to_string();
        let mut data = Vec::new();
        while let Ok(Some(chunk)) = field.chunk().await {
            data.extend_from_slice(&chunk);
        }
        if data.len() > 50_000_000 {
            return Ok(Json(serde_json::json!({"error": "File too large (max 50MB)"})));
        }
        let path = format!("{}/uploads/{}", data_dir, filename);
        std::fs::write(&path, &data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        files.push(format!("/api/files/{}", filename));
    }
    if files.is_empty() {
        return Ok(Json(serde_json::json!({"error": "No files found"})));
    }
    Ok(Json(serde_json::json!({"success": true, "files": files})))
}

async fn get_chat_file(
    Path(name): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let path = format!("{}/uploads/{}", data_dir, name);
    match std::fs::read(&path) {
        Ok(data) => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            data,
        )),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

async fn get_screenshot(
    Path(path): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let full_path = format!("{}/{}", data_dir, path);
    // Security: ensure path stays under data_dir
    let canonical = std::fs::canonicalize(&full_path).unwrap_or_default();
    let base = std::fs::canonicalize(&data_dir).unwrap_or_default();
    if !canonical.starts_with(&base) {
        return Err(StatusCode::FORBIDDEN);
    }
    let data = std::fs::read(&canonical).map_err(|_| StatusCode::NOT_FOUND)?;
    let ct = if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".gif") {
        "image/gif"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else {
        "application/octet-stream"
    };
    Ok(([(axum::http::header::CONTENT_TYPE, ct)], data))
}

/// Dashboard chat speech-to-text: accepts a browser MediaRecorder blob
/// (webm/ogg/wav), transcribes it with the user's configured STT engine
/// (ElevenLabs scribe by default) and returns the text.
async fn dashboard_stt(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<HashMap<String, String>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user_id = params
        .get("user_id")
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    // Accept both multipart form uploads (browser MediaRecorder) and raw
    // bodies (curl tests). The dashboard mic sends multipart with an 'audio'
    // field; a raw body is used as fallback.
    let body: Vec<u8> = {
        let mut collected: Vec<u8> = Vec::new();
        // Find the audio field (accept any field name; first non-empty wins).
        loop {
            match multipart.next_field().await {
                Ok(Some(mut field)) => {
                    let mut data = Vec::new();
                    while let Ok(Some(chunk)) = field.chunk().await {
                        data.extend_from_slice(&chunk);
                    }
                    if !data.is_empty() {
                        collected = data;
                        break;
                    }
                }
                _ => break,
            }
        }
        collected
    };
    if body.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let ctx = state.db.load_context(&user_id).map_err(|_| StatusCode::NOT_FOUND)?;
    let secrets = crate::db::secrets::get_secrets();
    let api_key = secrets
        .elevenlabs_api_key
        .clone()
        .unwrap_or_default();
    if ctx.settings.voice_stt_type == "elevenlabs" && api_key.is_empty() {
        return Ok(Json(serde_json::json!({"error": "ElevenLabs API key not configured"})));
    }
    let stt_config = crate::voice::STTConfig {
        engine: ctx.settings.voice_stt_type.clone(),
        api_key: (!api_key.is_empty()).then_some(api_key),
        model_path: if ctx.settings.voice_stt_type == "vosk" {
            ctx.settings.voice_vosk_model_path.clone()
        } else {
            ctx.settings.voice_whisper_model_path.clone()
        },
        vosk_url: ctx.settings.voice_vosk_url.clone(),
        elevenlabs_model: ctx.settings.elevenlabs_stt_model.clone(),
        elevenlabs_language: ctx.settings.elevenlabs_stt_language.clone(),
        elevenlabs_tag_audio_events: ctx.settings.elevenlabs_stt_tag_audio_events,
        elevenlabs_no_verbatim: ctx.settings.elevenlabs_stt_no_verbatim,
    };
    let stt_threshold = ctx.settings.stt_low_confidence_threshold;
    match crate::voice::transcribe_audio(&body, &stt_config).await {
        Ok(text) => Ok(Json(serde_json::json!({
            "text": text,
            "confidence": crate::voice::last_stt_confidence(),
            "low_confidence": crate::voice::last_stt_confidence().map_or(false, |c| c < stt_threshold),
            "threshold": stt_threshold,
        }))),
        Err(e) => Ok(Json(serde_json::json!({"error": e.to_string()}))),
    }
}

/// Context profiles: named, complete context snapshots ("Marvin-Default", ...)
/// stored in the DB so a fresh user can be configured with one click.
/// Stored in table `context_profiles (name, data, created_at)`.
async fn list_profiles(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS context_profiles (
            name TEXT PRIMARY KEY,
            data TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    );
    let mut stmt = conn
        .prepare("SELECT name, created_at FROM context_profiles ORDER BY created_at")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let profiles: Vec<_> = rows
        .filter_map(|row| {
            let (name, created_at) = match row {
                Ok((n, c)) => (n, c),
                Err(_) => return None,
            };
            Some(serde_json::json!({
                "name": name,
                "created_at": created_at,
            }))
        })
        .collect();
    Ok(Json(serde_json::json!({"profiles": profiles})))
}

async fn save_profile(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let name = req["name"].as_str().unwrap_or("").trim().to_string();
    let source_user = req["source_user_id"].as_str().unwrap_or("default");
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let ctx = state
        .db
        .load_context(source_user)
        .map_err(|_| StatusCode::NOT_FOUND)?;
    // Strip volatile fields: keep settings + custom_data + sm_file choice.
    let snapshot = serde_json::json!({
        "settings": ctx.settings,
        "custom_data": ctx.custom_data,
        "sm_file": ctx.sm_file,
    });
    let conn = state.db.conn();
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS context_profiles (
            name TEXT PRIMARY KEY,
            data TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    );
    conn.execute(
        "INSERT INTO context_profiles (name, data, created_at) VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(name) DO UPDATE SET data = ?2, created_at = datetime('now')",
        rusqlite::params![name, snapshot.to_string()],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"success": true, "name": name})))
}

async fn apply_profile(
    State(state): State<Arc<DashboardState>>,
    Path((name, user_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Read the profile snapshot in its own lock scope, then drop the guard
    // BEFORE merge_context takes the lock again (avoids self-deadlock).
    let update = {
        let conn = state.db.conn();
        let data: String = conn
            .query_row(
                "SELECT data FROM context_profiles WHERE name = ?1",
                rusqlite::params![name],
                |row| row.get(0),
            )
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let snapshot: serde_json::Value =
            serde_json::from_str(&data).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        serde_json::json!({
            "settings": snapshot.get("settings").cloned().unwrap_or_default(),
            "custom_data": snapshot.get("custom_data").cloned().unwrap_or_default(),
            "sm_file": snapshot.get("sm_file").cloned().unwrap_or_default(),
        })
    }; // MutexGuard dropped here
    let ctx = state
        .db
        .merge_context(&user_id, update)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(serde_json::to_value(ctx).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

async fn delete_profile(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = state.db.conn();
    conn.execute(
        "DELETE FROM context_profiles WHERE name = ?1",
        rusqlite::params![name],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({"success": true})))
}

async fn chat_query(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user_id = req["user_id"].as_str().unwrap_or("default");
    let message = req["message"].as_str().unwrap_or("");
    let is_option = req["is_option"].as_bool().unwrap_or(false);
    let option_index = req["option_index"].as_u64();
    let question_id = req["question_id"].as_str().unwrap_or("");
    tracing::info!(user_id = %user_id, message = %message, "[DASHBOARD] chat_query received");

    // Handle option selection
    if is_option && !question_id.is_empty() {
        crate::tools::web_interactive::handle_web_option(question_id, option_index.unwrap_or(0) as usize).await;
        return Ok(Json(serde_json::json!({"success": true, "type": "option"})));
    }

    // Handle text reply to pending question
    if !message.is_empty() {
        let consumed = crate::tools::web_interactive::handle_web_message_reply(user_id, message).await;
        if consumed {
            return Ok(Json(serde_json::json!({"success": true, "type": "question_reply"})));
        }
    }

    // Overwrite custom_data.user_prompt with the current message
    if !message.is_empty() {
        let _ = state.db.merge_context(user_id, serde_json::json!({"custom_data": {"user_prompt": message}}));
    }

    // Check if agent loop is active
    let has_loop = crate::gateway::agent_loop::get_user_input_sender(user_id).await.is_some();
    if !has_loop {
        // Auto-start agent loop via gateway API
        let gateway_key = state.gateway_api_key.clone();
        let uid = user_id.to_string();
        let msg = message.to_string();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let body = serde_json::json!({"user_id": uid, "message": msg});
            let _ = client
                .post("http://127.0.0.1:3537/v1/chat")
                .bearer_auth(&gateway_key)
                .json(&body)
                .send()
                .await;
        });
        return Ok(Json(serde_json::json!({
            "success": true,
            "type": "agent_started",
            "message": "Agent loop started. Response will appear shortly."
        })));
    }

    // Agent loop is running: inject message
    if let Some(sender) = crate::gateway::agent_loop::get_user_input_sender(user_id).await {
        let mut full = message.to_string();
        if let Some(atts) = req["attachments"].as_array() {
            for a in atts { if let Some(s) = a.as_str() { full.push_str(&format!("\n[Attachment: {}]", s)); } }
        }
        sender.send(full).ok();
        Ok(Json(serde_json::json!({"success": true, "type": "injected"})))
    } else {
        Ok(Json(serde_json::json!({"error": "Agent loop not available"})))
    }
}

async fn chat_stream_auth(
    Path(user_id): Path<String>,
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Sse<impl futures_util::stream::Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str());
    let valid = token.map_or(false, |t| {
        let jwt = jsonwebtoken::decode::<crate::gateway::auth::Claims>(
            t,
            &jsonwebtoken::DecodingKey::from_secret(state.gateway_api_key.as_bytes()),
            &jsonwebtoken::Validation::default(),
        );
        jwt.is_ok() || t == state.gateway_api_key
    });
    if !valid {
        tracing::warn!(user_id = %user_id, "[SSE] auth failed");
        return Err(StatusCode::UNAUTHORIZED);
    }
    tracing::info!(user_id = %user_id, "[SSE] connection opened");
    let rx = crate::runtime::events::get_or_create(&user_id).subscribe();
    let uid = user_id.clone();
    let stream = futures_util::stream::unfold(rx, move |mut r| {
        let uid = uid.clone();
        async move {
            loop {
                match r.recv().await {
                    Ok(ev) => {
                        tracing::debug!(
                            user_id = %uid,
                            event = %ev.event,
                            data_len = ev.data.len(),
                            "[SSE] forwarding event to client"
                        );
                        let data = serde_json::to_string(&ev).unwrap_or_default();
                        let event = Event::default().event(ev.event).data(data);
                        return Some((Ok(event), r));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(user_id = %uid, skipped = n, "[SSE] receiver lagged");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!(user_id = %uid, "[SSE] channel closed, ending stream");
                        return None;
                    }
                }
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// TTS-only side channel for the dashboard chat: forwards `chat_tts` and
/// `chat_tts_settings` events from the given user's stream. Used by the frontend to also hear
/// TTS generated for a different session (e.g. Discord traffic on the paired
/// user_id) while another chat session is active. All other events are
/// dropped here so the main stream stays the single source for chat content.
async fn chat_stream_tts_only(
    Path(user_id): Path<String>,
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Sse<impl futures_util::stream::Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    let token = params.get("token").map(|s| s.as_str());
    let valid = token.map_or(false, |t| {
        let jwt = jsonwebtoken::decode::<crate::gateway::auth::Claims>(
            t,
            &jsonwebtoken::DecodingKey::from_secret(state.gateway_api_key.as_bytes()),
            &jsonwebtoken::Validation::default(),
        );
        jwt.is_ok() || t == state.gateway_api_key
    });
    if !valid {
        tracing::warn!(user_id = %user_id, "[SSE-TTS] auth failed");
        return Err(StatusCode::UNAUTHORIZED);
    }
    tracing::info!(user_id = %user_id, "[SSE-TTS] connection opened");
    let rx = crate::runtime::events::get_or_create(&user_id).subscribe();
    let uid = user_id.clone();
    let stream = futures_util::stream::unfold(rx, move |mut r| {
        let uid = uid.clone();
        async move {
            loop {
                match r.recv().await {
                    Ok(ev) => {
                        if !matches!(ev.event.as_str(), "chat_tts" | "chat_tts_settings") {
                            continue; // Never mirror chat/context contents here.
                        }
                        tracing::debug!(user_id = %uid, event = %ev.event, "[SSE-TTS] forwarding audio event");
                        let data = serde_json::to_string(&ev).unwrap_or_default();
                        let event = Event::default().event(ev.event).data(data);
                        return Some((Ok(event), r));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(user_id = %uid, skipped = n, "[SSE-TTS] receiver lagged");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!(user_id = %uid, "[SSE-TTS] channel closed, ending stream");
                        return None;
                    }
                }
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn get_sm_info(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = state.db.load_context(&user_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let templates: Vec<String> = ctx.active_templates.clone();
    let sm_data = if ctx.sm_data.is_object() {
        let mut flat = serde_json::Map::new();
        for (k, v) in ctx.sm_data.as_object().unwrap() {
            if !v.is_null() {
                flat.insert(k.clone(), v.clone());
            }
        }
        serde_json::Value::Object(flat)
    } else {
        serde_json::json!({})
    };
    Ok(Json(serde_json::json!({
        "sm_file": crate::gateway::prompt::workflow_name(&ctx),
        "system_template": ctx.settings.system_template.as_deref().unwrap_or("standard"),
        "active_skill": ctx.settings.active_skill,
        "active_state": ctx.active_state,
        "active_templates": templates,
        "sm_data": sm_data,
    })))
}

#[cfg(test)]
#[path = "audio_tests.rs"]
mod audio_tests;

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tool_tests;

#[cfg(test)]
mod dashboard_tests {
    use super::*;
    use tempfile::TempDir;

    fn test_setup() -> (crate::db::Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        (db, dir)
    }

    #[test]
    fn test_routes_compiles() {
        assert!(true);
    }

    #[test]
    fn test_dashboard_state_clone() {
        let (db, _dir) = test_setup();
        let state = DashboardState { plugins: Arc::new(crate::plugins::PluginRegistry::new()),
            db,
            gateway_api_key: "test-api-key-12345678".to_string(),
            admin_password: "testpassword".to_string(),
        };
        let _cloned = state.clone();
    }

    #[test]
    fn test_tool_update_deserialize() {
        let json = r#"{"is_enabled": true}"#;
        let update: ToolUpdate = serde_json::from_str(json).unwrap();
        assert!(update.is_enabled);
    }

    #[tokio::test]
    async fn backend_memory_api_roundtrip_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(DashboardState { plugins: Arc::new(crate::plugins::PluginRegistry::new()),  db: crate::db::Database::new(dir.path()).unwrap(), gateway_api_key: String::new(), admin_password: String::new() });
        let update: MemoryUpdate = serde_json::from_value(serde_json::json!({"learned_facts":["one", "one"], "user_preferences":{"brief":true}, "custom_variables":{"n":3}})).unwrap();
        update_memory(State(state.clone()), Path("alice".into()), Json(update))
            .await
            .unwrap();
        let memory = get_memory(
            State(state.clone()),
            Path("alice".into()),
            Query(MemoryQuery::default()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(memory.learned_facts, vec!["one"]);
        assert_eq!(memory.user_preferences["brief"], true);
        assert_eq!(memory.custom_variables["n"], 3);
        let clear: MemoryUpdate =
            serde_json::from_value(serde_json::json!({"learned_facts":[], "user_preferences":{}}))
                .unwrap();
        update_memory(State(state.clone()), Path("alice".into()), Json(clear))
            .await
            .unwrap();
        let memory = get_memory(
            State(state.clone()),
            Path("alice".into()),
            Query(MemoryQuery::default()),
        )
        .await
        .unwrap()
        .0;
        assert!(memory.learned_facts.is_empty());
        assert!(memory.user_preferences.is_empty());
        assert_eq!(memory.custom_variables["n"], 3);
        assert!(get_memory(
            State(state),
            Path("bob".into()),
            Query(MemoryQuery::default())
        )
        .await
        .unwrap()
        .0
        .custom_variables
        .is_empty());
    }

    #[tokio::test]
    async fn backend_memory_api_profiles_do_not_change_selection_or_other_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(DashboardState { plugins: Arc::new(crate::plugins::PluginRegistry::new()),
            db: crate::db::Database::new(dir.path()).unwrap(),
            gateway_api_key: String::new(),
            admin_password: String::new(),
        });
        crate::db::memory_profiles::create_profile(&state.db, "alice", "language_instructor")
            .unwrap();
        let update: MemoryUpdate = serde_json::from_value(
            serde_json::json!({"profile":"language_instructor", "custom_variables":{"xp":2}}),
        )
        .unwrap();
        update_memory(State(state.clone()), Path("alice".into()), Json(update))
            .await
            .unwrap();
        let selected = get_memory(
            State(state.clone()),
            Path("alice".into()),
            Query(MemoryQuery::default()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(selected.profile, "standard");
        assert!(selected.custom_variables.is_empty());
        let lesson = get_memory(
            State(state.clone()),
            Path("alice".into()),
            Query(MemoryQuery {
                profile: Some("language_instructor".into()),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(lesson.custom_variables["xp"], 2);
        assert_eq!(lesson.active_profile, "standard");
        for body in [
            serde_json::json!({"profile":"shared", "custom_variables":{"name":"Alex"}}),
            serde_json::json!({"profile":"shared", "reason":"Explicit synthetic permission", "custom_variables":{"xp":2}}),
        ] {
            let bad: MemoryUpdate = serde_json::from_value(body).unwrap();
            assert_eq!(
                update_memory(State(state.clone()), Path("alice".into()), Json(bad))
                    .await
                    .unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
        let shared: MemoryUpdate = serde_json::from_value(serde_json::json!({"profile":"shared", "reason":"Explicit synthetic permission", "custom_variables":{"name":"Alex"}})).unwrap();
        update_memory(State(state.clone()), Path("alice".into()), Json(shared))
            .await
            .unwrap();
        assert_eq!(
            get_memory(
                State(state),
                Path("alice".into()),
                Query(MemoryQuery::default())
            )
            .await
            .unwrap()
            .0
            .shared["name"],
            "Alex"
        );
    }

    #[tokio::test]
    async fn backend_context_api_lists_only_canonical_sm_names() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(DashboardState { plugins: Arc::new(crate::plugins::PluginRegistry::new()),  db: crate::db::Database::new(dir.path()).unwrap(), gateway_api_key: String::new(), admin_password: String::new() });
        let old = serde_json::json!({"user_id":"alice", "cl_file":"legacy", "settings":{"cl_file":"selected"}}).to_string();
        state.db.conn().execute("INSERT INTO contexts (user_id, data) VALUES ('alice', ?1)", [old]).unwrap();
        let value = get_context(State(state.clone()), Path("alice".into())).await.unwrap().0;
        assert_eq!(value["sm_file"], "legacy");
        assert!(value.get("cl_file").is_none());
        let listed = list_contexts(State(state)).await.unwrap().0;
        let settings = &listed["contexts"][0]["data"]["settings"];
        assert_eq!(settings["sm_file"], "selected");
        assert!(settings.get("cl_file").is_none());
    }

    #[tokio::test]
    async fn backend_sm_status_distinguishes_system_selection_from_template_stack() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(DashboardState { plugins: Arc::new(crate::plugins::PluginRegistry::new()),  db: crate::db::Database::new(dir.path()).unwrap(), gateway_api_key: String::new(), admin_password: String::new() });
        let mut ctx = state.db.load_context("alice").unwrap();
        ctx.settings.system_template = Some("language_instructor".into());
        ctx.settings.active_skill = Some("poml_templates".into());
        state.db.save_context(&ctx).unwrap();
        let status = get_sm_info(State(state), Path("alice".into())).await.unwrap().0;
        assert_eq!(status["system_template"], "language_instructor");
        assert_eq!(status["active_skill"], "poml_templates");
        assert_eq!(status["active_templates"], serde_json::json!([]));
        assert_eq!(status["sm_file"], "standard");
    }

    #[test]
    fn test_memory_update_deserialize() {
        let json = r#"{"learned_facts": ["fact1", "fact2"]}"#;
        let update: MemoryUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.learned_facts.unwrap().len(), 2);
    }

    #[test]
    fn test_secrets_update_deserialize() {
        let json = r#"{"discord_bot_token": "new_token"}"#;
        let update: SecretsUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.discord_bot_token, Some("new_token".to_string()));
    }

    #[test]
    fn test_template_update_deserialize() {
        let json = r#"{"content": "Hello {{name}}"}"#;
        let update: TemplateUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.content, "Hello {{name}}");
    }

    #[test]
    fn test_sm_file_update_deserialize() {
        let json = r#"{"content": "[state test]\nmode = chat"}"#;
        let update: SmFileUpdate = serde_json::from_str(json).unwrap();
        assert!(update.content.contains("state test"));
    }
}

/// Media asset index: lists files in data/uploads with size and mtime,
/// newest first. Backed by the filesystem, no separate index file needed.
async fn list_media(
    State(_state): State<Arc<DashboardState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let uploads = format!("{}/uploads", data_dir);
    let filter = params.get("q").map(|q| q.to_lowercase()).unwrap_or_default();
    let mut files: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&uploads) {
        for entry in entries_flat(entries) {
            let path = entry.path();
            if !path.is_file() { continue; }
            let name = entry.file_name().to_string_lossy().to_string();
            if !filter.is_empty() && !name.to_lowercase().contains(&filter) { continue; }
            let meta = entry.metadata().ok();
            files.push(serde_json::json!({
                "name": name,
                "url": format!("/api/files/{}", name),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                "modified": meta.and_then(|m| m.modified().ok())
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
            }));
        }
    }
    files.sort_by(|a, b| {
        let am = a["modified"].as_str().unwrap_or("");
        let bm = b["modified"].as_str().unwrap_or("");
        bm.cmp(am)
    });
    Ok(Json(serde_json::json!({ "files": files, "count": files.len() })))
}

fn entries_flat(rd: std::fs::ReadDir) -> Vec<std::fs::DirEntry> {
    rd.filter_map(|e| e.ok()).collect()
}
