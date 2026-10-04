//! VM-owned web contribution, independent of the host dashboard/database.
mod assets;
mod routes;
pub use assets::ASSETS;
use axum::{extract::State, http::StatusCode, middleware, response::IntoResponse};
use praxis_plugin_api::web::{WebDescriptor, WebInfo, PRIVATE_HEADER, WEB_API_VERSION};
use praxis_vm::runtime::VmRuntime;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub fn descriptor() -> WebDescriptor {
    let mut descriptor = WebDescriptor::for_package("vm", "Virtual machines");
    descriptor.websockets.push("/api/plugins/vm/vnc/ws".into());
    descriptor
}

pub(crate) struct WebState {
    runtime: Arc<VmRuntime>,
    key: String,
    stop: CancellationToken,
}

/// Loopback only. The host chooses the private nonce; possession of a public
/// dashboard token does not authorize this internal listener directly.
pub struct WebServer {
    info: WebInfo,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl WebServer {
    pub async fn start(runtime: Arc<VmRuntime>, key: String) -> anyhow::Result<Self> {
        anyhow::ensure!(
            key.len() >= 16
                && key.len() <= 128
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "Invalid private web nonce"
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let info = WebInfo {
            version: WEB_API_VERSION,
            port: listener.local_addr()?.port(),
            descriptor: descriptor(),
        };
        let stop = CancellationToken::new();
        let state = Arc::new(WebState {
            runtime,
            key,
            stop: stop.clone(),
        });
        let app = routes::router()
            .layer(middleware::from_fn_with_state(state.clone(), private_auth))
            .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
            .with_state(state);
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        });
        Ok(Self { info, stop, task })
    }
    pub fn info(&self) -> WebInfo {
        self.info.clone()
    }
    pub fn stop(&self) {
        self.stop.cancel();
    }
}
impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

async fn private_auth(
    State(state): State<Arc<WebState>>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    if request
        .headers()
        .get(PRIVATE_HEADER)
        .and_then(|h| h.to_str().ok())
        != Some(state.key.as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tokio::select! {
        biased;
        _ = state.stop.cancelled() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        response = next.run(request) => response,
    }
}
