//! ComfyUI XTTS-v2 provider, returning WAV bytes to the unchanged native TTS,
//! RVC, persistence and Discord playback pipeline.
use praxis_comfyui::{config::ComfyUiConfig, workflows, ComfyUiClient};
use tokio_util::sync::CancellationToken;

pub async fn speak(
    config: &ComfyUiConfig,
    text: &str,
    cancellation: Option<&CancellationToken>,
) -> anyhow::Result<Vec<u8>> {
    let graph = workflows::xtts(config, text).await?;
    let client = ComfyUiClient::new(&config.base_url, config.timeout)?;
    // Speech staging is private and transient, not an extra dashboard upload.
    // The existing caller owns audio persistence and optional output copies.
    let directory = tempfile::tempdir()?;
    let output = client
        .execute(
            &graph,
            workflows::TTS_OUTPUT_NODE,
            directory.path(),
            cancellation,
        )
        .await?;
    output.into_bytes().await
}
