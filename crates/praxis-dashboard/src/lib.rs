//! The Praxis web dashboard as a package. It serves the unchanged browser UI
//! (`static/`) and implements the dashboard's `/api/*`, extension and alias
//! URLs as a thin adapter over Host API v1. It holds no database, secrets or
//! workflow authority of its own: every action is a scoped Host API call, and
//! the Host API token never reaches the browser.
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, FromRequestParts, Multipart, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use praxis_plugin_api::host::{HostApiGrant, HOST_API_PREFIX};
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Scopes the full dashboard needs (declared in its `plugin.json`).
pub const SCOPES: &[&str] = &[
    "sessions:read",
    "sessions:write",
    "agent",
    "events",
    "auth",
    "admin:read",
    "admin:write",
    "secrets",
    "media",
    "features",
];
const LIMIT: usize = 52 * 1024 * 1024;
const VERIFIED_FOR: Duration = Duration::from_secs(30);

pub struct App {
    host: HostApiGrant,
    http: reqwest::Client,
    static_dir: PathBuf,
    verified: Mutex<HashMap<String, Instant>>,
}

pub fn router(host: HostApiGrant, static_dir: PathBuf) -> Router {
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("fixed local HTTP client configuration");
    let app = Arc::new(App {
        host,
        http,
        static_dir: static_dir.clone(),
        verified: Mutex::default(),
    });
    let files = |name: &'static str| {
        let path = static_dir.join(name);
        get(move || {
            let path = path.clone();
            async move { file(&path).await }
        })
    };
    Router::new()
        .route("/", files("index.html"))
        .route("/logo.svg", files("logo.svg"))
        .route("/logo.png", files("logo.png"))
        .route("/favicon.ico", files("favicon.ico"))
        .route("/apple-touch-icon.png", files("apple-touch-icon.png"))
        .nest_service("/static", tower_http::services::ServeDir::new(static_dir))
        .fallback(handle)
        .layer(axum::extract::DefaultBodyLimit::max(LIMIT))
        .layer(axum::middleware::map_response(no_store))
        .with_state(app)
}

async fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, must-revalidate"),
    );
    response
}

async fn file(path: &std::path::Path) -> Response {
    let Ok(data) = tokio::fs::read(path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, kind)], data).into_response()
}

impl App {
    fn url(&self, path: &str) -> String {
        format!("{}{HOST_API_PREFIX}{path}", self.host.url)
    }
    fn call(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, self.url(path))
            .bearer_auth(&self.host.token)
    }
    /// Operator tokens are checked by the host (`auth/verify`); valid ones are
    /// cached briefly so every UI request does not round-trip.
    async fn operator(&self, token: Option<&str>) -> bool {
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return false;
        };
        if self
            .verified
            .lock()
            .unwrap()
            .get(token)
            .is_some_and(|at| at.elapsed() < VERIFIED_FOR)
        {
            return true;
        }
        let valid = match self
            .call(Method::POST, "/auth/verify")
            .json(&json!({"token": token}))
            .send()
            .await
        {
            Ok(response) => response
                .json::<Value>()
                .await
                .ok()
                .is_some_and(|v| v["valid"] == true),
            Err(_) => false,
        };
        let mut cache = self.verified.lock().unwrap();
        cache.retain(|_, at| at.elapsed() < VERIFIED_FOR);
        if valid {
            cache.insert(token.to_owned(), Instant::now());
        }
        valid
    }
}

