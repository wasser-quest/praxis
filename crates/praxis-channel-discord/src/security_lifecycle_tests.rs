use super::*;
use host::{
    ChannelEvent, ChannelHost, ContextJson, DiscordCredentials, MirroredMessage, Pairing,
    SkillPage, SkillSummary,
};
use std::path::Path;
use tokio::sync::{broadcast, mpsc};

struct TestHost {
    receiver: std::sync::Mutex<Option<broadcast::Receiver<ChannelEvent>>>,
}

#[async_trait::async_trait]
impl ChannelHost for TestHost {
    fn pairing_by_discord(&self, _: &str) -> anyhow::Result<Option<Pairing>> {
        Ok(None)
    }
    fn pairing_by_user(&self, _: &str) -> anyhow::Result<Option<Pairing>> {
        Ok(None)
    }
    fn list_pairings(&self) -> anyhow::Result<Vec<Pairing>> {
        Ok(vec![])
    }
    fn create_pending_pairing(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        unreachable!()
    }
    fn create_pairing(&self, _: &str, _: &str, _: Option<&str>) -> anyhow::Result<()> {
        unreachable!()
    }
    fn load_context(&self, _: &str) -> anyhow::Result<ContextJson> {
        unreachable!()
    }
    fn save_context(&self, _: &str, _: &ContextJson) -> anyhow::Result<()> {
        unreachable!()
    }
    fn merge_context(&self, _: &str, _: serde_json::Value) -> anyhow::Result<ContextJson> {
        unreachable!()
    }
    fn list_sessions(&self, _: &str) -> anyhow::Result<Vec<(String, String)>> {
        unreachable!()
    }
    fn create_session(&self, _: &str, _: &str, _: Option<&str>) -> anyhow::Result<()> {
        unreachable!()
    }
    fn rename_session(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        unreachable!()
    }
    fn delete_session(&self, _: &str, _: &str) -> anyhow::Result<()> {
        unreachable!()
    }
    fn context_command(&self, _: &str, _: &str) -> Result<String, String> {
        unreachable!()
    }
    fn mirror_message(&self, _: MirroredMessage) -> anyhow::Result<()> {
        unreachable!()
    }
    fn clear_messages(&self, _: &str) -> anyhow::Result<()> {
        unreachable!()
    }
    fn clear_session_messages(&self, _: &str, _: &str) -> anyhow::Result<()> {
        unreachable!()
    }
    async fn stop_agent(&self, _: &str) {
        unreachable!()
    }
    async fn question_sender(&self, _: &str) -> Option<mpsc::UnboundedSender<String>> {
        unreachable!()
    }
    async fn interaction_reply(&self, _: &str, _: &str, _: &str) -> bool {
        unreachable!()
    }
    async fn interaction_reaction(&self, _: &str, _: &str, _: &str) {
        unreachable!()
    }
    fn events(&self) -> broadcast::Receiver<ChannelEvent> {
        self.receiver.lock().unwrap().take().unwrap()
    }
    fn dashboard_event(&self, _: &str, _: &str, _: &str) {
        unreachable!()
    }
    fn tool_enabled(&self, _: &str) -> anyhow::Result<bool> {
        unreachable!()
    }
    fn skill_search(&self, _: &Path, _: &str) -> anyhow::Result<SkillPage> {
        unreachable!()
    }
    fn skill_browse(&self, _: &Path, _: &str) -> anyhow::Result<SkillPage> {
        unreachable!()
    }
    fn registered_skills(&self) -> Vec<SkillSummary> {
        unreachable!()
    }
    fn registered_skill(&self, _: &str) -> bool {
        unreachable!()
    }
    fn skill_exists(&self, _: &Path, _: &str) -> anyhow::Result<()> {
        unreachable!()
    }
    fn root(&self) -> &Path {
        unreachable!()
    }
}

fn test_host(receiver: broadcast::Receiver<ChannelEvent>) -> Arc<dyn ChannelHost> {
    Arc::new(TestHost {
        receiver: std::sync::Mutex::new(Some(receiver)),
    })
}

