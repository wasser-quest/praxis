//! OpenAI-compatible endpoints with optional explicit private-pipe credential grants.
//! CLI arguments carry configuration, never credentials. Stdout is protocol-only.
use praxis_provider_api::{
    worker::{serve_once, serve_once_with_credentials, SessionConfig},
    WireCapabilities, WireLimits,
};
use praxis_provider_openai::OpenAIProvider;
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
    let authenticated = args.len() == 3 && args[2] == "--credentials";
    if (args.len() != 2 && !authenticated)
        || args
            .iter()
            .any(|value| value.is_empty() || value.len() > 4096)
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
    let config = SessionConfig {
        provider_id: "openai".into(),
        capabilities: WireCapabilities {
            supports_output_limit: true,
            supports_list_models: false,
            supports_embeddings: false,
        },
        limits: WireLimits::default(),
        deadline: Duration::from_secs(300),
    };
    if authenticated {
        return serve_once_with_credentials(
            &config,
            &["api_key".into()],
            tokio::io::stdin(),
            tokio::io::stdout(),
            &CancellationToken::new(),
            |grant| {
                let key = grant
                    .get("api_key")
                    .filter(|key| {
                        !key.is_empty() && key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
                    })
                    .ok_or_else(|| {
                        praxis_provider_api::ProviderError::new(
                            praxis_provider_api::ErrorKind::InvalidRequest,
                        )
                    })?;
                Ok(OpenAIProvider::new(
                    key.to_owned(),
                    args[1].clone(),
                    args[0].trim_end_matches('/').into(),
                ))
            },
        )
        .await
        .map_err(|_| ());
    }
    let provider = OpenAIProvider::new(
        String::new(),
        args[1].clone(),
        args[0].trim_end_matches('/').into(),
    );
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
