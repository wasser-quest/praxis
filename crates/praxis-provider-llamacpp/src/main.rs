//! Anonymous one-shot llama.cpp worker. Configuration only; stdout is protocol-only.
use praxis_provider_api::{
    worker::{serve_once, SessionConfig},
    WireCapabilities, WireLimits,
};
use praxis_provider_llamacpp::LlamaCppProvider;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        // No request, URL, upstream body or credentials in diagnostics.
        eprintln!("provider worker failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2
        || args.iter().any(|value| {
            value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
        })
    {
        return Err(());
    }
    let url = reqwest::Url::parse(&args[0]).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    let provider = LlamaCppProvider::new(None, args[1].clone(), args[0].clone());
    let config = SessionConfig {
        provider_id: "llamacpp".into(),
        capabilities: WireCapabilities {
            supports_output_limit: true,
            supports_list_models: true,
            supports_embeddings: false,
        },
        limits: WireLimits::default(),
        deadline: Duration::from_secs(300),
    };
    serve_once(
        &provider,
        &config,
        tokio::io::stdin(),
        tokio::io::stdout(),
        &CancellationToken::new(),
    )
    .await
    .map_err(|_| ())
}
