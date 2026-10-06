//! Qwen3-TTS Base through the same private, bounded ComfyUI transport as XTTS.
use super::config::{effective_language, string_setting, timeout_setting};
use super::workflows;
use crate::config::Settings;
use anyhow::ensure;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

// No derived Debug: the optional reference transcript is private speech data.
pub struct Qwen3TtsConfig {
    pub base_url: String,
    pub tts_workflow: PathBuf,
    pub reference_audio: String,
    pub reference_text: String,
    pub language: String,
    pub timeout: Duration,
}

impl Qwen3TtsConfig {
    pub fn from_settings(
        settings: &Settings,
        legacy: Option<&Value>,
    ) -> anyhow::Result<Self> {
        Self::resolve(settings, legacy, |name| std::env::var(name).ok())
    }

    pub(super) fn resolve(
        settings: &Settings,
        legacy: Option<&Value>,
        env: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        let setting = |typed, key, variable, default| {
            string_setting(typed, legacy, key, variable, default, &env)
        };
        Ok(Self {
            base_url: setting(
                settings.comfyui_base_url.as_deref(),
                "comfyui_base_url",
                "COMFYUI_BASE_URL",
                "",
            )?,
            tts_workflow: setting(
                settings.comfyui_tts_workflow.as_deref(),
                "comfyui_tts_workflow",
                "COMFYUI_TTS_WORKFLOW",
                "workflows/tts-qwen3-api.json",
            )?
            .into(),
            reference_audio: setting(
                settings.comfyui_qwen_reference_audio.as_deref(),
                "comfyui_qwen_reference_audio",
                "COMFYUI_QWEN_REFERENCE_AUDIO",
                "reference.wav",
            )?,
            reference_text: setting(
                settings.comfyui_qwen_reference_text.as_deref(),
                "comfyui_qwen_reference_text",
                "COMFYUI_QWEN_REFERENCE_TEXT",
                "",
            )?,
            language: effective_language(settings, legacy, &env, || {
                setting(
                    settings.comfyui_qwen_language.as_deref(),
                    "comfyui_qwen_language",
                    "COMFYUI_QWEN_LANGUAGE",
                    "auto",
                )
            })?,
            timeout: timeout_setting(settings, legacy, &env)?,
        })
    }
}

pub async fn workflow(config: &Qwen3TtsConfig, text: &str) -> anyhow::Result<Value> {
    ensure!(
        !text.trim().is_empty() && text.chars().count() <= 5000,
        "Qwen3 TTS text must contain 1–5000 characters"
    );
    ensure!(
        ["auto", "en", "zh", "ja", "ko", "de", "fr", "ru", "pt", "es", "it", "de-ja"]
            .contains(&config.language.as_str()),
        "Unsupported Qwen3 TTS language code"
    );
    ensure!(
        config.reference_text.chars().count() <= 5000,
        "Qwen3 reference transcript exceeds 5000 characters"
    );
    workflows::validate_reference(&config.reference_audio)?;
    let mut graph = workflows::load(&config.tts_workflow).await?;
    ensure!(graph["1"]["class_type"] == "PraxisQwen3TTS" && ["text", "language", "reference_audio", "reference_text"].iter().all(|key| graph["1"]["inputs"].get(key).is_some()),
        "Qwen3 workflow must retain node 1 (PraxisQwen3TTS) and inputs text, language, reference_audio, reference_text");
    graph["1"]["inputs"]["text"] = json!(text);
    graph["1"]["inputs"]["language"] = json!(config.language);
    graph["1"]["inputs"]["reference_audio"] = json!(config.reference_audio);
    graph["1"]["inputs"]["reference_text"] = json!(config.reference_text);
    Ok(graph)
}
