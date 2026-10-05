//! The package against a fake Host API: operator auth via auth/verify,
//! legacy reply shapes, multipart → raw media, TTS-only SSE filtering, and the
//! package token never reaching the browser.
use axum::{extract::Request, routing::any, Json, Router};
use praxis_plugin_api::host::HostApiGrant;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

async fn fake_host(log: Arc<Mutex<Vec<String>>>) -> String {
    let app = Router::new().fallback(any(move |request: Request| {
        let log = log.clone();
        async move {
            let auth = request.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
            assert_eq!(auth, "Bearer package-token", "host calls must use the package token");
            let line = format!("{} {}", request.method(), request.uri());
            log.lock().unwrap().push(line.clone());
            let body = axum::body::to_bytes(request.into_body(), 1 << 20).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let path = line.split(' ').nth(1).unwrap().to_owned();
            let route = if path.starts_with("/host/v1/media/files") { path.clone() } else { path.split('?').next().unwrap().to_owned() };
            let response: axum::response::Response = match route.as_str() {
                "/host/v1/auth/verify" => Json(json!({"valid": body["token"] == "operator"})).into_response(),
                "/host/v1/admin/tools/shell" => Json(json!({"success": true})).into_response(),
                "/host/v1/admin/workflows/a.sm" => Json(json!({"name": "a.sm", "content": "[state a]"})).into_response(),
                "/host/v1/admin/pending-pairings/C1" => Json(json!({"success": true, "discord_user_id": "42"})).into_response(),
                p if p.starts_with("/host/v1/media/files?name=") => Json(json!({"url": format!("/api/files/{}", &p[26..])})).into_response(),
                "/host/v1/agent/discord%3A1/input" => Json(json!({"echo": body})).into_response(),
                "/host/v1/events/u" => (
                    [("content-type", "text/event-stream")],
                    "event: char\ndata: {\"data\":\"x\"}\n\nevent: chat_tts\ndata: {\"data\":\"audio\"}\n\n",
                ).into_response(),
                "/host/v1/features" => Json(json!({"features": [{"id": "vm"}], "routes": [
                    {"owner": "vm", "source": "/api/vm", "target": "/api/plugins/vm", "assets": false}
                ]})).into_response(),
                "/host/v1/features/vm/api/plugins/vm/guests" => Json(json!({"guests": []})).into_response(),
                _ => (axum::http::StatusCode::NOT_FOUND, "missing").into_response(),
            };
            response
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}
use axum::response::IntoResponse;

#[tokio::test]
async fn dashboard_package_adapts_the_legacy_api_onto_host_api_v1() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let host = fake_host(log.clone()).await;
    let statics = tempfile::tempdir().unwrap();
    std::fs::write(
        statics.path().join("index.html"),
        "<title>Praxis Dashboard</title>",
    )
    .unwrap();
    let grant = HostApiGrant {
        version: 1,
        url: host,
        token: "package-token".into(),
        scopes: vec![],
    };
    let app = praxis_dashboard::router(grant, statics.path().to_owned());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();

    let index = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(
        index.headers()["cache-control"],
        "no-store, must-revalidate"
    );
    let page = index.text().await.unwrap();
    assert!(page.contains("Praxis Dashboard") && !page.contains("package-token"));

    assert_eq!(
        http.get(format!("{base}/api/chat-sessions"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{base}/api/chat-sessions"))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    let op = |r: reqwest::RequestBuilder| r.bearer_auth("operator");
    let text = op(http.put(format!("{base}/api/tools/shell")))
        .json(&json!({"is_enabled": true}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(text, "Tool updated");
    let sm = op(http.get(format!("{base}/api/sm-files/a.sm")))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(sm, "[state a]");
    let approved = op(http.post(format!("{base}/api/pairings/pending/C1/approve")))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(approved, "Pairing approved for Discord user 42");
    let echo: Value = op(http.post(format!("{base}/api/agent/input")))
        .json(&json!({"user_id": "discord:1", "message": "hi", "unknown": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(echo["echo"], json!({"message": "hi"}));

    let multipart = "--b\r\nContent-Disposition: form-data; name=\"files\"; filename=\"../x y.txt\"\r\n\r\nhello\r\n--b--\r\n";
    let uploaded: Value = op(http.post(format!("{base}/api/upload-file")))
        .header("content-type", "multipart/form-data; boundary=b")
        .body(multipart)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(uploaded["success"], true, "{uploaded}");

    let tts = http
        .get(format!("{base}/api/chat/stream/u/tts?token=operator"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        tts.contains("chat_tts") && !tts.contains("event: char"),
        "{tts}"
    );
    let full = http
        .get(format!("{base}/api/chat/stream/u?token=operator"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(full.contains("event: char"));
    assert_eq!(
        http.get(format!("{base}/api/chat/stream/u"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    let extensions: Value = op(http.get(format!("{base}/api/dashboard/extensions")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(extensions["extensions"][0]["id"], "vm");
    let guests: Value = op(http.get(format!("{base}/api/vm/guests?token=operator&x=1")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(guests["guests"], json!([]));
    assert_eq!(
        http.get(format!("{base}/api/vm/guests"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{base}/api/..%2Fsecrets"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    let log = log.lock().unwrap().clone();
    assert!(
        log.iter().any(|l| l == "POST /host/v1/admin/tools/shell"),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l.starts_with("POST /host/v1/media/files?name=..%2Fx%20y.txt")),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l == "GET /host/v1/features/vm/api/plugins/vm/guests?x=1"),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|l| l.contains("token=")),
        "browser tokens are never forwarded: {log:?}"
    );
}
