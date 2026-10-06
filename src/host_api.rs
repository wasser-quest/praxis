//! Host API v1 server. Loopback-only listener, one bearer token per launched
//! package, scope checks per route. Handlers are thin adapters over
//! `crate::services`; the host keeps workflow, IR and receipt authority.
use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post},
    Json, Router,
};
use praxis_plugin_api::host::{valid_scope, HostApiGrant, HOST_API_PREFIX, HOST_API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, collections::HashMap, sync::Arc};
use tokio_util::sync::CancellationToken;

struct ApiState {
    db: crate::db::Database,
    plugins: Arc<crate::plugins::PluginRegistry>,
    http: reqwest::Client,
    token: String,
    scopes: BTreeSet<String>,
    gateway_port: u16,
    owner: String,
}

/// Running Host API binding for one package. Dropping it stops the listener.
pub struct HostApi {
    grant: HostApiGrant,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl HostApi {
    pub async fn start(
        db: crate::db::Database,
        plugins: Arc<crate::plugins::PluginRegistry>,
        owner: &str,
        scopes: &[String],
        gateway_port: u16,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(scopes.iter().all(|s| valid_scope(s)), "Unknown Host API scope");
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let state = Arc::new(ApiState {
            db,
            plugins,
            http: crate::runtime::web_proxy::client(),
            token: token.clone(),
            scopes: scopes.iter().cloned().collect(),
            gateway_port,
            owner: owner.into(),
        });
        let app = router(state.clone())
            .layer(middleware::from_fn_with_state(state.clone(), authenticate))
            .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
            .with_state(state);
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        });
        Ok(Self {
            grant: HostApiGrant {
                version: HOST_API_VERSION,
                url: format!("http://127.0.0.1:{port}"),
                token,
                scopes: scopes.to_vec(),
            },
            stop,
            task,
        })
    }
    pub fn grant(&self) -> &HostApiGrant {
        &self.grant
    }
    pub fn stop(&self) {
        self.stop.cancel();
    }
}
impl Drop for HostApi {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

fn route(path: &str) -> String {
    format!("{HOST_API_PREFIX}{path}")
}

fn router(_state: Arc<ApiState>) -> Router<Arc<ApiState>> {
    Router::new()
        .route(&route("/info"), get(info))
        .route(&route("/sessions"), get(sessions))
        .route(&route("/contexts"), get(contexts))
        .route(&route("/messages/:user/clear-chat"), post(clear_chat))
        .route(&route("/sessions/:user/fork"), post(fork))
        .route(&route("/graphs/:user"), get(graph))
        .route(&route("/execution/:user"), get(execution))
        .route(&route("/usage/:user"), get(usage))
        .route(&route("/agent/:user"), get(agent_status))
        .route(&route("/agent/:user/begin"), post(agent_begin))
        .route(&route("/agent/:user/input"), post(agent_input))
        .route(&route("/agent/:user/stop"), post(agent_stop))
        .route(&route("/events/:user"), get(events))
        .route(&route("/auth/login"), post(login))
        .route(&route("/auth/verify"), post(verify))
        .route(&route("/admin/tools"), get(admin_tools))
        .route(&route("/admin/tools/:name"), post(admin_set_tool))
        .route(&route("/admin/tool-packages"), get(admin_tool_packages))
        .route(&route("/admin/tool-packages/:id"), post(admin_set_tool_package))
        .route(&route("/admin/templates"), get(admin_templates).post(admin_create_template))
        .route(
            &route("/admin/templates/*name"),
            get(admin_template).put(admin_update_template).delete(admin_delete_template),
        )
        .route(&route("/admin/workflows"), get(admin_workflows))
        .route(&route("/admin/workflows/:name"), get(admin_workflow).put(admin_save_workflow))
        .route(&route("/admin/memory/:user"), get(admin_memory).put(admin_update_memory))
        .route(&route("/admin/pairings"), get(admin_pairings))
        .route(&route("/admin/pairings/:user"), delete(admin_delete_pairing))
        .route(&route("/admin/pending-pairings"), get(admin_pending))
        .route(
            &route("/admin/pending-pairings/:code"),
            post(admin_approve).delete(admin_delete_pending),
        )
        .route(&route("/admin/cron"), get(admin_cron))
        .route(&route("/admin/skills"), get(admin_skills))
        .route(&route("/admin/router"), get(admin_router))
        .route(&route("/admin/profiles"), get(admin_profiles).post(admin_save_profile))
        .route(&route("/admin/profiles/:name"), delete(admin_delete_profile))
        .route(&route("/admin/profiles/:name/apply/:user"), post(admin_apply_profile))
        .route(&route("/admin/decision-profiles"), get(admin_decisions))
        .route(
            &route("/admin/decision-profiles/:name"),
            get(admin_decision).put(admin_save_decision),
        )
        .route(&route("/contexts/:user"), get(context).put(update_context).delete(delete_context))
        .route(&route("/messages/:user"), get(messages).delete(clear_messages))
        .route(&route("/messages/:user/compact"), post(compact))
        .route(&route("/secrets"), get(secrets).put(update_secrets))
        .route(&route("/decision-probe"), post(decision_probe))
        .route(&route("/features"), get(features))
        .route(&route("/sm/:user"), get(sm_info))
        .route(&route("/contexts/:user/exec"), post(context_exec))
        .route(&route("/chat/send"), post(chat_send))
        .route(&route("/admin/tool-records"), get(admin_tool_records))
        .route(&route("/admin/tool-activity"), get(admin_tool_activity))
        .route(&route("/features/:owner"), axum::routing::any(feature))
        .route(&route("/features/:owner/*path"), axum::routing::any(feature))
        .route(&route("/media"), get(media_list))
        .route(
            &route("/media/files"),
            post(media_upload).layer(axum::extract::DefaultBodyLimit::max(
                crate::services::media::MAX_UPLOAD,
            )),
        )
        .route(&route("/media/files/:name"), get(media_file))
        .route(
            &route("/media/avatars/:name"),
            get(media_avatar).post(media_store_avatar),
        )
        .route(&route("/media/screenshots/*path"), get(media_screenshot))
        .route(&route("/media/audio/:user/:id"), get(media_audio))
        .route(
            &route("/media/stt/:user"),
            post(media_stt).layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024)),
        )
        .route(&route("/admin/delegations/:user"), get(admin_delegations))
}

