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
