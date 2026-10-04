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
    routing::{get, post},
    Json, Router,
};
use praxis_plugin_api::host::{valid_scope, HostApiGrant, HOST_API_PREFIX, HOST_API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, collections::HashMap, sync::Arc};
use tokio_util::sync::CancellationToken;

struct ApiState {
    db: crate::db::Database,
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
        .route(&route("/messages/:user"), get(messages))
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
}

/// Scope required for a request path; `None` means the route is unknown.
fn required_scope(method: &axum::http::Method, path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix(HOST_API_PREFIX)?;
    let first = rest.trim_start_matches('/').split('/').next().unwrap_or("");
    Some(match first {
        "info" => "",
        "sessions" if method == axum::http::Method::POST => "sessions:write",
        "messages" if method == axum::http::Method::POST => "sessions:write",
        "sessions" | "contexts" | "messages" | "graphs" | "execution" | "usage" => "sessions:read",
        "agent" => "agent",
        "events" => "events",
        "auth" => "auth",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_api_requires_token_and_enforces_declared_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let ctx = db.load_context("u").unwrap();
        db.save_context(&ctx).unwrap();
        db.add_message("u", &crate::db::messages::Message::user("hi".into()))
            .unwrap();
        let api = HostApi::start(db, "my_dashboard", &["sessions:read".into()], 3537)
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
        api.stop();
    }

    #[tokio::test]
    async fn event_scope_streams_runtime_events() {
        use futures_util::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let api = HostApi::start(db, "d", &["events".into()], 3537).await.unwrap();
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
