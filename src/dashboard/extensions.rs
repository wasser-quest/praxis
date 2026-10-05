//! Host authentication and bounded transport for trusted feature web services.
use super::routes::DashboardState;
use crate::runtime::{features::ServiceHandle, web::WebAlias};
use axum::{
    extract::{Request, State},
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use std::sync::Arc;

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
            let client = crate::runtime::web_proxy::client();
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
    if !crate::runtime::web_proxy::safe_path(&path) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let query: std::collections::HashMap<String, String> =
        axum::extract::Query::try_from_uri(request.uri())
            .map(|q| q.0)
            .unwrap_or_default();
    let is_socket = crate::runtime::web_proxy::is_socket(&proxy.service, &path);
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
    crate::runtime::web_proxy::forward(
        &proxy.service,
        &proxy.client,
        &path,
        query,
        is_socket,
        request,
        &proxy.host.db,
    )
    .await
}

#[cfg(test)]
#[path = "extension_tests.rs"]
mod tests;
