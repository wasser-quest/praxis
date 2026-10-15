//! Local HTTP contract tests. Never use real credentials or provider endpoints.
use super::{elevenlabs_stt::ElevenLabsSTT, pcm_to_wav, tts};
use tts::elevenlabs::{ElevenLabsTTS, ElevenLabsVoiceSettings};
use wiremock::{
    matchers::{body_partial_json, header, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

const KEY: &str = "local-test-key-not-a-real-credential";

#[tokio::test]
async fn tts_request_preserves_zero_settings_and_requests_mp3() {
    let server = MockServer::start().await;
    let audio = b"mock-audio-payload".to_vec();
    Mock::given(method("POST"))
        .and(path("/v1/text-to-speech/test-voice/stream"))
        .and(query_param("output_format", "mp3_44100_128"))
        .and(header("xi-api-key", KEY))
        .and(header("accept", "audio/mpeg"))
        .and(body_partial_json(serde_json::json!({
            "text": "Bonjour !", "model_id": "eleven_multilingual_v2",
            "language_code": "fr", "voice_settings": {
                "stability": 0.0, "similarity_boost": 0.0, "style": 0.0, "speed": 0.8
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(audio.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let client = ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), server.uri());
    let settings = ElevenLabsVoiceSettings {
        stability: 0.0,
        similarity_boost: 0.0,
        style: Some(0.0),
        speed: Some(0.8),
        language: Some("fr".into()),
    };
    let result = client
        .speak_with_settings("Bonjour !", "eleven_multilingual_v2", &settings)
        .await
        .unwrap();
    assert_eq!(result, audio);
}

#[tokio::test]
async fn tts_speed_boundaries_are_serialized_without_float_widening() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"mock audio".to_vec()))
        .expect(2)
        .mount(&server)
        .await;
    let client = ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), server.uri());
    for speed in [0.7_f32, 1.2_f32] {
        let settings = ElevenLabsVoiceSettings {
            speed: Some(speed),
            ..Default::default()
        };
        client
            .speak_with_settings("Bonjour", "eleven_multilingual_v2", &settings)
            .await
            .unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    for (request, expected) in requests.iter().zip([0.7, 1.2]) {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["voice_settings"]["speed"].as_f64(), Some(expected));
        assert!(body.get("language_code").is_none());
        assert!(body["voice_settings"].get("style").is_none());
    }
}

#[test]
fn tts_defaults_are_explicit() {
    let settings = ElevenLabsVoiceSettings::default();
    assert_eq!(settings.stability, 0.5);
    assert_eq!(settings.similarity_boost, 0.75);
    assert!(settings.language.is_none());
}

#[tokio::test]
async fn tts_rejects_invalid_settings_without_a_request() {
    let server = MockServer::start().await;
    let client = ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), server.uri());
    let invalid = [
        ElevenLabsVoiceSettings {
            stability: -0.1,
            ..Default::default()
        },
        ElevenLabsVoiceSettings {
            similarity_boost: 1.1,
            ..Default::default()
        },
        ElevenLabsVoiceSettings {
            style: Some(f32::NAN),
            ..Default::default()
        },
        ElevenLabsVoiceSettings {
            speed: Some(2.0),
            ..Default::default()
        },
    ];
    for settings in invalid {
        assert!(client
            .speak_with_settings("Bonjour", "eleven_multilingual_v2", &settings)
            .await
            .is_err());
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn tts_surfaces_http_errors_and_rejects_empty_audio() {
    for status in [401, 422, 200] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        let client = ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), server.uri());
        assert!(client.speak("Bonjour").await.is_err());
    }
}

#[tokio::test]
async fn stt_sends_mono_wav_and_configured_language_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/speech-to-text"))
        .and(header("xi-api-key", KEY))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "Bonjour !"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = ElevenLabsSTT::with_base_url(KEY.into(), server.uri());
    let wav = pcm_to_wav(&[1000, -1000, 2000, -2000], 16000, 1);
    let text = client
        .transcribe_with_config(&wav, "scribe_v2", Some("fr"), false, true)
        .await
        .unwrap();
    assert_eq!(text, "Bonjour !");
    let requests = server.received_requests().await.unwrap();
    let request = &requests[0];
    let body = String::from_utf8_lossy(&request.body);
    for (name, value) in [
        ("model_id", "scribe_v2"),
        ("language_code", "fr"),
        ("tag_audio_events", "false"),
        ("no_verbatim", "true"),
    ] {
        assert!(
            body.contains(&format!("name=\"{name}\"\r\n\r\n{value}\r\n")),
            "missing multipart field {name}"
        );
    }
    assert!(body.contains("filename=\"audio.wav\""));
    assert!(request.body.windows(wav.len()).any(|chunk| chunk == wav));
}

#[tokio::test]
async fn stt_auto_detects_language_and_accepts_silence() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": ""})))
        .expect(1)
        .mount(&server)
        .await;
    let client = ElevenLabsSTT::with_base_url(KEY.into(), server.uri());
    let wav = pcm_to_wav(&[0; 1600], 16000, 1);
    assert_eq!(client.transcribe(&wav).await.unwrap(), "");
    let requests = server.received_requests().await.unwrap();
    assert!(!String::from_utf8_lossy(&requests[0].body).contains("name=\"language_code\""));
}

#[tokio::test]
async fn stt_reports_authentication_and_malformed_responses() {
    for (status, body) in [
        (401, serde_json::json!({"detail": "invalid key"})),
        (200, serde_json::json!({})),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let client = ElevenLabsSTT::with_base_url(KEY.into(), server.uri());
        assert!(client
            .transcribe(&pcm_to_wav(&[500; 1600], 16000, 1))
            .await
            .is_err());
    }
}

