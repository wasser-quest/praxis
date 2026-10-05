//! Bounded loopback transport to trusted feature web services (HTTP and
//! WebSocket), shared by the built-in dashboard's extension routes and Host
//! API v1 feature slots. Callers authenticate first; this layer always talks
//! to the service's fixed loopback port with its private key and the operator
//! principal, strips caller credentials, and enforces size/time limits.
use super::features::ServiceHandle;
use axum::{
    extract::{
        ws::{Message, WebSocketUpgrade},
        FromRequestParts, Request,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::{SinkExt, StreamExt};
use praxis_plugin_api::web::{PRINCIPAL_HEADER, PRIVATE_HEADER};
use std::collections::HashMap;
use std::time::Duration;

pub const LIMIT: usize = 2 * 1024 * 1024;

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .build()
        .expect("fixed local HTTP client configuration")
}

/// Reject encoded, backslash and dot-segment paths before routing.
pub fn safe_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains('%')
        && !path.contains('\\')
        && !path.split('/').any(|segment| matches!(segment, "." | ".."))
}

pub fn is_socket(service: &ServiceHandle, path: &str) -> bool {
    service
        .descriptor()
        .web
        .as_ref()
        .is_some_and(|web| web.websockets.iter().any(|socket| socket == path))
}

/// Paths a service may be reached at: its own API and asset namespaces plus
/// host-registered alias targets. Returns whether the path is an asset path.
pub fn target_kind(service: &ServiceHandle, path: &str) -> Option<bool> {
    let owner = &service.descriptor().owner;
    let under = |prefix: &str| path == prefix || path.starts_with(&format!("{prefix}/"));
    if under(&format!("/api/plugins/{owner}")) {
        return Some(false);
    }
    if under(&format!("/plugins/{owner}")) {
        return Some(true);
    }
    service
        .web_aliases()
        .into_iter()
        .find(|alias| under(&alias.target))
        .map(|alias| alias.assets)
}