#[test]
fn credential_debug_redacts_every_value_in_both_formats() {
    let credentials = DiscordCredentials {
        bot_token: Some("bot-secret-sentinel".into()),
        application_id: Some("application-id-sentinel".into()),
        elevenlabs_api_key: Some("audio-secret-sentinel".into()),
    };
    for debug in [format!("{credentials:?}"), format!("{credentials:#?}")] {
        for secret in [
            "bot-secret-sentinel",
            "application-id-sentinel",
            "audio-secret-sentinel",
        ] {
            assert!(!debug.contains(secret), "credential Debug leaked a value");
        }
        assert!(debug.contains("DiscordCredentials"));
        assert!(debug.contains("[REDACTED]"));
    }
    assert!(format!("{:?}", DiscordCredentials::default()).contains("None"));
}

#[tokio::test]
async fn lagged_file_upload_uses_startup_http_credential_and_preserves_attachment() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = serenity::http::HttpBuilder::new("scoped-startup-token")
        .proxy(format!("http://{}", listener.local_addr().unwrap()))
        .ratelimiter_disabled(true)
        .build();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0, "request ended before attachment arrived");
            request.extend_from_slice(&buffer[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .expect("multipart length")
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 39\r\nConnection: close\r\n\r\n{\"code\":50035,\"message\":\"test rejection\"}").await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("upload.txt");
    tokio::fs::write(&path, b"attachment-content-sentinel")
        .await
        .unwrap();
    let (tx, rx) = broadcast::channel(1);
    for _ in 0..4 {
        tx.send(ChannelEvent::ChannelMessage {
            user_id: "user".into(),
            channel_id: "invalid".into(),
            message: "discarded".into(),
        })
        .unwrap();
    }
    tx.send(ChannelEvent::FileUpload {
        user_id: "user".into(),
        channel_id: "123".into(),
        file_path: path.to_str().unwrap().into(),
        file_name: "visible.txt".into(),
    })
    .unwrap();
    drop(tx);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        listen_for_events(test_host(rx), Arc::new(http)),
    )
    .await
    .expect("upload failure must not prevent closed-stream shutdown");
    let request = tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(request.starts_with("POST /api/v10/channels/123/messages "));
    assert!(request
        .to_ascii_lowercase()
        .contains("authorization: bot scoped-startup-token\r\n"));
    assert!(request.contains("filename=\"visible.txt\""));
    assert!(request.contains("attachment-content-sentinel"));
}

#[tokio::test]
async fn missing_upload_file_fails_before_network() {
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = serenity::http::HttpBuilder::new("unused")
        .proxy(format!("http://{}", listener.local_addr().unwrap()))
        .ratelimiter_disabled(true)
        .build();
    let error = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        upload_file(
            &http,
            serenity::all::ChannelId::new(123),
            directory.path().join("missing").to_str().unwrap(),
            "missing.txt",
        ),
    )
    .await
    .unwrap()
    .expect_err("must not upload an empty attachment for an unreadable file");
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err(),
        "missing file must not cause a network request"
    );
}

#[tokio::test]
async fn closed_delivery_stream_terminates() {
    let (tx, rx) = broadcast::channel(1);
    drop(tx);
    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        listen_for_events(test_host(rx), Arc::new(serenity::http::Http::new("unused"))),
    )
    .await
    .expect("closed delivery stream must terminate, not retry forever");
}

#[tokio::test]
async fn lagged_delivery_stream_drains_remaining_events_then_terminates() {
    let (tx, rx) = broadcast::channel(1);
    for _ in 0..4 {
        tx.send(ChannelEvent::ChannelMessage {
            user_id: "user".into(),
            channel_id: "invalid".into(),
            message: "ignored".into(),
        })
        .unwrap();
    }
    drop(tx);
    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        listen_for_events(test_host(rx), Arc::new(serenity::http::Http::new("unused"))),
    )
    .await
    .expect("lag must not prevent draining and shutdown");
}
