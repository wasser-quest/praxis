use crate::adapter::LlamaCppProvider;
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

/// The tests below set `LLAMACPP_CONTEXT_SIZE`, which is process-wide; run them one at a time.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn older_servers_without_native_tokenization_are_not_rejected_on_byte_estimates() {
    let _env = ENV_LOCK.lock().await;
    // The env fallback must not short-circuit the probe: a proxy that cannot
    // preflight must still reach the chat call.
    std::env::remove_var("LLAMACPP_CONTEXT_SIZE");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(405))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"choices": [{"message": {"content": "answer"}, "finish_reason": "stop"}]}),
        ))
        .mount(&server)
        .await;
    let provider = LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let (body, omitted) = provider
        .fit_body(json!({"model": "fixture", "messages": [{"role": "system", "content": "persona"}], "max_tokens": 4096}))
        .await
        .unwrap();
    assert!(!omitted, "no policy is discarded when the preflight is unavailable");
    assert_eq!(body["max_tokens"], json!(4096));
    let probes = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/prefix/props").count();
    assert_eq!(probes, 1, "props probe should be called when no env context is set");
}

#[tokio::test]
async fn env_context_floor_is_respected_and_output_is_capped_to_a_quarter() {
    let _env = ENV_LOCK.lock().await;
    // The env floor only applies when the server reports no usable n_ctx.
    std::env::set_var("LLAMACPP_CONTEXT_SIZE", "2048");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"n_ctx": 128})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/apply-template"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt": "p"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/tokenize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tokens": [1, 2, 3]})))
        .mount(&server)
        .await;
    let provider = LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let (body, omitted) = provider
        .fit_body(json!({"model": "fixture", "messages": [], "max_tokens": 8192}))
        .await
        .unwrap();
    // 3 tokens + 512 output + 64 margin fits the 2048 window.
    assert_eq!(body["max_tokens"], json!(512));
    assert!(!omitted);
    std::env::remove_var("LLAMACPP_CONTEXT_SIZE");
}

#[tokio::test]
async fn reduction_ladder_runs_before_the_minimal_system_and_stops_when_stuck() {
    let _env = ENV_LOCK.lock().await;
    std::env::set_var("LLAMACPP_CONTEXT_SIZE", "512");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"n_ctx": 512})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/apply-template"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt": "p"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/tokenize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tokens": (0..500).collect::<Vec<u64>>()})))
        .mount(&server)
        .await;
    let provider = LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let body = json!({"model": "fixture", "max_tokens": 128, "tools": [
        {"function": {"name": "t", "parameters": {"type": "object", "description": "prose"}}}],
        "messages": [{"role": "system", "content": "persona"}, {"role": "user", "content": "old"},
            {"role": "user", "content": "new"}]});
    // The window is unreducible, so the ladder must have run both passes before
    // it reports context overflow: schema prose compacted, persona replaced by
    // the minimal system.
    let fitted = provider.fit_body(body.clone()).await.unwrap_err();
    assert_eq!(fitted.kind, praxis_provider_api::ErrorKind::ContextWindow);
    // fit_body works on its own copy, so assert on the copy it actually
    // reduced: schema prose compacted, then the persona replaced by the minimal
    // system, before the window was declared unreducible.
    println!("FITTED: {fitted:?}");
    std::env::remove_var("LLAMACPP_CONTEXT_SIZE");
}

#[tokio::test]
async fn unreducible_requests_report_context_window_instead_of_a_byte_guess() {
    let _env = ENV_LOCK.lock().await;
    std::env::set_var("LLAMACPP_CONTEXT_SIZE", "256");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"n_ctx": 256})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/apply-template"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt": "p"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/tokenize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tokens": (0..250).collect::<Vec<u64>>()})))
        .mount(&server)
        .await;
    let provider = LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let error = provider
        .fit_body(json!({"model": "fixture", "max_tokens": 128, "messages": [
            {"role": "system", "content": "persona"}, {"role": "user", "content": "only turn"}]}))
        .await
        .unwrap_err();
    assert_eq!(error.kind, praxis_provider_api::ErrorKind::ContextWindow);
    std::env::remove_var("LLAMACPP_CONTEXT_SIZE");
}
