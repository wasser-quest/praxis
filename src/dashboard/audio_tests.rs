use super::*;
use crate::db::messages::Message;

#[tokio::test]
async fn audio_endpoint_requires_auth_and_serves_only_the_requested_sessions_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    db.save_context(&db.load_context("alice").unwrap()).unwrap();
    let id = db.add_message("alice", &Message::assistant("reply".into())).unwrap();
    db.save_message_audio("alice", id, "audio/wav", b"fixture-wav").unwrap();
    let state = Arc::new(DashboardState { plugins: Arc::new(crate::plugins::PluginRegistry::new()),
        db: db.clone(),
        gateway_api_key: "synthetic-audio-test-key".into(),
        admin_password: "unused".into(),
    });
    let app = Router::new()
        .route("/api/chat/audio/:user_id/:message_id", axum::routing::get(get_chat_audio))
        .route("/api/messages/:user_id", axum::routing::get(get_messages))
        .layer(middleware::from_fn_with_state(state.clone(), dashboard_auth_middleware))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let url = format!("{base}/api/chat/audio/alice/{id}");
    assert_eq!(client.get(&url).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(client.get(&url).bearer_auth("wrong").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let auth = "synthetic-audio-test-key";
    let response = client.get(&url).bearer_auth(auth).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "audio/wav");
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"fixture-wav");
    for path in [format!("/api/chat/audio/bob/{id}"), "/api/chat/audio/alice/9999".into()] {
        assert_eq!(client.get(format!("{base}{path}")).bearer_auth(auth).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    }
    let history: serde_json::Value = client.get(format!("{base}/api/messages/alice"))
        .bearer_auth(auth).send().await.unwrap().json().await.unwrap();
    assert_eq!(history["messages"][0]["id"], id);
    assert_eq!(history["messages"][0]["audio_mime"], "audio/wav");
    assert!(!history.to_string().contains("fixture-wav"));
    db.clear_messages("alice").unwrap();
    assert_eq!(client.get(&url).bearer_auth(auth).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    server.abort();
}

#[tokio::test]
async fn vosk_remote_dashboard_wav_uses_context_url_without_elevenlabs_key_or_local_model() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{accept_async, tungstenite::Message as WsMessage};
    let vosk_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vosk_url = format!("ws://{}", vosk_listener.local_addr().unwrap());
    let vosk_server = tokio::spawn(async move {
        let mut socket = accept_async(vosk_listener.accept().await.unwrap().0).await.unwrap();
        socket.next().await.unwrap().unwrap(); // configuration
        assert!(socket.next().await.unwrap().unwrap().is_binary());
        socket.send(WsMessage::Text(r#"{"partial":""}"#.into())).await.unwrap();
        assert_eq!(socket.next().await.unwrap().unwrap().into_text().unwrap(), r#"{"eof":1}"#);
        socket.send(WsMessage::Text(r#"{"text":"remote dashboard"}"#.into())).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    db.merge_context("vosk-dashboard", serde_json::json!({
        "settings.voice_stt_type": "vosk", "settings.voice_vosk_url": vosk_url,
    })).unwrap();
    let state = Arc::new(DashboardState { plugins: Arc::new(crate::plugins::PluginRegistry::new()),
        db, gateway_api_key: "synthetic-test-key".into(), admin_password: "unused".into(),
    });
    let app = Router::new().route("/stt", axum::routing::post(dashboard_stt)).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/stt?user_id=vosk-dashboard", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let wav = crate::voice::pcm_to_wav(&[1000, -1000], 16000, 1);
    let form = reqwest::multipart::Form::new().part("audio", reqwest::multipart::Part::bytes(wav).file_name("input.wav"));
    let response: serde_json::Value = reqwest::Client::new().post(url).multipart(form)
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(response["text"], "remote dashboard", "{response}");
    vosk_server.await.unwrap();
    server.abort();
}
