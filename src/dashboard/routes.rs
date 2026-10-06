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
        .route("/tool-packages", axum::routing::get(list_tool_packages))
        .route("/tool-packages/:id", axum::routing::put(set_tool_package))
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
        // Uploads write to DATA_DIR and always need an operator token.
        .route("/upload-avatar", axum::routing::post(upload_avatar))
        .route(
            "/upload-file",
            axum::routing::post(upload_chat_file).layer(axum::extract::DefaultBodyLimit::max(
                crate::services::media::MAX_UPLOAD + 1024 * 1024,
            )),
        )
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
    crate::services::sessions::chat_sessions(&state.db).map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn list_contexts(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::services::sessions::contexts(&state.db).map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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
        .ok_or(StatusCode::BAD_REQUEST)?;
    if new_user_id.is_empty() || new_user_id == parent_user_id {
        return Err(StatusCode::BAD_REQUEST);
    }
    let username = body.get("username").and_then(|v| v.as_str());
    crate::services::sessions::fork(&state.db, &parent_user_id, new_user_id, username)
        .map(Json)
        .map_err(|e| {
            tracing::error!(error = %e, "fork_context failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// Execute a `/context …` slash command. The body is `{user_id, line}`. The
/// shared parser at `crate::context_cmd` is the single source of truth for
/// the syntax — same parser drives the TUI and Discord.
async fn context_exec(
    State(state): State<Arc<DashboardState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user_id = body.get("user_id").and_then(|v| v.as_str()).ok_or(StatusCode::BAD_REQUEST)?;
    let line = body.get("line").and_then(|v| v.as_str()).ok_or(StatusCode::BAD_REQUEST)?;
    Ok(Json(crate::services::sessions::exec(&state.db, user_id, line)))
}

// ── Messages ─────────────────────────────────────────────────────────────────

async fn get_messages(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let chat_only = params.get("chat_only").map(|v| v == "1" || v == "true").unwrap_or(false);
    crate::services::sessions::messages(&state.db, &user_id, chat_only).map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Authenticated, on-demand replay. Never embed credentials or audio blobs in history.
async fn get_chat_audio(
    State(state): State<Arc<DashboardState>>,
    Path((user_id, message_id)): Path<(String, i64)>,
) -> Result<axum::response::Response, StatusCode> {
    let (mime, audio) = crate::services::media::message_audio(&state.db, &user_id, message_id)
        .map_err(admin_status)?;
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
    let max_id = crate::services::sessions::clear_chat_view(&state.db, &user_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    admin_json(crate::services::admin::skills(&state.db, std::path::Path::new("skills")))
}

/// Delegation records for a user (dashboard /delegations).
async fn list_delegations_route(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::delegations(&state.db, &user_id))
}

// ── Templates ────────────────────────────────────────────────────────────────

async fn list_templates(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::templates(&state.db))
}

async fn get_template(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::template(&state.db, &name))
}

async fn update_template(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
    Json(update): Json<TemplateUpdate>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(
        crate::services::admin::update_template(
            &state.db,
            &name,
            &update.content,
            update.user_id.as_deref(),
            update.user_prompt.as_deref(),
        )
        .await,
    )
}

async fn create_template(
    State(state): State<Arc<DashboardState>>,
    Json(create): Json<TemplateCreate>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::create_template(
        &state.db,
        std::path::Path::new("templates"),
        &create.name,
        &create.content,
        create.description.as_deref(),
    ))
}

async fn delete_template(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::delete_template(
        &state.db,
        std::path::Path::new("templates"),
        &name,
    ))
}

// ── Tools ────────────────────────────────────────────────────────────────────

async fn list_tool_packages(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::tool_packages(&state.db, &state.plugins))
}

async fn set_tool_package(
    State(state): State<Arc<DashboardState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let enabled = body.get("enabled").and_then(|v| v.as_bool()).ok_or(StatusCode::BAD_REQUEST)?;
    admin_json(crate::services::admin::set_tool_package(&state.db, &id, enabled))
}

async fn list_tools(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::tool_records(&state.db))
}

async fn list_all_tools(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(crate::services::admin::all_tools(&state.db, &state.plugins)))
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
    crate::services::admin::set_tool_enabled(db, plugins, name, enabled).map_err(admin_status)
}

