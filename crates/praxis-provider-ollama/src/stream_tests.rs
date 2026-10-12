//! Offline regression tests for text that will be spoken in Discord.
use super::{OllamaProvider, OllamaStreamState};
use crate::provider::LLMProvider;
use std::cell::RefCell;

#[test]
fn voice_reply_survives_empty_final_chunk_and_split_utf8() {
    let mut stream = OllamaStreamState::default();
    let spoken = RefCell::new(String::new());
    let on_token = |token: String| spoken.borrow_mut().push_str(&token);
    let frames = concat!(
        "{\"message\":{\"thinking\":\"not spoken\"},\"done\":false}\n",
        "{\"message\":{\"content\":\"Français: bonjour. 日本語。\"},\"done\":false}\n",
        "{\"message\":{\"content\":\"\"},\"done\":true}\n"
    );
    for byte in frames.as_bytes() {
        stream.push(&[*byte], &on_token).unwrap();
    }
    stream.finish(&on_token).unwrap();
    assert_eq!(stream.content, "Français: bonjour. 日本語。");
    assert_eq!(*spoken.borrow(), stream.content);
    assert!(stream.done);
}

#[test]
fn voice_reply_handles_final_delta_or_final_snapshot() {
    for final_text in [" !", "Bonjour !"] {
        let mut stream = OllamaStreamState::default();
        let output = RefCell::new(String::new());
        let on_token = |token: String| output.borrow_mut().push_str(&token);
        stream
            .push(
                b"{\"message\":{\"content\":\"Bonjour\"},\"done\":false}\n",
                &on_token,
            )
            .unwrap();
        // The final record may have no trailing newline.
        let end = serde_json::json!({"message": {"content": final_text}, "done": true}).to_string();
        stream.push(end.as_bytes(), &on_token).unwrap();
        stream.finish(&on_token).unwrap();
        assert_eq!(stream.content, "Bonjour !");
        assert_eq!(*output.borrow(), "Bonjour !");
    }
}

#[test]
fn voice_stream_keeps_tools_before_done_without_stopping_early() {
    let mut stream = OllamaStreamState::default();
    let tool_frame = serde_json::json!({"message": {"tool_calls": [{"function": {
        "name": "get_context", "arguments": {}
    }}]}, "done": false})
    .to_string()
        + "\n";
    stream.push(tool_frame.as_bytes(), &|_| {}).unwrap();
    assert!(!stream.done);
    assert_eq!(stream.tool_calls.len(), 1);
    stream
        .push(
            b"{\"message\":{\"content\":\"Ready\"},\"done\":false}\n{\"done\":true}\n",
            &|_| {},
        )
        .unwrap();
    stream.finish(&|_| {}).unwrap();
    assert_eq!(stream.content, "Ready");
    assert_eq!(stream.tool_calls[0].function.name, "get_context");
}

#[test]
fn voice_stream_reports_errors_instead_of_speaking_partial_success() {
    let mut stream = OllamaStreamState::default();
    assert!(stream
        .push(b"{\"error\":\"mock unavailable model\"}\n", &|_| {})
        .is_err());
    let mut stream = OllamaStreamState::default();
    assert!(stream.push(b"not-json\n", &|_| {}).is_err());
    let mut stream = OllamaStreamState::default();
    assert!(stream.push(&[0xff, b'\n'], &|_| {}).is_err());
    let mut stream = OllamaStreamState::default();
    stream
        .push(
            b"{\"message\":{\"content\":\"incomplete\"},\"done\":false}\n",
            &|_| {},
        )
        .unwrap();
    assert!(stream.finish(&|_| {}).is_err());
}

#[tokio::test]
async fn voice_provider_health_check_requires_success_and_sends_optional_auth() {
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };
    for (status, healthy) in [(200, true), (401, false), (500, false)] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .and(header("authorization", "Bearer local-test-only"))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        let mut provider = OllamaProvider::new(
            server.uri(),
            "test-only-model".into(),
            Some("local-test-only".into()),
        );
        provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(provider.health_check().await, healthy);
    }
}
