//! Generic web seam: run these in a core built without either VM crate.
use super::*;
use crate::{
    plugins::PluginRegistry,
    runtime::{
        features::{InvocationContext, NativeService},
        web::{WebAlias, WebEndpoint},
    },
};
use praxis_plugin_api::web::{WebDescriptor, WebInfo, PRIVATE_HEADER, WEB_API_VERSION};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

struct Fixture {
    port: u16,
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
            WebInfo {
                version: WEB_API_VERSION,
                port: self.port,
                descriptor: self.web_descriptor().unwrap(),
            },
            "private-host-fixture".into(),
        ))
    }
    fn web_aliases(&self) -> Vec<WebAlias> {
        vec![
            WebAlias::api("/api/vm", "/api/plugins/vm"),
            WebAlias::api("/websockify", "/api/plugins/vm/vnc/ws"),
            WebAlias::assets("/static/novnc", "/plugins/vm/novnc"),
        ]
    }
    async fn invoke(&self, _: InvocationContext, _: &str, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<DashboardState>,
    String,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
) {
    let backend = Router::new().route("/api/plugins/vm", axum::routing::any(|request: axum::extract::Request| async move {
        Json(json!({"private":request.headers().get(PRIVATE_HEADER).unwrap().to_str().unwrap(), "authorization":request.headers().get("authorization").is_some(), "cookie":request.headers().get("cookie").is_some(), "query":request.uri().query()}))
    })).route("/plugins/vm/novnc/core/rfb.js", axum::routing::get(|| async { "real-package-asset" }))
      .route("/api/plugins/vm/vnc/ws", axum::routing::get(|ws: axum::extract::ws::WebSocketUpgrade| async { ws.on_upgrade(|mut socket| async move { while let Some(Ok(message)) = socket.recv().await { if socket.send(message).await.is_err() { break; } } }) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let mut plugins = PluginRegistry::new();
    plugins
        .try_register(serde_json::from_str(include_str!("../../plugins/vm/plugin.json")).unwrap())
        .unwrap();
    plugins
        .register_service(
            "vm",
            "vm",
            1,
            crate::runtime::vm::TOOL_NAMES,
            Arc::new(Fixture { port }),
        )
        .unwrap();
    let state = Arc::new(DashboardState {
        db: crate::db::Database::new(dir.path()).unwrap(),
        plugins: Arc::new(plugins),
        gateway_api_key: "public-dashboard-key".into(),
        admin_password: "password".into(),
    });
    let app = super::super::routes::router_with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let host = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (dir, state, format!("http://{address}"), worker, host)
}

#[tokio::test]
async fn core_proxy_authenticates_before_io_and_strips_public_credentials() {
    let (_dir, _state, url, worker, host) = fixture().await;
    let client = reqwest::Client::new();
    for path in ["/api/plugins/vm", "/api/vm", "/api/dashboard/extensions"] {
        assert_eq!(
            client
                .get(format!("{url}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let result: Value = client
        .get(format!("{url}/api/vm?token=public-dashboard-key&limit=3"))
        .bearer_auth("public-dashboard-key")
        .header(PRIVATE_HEADER, "forged-private-key")
        .header("cookie", "session=secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["private"], "private-host-fixture");
    assert_eq!(result["authorization"], false);
    assert_eq!(result["cookie"], false);
    assert_eq!(result["query"], "limit=3");
    assert_eq!(
        client
            .get(format!("{url}/api/vm?token=public-dashboard-key"))
            .send()
            .await
            .unwrap()
            .status(),
        401,
        "query auth is only for declared sockets"
    );
    let list: Value = client
        .get(format!("{url}/api/dashboard/extensions"))
        .bearer_auth("public-dashboard-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["extensions"][0]["id"], "vm");
    assert!(!list.to_string().contains("private-host-fixture"));
    assert!(!list.to_string().contains("port"));
    worker.abort();
    host.abort();
}

#[tokio::test]
async fn assets_are_public_read_only_and_dead_or_disabled_binding_fails_closed() {
    let (_dir, state, url, worker, host) = fixture().await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{url}/static/novnc/core/rfb.js"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "real-package-asset"
    );
    assert_eq!(
        client
            .post(format!("{url}/static/novnc/core/rfb.js"))
            .send()
            .await
            .unwrap()
            .status(),
        405
    );
    worker.abort();
    let _ = worker.await;
    assert_eq!(
        client
            .get(format!("{url}/api/vm"))
            .bearer_auth("public-dashboard-key")
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    state
        .plugins
        .service_handle("vm", "vm")
        .unwrap()
        .disable(Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{url}/static/novnc/core/rfb.js"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    host.abort();
}

#[tokio::test]
async fn active_socket_is_authenticated_and_closes_when_binding_is_disabled() {
    use futures_util::{SinkExt, StreamExt};
    let (_dir, state, url, worker, host) = fixture().await;
    let ws_url = url.replace("http:", "ws:");
    assert!(
        tokio_tungstenite::connect_async(format!("{ws_url}/websockify"))
            .await
            .is_err()
    );
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("{ws_url}/websockify?token=public-dashboard-key"))
            .await
            .unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(vec![
            1, 2, 3,
        ]))
        .await
        .unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap().into_data(),
        vec![1, 2, 3]
    );
    assert!(
        !state
            .plugins
            .service_handle("vm", "vm")
            .unwrap()
            .disable(Duration::from_millis(20))
            .await
            .unwrap(),
        "active connection requires force after drain budget"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
        }
    })
    .await
    .unwrap();
    worker.abort();
    host.abort();
}