/// HTTP status for an administration-service failure.
fn admin_status(failure: crate::services::admin::Failure) -> StatusCode {
    use crate::services::admin::Failure;
    match failure {
        Failure::BadRequest(_) => StatusCode::BAD_REQUEST,
        Failure::NotFound => StatusCode::NOT_FOUND,
        Failure::Internal(error) => {
            tracing::error!(%error, "Dashboard-Admin-Aufruf fehlgeschlagen");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
fn admin_json(
    result: crate::services::admin::Outcome<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    result.map(Json).map_err(admin_status)
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
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::memory(&state.db, &user_id, query.profile.as_deref()))
}

async fn update_memory(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
    Json(update): Json<MemoryUpdate>,
) -> Result<String, StatusCode> {
    let update = crate::services::admin::MemoryUpdate {
        profile: update.profile,
        reason: update.reason,
        user_preferences: update.user_preferences,
        custom_variables: update.custom_variables,
        learned_facts: update.learned_facts,
        last_topics: update.last_topics,
    };
    crate::services::admin::update_memory(&state.db, &user_id, update).map_err(admin_status)?;
    Ok("Memory updated".to_string())
}

// ── Secrets ──────────────────────────────────────────────────────────────────





pub use crate::services::secrets::{SecretsInfo, SecretsUpdate};
#[cfg(test)]
use crate::services::secrets::apply_codex_secret;

async fn get_secrets() -> Result<Json<SecretsInfo>, StatusCode> {
    Ok(Json(crate::services::secrets::masked()))
}

async fn update_secrets(Json(update): Json<SecretsUpdate>) -> Result<String, StatusCode> {
    crate::services::secrets::update(update).map_err(admin_status)
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
    admin_json(crate::services::admin::pairings(&state.db))
}

async fn delete_pairing(
    State(state): State<Arc<DashboardState>>,
    Path(user_id): Path<String>,
) -> Result<String, StatusCode> {
    crate::services::admin::delete_pairing(&state.db, &user_id).map_err(admin_status)?;
    Ok("Pairing deleted".to_string())
}

async fn list_pending_pairings(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::pending_pairings(&state.db))
}

async fn approve_pending_pairing(
    State(state): State<Arc<DashboardState>>,
    Path(code): Path<String>,
) -> Result<String, StatusCode> {
    let discord_user_id =
        crate::services::admin::approve_pairing(&state.db, &code).map_err(admin_status)?;
    Ok(format!("Pairing approved for Discord user {discord_user_id}"))
}

async fn delete_pending_pairing(
    State(state): State<Arc<DashboardState>>,
    Path(code): Path<String>,
) -> Result<String, StatusCode> {
    crate::services::admin::delete_pending_pairing(&state.db, &code).map_err(admin_status)?;
    Ok("Pending pairing deleted".to_string())
}

// ── Statemachine Files ────────────────────────────────────────────────

async fn list_sm_files(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = state; // Workflows use the same canonical root as runtime routing.
    Ok(Json(crate::services::admin::sm_files(std::path::Path::new("contexts"))))
}

async fn get_sm_file(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<String, StatusCode> {
    let _ = state;
    crate::services::admin::sm_file(std::path::Path::new("contexts"), &name).map_err(admin_status)
}

async fn save_sm_file(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
    Json(update): Json<SmFileUpdate>,
) -> Result<String, StatusCode> {
    let _ = state;
    crate::services::admin::save_sm_file(std::path::Path::new("contexts"), &name, &update.content)
        .map_err(admin_status)?;
    Ok("SM file saved".to_string())
}

// ── Cron Jobs ────────────────────────────────────────────────────────────────

async fn list_cron_jobs(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::admin::cron_jobs(&state.db))
}

pub(crate) async fn tool_activity(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(crate::runtime::web_proxy::activity(&state.db, &params)))
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
    let full = crate::services::agent::with_attachments(
        &req.message,
        req.attachments.as_deref().unwrap_or_default(),
    );
    Ok(Json(crate::services::agent::send_input(&req.user_id, full).await))
}

async fn begin_agent(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::services::agent::dispatch_via_gateway(
        state.gateway_api_key.clone(),
        req["user_id"].as_str().unwrap_or("default").to_string(),
        req["message"].as_str().unwrap_or("").to_string(),
    );
    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Agent dispatched to gateway"
    })))
}

async fn get_agent_status(
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(serde_json::json!({ "active": crate::services::agent::active(&user_id).await })))
}

async fn stop_agent(
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    crate::services::agent::stop(&user_id).await;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn upload_avatar(
    State(_state): State<Arc<DashboardState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let root = crate::services::media::data_dir();
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("unknown").to_string();
        let mut data = Vec::new();
        while let Ok(Some(chunk)) = field.chunk().await {
            data.extend_from_slice(&chunk);
            if data.len() > crate::services::media::MAX_AVATAR {
                break;
            }
        }
        return Ok(Json(match crate::services::media::store_avatar(&root, &name, &data) {
            Ok(url) => serde_json::json!({"success": true, "url": url}),
            Err(crate::services::admin::Failure::BadRequest(error)) => serde_json::json!({"error": error}),
            Err(other) => return Err(admin_status(other)),
        }));
    }
    Ok(Json(serde_json::json!({"error": "No file uploaded"})))
}