#[tokio::test]
async fn empty_keys_do_not_send_requests() {
    let server = MockServer::start().await;
    let stt = ElevenLabsSTT::with_base_url(" ".into(), server.uri());
    assert!(stt
        .transcribe(&pcm_to_wav(&[500; 1600], 16000, 1))
        .await
        .is_err());
    let tts = ElevenLabsTTS::with_base_url(" ".into(), "test-voice".into(), server.uri());
    assert!(tts.speak("Bonjour").await.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn assert_safe_failure(error: &str, url: &str) {
    for private in ["PRIVATE_BODY", KEY, url] {
        assert!(!error.contains(private), "unsafe error: {error}");
    }
}

#[tokio::test]
async fn failures_are_private_and_requests_are_not_retried() {
    for status in [401, 429, 500, 503] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status).set_body_string(format!("PRIVATE_BODY {KEY}")),
            )
            .expect(2)
            .mount(&server)
            .await;
        let stt = ElevenLabsSTT::with_base_url(KEY.into(), server.uri());
        let tts = ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), server.uri());
        assert_safe_failure(
            &stt.transcribe(b"audio").await.unwrap_err().to_string(),
            &server.uri(),
        );
        assert_safe_failure(
            &tts.speak("hello").await.unwrap_err().to_string(),
            &server.uri(),
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(request.headers.get("xi-api-key").unwrap(), KEY);
            assert!(!request.url.as_str().contains(KEY));
            assert!(!String::from_utf8_lossy(&request.body).contains(KEY));
            for (name, value) in &request.headers {
                if name != "xi-api-key" {
                    assert!(!value.to_str().unwrap().contains(KEY));
                }
            }
        }
    }
}

#[tokio::test]
async fn stt_json_errors_and_transport_errors_are_private() {
    for body in [
        format!(r#"{{"error":"PRIVATE_BODY {KEY}"}}"#),
        format!("PRIVATE_BODY {KEY}"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let stt = ElevenLabsSTT::with_base_url(KEY.into(), server.uri());
        assert_safe_failure(
            &stt.transcribe(b"audio").await.unwrap_err().to_string(),
            &server.uri(),
        );
    }
    let url = "http://127.0.0.1:1/PRIVATE_BODY";
    let stt = ElevenLabsSTT::with_base_url(KEY.into(), url.into());
    let tts = ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), url.into());
    assert_safe_failure(
        &stt.transcribe(b"audio").await.unwrap_err().to_string(),
        url,
    );
    assert_safe_failure(&tts.speak("hello").await.unwrap_err().to_string(), url);
}

#[tokio::test]
async fn oversized_success_with_content_length_is_rejected() {
    for is_stt in [true, false] {
        let server = MockServer::start().await;
        let limit = if is_stt {
            1024 * 1024
        } else {
            32 * 1024 * 1024
        };
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; limit + 1]))
            .expect(1)
            .mount(&server)
            .await;
        let error = if is_stt {
            ElevenLabsSTT::with_base_url(KEY.into(), server.uri())
                .transcribe(b"audio")
                .await
                .unwrap_err()
                .to_string()
        } else {
            ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), server.uri())
                .speak("hello")
                .await
                .unwrap_err()
                .to_string()
        };
        assert!(error.contains("response too large"), "{error}");
    }
}

// A deliberately unfinished chunked response: rejection must happen before EOF,
// without Content-Length. Wiremock supplies the other real HTTP contract tests.
#[tokio::test]
async fn chunked_success_and_errors_fail_without_waiting_for_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for is_stt in [true, false] {
        for status in [200, 503] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let limit = if is_stt {
                1024 * 1024
            } else {
                32 * 1024 * 1024
            };
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                // Consume headers and the complete small request before responding.
                let mut request = Vec::new();
                let mut buf = [0; 4096];
                loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length: "))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let headers =
                    format!("HTTP/1.1 {status} Test\r\nTransfer-Encoding: chunked\r\n\r\n");
                if socket.write_all(headers.as_bytes()).await.is_err() {
                    return;
                }
                let chunk = vec![b'x'; 64 * 1024];
                for _ in 0..=limit / chunk.len() {
                    if socket.write_all(b"10000\r\n").await.is_err() {
                        return;
                    }
                    if socket.write_all(&chunk).await.is_err() {
                        return;
                    }
                    if socket.write_all(b"\r\n").await.is_err() {
                        return;
                    }
                }
                std::future::pending::<()>().await;
            });
            let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                if is_stt {
                    ElevenLabsSTT::with_base_url(KEY.into(), url.clone())
                        .transcribe(b"audio")
                        .await
                        .unwrap_err()
                        .to_string()
                } else {
                    ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), url.clone())
                        .speak("hello")
                        .await
                        .unwrap_err()
                        .to_string()
                }
            })
            .await;
            task.abort();
            let error = result.expect("must reject before EOF, not aggregate the entire body");
            assert_safe_failure(&error, &url);
            if status == 200 {
                assert!(error.contains("response too large"), "{error}");
            }
        }
    }
}

#[tokio::test]
async fn redirects_are_not_followed_with_credentials() {
    let target = MockServer::start().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", target.uri()))
        .expect(2)
        .mount(&server)
        .await;
    assert!(ElevenLabsSTT::with_base_url(KEY.into(), server.uri())
        .transcribe(b"audio")
        .await
        .is_err());
    assert!(
        ElevenLabsTTS::with_base_url(KEY.into(), "test-voice".into(), server.uri())
            .speak("hello")
            .await
            .is_err()
    );
    assert!(target.received_requests().await.unwrap().is_empty());
}