/// Scope required for a request path; `None` means the route is unknown.
fn required_scope(method: &axum::http::Method, path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix(HOST_API_PREFIX)?;
    let first = rest.trim_start_matches('/').split('/').next().unwrap_or("");
    let read = method == axum::http::Method::GET;
    Some(match first {
        "info" => "",
        "sessions" | "messages" if !read => "sessions:write",
        // Context writes can change workflow, templates and permissions.
        "contexts" if !read => "admin:write",
        "sessions" | "contexts" | "messages" | "graphs" | "execution" | "usage" | "sm" => "sessions:read",
        "chat" => "agent",
        "agent" => "agent",
        "events" => "events",
        "auth" => "auth",
        "secrets" => "secrets",
        "media" => "media",
        "decision-probe" => "agent",
        "features" => "features",
        "admin" if read => "admin:read",
        "admin" => "admin:write",
        _ => return None,
    })
}

async fn authenticate(State(state): State<Arc<ApiState>>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    if presented != Some(state.token.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match required_scope(request.method(), request.uri().path()) {
        None => StatusCode::NOT_FOUND.into_response(),
        Some(scope) if !scope.is_empty() && !state.scopes.contains(scope) => {
            StatusCode::FORBIDDEN.into_response()
        }
        Some(_) => next.run(request).await,
    }
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn fail(status: StatusCode, error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": error.to_string()})))
}
fn internal(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    fail(StatusCode::INTERNAL_SERVER_ERROR, error)
}

async fn info(State(state): State<Arc<ApiState>>) -> Json<Value> {
    Json(json!({
        "version": HOST_API_VERSION, "owner": state.owner,
        "scopes": state.scopes, "gateway_port": state.gateway_port,
        "praxis_version": env!("CARGO_PKG_VERSION"),
    }))
}
async fn sessions(State(s): State<Arc<ApiState>>) -> ApiResult {
    crate::services::sessions::chat_sessions(&s.db).map(Json).map_err(internal)
}
async fn contexts(State(s): State<Arc<ApiState>>) -> ApiResult {
    crate::services::sessions::contexts(&s.db).map(Json).map_err(internal)
}
async fn messages(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let chat_only = q.get("chat_only").is_some_and(|v| v == "1" || v == "true");
    crate::services::sessions::messages(&s.db, &user, chat_only)
        .map(Json)
        .map_err(internal)
}
async fn clear_chat(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    crate::services::sessions::clear_chat_view(&s.db, &user)
        .map(|id| Json(json!({"success": true, "cleared_before_id": id})))
        .map_err(internal)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkRequest {
    new_user_id: String,
    username: Option<String>,
}
async fn fork(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    Json(req): Json<ForkRequest>,
) -> ApiResult {
    crate::services::sessions::fork(&s.db, &user, &req.new_user_id, req.username.as_deref())
        .map(Json)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))
}
#[derive(Deserialize)]
struct GraphQuery {
    workflow: Option<String>,
}
async fn graph(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    Query(q): Query<GraphQuery>,
) -> ApiResult {
    crate::services::graphs::graph(&s.db, &user, q.workflow.as_deref())
        .await
        .map(Json)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))
}
async fn execution(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    crate::services::graphs::execution(&s.db, &user).map(Json).map_err(internal)
}
async fn usage(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    crate::services::graphs::usage(&s.db, &user).map(Json).map_err(internal)
}
async fn agent_status(Path(user): Path<String>) -> Json<Value> {
    Json(json!({"active": crate::services::agent::active(&user).await}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentMessage {
    message: String,
    #[serde(default)]
    attachments: Vec<String>,
}
async fn agent_begin(Path(user): Path<String>, Json(req): Json<AgentMessage>) -> Json<Value> {
    // Same host path as the built-in dashboard: the gateway chat API applies
    // routing, budgets, IR enforcement and receipts.
    let auth = crate::services::auth::OperatorAuth::resolve();
    crate::services::agent::dispatch_via_gateway(
        auth.gateway_api_key,
        user,
        crate::services::agent::with_attachments(&req.message, &req.attachments),
    );
    Json(json!({"success": true, "message": "Agent dispatched to gateway"}))
}
async fn agent_input(Path(user): Path<String>, Json(req): Json<AgentMessage>) -> Json<Value> {
    let message = crate::services::agent::with_attachments(&req.message, &req.attachments);
    Json(crate::services::agent::send_input(&user, message).await)
}
async fn agent_stop(Path(user): Path<String>) -> Json<Value> {
    crate::services::agent::stop(&user).await;
    Json(json!({"success": true}))
}
async fn events(
    Path(user): Path<String>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = crate::runtime::events::get_or_create(&user).subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let data = serde_json::to_string(&ev).unwrap_or_default();
                    return Some((Ok(Event::default().event(ev.event).data(data)), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    password: String,
}
async fn login(Json(req): Json<Login>) -> ApiResult {
    crate::services::auth::OperatorAuth::resolve()
        .login(&req.password)
        .map(|token| Json(json!({"token": token})))
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "Invalid password"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Verify {
    token: String,
}
async fn verify(Json(req): Json<Verify>) -> Json<Value> {
    Json(json!({"valid": crate::services::auth::OperatorAuth::resolve().token_valid(&req.token)}))
}

// ── Administration ──────────────────────────────────────────────────────

use crate::services::admin::{self, Failure};
const TEMPLATES: &str = "templates";
const WORKFLOWS: &str = "contexts";

fn admin_fail(failure: Failure) -> (StatusCode, Json<Value>) {
    match failure {
        Failure::BadRequest(message) => fail(StatusCode::BAD_REQUEST, message),
        Failure::NotFound => fail(StatusCode::NOT_FOUND, "Not found"),
        Failure::Internal(error) => internal(error),
    }
}
fn admin_ok(result: admin::Outcome<Value>) -> ApiResult {
    result.map(Json).map_err(admin_fail)
}
fn done(result: admin::Outcome<()>) -> ApiResult {
    result.map(|()| Json(json!({"success": true}))).map_err(admin_fail)
}

async fn admin_tools(State(s): State<Arc<ApiState>>) -> Json<Value> {
    Json(admin::all_tools(&s.db, &s.plugins))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolToggle {
    is_enabled: bool,
}
async fn admin_set_tool(
    State(s): State<Arc<ApiState>>,
    Path(name): Path<String>,
    Json(req): Json<ToolToggle>,
) -> ApiResult {
    done(admin::set_tool_enabled(&s.db, &s.plugins, &name, req.is_enabled))
}
async fn admin_tool_packages(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(admin::tool_packages(&s.db, &s.plugins))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageToggle {
    enabled: bool,
}
async fn admin_set_tool_package(
    State(s): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(req): Json<PackageToggle>,
) -> ApiResult {
    admin_ok(admin::set_tool_package(&s.db, &id, req.enabled))
}
async fn admin_templates(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(admin::templates(&s.db))
}
async fn admin_template(State(s): State<Arc<ApiState>>, Path(name): Path<String>) -> ApiResult {
    admin_ok(admin::template(&s.db, &name))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateCreate {
    name: String,
    content: String,
    description: Option<String>,
}
async fn admin_create_template(
    State(s): State<Arc<ApiState>>,
    Json(req): Json<TemplateCreate>,
) -> ApiResult {
    admin_ok(admin::create_template(
        &s.db,
        std::path::Path::new(TEMPLATES),
        &req.name,
        &req.content,
        req.description.as_deref(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateUpdate {
    content: String,
    user_id: Option<String>,
    user_prompt: Option<String>,
}
async fn admin_update_template(
    State(s): State<Arc<ApiState>>,
    Path(name): Path<String>,
    Json(req): Json<TemplateUpdate>,
) -> ApiResult {
    admin_ok(
        admin::update_template(
            &s.db,
            &name,
            &req.content,
            req.user_id.as_deref(),
            req.user_prompt.as_deref(),
        )
        .await,
    )
}
async fn admin_delete_template(
    State(s): State<Arc<ApiState>>,
    Path(name): Path<String>,
) -> ApiResult {
    admin_ok(admin::delete_template(&s.db, std::path::Path::new(TEMPLATES), &name))
}
async fn admin_workflows() -> Json<Value> {
    Json(admin::sm_files(std::path::Path::new(WORKFLOWS)))
}
async fn admin_workflow(Path(name): Path<String>) -> ApiResult {
    admin::sm_file(std::path::Path::new(WORKFLOWS), &name)
        .map(|content| Json(json!({"name": name, "content": content})))
        .map_err(admin_fail)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Content {
    content: String,
}
async fn admin_save_workflow(Path(name): Path<String>, Json(req): Json<Content>) -> ApiResult {
    done(admin::save_sm_file(std::path::Path::new(WORKFLOWS), &name, &req.content))
}
async fn admin_memory(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    admin_ok(admin::memory(&s.db, &user, q.get("profile").map(String::as_str)))
}
async fn admin_update_memory(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    Json(req): Json<admin::MemoryUpdate>,
) -> ApiResult {
    done(admin::update_memory(&s.db, &user, req))
}
async fn admin_pairings(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(admin::pairings(&s.db))
}
async fn admin_delete_pairing(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    done(admin::delete_pairing(&s.db, &user))
}
async fn admin_pending(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(admin::pending_pairings(&s.db))
}
async fn admin_approve(State(s): State<Arc<ApiState>>, Path(code): Path<String>) -> ApiResult {
    admin::approve_pairing(&s.db, &code)
        .map(|id| Json(json!({"success": true, "discord_user_id": id})))
        .map_err(admin_fail)
}
async fn admin_delete_pending(State(s): State<Arc<ApiState>>, Path(code): Path<String>) -> ApiResult {
    done(admin::delete_pending_pairing(&s.db, &code))
}
async fn admin_cron(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(admin::cron_jobs(&s.db))
}
async fn admin_delegations(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    admin_ok(admin::delegations(&s.db, &user))
}

async fn admin_skills(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(admin::skills(&s.db, std::path::Path::new("skills")))
}
async fn admin_router() -> Json<Value> {
    Json(match crate::gpu_router::state().await {
        Some(state) => serde_json::to_value(&state).unwrap_or_else(|_| json!({"configured": false})),
        None => json!({"configured": false}),
    })
}
async fn admin_profiles(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(crate::services::profiles::list(&s.db))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileSave {
    name: String,
    source_user_id: String,
}
async fn admin_save_profile(State(s): State<Arc<ApiState>>, Json(req): Json<ProfileSave>) -> ApiResult {
    admin_ok(crate::services::profiles::save(&s.db, &req.name, &req.source_user_id))
}
async fn admin_delete_profile(State(s): State<Arc<ApiState>>, Path(name): Path<String>) -> ApiResult {
    admin_ok(crate::services::profiles::delete(&s.db, &name))
}
async fn admin_apply_profile(
    State(s): State<Arc<ApiState>>,
    Path((name, user)): Path<(String, String)>,
) -> ApiResult {
    admin_ok(crate::services::profiles::apply(&s.db, &name, &user))
}
async fn admin_decisions() -> ApiResult {
    admin_ok(admin::decision_profiles())
}
async fn admin_decision(Path(name): Path<String>) -> ApiResult {
    admin_ok(admin::decision_profile(&name))
}
async fn admin_save_decision(Path(name): Path<String>, Json(req): Json<Content>) -> ApiResult {
    done(admin::save_decision_profile(&name, &req.content))
}
async fn context(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    let ctx = s.db.load_context(&user).map_err(internal)?;
    serde_json::to_value(ctx).map(Json).map_err(|e| internal(e.into()))
}
async fn update_context(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    Json(update): Json<Value>,
) -> ApiResult {
    let ctx = s
        .db
        .merge_context(&user, update)
        .map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    serde_json::to_value(ctx).map(Json).map_err(|e| internal(e.into()))
}
async fn delete_context(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    s.db.delete_context(&user).map_err(internal)?;
    Ok(Json(json!({"success": true})))
}
async fn clear_messages(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    s.db.clear_messages(&user).map_err(internal)?;
    Ok(Json(json!({"success": true})))
}
/// Same compaction path as the dashboard and WebSocket `/compact`.
async fn compact(Path(user): Path<String>) -> ApiResult {
    let gateway = crate::gateway::state_ref()
        .ok_or_else(|| fail(StatusCode::SERVICE_UNAVAILABLE, "Gateway not running"))?;
    crate::gateway::ws_handler::compact_history(gateway, &user)
        .await
        .map(|summary| Json(json!({"success": true, "summary": summary})))
        .map_err(internal)
}
async fn secrets() -> Json<crate::services::secrets::SecretsInfo> {
    Json(crate::services::secrets::masked())
}
async fn update_secrets(Json(req): Json<crate::services::secrets::SecretsUpdate>) -> ApiResult {
    crate::services::secrets::update(req)
        .map(|message| Json(json!({"message": message})))
        .map_err(admin_fail)
}

async fn sm_info(State(s): State<Arc<ApiState>>, Path(user): Path<String>) -> ApiResult {
    crate::services::sessions::sm_info(&s.db, &user).map(Json).map_err(internal)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecLine {
    line: String,
}
async fn context_exec(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    Json(req): Json<ExecLine>,
) -> Json<Value> {
    Json(crate::services::sessions::exec(&s.db, &user, &req.line))
}
/// Same semantics as the built-in dashboard chat box (question replies,
/// options, start-or-inject through the gateway path).
async fn chat_send(State(s): State<Arc<ApiState>>, Json(req): Json<Value>) -> Json<Value> {
    let key = crate::services::auth::OperatorAuth::resolve().gateway_api_key;
    Json(crate::services::agent::chat(&s.db, key, &req).await)
}
async fn admin_tool_records(State(s): State<Arc<ApiState>>) -> ApiResult {
    admin_ok(admin::tool_records(&s.db))
}
async fn admin_tool_activity(
    State(s): State<Arc<ApiState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    Json(crate::runtime::web_proxy::activity(&s.db, &q))
}

// ── Feature page slots ──────────────────────────────────────────────────

/// Installed feature web services (same descriptors as the built-in
/// dashboard's extension list): page title, entry asset, API and sockets.
async fn features(State(s): State<Arc<ApiState>>) -> Json<Value> {
    let mut descriptors: Vec<_> = Vec::new();
    let mut routes: Vec<Value> = Vec::new();
    for service in s.plugins.web_services() {
        let Some(endpoint) = service.web_endpoint() else { continue };
        let owner = service.descriptor().owner.clone();
        // Host-registered public aliases, so a frontend can keep legacy URLs
        // (e.g. `/api/vm`, `/websockify`) pointing at the same service paths.
        for alias in service.web_aliases() {
            routes.push(json!({
                "owner": owner, "source": alias.source, "target": alias.target, "assets": alias.assets,
            }));
        }
        descriptors.push(endpoint.info.descriptor);
    }
    descriptors.sort_by(|a, b| a.id.cmp(&b.id));
    Json(json!({
        "features": descriptors,
        "routes": routes,
        // Registered slots so a client can explain absent features instead of
        // hiding them. Read-only: nothing here starts a feature service.
        "slots": crate::runtime::feature_slots::feature_slots(&s.plugins),
    }))
}

/// `/host/v1/features/<owner>/<service path>`: the service path must be in
/// the owner's `/api/plugins/<owner>` or `/plugins/<owner>` namespace or a
/// host-registered alias target. Forwarded as operator over loopback.
async fn feature(
    State(s): State<Arc<ApiState>>,
    Path(params): Path<HashMap<String, String>>,
    request: Request,
) -> Response {
    let owner = params.get("owner").cloned().unwrap_or_default();
    let path = format!("/{}", params.get("path").map(String::as_str).unwrap_or(""));
    let path = path.trim_end_matches('/').to_string();
    let path = if path.is_empty() { "/".to_string() } else { path };
    if !crate::runtime::web_proxy::safe_path(&path) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(service) = s
        .plugins
        .web_services()
        .into_iter()
        .find(|service| service.descriptor().owner == owner)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(assets) = crate::runtime::web_proxy::target_kind(&service, &path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if assets && !matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let query: HashMap<String, String> = axum::extract::Query::try_from_uri(request.uri())
        .map(|q| q.0)
        .unwrap_or_default();
    let is_socket = crate::runtime::web_proxy::is_socket(&service, &path);
    crate::runtime::web_proxy::forward(&service, &s.http, &path, query, is_socket, request, &s.db).await
}

// ── Media ───────────────────────────────────────────────────────────────

use crate::services::media;

fn bytes(content_type: impl Into<String>, data: Vec<u8>) -> Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, content_type.into()),
            (axum::http::header::CACHE_CONTROL, "private, no-store".to_string()),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        data,
    )
        .into_response()
}
fn media_fail(failure: Failure) -> Response {
    admin_fail(failure).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    profile: crate::gateway::decision_profiles::DecisionProfile,
    contexts: Vec<String>,
}
async fn decision_probe(Json(req): Json<Probe>) -> ApiResult {
    admin_ok(admin::decision_probe(&req.profile, req.contexts).await)
}
async fn media_list(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    Json(media::list(&media::data_dir(), q.get("q").map(String::as_str)))
}
/// Raw body upload: `POST /media/files?name=<file name>`.
async fn media_upload(Query(q): Query<HashMap<String, String>>, body: axum::body::Bytes) -> ApiResult {
    let name = q.get("name").map(String::as_str).unwrap_or("file");
    media::store_upload(&media::data_dir(), name, &body)
        .map(|url| Json(json!({"success": true, "url": url})))
        .map_err(admin_fail)
}
async fn media_file(Path(name): Path<String>) -> Response {
    match media::read_upload(&media::data_dir(), &name) {
        Ok(data) => bytes("application/octet-stream", data),
        Err(e) => media_fail(e),
    }
}
async fn media_avatar(Path(name): Path<String>) -> Response {
    match media::read_avatar(&media::data_dir(), &name) {
        Ok((kind, data)) => bytes(kind, data),
        Err(e) => media_fail(e),
    }
}
async fn media_store_avatar(Path(name): Path<String>, body: axum::body::Bytes) -> ApiResult {
    media::store_avatar(&media::data_dir(), &name, &body)
        .map(|url| Json(json!({"success": true, "url": url})))
        .map_err(admin_fail)
}
async fn media_screenshot(Path(path): Path<String>) -> Response {
    match media::read_screenshot(&media::data_dir(), &path) {
        Ok((kind, data)) => bytes(kind, data),
        Err(e) => media_fail(e),
    }
}
async fn media_audio(State(s): State<Arc<ApiState>>, Path((user, id)): Path<(String, i64)>) -> Response {
    match media::message_audio(&s.db, &user, id) {
        Ok((kind, data)) => bytes(kind, data),
        Err(e) => media_fail(e),
    }
}
async fn media_stt(
    State(s): State<Arc<ApiState>>,
    Path(user): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    admin_ok(media::transcribe(&s.db, &user, &body).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_package_routes_have_scopes() {
        use axum::http::Method;
        let scope = |m: Method, p: &str| required_scope(&m, &format!("{HOST_API_PREFIX}{p}"));
        assert_eq!(scope(Method::GET, "/sm/u"), Some("sessions:read"));
        assert_eq!(scope(Method::POST, "/chat/send"), Some("agent"));
        assert_eq!(scope(Method::POST, "/contexts/u/exec"), Some("admin:write"));
        assert_eq!(scope(Method::GET, "/admin/tool-records"), Some("admin:read"));
        assert_eq!(scope(Method::GET, "/admin/tool-activity"), Some("admin:read"));
        assert_eq!(scope(Method::GET, "/admin/tool-packages"), Some("admin:read"));
        assert_eq!(scope(Method::POST, "/admin/tool-packages/shell"), Some("admin:write"));
    }

    #[tokio::test]
    async fn host_api_requires_token_and_enforces_declared_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let ctx = db.load_context("u").unwrap();
        db.save_context(&ctx).unwrap();
        db.add_message("u", &crate::db::messages::Message::user("hi".into()))
            .unwrap();
        let api = HostApi::start(db, Arc::new(crate::plugins::PluginRegistry::new()), "my_dashboard", &["sessions:read".into()], 3537)
            .await
            .unwrap();
        let grant = api.grant().clone();
        assert!(grant.url.starts_with("http://127.0.0.1:"));
        let http = reqwest::Client::new();
        let get = |path: &str, token: Option<&str>| {
            let mut r = http.get(format!("{}{HOST_API_PREFIX}{path}", grant.url));
            if let Some(t) = token {
                r = r.bearer_auth(t);
            }
            r
        };
        assert_eq!(get("/sessions", None).send().await.unwrap().status(), 401);
        assert_eq!(get("/sessions", Some("wrong")).send().await.unwrap().status(), 401);
        let t = Some(grant.token.as_str());
        let info: Value = get("/info", t).send().await.unwrap().json().await.unwrap();
        assert_eq!(info["version"], 1);
        assert_eq!(info["owner"], "my_dashboard");
        let sessions: Value = get("/sessions", t).send().await.unwrap().json().await.unwrap();
        assert_eq!(sessions["sessions"][0]["user_id"], "u");
        let msgs: Value = get("/messages/u", t).send().await.unwrap().json().await.unwrap();
        assert_eq!(msgs["message_count"], 1);
        // Undeclared scopes are refused.
        assert_eq!(get("/agent/u", t).send().await.unwrap().status(), 403);
        assert_eq!(get("/events/u", t).send().await.unwrap().status(), 403);
        let post = http
            .post(format!("{}{HOST_API_PREFIX}/messages/u/clear-chat", grant.url))
            .bearer_auth(&grant.token)
            .send()
            .await
            .unwrap();
        assert_eq!(post.status(), 403);
        assert_eq!(get("/nope", t).send().await.unwrap().status(), 404);
        // Administration and secrets need their own scopes.
        assert_eq!(get("/admin/tools", t).send().await.unwrap().status(), 403);
        assert_eq!(get("/secrets", t).send().await.unwrap().status(), 403);
        assert_eq!(get("/media", t).send().await.unwrap().status(), 403);
        let probe = http
            .post(format!("{}{HOST_API_PREFIX}/decision-probe", grant.url))
            .bearer_auth(&grant.token)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(probe.status(), 403);
        // Context and history writes are not covered by sessions:read.
        let ctx = get("/contexts/u", t).send().await.unwrap();
        assert_eq!(ctx.status(), 200);
        for (method, path) in [
            (reqwest::Method::PUT, "/contexts/u"),
            (reqwest::Method::DELETE, "/contexts/u"),
            (reqwest::Method::DELETE, "/messages/u"),
            (reqwest::Method::POST, "/messages/u/compact"),
        ] {
            let status = http
                .request(method, format!("{}{HOST_API_PREFIX}{path}", grant.url))
                .bearer_auth(&grant.token)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, 403, "{path}");
        }
        api.stop();
    }

    #[tokio::test]
    async fn admin_scopes_separate_reads_from_validated_writes() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let api = HostApi::start(db, Arc::new(crate::plugins::PluginRegistry::new()), "d", &["admin:read".into()], 3537)
            .await
            .unwrap();
        let grant = api.grant().clone();
        let http = reqwest::Client::new();
        let url = |p: &str| format!("{}{HOST_API_PREFIX}{p}", grant.url);
        let tools: Value = http.get(url("/admin/tools")).bearer_auth(&grant.token)
            .send().await.unwrap().json().await.unwrap();
        assert!(tools["total"].as_u64().unwrap() > 0);
        assert!(tools["tools"].as_array().unwrap().iter().any(|t| t["source"] == "builtin"));
        for path in [
            "/admin/pairings", "/admin/pending-pairings", "/admin/cron", "/admin/memory/u",
            "/admin/skills", "/admin/profiles", "/admin/router",
        ] {
            let status = http.get(url(path)).bearer_auth(&grant.token).send().await.unwrap().status();
            assert_eq!(status, 200, "{path}");
        }
        // Read-only grant cannot write.
        let write = http.post(url("/admin/tools/shell")).bearer_auth(&grant.token)
            .json(&json!({"is_enabled": false})).send().await.unwrap();
        assert_eq!(write.status(), 403);
        let delete = http.delete(url("/admin/templates/standard")).bearer_auth(&grant.token)
            .send().await.unwrap();
        assert_eq!(delete.status(), 403);
        api.stop();

        let db = crate::db::Database::new(&dir.path().join("w")).unwrap();
        let api = HostApi::start(db, Arc::new(crate::plugins::PluginRegistry::new()), "d", &["admin:write".into()], 3537)
            .await
            .unwrap();
        let grant = api.grant().clone();
        let url = |p: &str| format!("{}{HOST_API_PREFIX}{p}", grant.url);
        // Unknown tools and escaping template names are rejected by the service.
        let missing = http.post(url("/admin/tools/no_such_tool")).bearer_auth(&grant.token)
            .json(&json!({"is_enabled": false})).send().await.unwrap();
        assert_eq!(missing.status(), 404);
        let escape = http.post(url("/admin/templates")).bearer_auth(&grant.token)
            .json(&json!({"name": "../../evil", "content": "x"})).send().await.unwrap();
        assert_eq!(escape.status(), 400);
        // Invalid workflow names are refused before anything is written.
        let workflow = http.put(url("/admin/workflows/bad.name.sm")).bearer_auth(&grant.token)
            .json(&json!({"content": "x"})).send().await.unwrap();
        assert_eq!(workflow.status(), 400);
        assert!(!std::path::Path::new(WORKFLOWS).join("bad.name.sm").exists());
    }

    #[tokio::test]
    async fn secrets_scope_is_masked_and_keeps_lockout_guards() {
        let _lock = crate::db::secrets::test_lock();
        let original = crate::db::secrets::get_secrets();
        let mut secrets = crate::db::secrets::Secrets::default();
        secrets.openai_api_key = Some("sk-very-secret-value-123456".into());
        secrets.gateway_api_key = Some("gateway-key-abcdef-123456".into());
        crate::db::secrets::init_secrets(secrets);
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let api = HostApi::start(db, Arc::new(crate::plugins::PluginRegistry::new()), "d", &["secrets".into()], 3537)
            .await
            .unwrap();
        let grant = api.grant().clone();
        let http = reqwest::Client::new();
        let url = format!("{}{HOST_API_PREFIX}/secrets", grant.url);
        let body = http.get(&url).bearer_auth(&grant.token).send().await.unwrap().text().await.unwrap();
        assert!(!body.contains("sk-very-secret-value"), "{body}");
        assert!(!body.contains("gateway-key-abcdef"), "{body}");
        let update: Value = http
            .put(&url)
            .bearer_auth(&grant.token)
            .json(&json!({"gateway_api_key": "  "}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(update["message"].as_str().unwrap().contains("gateway_api_key"), "{update}");
        assert_eq!(
            crate::db::secrets::get_secrets().gateway_api_key.as_deref(),
            Some("gateway-key-abcdef-123456")
        );
        crate::db::secrets::init_secrets(original);
    }

    #[tokio::test]
    async fn media_scope_serves_message_audio_and_refuses_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        db.save_context(&db.load_context("alice").unwrap()).unwrap();
        let id = db
            .add_message("alice", &crate::db::messages::Message::assistant("r".into()))
            .unwrap();
        db.save_message_audio("alice", id, "audio/wav", b"wav").unwrap();
        let api = HostApi::start(db, Arc::new(crate::plugins::PluginRegistry::new()), "d", &["media".into()], 3537)
            .await
            .unwrap();
        let grant = api.grant().clone();
        let http = reqwest::Client::new();
        let get = |p: &str| http.get(format!("{}{HOST_API_PREFIX}{p}", grant.url)).bearer_auth(&grant.token);
        let audio = get(&format!("/media/audio/alice/{id}")).send().await.unwrap();
        assert_eq!(audio.status(), 200);
        assert_eq!(audio.headers()["content-type"], "audio/wav");
        assert_eq!(audio.bytes().await.unwrap().as_ref(), b"wav");
        assert_eq!(get(&format!("/media/audio/bob/{id}")).send().await.unwrap().status(), 404);
        for path in ["/media/files/..%2F..%2FCargo.toml", "/media/screenshots/praxis.db", "/media/avatars/..%2Fx"] {
            assert_eq!(get(path).send().await.unwrap().status(), 404, "{path}");
        }
        let empty = http
            .post(format!("{}{HOST_API_PREFIX}/media/stt/alice", grant.url))
            .bearer_auth(&grant.token)
            .send()
            .await
            .unwrap();
        assert_eq!(empty.status(), 400);
    }

    mod feature_fixture {
        use crate::runtime::{
            features::{InvocationContext, NativeService},
            web::{WebAlias, WebEndpoint},
        };
        use praxis_plugin_api::web::{WebDescriptor, WebInfo, WEB_API_VERSION};
        use serde_json::{json, Value};
        pub struct Fixture {
            pub port: u16,
        }
        #[async_trait::async_trait]
        impl NativeService for Fixture {
            fn web_descriptor(&self) -> Option<WebDescriptor> {
                let mut descriptor = WebDescriptor::for_package("vm", "Fixture");
                descriptor.websockets.push("/api/plugins/vm/vnc/ws".into());
                Some(descriptor)
            }
            fn web_endpoint(&self) -> Option<WebEndpoint> {
                Some(WebEndpoint::new(
                    WebInfo { version: WEB_API_VERSION, port: self.port, descriptor: self.web_descriptor().unwrap() },
                    "private-host-fixture".into(),
                ))
            }
            fn web_aliases(&self) -> Vec<WebAlias> {
                vec![WebAlias::api("/api/vm/activity", "/api/tool-activity")]
            }
            async fn invoke(&self, _: InvocationContext, _: &str, _: Value) -> anyhow::Result<Value> {
                Ok(json!({}))
            }
        }
    }

    #[tokio::test]
    async fn feature_slots_reach_only_the_owners_namespace_as_operator() {
        use futures_util::{SinkExt, StreamExt};
        use praxis_plugin_api::web::{PRINCIPAL_HEADER, PRIVATE_HEADER};
        let backend = axum::Router::new()
            .route("/api/plugins/vm/guests", axum::routing::any(|request: axum::extract::Request| async move {
                Json(json!({
                    "private": request.headers().get(PRIVATE_HEADER).and_then(|v| v.to_str().ok()),
                    "principal": request.headers().get(PRINCIPAL_HEADER).and_then(|v| v.to_str().ok()),
                    "authorization": request.headers().get("authorization").is_some(),
                    "query": request.uri().query(),
                }))
            }))
            .route("/plugins/vm/index.html", get(|| async { "vm-page" }))
            .route("/api/plugins/vm/vnc/ws", get(|ws: axum::extract::ws::WebSocketUpgrade| async {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if socket.send(message).await.is_err() { break; }
                    }
                })
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
        let mut plugins = crate::plugins::PluginRegistry::new();
        plugins
            .try_register(serde_json::from_str(include_str!("../plugins/vm/plugin.json")).unwrap())
            .unwrap();
        plugins
            .register_service("vm", "vm", 1, crate::runtime::vm::TOOL_NAMES, Arc::new(feature_fixture::Fixture { port }))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let api = HostApi::start(db, Arc::new(plugins), "d", &["features".into()], 3537).await.unwrap();
        let grant = api.grant().clone();
        let http = reqwest::Client::new();
        let url = |p: &str| format!("{}{HOST_API_PREFIX}{p}", grant.url);
        let list: Value = http.get(url("/features")).bearer_auth(&grant.token).send().await.unwrap().json().await.unwrap();
        assert_eq!(list["features"][0]["id"], "vm", "{list}");
        let forwarded: Value = http
            .get(url("/features/vm/api/plugins/vm/guests?limit=2"))
            .bearer_auth(&grant.token)
            .header(PRIVATE_HEADER, "forged")
            .send().await.unwrap().json().await.unwrap();
        assert_eq!(forwarded["private"], "private-host-fixture");
        assert_eq!(forwarded["principal"], "operator");
        assert_eq!(forwarded["authorization"], false);
        assert_eq!(forwarded["query"], "limit=2");
        let page = http.get(url("/features/vm/plugins/vm/index.html")).bearer_auth(&grant.token).send().await.unwrap();
        assert_eq!(page.text().await.unwrap(), "vm-page");
        let asset_write = http.post(url("/features/vm/plugins/vm/index.html")).bearer_auth(&grant.token).send().await.unwrap();
        assert_eq!(asset_write.status(), 405);
        // Host-owned alias target (VM activity) is served by the host.
        let activity: Value = http.get(url("/features/vm/api/tool-activity")).bearer_auth(&grant.token).send().await.unwrap().json().await.unwrap();
        assert_eq!(activity["activities"], json!([]));
        for path in [
            "/features/vm/api/plugins/other",
            "/features/vm/api/sessions",
            "/features/vm/api/plugins/vm/..%2F..%2Fsecrets",
            "/features/nope/api/plugins/nope",
        ] {
            let status = http.get(url(path)).bearer_auth(&grant.token).send().await.unwrap().status();
            assert!(status == 404 || status == 400, "{path}: {status}");
        }
        // WebSocket upgrade with the package token header (never a query token).
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = url("/features/vm/api/plugins/vm/vnc/ws").replacen("http:", "ws:", 1).into_client_request().unwrap();
        request.headers_mut().insert("authorization", format!("Bearer {}", grant.token).parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        socket.send(tokio_tungstenite::tungstenite::Message::Text("ping".into())).await.unwrap();
        let echoed = socket.next().await.unwrap().unwrap();
        assert_eq!(echoed.into_text().unwrap(), "ping");
        let unauthenticated = url("/features/vm/api/plugins/vm/vnc/ws").replacen("http:", "ws:", 1);
        assert!(tokio_tungstenite::connect_async(format!("{unauthenticated}?token={}", grant.token)).await.is_err());
        worker.abort();
    }

    #[tokio::test]
    async fn event_scope_streams_runtime_events() {
        use futures_util::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let api = HostApi::start(db, Arc::new(crate::plugins::PluginRegistry::new()), "d", &["events".into()], 3537).await.unwrap();
        let grant = api.grant().clone();
        let user = format!("ev-{}", uuid::Uuid::new_v4());
        let response = reqwest::Client::new()
            .get(format!("{}{HOST_API_PREFIX}/events/{user}", grant.url))
            .bearer_auth(&grant.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut body = response.bytes_stream();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        crate::runtime::events::send(&user, "char", "x");
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = body.next().await.unwrap().unwrap();
                let text = String::from_utf8_lossy(&bytes).to_string();
                if text.contains("event: char") {
                    return text;
                }
            }
        })
        .await
        .unwrap();
        assert!(chunk.contains("\"data\":\"x\""), "{chunk}");
    }
}
