//! XTTS adapter for the deployment's API-format workflow. No image integration.
use super::{config::ComfyUiConfig, safe_relative};
use anyhow::{ensure, Context as _};
use serde_json::{json, Value};
use std::path::Path;
use tokio::io::AsyncReadExt;

pub const TTS_OUTPUT_NODE: &str = "1";
pub const XTTS_LANGUAGES: &[&str] = &[
    "en", "es", "fr", "de", "it", "pt", "pl", "tr", "ru", "nl", "cs", "ar", "zh-cn", "hu", "ko",
    "ja", "hi", "de-ja",
];

pub(super) async fn load(path: &Path) -> anyhow::Result<Value> {
    const LIMIT: u64 = 1024 * 1024;
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Cannot open local ComfyUI workflow {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).await?;
    ensure!(bytes.len() as u64 <= LIMIT, "Workflow exceeds 1 MiB");
    let graph: Value = serde_json::from_slice(&bytes).context("Invalid workflow JSON")?;
    let nodes = graph
        .as_object()
        .context("Expected API-format workflow object, not a UI workflow")?;
    ensure!(
        !nodes.is_empty()
            && nodes
                .values()
                .all(|node| node["class_type"].is_string() && node["inputs"].is_object()),
        "Expected API-format nodes with class_type and inputs in {} (export API format, not the GUI workflow)", path.display()
    );
    Ok(graph)
}

pub async fn xtts(config: &ComfyUiConfig, text: &str) -> anyhow::Result<Value> {
    ensure!(
        !text.trim().is_empty() && text.chars().count() <= 5000,
        "XTTS text must contain 1–5000 characters"
    );
    ensure!(
        XTTS_LANGUAGES.contains(&config.language.as_str()),
        "Unsupported XTTS language code (use en, de, es, fr, etc., not language names)"
    );
    validate_reference(&config.reference_audio)?;
    let mut graph = load(&config.tts_workflow).await?;
    ensure!(
        graph[TTS_OUTPUT_NODE]["class_type"] == "PraxisXTTS"
            && ["text", "language", "reference_audio"]
                .iter()
                .all(|field| graph[TTS_OUTPUT_NODE]["inputs"].get(field).is_some()),
        "Workflow must retain node 1 (PraxisXTTS) and inputs text, language, reference_audio"
    );
    graph[TTS_OUTPUT_NODE]["inputs"]["text"] = json!(text);
    graph[TTS_OUTPUT_NODE]["inputs"]["language"] = json!(config.language);
    graph[TTS_OUTPUT_NODE]["inputs"]["reference_audio"] = json!(config.reference_audio);
    // No upload endpoint, local reference-file reads, or license acceptance.
    Ok(graph)
}

pub(super) fn validate_reference(reference: &str) -> anyhow::Result<()> {
    ensure!(safe_relative(reference), "reference_audio must be relative to ComfyUI's SERVER input directory, not a local Praxis path");
    let extension = Path::new(reference)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    ensure!(
        ["wav", "mp3", "flac", "ogg"].contains(&extension.as_str()),
        "Unsupported speech reference audio extension"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ComfyUiConfig {
        ComfyUiConfig {
            base_url: "http://127.0.0.1:8188".into(),
            tts_workflow: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workflows/tts-api.json"),
            reference_audio: "voices/person.wav".into(),
            language: "de".into(),
            timeout: std::time::Duration::from_secs(900),
        }
    }

    #[tokio::test]
    async fn comfyui_workflow_populates_text_language_and_server_reference() {
        let config = config();
        let graph = xtts(&config, "Grüße aus Praxis").await.unwrap();
        assert_eq!(graph["1"]["class_type"], "PraxisXTTS");
        assert_eq!(
            graph["1"]["inputs"],
            json!({"text": "Grüße aus Praxis", "language": "de", "reference_audio": "voices/person.wav"})
        );
        assert_eq!(
            serde_json::from_str::<Value>(include_str!("../../../workflows/tts-api.json")).unwrap()
                ["1"]["inputs"]["text"],
            "Hello from Praxis."
        );
    }

    #[tokio::test]
    async fn comfyui_workflow_inputs_and_templates_fail_before_submission() {
        let mut config = config();
        assert!(xtts(&config, " ").await.is_err());
        assert!(xtts(&config, &"a".repeat(5001)).await.is_err());
        config.language = "German".into();
        assert!(xtts(&config, "Hallo").await.is_err());
        config.language = "en".into();
        for path in [
            "/workspace/input/reference.wav",
            "../reference.wav",
            "C:\\reference.wav",
            "a/%2e%2e/ref.wav",
            "reference.exe",
        ] {
            config.reference_audio = path.into();
            assert!(xtts(&config, "Hello").await.is_err());
        }
        config.reference_audio = "reference.wav".into();
        let file = tempfile::NamedTempFile::new().unwrap();
        config.tts_workflow = file.path().into();
        for body in [
            json!({"nodes": []}),
            json!({"1": {"class_type": "WrongNode", "inputs": {}}}),
            json!({"1": {"class_type": "PraxisXTTS", "inputs": {"text": "hi", "language": "en"}}}),
        ] {
            std::fs::write(file.path(), body.to_string()).unwrap();
            assert!(xtts(&config, "Hello").await.is_err());
        }
    }
}
