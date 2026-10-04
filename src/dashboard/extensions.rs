//! Host authentication and bounded transport for trusted feature web services.
use super::routes::DashboardState;
use crate::runtime::{features::ServiceHandle, web::WebAlias};
use axum::{
    extract::{
        ws::{Message, WebSocketUpgrade},
        FromRequestParts, Request, State,
    },
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use praxis_plugin_api::web::PRIVATE_HEADER;
use std::sync::Arc;
use std::time::Duration;
const LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone)]
struct Proxy {
    host: Arc<DashboardState>,
    service: ServiceHandle,
    source: String,
    target: String,
    assets: bool,
    client: reqwest::Client,
}

pub(crate) fn router(state: Arc<DashboardState>) -> Router {
    let mut app = Router::new()
        .route("/api/dashboard/extensions", get(list))
        .with_state(state.clone());
    for service in state.plugins.web_services() {
        let owner = &service.descriptor().owner;
        let mut routes = vec![
            WebAlias::api(
                &format!("/api/plugins/{owner}"),
                &format!("/api/plugins/{owner}"),
            ),
            WebAlias::assets(&format!("/plugins/{owner}"), &format!("/plugins/{owner}")),
        ];
        routes.extend(service.web_aliases());
        for alias in routes {
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(300))
                .build()
                .expect("fixed local HTTP client configuration");
            let proxy = Proxy {
                host: state.clone(),
                service: service.clone(),
                source: alias.source,
                target: alias.target,
                assets: alias.assets,
                client,
            };
            let root = proxy.source.clone();
            let child = format!("{root}/*path");
            app = app.merge(
                Router::new()
                    .route(&root, any(forward))
                    .route(&child, any(forward))
                    .with_state(proxy),
            );
        }
    }
    app
}

fn bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
async fn list(State(state): State<Arc<DashboardState>>, request: Request) -> Response {
    if !bearer(&request).is_some_and(|token| super::routes::dashboard_token_valid(&state, token)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut descriptors: Vec<_> = state
        .plugins
        .web_services()
        .into_iter()
        .filter_map(|service| {
            service
                .web_endpoint()
                .map(|endpoint| endpoint.info.descriptor)
        })
        .collect();
    descriptors.sort_by(|a, b| a.id.cmp(&b.id));
    Json(serde_json::json!({"extensions":descriptors})).into_response()
}

async fn forward(State(proxy): State<Proxy>, request: Request) -> Response {
    let suffix = request
        .uri()
        .path()
        .strip_prefix(&proxy.source)
        .unwrap_or("");
    let path = format!("{}{suffix}", proxy.target);
    if path.contains('%')
        || path.contains('\\')
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let query: std::collections::HashMap<String, String> =
        axum::extract::Query::try_from_uri(request.uri())
            .map(|q| q.0)
            .unwrap_or_default();
    let is_socket = proxy
        .service
        .descriptor()
        .web
        .as_ref()
        .is_some_and(|web| web.websockets.contains(&path));
    if proxy.assets {
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
    } else {
        let token = bearer(&request).or_else(|| {
            if is_socket {
                query.get("token").map(String::as_str)
            } else {
                None
            }
        });
        if !token.is_some_and(|token| super::routes::dashboard_token_valid(&proxy.host, token)) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let Some(endpoint) = proxy.service.web_endpoint() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(lease) = proxy.service.web_lease() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let stop = proxy.service.stop_token();
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
        return super::routes::tool_activity(
            State(proxy.host.clone()),
            axum::extract::Query(query),
        )
        .await
        .into_response();
    }
    let method = request.method().clone();
    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
    let send = async {
        let body = axum::body::to_bytes(request.into_body(), LIMIT)
            .await
            .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
        let mut outgoing = proxy
            .client
            .request(method, url)
            .header(PRIVATE_HEADER, &endpoint.key)
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

#[cfg(test)]
#[path = "extension_tests.rs"]
mod tests;
