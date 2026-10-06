//! Per-user settings -> legacy custom_data -> process environment -> defaults.
use anyhow::{ensure, Context as _};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

/// The typed settings a host maps from its own configuration. Kept plain so a
/// media provider never depends on host types.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub comfyui_base_url: Option<String>,
    pub comfyui_tts_workflow: Option<String>,
    pub comfyui_xtts_reference_audio: Option<String>,
    pub comfyui_xtts_language: Option<String>,
    pub comfyui_timeout_seconds: Option<u64>,
    pub comfyui_tts_language_mode: Option<String>,
    pub qwen_tts_language: Option<String>,
    pub comfyui_qwen_reference_audio: Option<String>,
    pub comfyui_qwen_reference_text: Option<String>,
    pub comfyui_qwen_language: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ComfyUiConfig {
    pub base_url: String,
    pub tts_workflow: PathBuf,
    pub reference_audio: String,
    pub language: String,
    pub timeout: Duration,
}

impl ComfyUiConfig {
    pub fn from_settings(
        settings: &Settings,
        legacy: Option<&Value>,
    ) -> anyhow::Result<Self> {
        Self::resolve(settings, legacy, |name| std::env::var(name).ok())
    }

    fn resolve(
        settings: &Settings,
        legacy: Option<&Value>,
        env: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        let setting = |typed, key, variable, default| {
            string_setting(typed, legacy, key, variable, default, &env)
        };
        let timeout = timeout_setting(settings, legacy, &env)?;
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
                "workflows/tts-api.json",
            )?
            .into(),
            reference_audio: setting(
                settings.comfyui_xtts_reference_audio.as_deref(),
                "comfyui_xtts_reference_audio",
                "COMFYUI_XTTS_REFERENCE_AUDIO",
                "reference.wav",
            )?,
            language: effective_language(settings, legacy, &env, || {
                setting(
                    settings.comfyui_xtts_language.as_deref(),
                    "comfyui_xtts_language",
                    "COMFYUI_XTTS_LANGUAGE",
                    "en",
                )
            })?,
            timeout,
        })
    }
}

