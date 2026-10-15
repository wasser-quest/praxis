//! Terminal contributions through the authenticated gateway: listing, status
//! line and `/` command, run as the package's own tool with sanitized output.
use super::super::{management_state, routes};
use crate::plugins::{Plugin, PluginRegistry};
use reqwest::StatusCode;
use serde_json::{json, Value};

const KEY: &str = "terminal-api-key-12345";

fn state(dir: &std::path::Path) -> super::GatewayState {
    let script = dir.join("bili.py");
    std::fs::write(
        &script,
        "import json, os\nargs = json.loads(os.environ['PLUGIN_ARGS'])\n\
         print('\\x1b[31mcache 12k\\x1b[0m' if 'text' not in args else 'echo: ' + args['text'])\n",
    )
    .unwrap();
    let plugin: Plugin = serde_json::from_value(json!({
        "name": "bili", "description": "test", "version": "1", "enabled": true,
        "tools": [{"name": "bili_status", "description": "status", "parameters": {"type": "object"},
            "handler": {"type": "script", "path": script, "interpreter": "python3"}}],
        "provides": {"terminal": {
            "status": {"tool": "bili_status", "every_seconds": 30},
            "commands": [{"name": "bili", "tool": "bili_status", "description": "Show bili cache"}]}}
    }))
    .unwrap();
    let mut plugins = PluginRegistry::new();
    plugins.register(plugin);
    let db = crate::db::Database::new(&dir.join("data")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut config = crate::config::Config::from_env();
    config.root_dir = dir.to_string_lossy().into_owned();
    config.workspace_dir = None;
    config.gateway_api_key = KEY.into();
    config.dashboard_admin_password = "terminal-password".into();
    management_state(db, config, Default::default(), plugins).unwrap()
}

async fn call(base: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let client = reqwest::Client::new();
    let mut request = client
        .request(method.parse().unwrap(), format!("{base}{path}"))
        .bearer_auth(KEY);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn packages_add_status_lines_and_commands_to_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let app = routes(state(dir.path()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let app = base.as_str();
    let (status, listing) = call(&app, "GET", "/v1/terminal", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["status"][0]["plugin"], "bili");
    assert_eq!(listing["commands"][0]["name"], "bili");

    let (status, line) = call(&app, "POST", "/v1/terminal/u1/status/bili", None).await;
    assert_eq!(status, StatusCode::OK, "{line}");
    assert_eq!(line["text"], "cache 12k", "escape sequences are stripped");

    let (status, output) = call(&app, "POST", "/v1/terminal/u1/command/bili", Some(json!({"text": "hello"}))).await;
    assert_eq!(status, StatusCode::OK, "{output}");
    assert_eq!(output["text"].as_str().unwrap().trim(), "echo: hello");

    let (status, _) = call(&app, "POST", "/v1/terminal/u1/command/missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let unauthenticated = reqwest::get(format!("{base}/v1/terminal")).await.unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    server.abort();
}
