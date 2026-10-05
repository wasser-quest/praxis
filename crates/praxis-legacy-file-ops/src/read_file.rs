//! Shared text-file capture: never silently cut at the old 10,000-character mark.
use tokio::io::AsyncReadExt;

pub async fn run(path: &str) -> anyhow::Result<String> {
    let limit = praxis_plugin_api::executable::MAX_RESULT_BYTES;
    let meta = tokio::fs::metadata(path).await?;
    anyhow::ensure!(meta.is_file(), "read_file requires a regular text file");
    anyhow::ensure!(meta.len() <= limit as u64, "File exceeds the 8 MiB capture limit; request a bounded file range with an appropriate read-only tool instead");
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).await?;
    anyhow::ensure!(
        bytes.len() <= limit,
        "File grew beyond the 8 MiB capture limit; no truncated text was returned"
    );
    Ok(String::from_utf8(bytes)?)
}
