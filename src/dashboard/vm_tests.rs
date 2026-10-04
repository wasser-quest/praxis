//! Exercise the legacy UI routes without QEMU or model inference.
use super::*;

#[tokio::test]
async fn vm_viewer_does_not_embed_untrusted_names_or_tokens_in_html() {
    let injected = "</script><script>alert('query')</script>";
    let params = HashMap::from([
        ("vm".into(), injected.into()),
        ("token".into(), injected.into()),
    ]);
    let page = vm_vnc_viewer(Query(params)).await.0;
    assert!(
        !page.contains(injected),
        "query parameters must not become HTML or JavaScript"
    );
    assert!(
        page.contains("localStorage.getItem('praxis_token')"),
        "standalone viewer must reuse the authenticated dashboard session"
    );
}

#[tokio::test]
async fn vm_dashboard_without_binding_is_read_only_and_cannot_lazy_start() {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(DashboardState {
        db: crate::db::Database::new(dir.path()).unwrap(),
        plugins: Arc::new(crate::plugins::PluginRegistry::new()),
        gateway_api_key: "fixture-key".into(),
        admin_password: "fixture-password".into(),
    });
    assert_eq!(
        list_vm_status(State(state.clone())).await.0["config"]["vm_enabled"],
        false
    );
    let request: VmStartRequest = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(
        vm_start(State(state.clone()), Json(request))
            .await
            .unwrap_err(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!dir.path().join("vm").exists());
    assert!(!dir.path().join("shared").exists());
}

#[tokio::test]
async fn vm_dashboard_flag_updates_target_the_plugin_override_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut config = crate::config::Config::from_env();
    config.data_dir = dir.path().to_str().unwrap().into();
    config.vm_enabled = true;
    let mut plugins = crate::plugins::PluginRegistry::new();
    plugins
        .try_register(serde_json::from_str(include_str!("../../plugins/vm/plugin.json")).unwrap())
        .unwrap();
    crate::runtime::vm::configure(&db, &config, &mut plugins).unwrap();
    crate::db::tools::enable_vm_compatibility(&db).unwrap();
    set_dashboard_tool_enabled(&db, &plugins, "vm_shell", false).unwrap();
    assert!(!crate::db::tools::get_plugin_tool_enabled(&db, "vm_shell"));
    let state = Arc::new(DashboardState {
        db,
        plugins: Arc::new(plugins),
        gateway_api_key: "fixture-key".into(),
        admin_password: "fixture-password".into(),
    });
    let tools = list_all_tools(State(state)).await.unwrap().0;
    let entries: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"] == "vm_shell")
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["source"], "plugin");
    assert_eq!(entries[0]["is_enabled"], false);
}

#[tokio::test]
async fn vm_vnc_legacy_alias_rejects_anonymous_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(DashboardState {
        db: crate::db::Database::new(dir.path()).unwrap(),
        plugins: Arc::new(crate::plugins::PluginRegistry::new()),
        gateway_api_key: "fixture-key".into(),
        admin_password: "fixture-password".into(),
    });
    let app = Router::new()
        .route("/websockify", axum::routing::get(vnc_ws_proxy_noauth))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/websockify"))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    server.abort();
    assert!(!dir.path().join("vm").exists());
}