fn bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
fn query_map(query: &str) -> HashMap<String, String> {
    url_pairs(query).into_iter().collect()
}
fn url_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |s: &str| {
                urlencoding::decode(&s.replace('+', " "))
                    .map(|c| c.into_owned())
                    .unwrap_or_default()
            };
            (decode(k), decode(v))
        })
        .collect()
}
fn encode(segment: &str) -> String {
    urlencoding::encode(segment).into_owned()
}
fn with_query(path: String, query: &str) -> String {
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// How a legacy dashboard response is produced from the Host API response.
enum Reply {
    Pass,
    Text(&'static str),
    Field(&'static str),
    Approved,
}
struct Call {
    method: Method,
    path: String,
    body: Option<Value>,
    reply: Reply,
}
fn pick(body: &Value, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for key in keys {
        if let Some(v) = body.get(*key).filter(|v| !v.is_null()) {
            out.insert((*key).into(), v.clone());
        }
    }
    Value::Object(out)
}
fn call(method: Method, path: String, body: Option<Value>, reply: Reply) -> Option<Call> {
    Some(Call {
        method,
        path,
        body,
        reply,
    })
}

/// Legacy `/api/<segments>` → Host API v1. `None` means not a dashboard route.
fn legacy(method: &Method, segments: &[&str], query: &str, body: &Value) -> Option<Call> {
    use Reply::*;
    let (get, post, put, delete) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
    let m = method.clone();
    let user_of = |body: &Value| encode(body["user_id"].as_str().unwrap_or("default"));
    match (m.as_str(), segments) {
        ("GET", ["contexts"]) => call(get, "/contexts".into(), None, Pass),
        ("GET", ["chat-sessions"]) => call(get, "/sessions".into(), None, Pass),
        ("GET", ["contexts", u]) => call(get, format!("/contexts/{u}"), None, Pass),
        ("PUT", ["contexts", u]) => call(put, format!("/contexts/{u}"), Some(body.clone()), Pass),
        ("DELETE", ["contexts", u]) => call(delete, format!("/contexts/{u}"), None, Pass),
        ("POST", ["contexts", u, "fork"]) => call(
            post,
            format!("/sessions/{u}/fork"),
            Some(pick(body, &["new_user_id", "username"])),
            Pass,
        ),
        ("GET", ["messages", u]) => {
            call(get, with_query(format!("/messages/{u}"), query), None, Pass)
        }
        ("DELETE", ["messages", u]) => call(delete, format!("/messages/{u}"), None, Pass),
        ("POST", ["messages", u, "clear-chat"]) => {
            call(post, format!("/messages/{u}/clear-chat"), None, Pass)
        }
        ("POST", ["messages", u, "compact"]) => {
            call(post, format!("/messages/{u}/compact"), None, Pass)
        }
        ("GET", ["graphs", u]) => call(get, with_query(format!("/graphs/{u}"), query), None, Pass),
        ("GET", ["execution", u]) => call(get, format!("/execution/{u}"), None, Pass),
        ("GET", ["usage", u]) => call(get, format!("/usage/{u}"), None, Pass),
        ("GET", ["skills"]) => call(get, "/admin/skills".into(), None, Pass),
        ("GET", ["router-state"]) => call(get, "/admin/router".into(), None, Pass),
        ("GET", ["delegations", u]) => call(get, format!("/admin/delegations/{u}"), None, Pass),
        ("GET", ["decision-profiles"]) => call(get, "/admin/decision-profiles".into(), None, Pass),
        ("GET", ["decision-profiles", n]) => {
            call(get, format!("/admin/decision-profiles/{n}"), None, Pass)
        }
        ("PUT", ["decision-profiles", n]) => call(
            put,
            format!("/admin/decision-profiles/{n}"),
            Some(pick(body, &["content"])),
            Pass,
        ),
        ("POST", ["decision-probe"]) => call(
            post,
            "/decision-probe".into(),
            Some(pick(body, &["profile", "contexts"])),
            Pass,
        ),
        ("GET", ["templates"]) => call(get, "/admin/templates".into(), None, Pass),
        ("POST", ["templates"]) => call(
            post,
            "/admin/templates".into(),
            Some(pick(body, &["name", "content", "description"])),
            Pass,
        ),
        ("GET", ["templates", n]) => call(get, format!("/admin/templates/{n}"), None, Pass),
        ("PUT", ["templates", n]) => call(
            put,
            format!("/admin/templates/{n}"),
            Some(pick(body, &["content", "user_id", "user_prompt"])),
            Pass,
        ),
        ("DELETE", ["templates", n]) => call(delete, format!("/admin/templates/{n}"), None, Pass),
        ("GET", ["tools"]) => call(get, "/admin/tool-records".into(), None, Pass),
        ("GET", ["tool-packages"]) => call(get, "/admin/tool-packages".into(), None, Pass),
        ("PUT", ["tool-packages", id]) => call(post, format!("/admin/tool-packages/{id}"), Some(pick(body, &["enabled"])), Pass),
        ("GET", ["tools", "all"]) => call(get, "/admin/tools".into(), None, Pass),
        ("PUT", ["tools", n]) => call(
            post,
            format!("/admin/tools/{n}"),
            Some(pick(body, &["is_enabled"])),
            Text("Tool updated"),
        ),
        ("GET", ["memory", u]) => call(
            get,
            with_query(format!("/admin/memory/{u}"), query),
            None,
            Pass,
        ),
        ("PUT", ["memory", u]) => call(
            put,
            format!("/admin/memory/{u}"),
            Some(pick(
                body,
                &[
                    "profile",
                    "reason",
                    "user_preferences",
                    "custom_variables",
                    "learned_facts",
                    "last_topics",
                ],
            )),
            Text("Memory updated"),
        ),
        ("GET", ["secrets"]) => call(get, "/secrets".into(), None, Pass),
        ("PUT", ["secrets"]) => call(put, "/secrets".into(), Some(body.clone()), Field("message")),
        ("GET", ["pairings"]) => call(get, "/admin/pairings".into(), None, Pass),
        ("GET", ["pairings", "pending"]) => call(get, "/admin/pending-pairings".into(), None, Pass),
        ("POST", ["pairings", "pending", c, "approve"]) => {
            call(post, format!("/admin/pending-pairings/{c}"), None, Approved)
        }
        ("DELETE", ["pairings", "pending", c]) => call(
            delete,
            format!("/admin/pending-pairings/{c}"),
            None,
            Text("Pending pairing deleted"),
        ),
        ("DELETE", ["pairings", u]) => call(
            delete,
            format!("/admin/pairings/{u}"),
            None,
            Text("Pairing deleted"),
        ),
        ("GET", ["sm-files"]) => call(get, "/admin/workflows".into(), None, Pass),
        ("GET", ["sm-files", n]) => {
            call(get, format!("/admin/workflows/{n}"), None, Field("content"))
        }
        ("PUT", ["sm-files", n]) => call(
            put,
            format!("/admin/workflows/{n}"),
            Some(pick(body, &["content"])),
            Text("SM file saved"),
        ),
        ("GET", ["cron-jobs"]) => call(get, "/admin/cron".into(), None, Pass),
        ("GET", ["tool-activity"]) => call(
            get,
            with_query("/admin/tool-activity".into(), query),
            None,
            Pass,
        ),
        ("POST", ["agent", "input"]) => call(
            post,
            format!("/agent/{}/input", user_of(body)),
            Some(pick(body, &["message", "attachments"])),
            Pass,
        ),
        ("POST", ["agent", "begin"]) => call(
            post,
            format!("/agent/{}/begin", user_of(body)),
            Some(json!({"message": body["message"].as_str().unwrap_or("")})),
            Pass,
        ),
        ("GET", ["agent", "status", u]) => call(get, format!("/agent/{u}"), None, Pass),
        ("POST", ["agent", "stop", u]) => call(post, format!("/agent/{u}/stop"), None, Pass),
        ("GET", ["sm" | "cl", u]) => call(get, format!("/sm/{u}"), None, Pass),
        ("POST", ["chat", "send"]) => call(post, "/chat/send".into(), Some(body.clone()), Pass),
        ("GET", ["chat", "audio", u, id]) => {
            call(get, format!("/media/audio/{u}/{id}"), None, Pass)
        }
        ("POST", ["context", "exec"]) => call(
            post,
            format!("/contexts/{}/exec", user_of(body)),
            Some(pick(body, &["line"])),
            Pass,
        ),
        ("GET", ["profiles"]) => call(get, "/admin/profiles".into(), None, Pass),
        ("POST", ["profiles"]) => call(
            post,
            "/admin/profiles".into(),
            Some(json!({
                "name": body["name"].as_str().unwrap_or(""),
                "source_user_id": body["source_user_id"].as_str().unwrap_or("default"),
            })),
            Pass,
        ),
        ("POST", ["profiles", n, "apply", u]) => {
            call(post, format!("/admin/profiles/{n}/apply/{u}"), None, Pass)
        }
        ("DELETE", ["profiles", n]) => call(delete, format!("/admin/profiles/{n}"), None, Pass),
        ("GET", ["media"]) => call(get, with_query("/media".into(), query), None, Pass),
        _ => None,
    }
}

/// Copy a Host API response (status, content type, body) to the browser.
async fn relay(response: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let kind = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let mut out = Response::new(Body::from_stream(response.bytes_stream()));
    *out.status_mut() = status;
    if let Some(kind) = kind.and_then(|k| HeaderValue::from_bytes(k.as_bytes()).ok()) {
        out.headers_mut().insert(header::CONTENT_TYPE, kind);
    }
    out
}
fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "Praxis host unavailable"})),
    )
        .into_response()
}

