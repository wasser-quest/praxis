//! End-to-end smoke check of the Rust fixture worker using owned OS pipes.
//! Build both examples, then run this executable alongside fixture_worker.
use praxis_provider_api::{
    bridge, exchange, handshake::WorkerIdentity, process::WorkerLaunch, CallBinding,
    RequestContext, RequestEnvelope, WireCapabilities, WireChatRequest, WireErrorCode, WireLimits,
    WIRE_VERSION,
};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    for mode in ["success", "error", "overflow"] {
        check(mode).await?;
    }
    println!("Rust worker real-pipe smoke checks passed: success, error, overflow");
    Ok(())
}

async fn check(mode: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut executable = std::env::current_exe()?;
    executable.set_file_name(format!("fixture_worker{}", std::env::consts::EXE_SUFFIX));
    let limits = WireLimits::default();
    let context = RequestContext {
        binding: CallBinding {
            provider: "fixture".into(),
            worker_generation: 1,
            call_id: [1; 16],
        },
        request_id: [2; 16],
        nonce: [3; 16],
    };
    let launch = WorkerLaunch {
        executable,
        arguments: vec![mode.into()],
        environment: BTreeMap::new(),
        identity: WorkerIdentity {
            provider: "fixture".into(),
            generation: 1,
            nonce: context.nonce,
        },
        capabilities: WireCapabilities {
            supports_output_limit: false,
            supports_list_models: false,
            supports_embeddings: false,
        },
        limits,
    };
    let request = RequestEnvelope {
        version: WIRE_VERSION,
        context: context.clone(),
        continuation: None,
        request: WireChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: None,
            model: None,
            thinking: None,
        },
    };
    let deltas = Mutex::new(Vec::new());
    let result = bridge::invoke(
        launch,
        &request,
        Duration::from_secs(5),
        &CancellationToken::new(),
        &|delta| deltas.lock().unwrap().push(delta),
    )
    .await;
    match mode {
        "success" => assert_eq!(
            result.unwrap().response.content.as_deref(),
            Some("fixture answer")
        ),
        "error" => assert!(
            matches!(result, Err(bridge::InvocationError::Exchange(exchange::ExchangeError::Provider(error))) if error.code == WireErrorCode::Transport)
        ),
        _ => assert!(matches!(
            result,
            Err(bridge::InvocationError::Exchange(
                exchange::ExchangeError::MissingTerminal
            ))
        )),
    }
    let deltas = deltas.into_inner().unwrap();
    assert_eq!(deltas.len(), if mode == "overflow" { 0 } else { 2 });
    for (i, delta) in deltas.iter().enumerate() {
        assert!(
            matches!(delta, praxis_provider_api::StreamDelta::Text { text } if text == &i.to_string())
        );
    }
    Ok(())
}