/// Recent VM tool activity (host-owned table, served for the VM page).
pub fn activity(db: &crate::db::Database, query: &HashMap<String, String>) -> serde_json::Value {
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let conn = db.conn();
    let activities: Vec<serde_json::Value> = conn
        .prepare("SELECT vm_id, action, input, output, created_at FROM vm_activity_log ORDER BY id DESC LIMIT ?1")
        .and_then(|mut stmt| {
            stmt.query_map([limit], |row| {
                Ok(serde_json::json!({
                    "vm_id": row.get::<_, String>(0)?,
                    "action": row.get::<_, String>(1)?,
                    "input": row.get::<_, Option<String>>(2)?,
                    "output": row.get::<_, Option<String>>(3)?,
                    "created_at": row.get::<_, Option<String>>(4)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    serde_json::json!({ "activities": activities })
}

pub async fn forward(
    service: &ServiceHandle,
    client: &reqwest::Client,
    path: &str,
    query: HashMap<String, String>,
    is_socket: bool,
    request: Request,
    db: &crate::db::Database,
) -> Response {
    let Some(endpoint) = service.web_endpoint() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(lease) = service.web_lease() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let stop = service.stop_token();
    // Fixed loopback destination; discard public auth/cookies/private headers
    // and query credentials before forwarding. Never honor a caller URL.
    let Ok(mut url) =
        reqwest::Url::parse(&format!("http://127.0.0.1:{}{path}", endpoint.info.port))
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    if !query.is_empty() {
        let mut pairs: Vec<_> = query
            .iter()
            .filter(|(key, _)| key.as_str() != "token")
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        if !pairs.is_empty() {
            url.query_pairs_mut().extend_pairs(pairs);
        }
    }
    if is_socket {
        let (mut parts, _body) = request.into_parts();
        let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(ws) => ws,
            Err(error) => return error.into_response(),
        };
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let socket_url = url.as_str().replacen("http:", "ws:", 1);
        let Ok(mut upstream_request) = socket_url.into_client_request() else {
            return StatusCode::BAD_GATEWAY.into_response();
        };
        let Ok(key) = endpoint.key.parse() else {
            return StatusCode::BAD_GATEWAY.into_response();
        };
        upstream_request.headers_mut().insert(PRIVATE_HEADER, key);
        // Dashboard tokens are operator credentials (sub = "admin").
        upstream_request
            .headers_mut()
            .insert(PRINCIPAL_HEADER, axum::http::HeaderValue::from_static("operator"));
        let upstream = tokio::select! {
            _ = stop.cancelled() => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            result = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async_with_config(upstream_request, Some(tokio_tungstenite::tungstenite::protocol::WebSocketConfig { max_message_size:Some(LIMIT), max_frame_size:Some(LIMIT), ..Default::default() }), false)) => match result { Ok(Ok((socket,_))) => socket, Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) if response.status().is_client_error() => return response.status().into_response(), _ => return StatusCode::SERVICE_UNAVAILABLE.into_response() },
        };
        return ws.max_message_size(LIMIT).max_frame_size(LIMIT).on_upgrade(move |mut downstream| async move {
            let _lease = lease;
            let mut upstream = upstream;
            let transfer = async {
            loop {
                tokio::select! {
                    biased;
                    message = downstream.recv() => {
                        let Some(Ok(message)) = message else { break; };
                        use tokio_tungstenite::tungstenite::Message as M;
                        let message = match message { Message::Binary(bytes) => M::Binary(bytes), Message::Text(text) => M::Text(text), Message::Ping(bytes) => M::Ping(bytes), Message::Pong(bytes) => M::Pong(bytes), Message::Close(_) => break };
                        if upstream.send(message).await.is_err() { break; }
                    }
                    message = upstream.next() => {
                        let Some(Ok(message)) = message else { break; };
                        use tokio_tungstenite::tungstenite::Message as M;
                        let message = match message { M::Binary(bytes) => Message::Binary(bytes), M::Text(text) => Message::Text(text), M::Ping(bytes) => Message::Ping(bytes), M::Pong(bytes) => Message::Pong(bytes), M::Close(_) => break, _ => continue };
                        if downstream.send(message).await.is_err() { break; }
                    }
                }
            }
            };
            tokio::select! { biased; _ = stop.cancelled() => {}, _ = transfer => {} }
            let _ = tokio::time::timeout(Duration::from_millis(200), downstream.send(Message::Close(None))).await;
            let _ = tokio::time::timeout(Duration::from_millis(200), upstream.close(None)).await;
        }).into_response();
    }
    let _lease = lease;
    if path == "/api/tool-activity" {
        return Json(activity(db, &query)).into_response();
    }
    let method = request.method().clone();
    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
    let send = async {
        let body = axum::body::to_bytes(request.into_body(), LIMIT)
            .await
            .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
        let mut outgoing = client
            .request(method, url)
            .header(PRIVATE_HEADER, &endpoint.key)
            .header(PRINCIPAL_HEADER, "operator")
            .body(body);
        if let Some(content_type) = content_type {
            outgoing = outgoing.header(header::CONTENT_TYPE, content_type);
        }
        let mut incoming = outgoing
            .send()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if incoming
            .content_length()
            .is_some_and(|len| len > LIMIT as u64)
        {
            return Err(StatusCode::BAD_GATEWAY);
        }
        let status = incoming.status();
        let content_type = incoming.headers().get(header::CONTENT_TYPE).cloned();
        let mut bytes = Vec::new();
        while let Some(chunk) = incoming
            .chunk()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?
        {
            if bytes.len() + chunk.len() > LIMIT {
                return Err(StatusCode::BAD_GATEWAY);
            }
            bytes.extend_from_slice(&chunk);
        }
        let mut response = (status, bytes).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store, must-revalidate"),
        );
        if let Some(content_type) = content_type {
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type);
        }
        Ok::<_, StatusCode>(response)
    };
    tokio::select! {
        biased;
        _ = stop.cancelled() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        result = tokio::time::timeout(Duration::from_secs(300), send) => match result { Ok(Ok(response)) => response, Ok(Err(status)) => status.into_response(), Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response() },
    }
}
