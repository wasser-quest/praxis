//! Secret store administration shared by the built-in dashboard and Host API
//! v1 (`secrets` scope). Reads are always masked; writes keep the
//! master-password and empty-self-credential guards.
use super::admin::{Failure, Outcome};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct SecretsInfo {
    /// Write-only auth.json import; never expose token suffixes from JSON.
    pub codex_auth: String,
    pub discord_bot_token: String,
    pub openai_api_key: String,
    pub anthropic_api_key: String,
    pub ollama_api_key: String,
    pub llamacpp_api_key: String,
    pub minimax_api_key: String,
    pub mimo_api_key: String,
    pub elevenlabs_api_key: String,
    pub gateway_api_key: String,
    pub dashboard_admin_password: String,
    #[serde(flatten)]
    pub custom: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
pub struct SecretsUpdate {
    /// CLI auth.json or normalized CodexAuth JSON. Empty removes, *** preserves.
    pub codex_auth: Option<String>,
    pub discord_bot_token: Option<String>,
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub ollama_api_key: Option<String>,
    pub llamacpp_api_key: Option<String>,
    pub minimax_api_key: Option<String>,
    pub mimo_api_key: Option<String>,
    pub elevenlabs_api_key: Option<String>,
    pub gateway_api_key: Option<String>,
    pub dashboard_admin_password: Option<String>,
    pub master_password: Option<String>,
    #[serde(flatten)]
    pub custom: std::collections::HashMap<String, String>,
}

/// Masked view; values are never returned.
pub fn masked() -> SecretsInfo {
    let secrets = crate::db::secrets::get_secrets();
    let custom_masked: std::collections::HashMap<String, String> = secrets
        .custom
        .iter()
        .filter(|(k, _)| k.as_str() != crate::gateway::llm::codex::SECRET_KEY)
        .map(|(k, v)| (k.clone(), crate::db::secrets::mask_secret(&Some(v.clone()))))
        .collect();
    SecretsInfo {
        codex_auth: if crate::gateway::llm::codex::CodexAuth::from_secrets(&secrets).is_some() { "***".into() } else { String::new() },
        discord_bot_token: crate::db::secrets::mask_secret(&secrets.discord_bot_token),
        openai_api_key: crate::db::secrets::mask_secret(&secrets.openai_api_key),
        anthropic_api_key: crate::db::secrets::mask_secret(&secrets.anthropic_api_key),
        ollama_api_key: crate::db::secrets::mask_secret(&secrets.ollama_api_key),
        llamacpp_api_key: crate::db::secrets::mask_secret(&secrets.llamacpp_api_key),
        minimax_api_key: crate::db::secrets::mask_secret(&secrets.minimax_api_key),
        mimo_api_key: crate::db::secrets::mask_secret(&secrets.mimo_api_key),
        elevenlabs_api_key: crate::db::secrets::mask_secret(&secrets.elevenlabs_api_key),
        gateway_api_key: crate::db::secrets::mask_secret(&secrets.gateway_api_key),
        dashboard_admin_password: crate::db::secrets::mask_secret(
            &secrets.dashboard_admin_password,
        ),
        custom: custom_masked,
    }
}

/// Apply an update. Empty self-credentials are ignored (lockout guard); with
/// `master_password` the store is persisted only if that password opens the
/// existing store. Returns the operator-facing status message.
pub fn update(update: SecretsUpdate) -> Outcome<String> {
    let mut secrets = crate::db::secrets::get_secrets();
    if let Some(auth) = update.codex_auth.as_deref() {
        apply_codex_secret(&mut secrets, auth).map_err(|e| Failure::BadRequest(e.to_string()))?;
    }
    // Felder, die wegen Leer-Werten übersprungen wurden (Selbst-Zugangs-
    // daten dürfen nie mit "" in den Store — sonst Login/Gateway-401).
    let mut skipped: Vec<&str> = Vec::new();

    if let Some(v) = update.discord_bot_token {
        secrets.discord_bot_token = Some(v);
    }
    if let Some(v) = update.openai_api_key {
        secrets.openai_api_key = Some(v);
    }
    if let Some(v) = update.anthropic_api_key {
        secrets.anthropic_api_key = Some(v);
    }
    if let Some(v) = update.ollama_api_key {
        secrets.ollama_api_key = Some(v);
    }
    if let Some(v) = update.llamacpp_api_key {
        secrets.llamacpp_api_key = Some(v);
    }
    if let Some(v) = update.minimax_api_key {
        secrets.minimax_api_key = Some(v);
    }
    if let Some(v) = update.mimo_api_key {
        secrets.mimo_api_key = Some(v);
    }
    if let Some(v) = update.elevenlabs_api_key {
        secrets.elevenlabs_api_key = Some(v);
    }
    if let Some(v) = update.gateway_api_key {
        // Selbst-Zugangsdaten: Leere Werte würden Login/Gateway still mit ""
        // in den Store schreiben (Store > Env) → Lockout/401 bis Store-Reset.
        // Leere = überspringen (Nichts senden = unverändert).
        if v.trim().is_empty() {
            skipped.push("gateway_api_key");
        } else {
            secrets.gateway_api_key = Some(v);
        }
    }
    if let Some(v) = update.dashboard_admin_password {
        if v.trim().is_empty() {
            skipped.push("dashboard_admin_password");
        } else {
            secrets.dashboard_admin_password = Some(v);
        }
    }

    for (k, v) in update.custom {
        if v.is_empty() {
            secrets.custom.remove(&k);
        } else {
            secrets.custom.insert(k, v);
        }
    }

    let skipped_note = if skipped.is_empty() {
        String::new()
    } else {
        format!(" (leere Werte ignoriert: {})", skipped.join(", "))
    };

    // Persist to enc2 if master password provided
    if let Some(ref password) = update.master_password {
        // Trim wie beim Start (MASTER_KEY_FILE wird beim Lesen getrimmt):
        // Copy-Paste-Zeilenümbrüche dürfen kein Re-Keying auslösen.
        let password = password.trim();
        // Guard: Bei vorhandenem Store MUSS das Feld den AKTUELLEN Master-Key
        // öffnen (echter Decrypt-Test). Ohne Check verschlüsselt save_secrets
        // den Store still mit einem evtl. falschen Wert NEU → nächster Start
        // „Invalid MASTER_KEY (hash mismatch)" (21.09. live passiert: Secret
        // im Dashboard geändert, Restart brickte).
        if crate::db::secrets::has_secrets() && !crate::db::enc2::verify_password(password) {
            return Ok("Falsches Master-Passwort — NICHTS gespeichert, Store unverändert. (Feld = exakter Inhalt von vps/master_key)".to_string());
        }
        crate::db::secrets::save_secrets(&secrets, password)?;
        crate::db::secrets::init_secrets(secrets.clone());
        reload_llm_router(&secrets);
        Ok(format!("Secrets saved and encrypted.{}", skipped_note))
    } else {
        crate::db::secrets::init_secrets(secrets.clone());
        reload_llm_router(&secrets);
        Ok(format!(
            "Secrets updated in memory. Provide master_password to persist to disk.{}",
            skipped_note
        ))
    }
}

/// Provider keys changed in the dashboard take effect without a restart.
pub fn reload_llm_router(secrets: &crate::db::secrets::Secrets) {
    if let Some(state) = crate::gateway::state_ref() {
        let config = crate::gateway::providers::effective_config(&state.config, secrets);
        state.llm.swap(crate::gateway::llm::LLMRouter::new(&config, secrets));
    }
}

pub fn apply_codex_secret(secrets: &mut crate::db::secrets::Secrets, value: &str) -> anyhow::Result<()> {
    use crate::gateway::llm::codex::{CodexAuth, SECRET_KEY};
    if value.starts_with("***") { return Ok(()); }
    if value.trim().is_empty() {
        secrets.custom.remove(SECRET_KEY);
    } else {
        CodexAuth::from_json(value)?.store(secrets);
    }
    Ok(())
}
