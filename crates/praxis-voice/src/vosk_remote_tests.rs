use super::*;
use crate::{pcm_to_wav, transcribe_audio, STTConfig};
use tokio_tungstenite::{accept_async, tungstenite::Message};

fn stt_config(url: String) -> STTConfig {
    STTConfig {
        engine: "vosk".into(),
        api_key: None,
        model_path: None,
        vosk_url: Some(url),
        elevenlabs_model: "scribe_v2".into(),
        elevenlabs_language: None,
        elevenlabs_tag_audio_events: false,
        elevenlabs_no_verbatim: true,
    }
}

async fn listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    (listener, url)
}

#[tokio::test]
async fn vosk_remote_works_without_local_model_library_or_api_key() {
    let (listener, url) = listener().await;
    let samples: Vec<i16> = (0..6000).map(|n| (n % 2000) as i16).collect();
    let expected = samples.clone();
    let server = tokio::spawn(async move {
        let mut socket = accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        let config: Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap();
        assert_eq!(config, json!({"config": {"sample_rate": 16000}}));
        let mut bytes = Vec::new();
        for text in [
            json!({"partial": "do not duplicate"}),
            json!({"text": "hello"}),
        ] {
            match socket.next().await.unwrap().unwrap() {
                Message::Binary(chunk) => bytes.extend(chunk),
                other => panic!("expected PCM not {other:?}"),
            }
            socket.send(Message::Text(text.to_string())).await.unwrap();
        }
        assert_eq!(
            bytes,
            expected
                .iter()
                .flat_map(|n| n.to_le_bytes())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_text().unwrap(),
            r#"{"eof":1}"#
        );
        socket
            .send(Message::Text(json!({"text": "world"}).to_string()))
            .await
            .unwrap();
    });
    let text = transcribe_audio(&pcm_to_wav(&samples, 16000, 1), &stt_config(url))
        .await
        .unwrap();
    assert_eq!(text, "hello world");
    server.await.unwrap();
}

#[tokio::test]
async fn vosk_remote_reuses_wav_downmix_and_sends_actual_sample_rate() {
    let (listener, url) = listener().await;
    let server = tokio::spawn(async move {
        let mut socket = accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        let config: Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap();
        assert_eq!(config["config"]["sample_rate"], 48000);
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_data(),
            [1500i16, -500]
                .iter()
                .flat_map(|n| n.to_le_bytes())
                .collect::<Vec<_>>()
        );
        socket
            .send(Message::Text(r#"{"partial":""}"#.into()))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_text().unwrap(),
            r#"{"eof":1}"#
        );
        socket
            .send(Message::Text(r#"{"text":""}"#.into()))
            .await
            .unwrap();
    });
    let wav = pcm_to_wav(&[1000, 2000, -1000, 0], 48000, 2);
    assert_eq!(
        VoskRemote::new(&url, Duration::from_secs(2))
            .unwrap()
            .transcribe(&wav, None)
            .await
            .unwrap(),
        ""
    );
    server.await.unwrap();
}

#[tokio::test]
async fn vosk_remote_rejects_bad_responses_and_disconnects_without_local_fallback() {
    for body in [
        Some("not JSON"),
        Some("[]"),
        Some(r#"{"text":42}"#),
        Some(r#"{"error":"private detail"}"#),
        None,
    ] {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            let mut socket = accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap();
            socket.next().await.unwrap().unwrap(); // config
            socket.next().await.unwrap().unwrap(); // PCM
            if let Some(body) = body {
                socket.send(Message::Text(body.into())).await.unwrap();
            }
        });
        let error = transcribe_audio(&pcm_to_wav(&[1, 2, 3], 16000, 1), &stt_config(url))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Vosk remote"), "{error}");
        assert!(!error.contains("model path"));
        assert!(!error.contains("private detail"));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn vosk_remote_timeout_and_cancel_drop_only_the_owned_connection() {
    for cancel in [false, true] {
        let (listener, url) = listener().await;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut socket = accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap();
            socket.next().await.unwrap().unwrap();
            socket.next().await.unwrap().unwrap();
            ready_tx.send(()).unwrap();
            // Client closes/drops this connection; there is no shared interrupt API.
            let _ = socket.next().await;
        });
        let client = VoskRemote::new(&url, Duration::from_millis(200)).unwrap();
        let token = CancellationToken::new();
        let audio = pcm_to_wav(&[1, 2], 16000, 1);
        let result = client.transcribe(&audio, Some(&token));
        let stop = async {
            ready_rx.await.unwrap();
            if cancel {
                token.cancel();
            }
        };
        let (result, _) = tokio::join!(result, stop);
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(if cancel { "cancelled" } else { "timed out" }),
            "{error}"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn vosk_remote_rejects_unsupported_audio_before_connect_and_connection_failure() {
    let (listener, url) = listener().await;
    let client = VoskRemote::new(&url, Duration::from_secs(1)).unwrap();
    for audio in [
        vec![],
        vec![0; 100],
        b"OggSunsupported browser audio".to_vec(),
        pcm_to_wav(&[0; 4], 16000, 4),
    ] {
        assert!(client.transcribe(&audio, None).await.is_err());
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    drop(listener);
    assert!(client
        .transcribe(&pcm_to_wav(&[1, 2], 16000, 1), None)
        .await
        .unwrap_err()
        .to_string()
        .contains("connection"));
}

#[test]
fn vosk_remote_url_and_context_defaults() {
    assert!(VoskRemote::new("ws://100.80.1.2:2700", Duration::from_secs(1)).is_ok());
    assert!(VoskRemote::new("wss://vosk.example.org/asr", Duration::from_secs(1)).is_ok());
    for url in [
        "http://100.80.1.2:2700",
        "ws://user:pass@100.80.1.2",
        "ws://100.80.1.2/#fragment",
        "ws://100.80.1.2/?token=secret",
    ] {
        assert!(VoskRemote::new(url, Duration::from_secs(1)).is_err());
    }
}