pub(super) fn string_setting(
    typed: Option<&str>,
    legacy: Option<&Value>,
    key: &str,
    variable: &str,
    default: &str,
    env: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<String> {
    if let Some(value) = typed.filter(|v| !v.is_empty()) {
        return Ok(value.to_string());
    }
    if let Some(value) = legacy.and_then(|ctx| ctx.get(key)).filter(|v| !v.is_null()) {
        let value = value
            .as_str()
            .with_context(|| format!("Legacy custom_data.{key} must be a string"))?;
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    Ok(env(variable)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string()))
}

pub(super) fn timeout_setting(
    settings: &Settings,
    legacy: Option<&Value>,
    env: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Duration> {
    let seconds = if let Some(seconds) = settings.comfyui_timeout_seconds {
        seconds as u64
    } else if let Some(value) = legacy
        .and_then(|v| v.get("comfyui_timeout_seconds"))
        .filter(|v| !v.is_null())
    {
        value
            .as_u64()
            .context("Legacy custom_data.comfyui_timeout_seconds must be a positive integer")?
    } else {
        env("COMFYUI_TIMEOUT_SECONDS")
            .unwrap_or_else(|| "900".into())
            .parse::<u64>()
            .context("COMFYUI_TIMEOUT_SECONDS must be a positive integer")?
    };
    ensure!(
        (1..=3600).contains(&seconds),
        "ComfyUI timeout must be 1–3600 seconds"
    );
    Ok(Duration::from_secs(seconds))
}

pub(super) fn effective_language(
    settings: &Settings,
    legacy: Option<&Value>,
    env: &impl Fn(&str) -> Option<String>,
    single: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    let mode = string_setting(
        settings.comfyui_tts_language_mode.as_deref(),
        legacy,
        "comfyui_tts_language_mode",
        "COMFYUI_TTS_LANGUAGE_MODE",
        "single",
        env,
    )?;
    match mode.as_str() {
        "single" => single(),
        "de-ja" => Ok(mode),
        _ => anyhow::bail!("ComfyUI speech language mode must be single or de-ja"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comfyui_settings_override_legacy_and_environment_per_field() {
        let settings = Settings {
            comfyui_base_url: Some("http://100.80.1.2:8188".into()),
            comfyui_tts_workflow: Some("/local/tts.json".into()),
            comfyui_xtts_reference_audio: Some("voices/person.wav".into()),
            comfyui_xtts_language: Some("de".into()),
            comfyui_tts_language_mode: Some("single".into()),
            comfyui_timeout_seconds: Some(120),
            qwen_tts_language: Some("French".into()),
            ..Default::default()
        };
        // Even malformed legacy values are irrelevant when settings override.
        let legacy = json!({"comfyui_base_url": false, "comfyui_tts_workflow": [],
            "comfyui_xtts_reference_audio": 42, "comfyui_xtts_language": "es", "comfyui_timeout_seconds": 0});
        let config = ComfyUiConfig::resolve(&settings, Some(&legacy), |_| {
            panic!("must not read environment for overridden fields")
        })
        .unwrap();
        assert_eq!(config.base_url, "http://100.80.1.2:8188");
        assert_eq!(config.tts_workflow, PathBuf::from("/local/tts.json"));
        assert_eq!(config.reference_audio, "voices/person.wav");
        assert_eq!(config.language, "de");
        assert_eq!(config.timeout, Duration::from_secs(120));
        assert_eq!(settings.qwen_tts_language.as_deref(), Some("French"));
    }

    #[test]
    fn comfyui_legacy_configuration_and_environment_fallback_remain_supported() {
        let settings = Settings::default();
        assert!(settings.comfyui_base_url.is_none());
        assert!(settings.comfyui_tts_workflow.is_none());
        assert!(settings.comfyui_xtts_reference_audio.is_none());
        assert!(settings.comfyui_xtts_language.is_none());
        assert!(settings.comfyui_timeout_seconds.is_none());
        let legacy = json!({"comfyui_xtts_language": "de", "comfyui_xtts_reference_audio": "voices/person.wav",
            "comfyui_timeout_seconds": 120});
        let config = ComfyUiConfig::resolve(&settings, Some(&legacy), |name| match name {
            "COMFYUI_XTTS_LANGUAGE" => Some("es".into()),
            "COMFYUI_BASE_URL" => Some("http://100.80.1.2:8188".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.language, "de");
        assert_eq!(config.reference_audio, "voices/person.wav");
        assert_eq!(config.base_url, "http://100.80.1.2:8188");
        assert_eq!(config.timeout, Duration::from_secs(120));
        let defaults = ComfyUiConfig::resolve(&settings, None, |_| None).unwrap();
        assert_eq!(defaults.language, "en");
        assert_eq!(defaults.timeout, Duration::from_secs(900));
        assert_eq!(
            defaults.tts_workflow,
            PathBuf::from("workflows/tts-api.json")
        );
        assert_eq!(defaults.reference_audio, "reference.wav");
    }

    #[test]
    fn comfyui_empty_settings_inherit_without_hiding_invalid_timeouts() {
        let settings = Settings {
            comfyui_base_url: Some(String::new()),
            comfyui_xtts_language: Some(String::new()),
            ..Default::default()
        };
        let legacy = json!({"comfyui_base_url": "", "comfyui_xtts_language": "fr"});
        let config = ComfyUiConfig::resolve(&settings, Some(&legacy), |name| match name {
            "COMFYUI_BASE_URL" => Some("http://100.80.1.2:8188".into()),
            "COMFYUI_TIMEOUT_SECONDS" => Some("300".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.base_url, "http://100.80.1.2:8188");
        assert_eq!(config.language, "fr");
        assert_eq!(config.timeout, Duration::from_secs(300));
        for seconds in [0, 3601] {
            let invalid = Settings {
                comfyui_timeout_seconds: Some(seconds),
                ..Default::default()
            };
            assert!(ComfyUiConfig::resolve(
                &invalid,
                Some(&json!({"comfyui_timeout_seconds": 120})),
                |_| None
            )
            .is_err());
        }
        for legacy in [
            json!({"comfyui_timeout_seconds": 1.5}),
            json!({"comfyui_xtts_language": false}),
        ] {
            assert!(ComfyUiConfig::resolve(&settings, Some(&legacy), |_| None).is_err());
        }
        assert!(ComfyUiConfig::resolve(&settings, Some(&json!([])), |_| None).is_ok());
    }

}