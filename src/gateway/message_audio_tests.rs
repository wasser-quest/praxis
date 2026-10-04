use super::*;
use base64::Engine;
use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};

#[test]
fn audio_mime_matches_provider_output_instead_of_always_mpeg() {
    assert_eq!(tts_audio_mime(b"RIFFxxxxWAVEdata"), "audio/wav");
    assert_eq!(tts_audio_mime(b"OggSfixture"), "audio/ogg");
    assert_eq!(tts_audio_mime(b"fLaCfixture"), "audio/flac");
    assert_eq!(tts_audio_mime(b"ID3fixture"), "audio/mpeg");
    assert_eq!(tts_audio_mime(b"RIFF"), "audio/mpeg"); // no out-of-bounds reads
}

#[tokio::test]
async fn audio_tts_is_saved_without_subscribers_and_notifications_reference_saved_bytes() {
    // Local synthetic Qwen endpoint only. No real model, credentials or TTS costs.
    let server = MockServer::start().await;
    let audio = b"RIFFxxxxWAVEsynthetic";
    Mock::given(method("POST")).and(path("/tts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "audio": base64::engine::general_purpose::STANDARD.encode(audio),
        })))
        .expect(2).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let user = format!("audio-test-{}", uuid::Uuid::new_v4());
    db.save_context(&db.load_context(&user).unwrap()).unwrap();
    let settings = crate::db::contexts::ContextSettings {
        voice_tts_type: "qwen_tts".into(),
        qwen_tts_server: Some(server.uri()),
        web_chat_tts: true,
        ..Default::default()
    };
    let mut ctx = db.load_context(&user).unwrap();
    ctx.settings = settings.clone();
    db.save_context(&ctx).unwrap();
    for subscribed in [false, true] {
        let mut rx = subscribed.then(|| crate::runtime::events::get_or_create(&user).subscribe());
        let id = db.add_message(&user, &crate::db::messages::Message::assistant("fixture reply".into())).unwrap();
        spawn_tts("fixture reply".into(), &settings, &Default::default(), &user, &db, Some(id), Some("web"));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while db.get_message_audio(&user, id).unwrap().is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(db.get_message_audio(&user, id).unwrap(), Some(("audio/wav".into(), audio.to_vec())));
        if let Some(rx) = rx.as_mut() {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
            assert_eq!(event.event, "chat_tts");
            let payload: serde_json::Value = serde_json::from_str(&event.data).unwrap();
            assert_eq!(payload["message_id"], id);
            assert_eq!(payload["mime"], "audio/wav");
            assert!(payload.get("audio").is_none(), "SSE should not buffer huge base64 blobs for saved replies");
        }
    }
    crate::runtime::events::remove(&user);
}

#[tokio::test]
async fn audio_discord_still_synthesizes_when_web_tts_is_off() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/tts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "audio": base64::engine::general_purpose::STANDARD.encode(b"RIFFxxxxWAVEtest"),
        }))).expect(2).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    for channel in ["123456789", "voice:123456789"] {
        let user = format!("discord-independent-{}", uuid::Uuid::new_v4());
        let ctx = db.merge_context(&user, serde_json::json!({
            "settings.voice_tts_type":"qwen_tts", "settings.qwen_tts_server":server.uri(),
            "settings.web_chat_tts":false, "settings.use_tts":true,
        })).unwrap();
        let id = db.add_message(&user, &crate::db::messages::Message::assistant("Discord reply".into())).unwrap();
        spawn_tts("Discord reply".into(), &ctx.settings, &Default::default(), &user, &db, Some(id), Some(channel));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while db.get_message_audio(&user, id).unwrap().is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
    }
}

#[test]
fn audio_tts_permission_is_channel_specific() {
    for use_tts in [false, true] {
        for web_chat_tts in [false, true] {
            let settings = crate::db::contexts::ContextSettings {
                use_tts, web_chat_tts, voice_tts_enabled: true, ..Default::default()
            };
            assert_eq!(reply_tts_enabled(&settings, Some("web")), web_chat_tts);
            for channel in [None, Some(""), Some("123456789")] {
                assert_eq!(reply_tts_enabled(&settings, channel), use_tts);
            }
            assert!(reply_tts_enabled(&settings, Some("voice:123456789")));
        }
    }
}