async fn get_avatar(
    Path(name): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let (content_type, data) =
        crate::services::media::read_avatar(&crate::services::media::data_dir(), &name)
            .map_err(admin_status)?;
    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], data))
}

async fn upload_chat_file(
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let root = crate::services::media::data_dir();
    let mut files = Vec::new();
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let filename = field.file_name().unwrap_or("file").to_string();
        let mut data = Vec::new();
        while let Ok(Some(chunk)) = field.chunk().await {
            data.extend_from_slice(&chunk);
            if data.len() > crate::services::media::MAX_UPLOAD {
                break;
            }
        }
        match crate::services::media::store_upload(&root, &filename, &data) {
            Ok(url) => files.push(url),
            Err(crate::services::admin::Failure::BadRequest(error)) => {
                return Ok(Json(serde_json::json!({"error": error})))
            }
            Err(other) => return Err(admin_status(other)),
        }
    }
    if files.is_empty() {
        return Ok(Json(serde_json::json!({"error": "No files found"})));
    }
    Ok(Json(serde_json::json!({"success": true, "files": files})))
}

async fn get_chat_file(
    Path(name): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let data = crate::services::media::read_upload(&crate::services::media::data_dir(), &name)
        .map_err(admin_status)?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        data,
    ))
}

async fn get_screenshot(
    Path(path): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let (content_type, data) =
        crate::services::media::read_screenshot(&crate::services::media::data_dir(), &path)
            .map_err(admin_status)?;
    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], data))
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
    crate::services::media::transcribe(&state.db, &user_id, &body)
        .await
        .map(Json)
        .map_err(admin_status)
}

/// Context profiles: named, complete context snapshots ("Marvin-Default", ...)
/// stored in the DB so a fresh user can be configured with one click.
/// Stored in table `context_profiles (name, data, created_at)`.
async fn list_profiles(
    State(state): State<Arc<DashboardState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::profiles::list(&state.db))
}

async fn save_profile(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let name = req["name"].as_str().unwrap_or("");
    let source_user = req["source_user_id"].as_str().unwrap_or("default");
    admin_json(crate::services::profiles::save(&state.db, name, source_user))
}

async fn apply_profile(
    State(state): State<Arc<DashboardState>>,
    Path((name, user_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::profiles::apply(&state.db, &name, &user_id))
}

async fn delete_profile(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    admin_json(crate::services::profiles::delete(&state.db, &name))
}

async fn chat_query(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    tracing::info!(user_id = %req["user_id"].as_str().unwrap_or("default"), "[DASHBOARD] chat_query received");
    Ok(Json(crate::services::agent::chat(&state.db, state.gateway_api_key.clone(), &req).await))
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
    crate::services::sessions::sm_info(&state.db, &user_id)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
#[cfg(feature = "voice")]
#[path = "audio_tests.rs"]
mod audio_tests;

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tool_tests;

#[cfg(test)]
#[path = "media_tests.rs"]
mod media_tests;

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
        assert_eq!(memory["learned_facts"], serde_json::json!(["one"]));
        assert_eq!(memory["user_preferences"]["brief"], true);
        assert_eq!(memory["custom_variables"]["n"], 3);
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
        assert_eq!(memory["learned_facts"], serde_json::json!([]));
        assert_eq!(memory["user_preferences"], serde_json::json!({}));
        assert_eq!(memory["custom_variables"]["n"], 3);
        assert!(get_memory(
            State(state),
            Path("bob".into()),
            Query(MemoryQuery::default())
        )
        .await
        .unwrap()
        .0["custom_variables"]
        .as_object()
        .unwrap()
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
        assert_eq!(selected["profile"], "standard");
        assert!(selected["custom_variables"].as_object().unwrap().is_empty());
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
        assert_eq!(lesson["custom_variables"]["xp"], 2);
        assert_eq!(lesson["active_profile"], "standard");
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
            ["shared"]["name"],
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
    let root = crate::services::media::data_dir();
    Ok(Json(crate::services::media::list(&root, params.get("q").map(String::as_str))))
}

fn entries_flat(rd: std::fs::ReadDir) -> Vec<std::fs::DirEntry> {
    rd.filter_map(|e| e.ok()).collect()
}
