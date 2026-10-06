//! Qwen3-TTS output rejoins native RVC, persistence, dashboard and Discord playback.
use praxis_comfyui::{
    qwen3::{self, Qwen3TtsConfig},
    workflows::TTS_OUTPUT_NODE,
    ComfyUiClient,
};
use tokio_util::sync::CancellationToken;

pub async fn speak(
    config: &Qwen3TtsConfig,
    text: &str,
    cancellation: Option<&CancellationToken>,
) -> anyhow::Result<Vec<u8>> {
    let graph = qwen3::workflow(config, text).await?;
    let client = ComfyUiClient::new(&config.base_url, config.timeout)?;
    let directory = tempfile::tempdir()?;
    client
        .execute(&graph, TTS_OUTPUT_NODE, directory.path(), cancellation)
        .await?
        .into_bytes()
        .await
}
