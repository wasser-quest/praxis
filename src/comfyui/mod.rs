//! The ComfyUI media client lives in `crates/praxis-comfyui`; this maps the
//! host's typed settings onto its provider settings and keeps the kernel's
//! call sites free of provider configuration details.
pub use praxis_comfyui::{config, qwen3, workflows, ComfyUiClient, DownloadedFile};

/// Map the host's typed context settings onto the provider's plain settings.
/// Empty typed values are left unset so legacy/environment fallbacks apply.
pub fn settings_from(
    typed: &crate::db::contexts::ContextSettings,
) -> praxis_comfyui::config::Settings {
    let set = |value: &Option<String>| {
        value
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    praxis_comfyui::config::Settings {
        comfyui_base_url: set(&typed.comfyui_base_url),
        comfyui_tts_workflow: set(&typed.comfyui_tts_workflow),
        comfyui_xtts_reference_audio: set(&typed.comfyui_xtts_reference_audio),
        comfyui_xtts_language: set(&typed.comfyui_xtts_language),
        comfyui_timeout_seconds: typed.comfyui_timeout_seconds.map(|seconds| seconds as u64),
        comfyui_tts_language_mode: set(&typed.comfyui_tts_language_mode),
        qwen_tts_language: set(&typed.qwen_tts_language),
        comfyui_qwen_reference_audio: set(&typed.comfyui_qwen_reference_audio),
        comfyui_qwen_reference_text: set(&typed.comfyui_qwen_reference_text),
        comfyui_qwen_language: set(&typed.comfyui_qwen_language),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_map_typed_values_and_keep_typed_json_validation() {
        let typed: crate::db::contexts::ContextSettings = serde_json::from_value(json!({
            "comfyui_base_url": "http://100.80.1.2:8188",
            "comfyui_tts_workflow": "workflows/tts-api.json",
            "comfyui_xtts_language": "de",
            "comfyui_timeout_seconds": 300,
            "comfyui_tts_language_mode": "de-ja",
            "qwen_tts_language": "French"
        }))
        .unwrap();
        let settings = settings_from(&typed);
        assert_eq!(settings.comfyui_base_url.as_deref(), Some("http://100.80.1.2:8188"));
        assert_eq!(settings.comfyui_timeout_seconds, Some(300));
        assert_eq!(settings.comfyui_tts_language_mode.as_deref(), Some("de-ja"));
        assert_eq!(settings.qwen_tts_language.as_deref(), Some("French"));
        let config = config::ComfyUiConfig::from_settings(&settings, None).unwrap();
        assert_eq!(config.language, "de-ja");
        assert_eq!(config.timeout, std::time::Duration::from_secs(300));
        // Empty typed values fall back like an unset one.
        let empty: crate::db::contexts::ContextSettings =
            serde_json::from_value(json!({"comfyui_xtts_language": ""})).unwrap();
        assert!(settings_from(&empty).comfyui_xtts_language.is_none());
        // The host's typed settings keep rejecting mistyped values.
        for invalid in [
            json!({"comfyui_xtts_language": false}),
            json!({"comfyui_timeout_seconds": -1}),
            json!({"comfyui_timeout_seconds": 1.5}),
            json!({"comfyui_tts_language_mode": true}),
            json!({"comfyui_qwen_reference_text": 42}),
        ] {
            assert!(serde_json::from_value::<crate::db::contexts::ContextSettings>(invalid).is_err());
        }
    }
}