#[test]
fn audio_web_off_blocks_stale_and_explicit_feedback_before_spawning() {
    // No Tokio runtime: trying to spawn any provider task fails this test.
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let settings = crate::db::contexts::ContextSettings {
        use_tts: true, web_chat_tts: true, ..Default::default()
    };
    spawn_tts("must not synthesize".into(), &settings, &Default::default(), "muted", &db, None, Some("web"));
}

#[tokio::test]
async fn audio_web_disabled_before_worker_runs_does_not_call_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/tts"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let user = format!("audio-before-worker-{}", uuid::Uuid::new_v4());
    let ctx = db.merge_context(&user, serde_json::json!({
        "settings.voice_tts_type": "qwen_tts",
        "settings.qwen_tts_server": server.uri(),
        "settings.web_chat_tts": true,
    })).unwrap();
    spawn_tts("not started yet".into(), &ctx.settings, &Default::default(), &user, &db, None, Some("web"));
    // The single-threaded test runtime cannot poll the spawned task until we yield.
    db.merge_context(&user, serde_json::json!({"settings.web_chat_tts": false})).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    server.verify().await;
}

#[test]
fn audio_context_saves_publish_only_web_tts_permission() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let user = format!("audio-settings-{}", uuid::Uuid::new_v4());
    let mut rx = crate::runtime::events::get_or_create(&user).subscribe();
    for (updates, enabled) in [
        (serde_json::json!({"settings": {"web_chat_tts": true}, "custom_data": {"private": "do not publish"}}), true),
        (serde_json::json!({"settings.web_chat_tts": false}), false),
        (serde_json::json!({"settings.use_tts": true}), false),
    ] {
        db.merge_context(&user, updates).unwrap();
        let event = rx.try_recv().expect("context edits must update connected dashboards");
        assert_eq!(event.event, "chat_tts_settings");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&event.data).unwrap(), serde_json::json!({"enabled": enabled}));
        assert!(rx.try_recv().is_err());
    }
    db.delete_context(&user).unwrap();
    assert_eq!(rx.try_recv().unwrap().data, "{\"enabled\":false}");
    crate::runtime::events::remove(&user);
}

#[tokio::test]
async fn audio_disable_during_synthesis_keeps_replay_without_late_notification() {
    let server = MockServer::start().await;
    let audio = b"RIFFxxxxWAVEsynthetic";
    Mock::given(method("POST")).and(path("/tts"))
        .respond_with(ResponseTemplate::new(200)
            .set_delay(std::time::Duration::from_millis(200))
            .set_body_json(serde_json::json!({
                "audio": base64::engine::general_purpose::STANDARD.encode(audio),
            })))
        .expect(2).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    // Cover both a web reply and Discord audio mirrored to the dashboard.
    for channel in ["web", "123456789"] {
        let user = format!("audio-disable-{}", uuid::Uuid::new_v4());
        let ctx = db.merge_context(&user, serde_json::json!({
            "settings.voice_tts_type": "qwen_tts",
            "settings.qwen_tts_server": server.uri(),
            "settings.web_chat_tts": true,
            "settings.use_tts": true,
        })).unwrap();
        let mut rx = crate::runtime::events::get_or_create(&user).subscribe();
        let id = db.add_message(&user, &crate::db::messages::Message::assistant("delayed".into())).unwrap();
        let previous_requests = server.received_requests().await.unwrap().len();
        spawn_tts("delayed".into(), &ctx.settings, &Default::default(), &user, &db, Some(id), Some(channel));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while server.received_requests().await.unwrap().len() == previous_requests {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        db.merge_context(&user, serde_json::json!({"settings.web_chat_tts": false})).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while db.get_message_audio(&user, id).unwrap().is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(db.get_message_audio(&user, id).unwrap(), Some(("audio/wav".into(), audio.to_vec())));
        assert_eq!(rx.recv().await.unwrap().event, "chat_tts_settings");
        assert!(tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await.is_err(),
            "late synthesis must not announce chat_tts after OFF");
        crate::runtime::events::remove(&user);
    }
}