async fn handle(State(app): State<Arc<App>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    let method = request.method().clone();
    if path.contains('\\') || path.split('/').any(|s| s == "." || s == "..") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method.as_str(), segments.as_slice()) {
        ("GET", ["api", "status"]) => {
            let version = match app.call(Method::GET, "/info").send().await {
                Ok(r) => r
                    .json::<Value>()
                    .await
                    .ok()
                    .map(|v| v["praxis_version"].clone()),
                Err(_) => None,
            };
            return Json(json!({"status": "ok", "version": version.unwrap_or(Value::Null)}))
                .into_response();
        }
        ("POST", ["api", "auth", "login"]) => {
            let body = axum::body::to_bytes(request.into_body(), 64 * 1024)
                .await
                .unwrap_or_default();
            let body: Value = serde_json::from_slice(&body).unwrap_or_default();
            return match app
                .call(Method::POST, "/auth/login")
                .json(&pick(&body, &["password"]))
                .send()
                .await
            {
                Ok(r) => relay(r).await,
                Err(_) => unavailable(),
            };
        }
        // Public reads (img/audio tags cannot send headers), same as before:
        // the host confines names to their directories.
        ("GET", ["api", "avatar", name]) => {
            return forward_get(&app, &format!("/media/avatars/{name}")).await
        }
        ("GET", ["api", "files", name]) => {
            return forward_get(&app, &format!("/media/files/{name}")).await
        }
        ("GET", ["api", "screenshots", rest @ ..]) => {
            return forward_get(&app, &format!("/media/screenshots/{}", rest.join("/"))).await
        }
        ("GET", ["api", "chat", "stream", user]) => return stream(&app, user, &query, false).await,
        ("GET", ["api", "chat", "stream", user, "tts"]) => {
            return stream(&app, user, &query, true).await
        }
        _ => {}
    }
    if let Some(response) = extension(&app, &path, &query, request).await {
        return response;
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn forward_get(app: &App, host_path: &str) -> Response {
    match app.call(Method::GET, host_path).send().await {
        Ok(r) => relay(r).await,
        Err(_) => unavailable(),
    }
}

/// SSE for the chat UI; the browser passes its operator token in `?token=`.
/// The TTS side channel only forwards `chat_tts` / `chat_tts_settings`.
async fn stream(app: &App, user: &str, query: &str, tts_only: bool) -> Response {
    let token = query_map(query).remove("token");
    if !app.operator(token.as_deref()).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let upstream = match app
        .call(Method::GET, &format!("/events/{user}"))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => return relay(r).await,
        Err(_) => return unavailable(),
    };
    let body = upstream.bytes_stream();
    let body: Body = if tts_only {
        let filtered = futures_util::stream::unfold(
            (body, String::new()),
            |(mut body, mut buffer)| async move {
                loop {
                    if let Some(end) = buffer.find("\n\n") {
                        let block: String = buffer.drain(..end + 2).collect();
                        let event = block
                            .lines()
                            .find_map(|l| l.strip_prefix("event:"))
                            .map(str::trim);
                        let keep = matches!(event, Some("chat_tts" | "chat_tts_settings"))
                            || block.starts_with(':');
                        if keep {
                            return Some((
                                Ok::<_, std::io::Error>(axum::body::Bytes::from(block)),
                                (body, buffer),
                            ));
                        }
                        continue;
                    }
                    match body.next().await {
                        Some(Ok(chunk)) => {
                            buffer.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"))
                        }
                        _ => return None,
                    }
                }
            },
        );
        Body::from_stream(filtered)
    } else {
        Body::from_stream(body)
    };
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// Feature routes from the host: (owner, source, target, assets).
async fn feature_routes(app: &App) -> Option<(Value, Vec<(String, String, String, bool)>)> {
    let value: Value = app
        .call(Method::GET, "/features")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let routes = value["routes"]
        .as_array()
        .map(|routes| {
            routes
                .iter()
                .filter_map(|r| {
                    Some((
                        r["owner"].as_str()?.to_owned(),
                        r["source"].as_str()?.to_owned(),
                        r["target"].as_str()?.to_owned(),
                        r["assets"].as_bool()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some((value, routes))
}

async fn extension(app: &Arc<App>, path: &str, query: &str, request: Request) -> Option<Response> {
    let is_api = path.starts_with("/api/");
    // Legacy dashboard routes first (they own `/api/<name>` URLs).
    if is_api && path != "/api/dashboard/extensions" && !path.starts_with("/api/plugins/") {
        let segments: Vec<String> = path
            .trim_start_matches("/api/")
            .split('/')
            .map(str::to_owned)
            .collect();
        let segment_refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        let method = request.method().clone();
        let multipart = matches!(
            (method.as_str(), segment_refs.as_slice()),
            ("POST", ["upload-file" | "upload-avatar" | "stt"])
        );
        let probe = legacy(&method, &segment_refs, query, &Value::Null);
        if probe.is_some() || multipart {
            if !app.operator(bearer(&request)).await {
                return Some(StatusCode::UNAUTHORIZED.into_response());
            }
            if multipart {
                return Some(uploads(app, segment_refs[0], query, request).await);
            }
            let bytes = axum::body::to_bytes(request.into_body(), LIMIT)
                .await
                .unwrap_or_default();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if let ("POST", ["contexts", parent, "fork"]) =
                (method.as_str(), segment_refs.as_slice())
            {
                let new = body["new_user_id"].as_str().unwrap_or("");
                if new.is_empty() || encode(new) == *parent || new == *parent {
                    return Some(StatusCode::BAD_REQUEST.into_response());
                }
            }
            let call = legacy(&method, &segment_refs, query, &body)?;
            return Some(execute(app, call).await);
        }
    }
    // Feature pages: extension list, primary namespaces and host aliases.
    let (features, routes) = feature_routes(app).await?;
    if path == "/api/dashboard/extensions" {
        if !app.operator(bearer(&request)).await {
            return Some(StatusCode::UNAUTHORIZED.into_response());
        }
        return Some(Json(json!({"extensions": features["features"]})).into_response());
    }
    let under = |prefix: &str| path == prefix || path.starts_with(&format!("{prefix}/"));
    let owner_in = |prefix: &str| {
        path.strip_prefix(prefix)
            .and_then(|rest| rest.split('/').next())
            .map(str::to_owned)
    };
    let (owner, target, assets) = if path.starts_with("/api/plugins/") {
        (owner_in("/api/plugins/")?, path.to_owned(), false)
    } else if path.starts_with("/plugins/") {
        (owner_in("/plugins/")?, path.to_owned(), true)
    } else {
        let (owner, source, target, assets) =
            routes.iter().find(|(_, source, _, _)| under(source))?;
        (
            owner.clone(),
            format!("{target}{}", &path[source.len()..]),
            *assets,
        )
    };
    let pairs: Vec<(String, String)> = url_pairs(query);
    let token = pairs
        .iter()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.clone());
    let socket = request
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    if !assets {
        let presented = bearer(&request)
            .map(str::to_owned)
            .or(if socket { token } else { None });
        if !app.operator(presented.as_deref()).await {
            return Some(StatusCode::UNAUTHORIZED.into_response());
        }
    }
    let forwarded: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k != "token")
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    let host_path = with_query(format!("/features/{owner}{target}"), &forwarded.join("&"));
    Some(if socket {
        feature_socket(app.clone(), host_path, request).await
    } else {
        feature_http(app, &host_path, request).await
    })
}

async fn execute(app: &App, call: Call) -> Response {
    let mut builder = app.call(call.method, &call.path);
    if let Some(body) = &call.body {
        builder = builder.json(body);
    }
    let response = match builder.send().await {
        Ok(r) => r,
        Err(_) => return unavailable(),
    };
    if !response.status().is_success() || matches!(call.reply, Reply::Pass) {
        return relay(response).await;
    }
    let value: Value = response.json().await.unwrap_or_default();
    match call.reply {
        Reply::Pass => unreachable!(),
        Reply::Text(text) => text.into_response(),
        Reply::Field(field) => value[field]
            .as_str()
            .unwrap_or_default()
            .to_owned()
            .into_response(),
        Reply::Approved => format!(
            "Pairing approved for Discord user {}",
            value["discord_user_id"].as_str().unwrap_or_default()
        )
        .into_response(),
    }
}

/// Browser multipart uploads become raw Host API media uploads.
async fn uploads(app: &App, kind: &str, query: &str, request: Request) -> Response {
    let mut multipart = match Multipart::from_request(request, &()).await {
        Ok(m) => m,
        Err(e) => return e.into_response(),
    };
    let mut files = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("unknown").to_owned();
        let file_name = field.file_name().unwrap_or("file").to_owned();
        let Ok(data) = field.bytes().await else {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        };
        let (path, last) = match kind {
            "upload-file" => (format!("/media/files?name={}", encode(&file_name)), false),
            "upload-avatar" => (format!("/media/avatars/{}", encode(&field_name)), true),
            _ => {
                if data.is_empty() {
                    continue;
                }
                let user = query_map(query)
                    .remove("user_id")
                    .unwrap_or_else(|| "default".into());
                (format!("/media/stt/{}", encode(&user)), true)
            }
        };
        let response = match app.call(Method::POST, &path).body(data).send().await {
            Ok(r) => r,
            Err(_) => return unavailable(),
        };
        let status = response.status();
        let value: Value = response.json().await.unwrap_or_default();
        if kind == "stt" {
            return (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                Json(value),
            )
                .into_response();
        }
        if !status.is_success() {
            // Legacy upload errors are 200 + {"error"} for validation failures.
            return Json(json!({"error": value["error"]})).into_response();
        }
        if last {
            return Json(json!({"success": true, "url": value["url"]})).into_response();
        }
        files.push(value["url"].clone());
    }
    match kind {
        "upload-file" if !files.is_empty() => {
            Json(json!({"success": true, "files": files})).into_response()
        }
        "upload-file" => Json(json!({"error": "No files found"})).into_response(),
        "upload-avatar" => Json(json!({"error": "No file uploaded"})).into_response(),
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

use axum::extract::FromRequest;

async fn feature_http(app: &App, host_path: &str, request: Request) -> Response {
    let method = request.method().clone();
    let kind = request.headers().get(header::CONTENT_TYPE).cloned();
    let Ok(body) = axum::body::to_bytes(request.into_body(), 2 * 1024 * 1024).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let mut builder = app.call(method, host_path).body(body);
    if let Some(kind) = kind {
        builder = builder.header(reqwest::header::CONTENT_TYPE, kind.as_bytes());
    }
    match builder.send().await {
        Ok(r) => relay(r).await,
        Err(_) => unavailable(),
    }
}

async fn feature_socket(app: Arc<App>, host_path: String, request: Request) -> Response {
    let (mut parts, _) = request.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(e) => return e.into_response(),
    };
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as M};
    let Ok(mut upstream) = app
        .url(&host_path)
        .replacen("http:", "ws:", 1)
        .into_client_request()
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let Ok(auth) = format!("Bearer {}", app.host.token).parse() else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    upstream.headers_mut().insert("authorization", auth);
    let upstream = match tokio_tungstenite::connect_async(upstream).await {
        Ok((socket, _)) => socket,
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
            return StatusCode::from_u16(r.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY)
                .into_response()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    ws.on_upgrade(move |mut downstream| async move {
        use axum::extract::ws::Message;
        let mut upstream = upstream;
        loop {
            tokio::select! {
                message = downstream.recv() => {
                    let Some(Ok(message)) = message else { break };
                    let message = match message {
                        Message::Binary(b) => M::Binary(b), Message::Text(t) => M::Text(t),
                        Message::Ping(b) => M::Ping(b), Message::Pong(b) => M::Pong(b), Message::Close(_) => break,
                    };
                    if upstream.send(message).await.is_err() { break; }
                }
                message = upstream.next() => {
                    let Some(Ok(message)) = message else { break };
                    let message = match message {
                        M::Binary(b) => Message::Binary(b), M::Text(t) => Message::Text(t),
                        M::Ping(b) => Message::Ping(b), M::Pong(b) => Message::Pong(b), M::Close(_) => break, _ => continue,
                    };
                    if downstream.send(message).await.is_err() { break; }
                }
            }
        }
        let _ = downstream.send(Message::Close(None)).await;
        let _ = upstream.close(None).await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_routes_map_to_host_api_v1() {
        let body = json!({"user_id": "discord:1", "message": "hi", "attachments": ["/api/files/a"], "extra": 1});
        let c = legacy(&Method::POST, &["agent", "input"], "", &body).unwrap();
        assert_eq!(c.path, "/agent/discord%3A1/input");
        assert_eq!(
            c.body.unwrap(),
            json!({"message": "hi", "attachments": ["/api/files/a"]})
        );
        let c = legacy(
            &Method::PUT,
            &["tools", "shell"],
            "",
            &json!({"is_enabled": false}),
        )
        .unwrap();
        assert_eq!(
            (c.method, c.path.as_str()),
            (Method::POST, "/admin/tools/shell")
        );
        let c = legacy(
            &Method::GET,
            &["messages", "u"],
            "chat_only=1",
            &Value::Null,
        )
        .unwrap();
        assert_eq!(c.path, "/messages/u?chat_only=1");
        assert_eq!(
            legacy(
                &Method::DELETE,
                &["pairings", "pending", "C1"],
                "",
                &Value::Null
            )
            .unwrap()
            .path,
            "/admin/pending-pairings/C1"
        );
        assert_eq!(
            legacy(&Method::DELETE, &["pairings", "u1"], "", &Value::Null)
                .unwrap()
                .path,
            "/admin/pairings/u1"
        );
        assert_eq!(
            legacy(&Method::GET, &["cl", "u"], "", &Value::Null)
                .unwrap()
                .path,
            "/sm/u"
        );
        let c = legacy(&Method::PUT, &["tool-packages", "shell"], "", &json!({"enabled": false, "x": 1})).unwrap();
        assert_eq!((c.method, c.path.as_str(), c.body.unwrap()), (Method::POST, "/admin/tool-packages/shell", json!({"enabled": false})));
        assert!(legacy(&Method::GET, &["nope"], "", &Value::Null).is_none());
        assert!(legacy(&Method::POST, &["secrets"], "", &Value::Null).is_none());
    }
}
