use serde::{Deserialize, Serialize};
use std::sync::RwLock;

static SECRETS: RwLock<Option<Secrets>> = RwLock::new(None);
/// Master key retained by the running gateway so `/login` and token refreshes
/// can persist credentials without prompting. Process memory only; never
/// exported to env, argv or child processes. Explicit opt-in only:
/// `PRAXIS_RETAIN_MASTER_KEY=1`. Without it, logins live until restart.
static MASTER_KEY: RwLock<Option<zeroize::Zeroizing<String>>> = RwLock::new(None);

/// Tests that mutate the process-global store serialize on this lock.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn retention_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub fn retain_master_password(password: &str) {
    let enabled = retention_enabled(std::env::var("PRAXIS_RETAIN_MASTER_KEY").ok().as_deref());
    if let Ok(mut guard) = MASTER_KEY.write() {
        // Clearing/replacing also zeroizes the old allocation.
        *guard = enabled.then(|| zeroize::Zeroizing::new(password.to_string()));
    }
}

pub fn master_key_retained() -> bool {
    MASTER_KEY.read().is_ok_and(|g| g.is_some())
}

/// Save to the encrypted store when the gateway holds the master key.
/// Returns Ok(false) when it cannot persist (in-memory update only).
pub fn persist_if_unlocked(secrets: &Secrets) -> anyhow::Result<bool> {
    let guard = MASTER_KEY.read().map_err(|_| anyhow::anyhow!("Master key lock unavailable"))?;
    match guard.as_ref() {
        Some(key) => {
            save_secrets(secrets, key)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Secrets {
    #[serde(default)]
    pub discord_bot_token: Option<String>,
    #[serde(default)]
    pub minimax_api_key: Option<String>,
    #[serde(default)]
    pub mimo_api_key: Option<String>,
    #[serde(default)]
    pub openai_api_key: Option<String>,
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    #[serde(default)]
    pub openrouter_api_key: Option<String>,
    #[serde(default)]
    pub ollama_api_key: Option<String>,
    #[serde(default)]
    pub llamacpp_api_key: Option<String>,
    #[serde(default)]
    pub elevenlabs_api_key: Option<String>,
    #[serde(default)]
    pub gateway_api_key: Option<String>,
    #[serde(default)]
    pub dashboard_admin_password: Option<String>,
    #[serde(default)]
    pub voice_elevenlabs_api_key: Option<String>,
    #[serde(default)]
    pub voice_elevenlabs_stt_api_key: Option<String>,
    #[serde(default)]
    pub custom: std::collections::HashMap<String, String>,
}

impl Secrets {
    /// Resolve a declared plugin credential without serializing the full store.
    /// Explicit custom values win; empty/onboarding placeholders never shadow
    /// existing native media credentials. Native gateway/admin secrets stay private.
    pub fn plugin_secret(&self, key: &str) -> Option<&str> {
        let usable = |value: &&str| !value.trim().is_empty() && value.trim() != "CHANGE_ME";
        self.custom.get(key).map(String::as_str).filter(usable).or_else(|| {
            match key {
                "elevenlabs_api_key" => self.elevenlabs_api_key.as_deref(),
                "openrouter_api_key" => self.openrouter_api_key.as_deref(),
                _ => None,
            }.filter(usable)
        })
    }
}

pub fn init_secrets(secrets: Secrets) {
    if let Ok(mut guard) = SECRETS.write() {
        *guard = Some(secrets);
    }
}

/// Atomic live-store mutation and persistence. Callers never publish a failed
/// disk write, nor overwrite unrelated credentials from an old snapshot.
pub fn update_secrets(update: impl FnOnce(&mut Secrets) -> anyhow::Result<()>) -> anyhow::Result<(Secrets, bool)> {
    let mut guard = SECRETS.write().map_err(|_| anyhow::anyhow!("Secret store lock unavailable"))?;
    let mut next = guard.clone().unwrap_or_default();
    update(&mut next)?;
    let persisted = persist_if_unlocked(&next)?;
    *guard = Some(next.clone());
    Ok((next, persisted))
}

/// OAuth rotation cannot be rolled back if disk persistence fails: keep the
/// new credentials in the live store, serialize writes, and let the caller warn.
/// Returning false from the closure means a stale callback made no change.
pub fn update_runtime_secrets(update: impl FnOnce(&mut Secrets) -> bool) -> anyhow::Result<bool> {
    let mut guard = SECRETS.write().map_err(|_| anyhow::anyhow!("Secret store lock unavailable"))?;
    let current = guard.get_or_insert_with(Secrets::default);
    if !update(current) { return Ok(false); }
    persist_if_unlocked(current)
}

/// Fallible authoritative snapshot for registry authorization. Never turn a
/// poisoned store into successful anonymous authorization.
pub(crate) fn registry_chat_snapshot(names: &[String]) -> Result<Secrets, praxis_provider_api::ProviderError> {
    let guard = SECRETS.try_read()
        .map_err(|_| praxis_provider_api::ProviderError::new(praxis_provider_api::ErrorKind::Configuration))?;
    let mut scoped = Secrets::default();
    if let Some(current) = guard.as_ref() {
        for name in names {
            if let Some(value) = current.plugin_secret(name) {
                scoped.custom.insert(name.clone(), value.to_owned());
            }
        }
    }
    Ok(scoped)
}

pub fn get_secrets() -> Secrets {
    SECRETS
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

pub fn load_secrets_with_password(password: &str) -> anyhow::Result<Secrets> {
    if super::enc2::has_encrypted_secrets() {
        let json = super::enc2::load_encrypted_secrets(password)?;
        let secrets: Secrets = serde_json::from_str(&json)?;
        tracing::info!("Loaded encrypted secrets");
        return Ok(secrets);
    }

    Err(anyhow::anyhow!(
        "No encrypted secrets found. Run 'praxis onboard --interactive' to set up."
    ))
}

pub fn save_secrets(secrets: &Secrets, password: &str) -> anyhow::Result<()> {
    let content = serde_json::to_string(secrets)?;
    super::enc2::save_encrypted_secrets(&content, password)?;
    tracing::info!("Saved encrypted secrets");
    Ok(())
}

pub fn has_secrets() -> bool {
    super::enc2::has_encrypted_secrets()
}

pub fn mask_secret(secret: &Option<String>) -> String {
    match secret {
        Some(s) if s.chars().count() > 4 => format!("***{}", s.chars().skip(s.chars().count() - 4).collect::<String>()),
        Some(_) => "***".to_string(),
        None => "".to_string(),
    }
}

pub fn migrate_plaintext_to_encrypted(password: &str) -> anyhow::Result<()> {
    let path = std::path::Path::new("secrets.json");
    if !path.exists() {
        tracing::info!("No plaintext secrets.json to migrate");
        return Ok(());
    }

    let content = std::fs::read_to_string(path)?;
    let secrets: Secrets = serde_json::from_str(&content)?;

    save_secrets(&secrets, password)?;

    let backup = "secrets.json.migrated";
    std::fs::rename(path, backup)?;
    tracing::info!(
        "Migrated secrets.json to encrypted format, backup at {}",
        backup
    );

    Ok(())
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn master_key_retention_requires_explicit_opt_in() {
        for value in [None, Some(""), Some("0"), Some("false"), Some("typo")] {
            assert!(!retention_enabled(value));
        }
        for value in [Some("1"), Some("true"), Some("TRUE")] {
            assert!(retention_enabled(value));
        }
    }

    #[test]
    fn secret_updates_are_atomic_on_validation_failure() {
        let _lock = test_lock();
        let old = get_secrets();
        let (saved, _) = update_secrets(|s| { s.custom.insert("atomic-test".into(), "before".into()); Ok(()) }).unwrap();
        assert_eq!(saved.custom["atomic-test"], "before");
        assert!(update_secrets(|s| { s.custom.clear(); anyhow::bail!("invalid update") }).is_err());
        assert_eq!(get_secrets().custom["atomic-test"], "before");
        init_secrets(old);
    }

    #[test]
    fn test_secrets_default() {
        let secrets = Secrets::default();
        assert!(secrets.openai_api_key.is_none());
        assert!(secrets.discord_bot_token.is_none());
        assert!(secrets.minimax_api_key.is_none());
    }

    #[test]
    fn test_secrets_serialization() {
        let secrets = Secrets {
            discord_bot_token: Some("token".to_string()),
            minimax_api_key: Some("key".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&secrets).unwrap();
        let deserialized: Secrets = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.discord_bot_token, Some("token".to_string()));
        assert_eq!(deserialized.minimax_api_key, Some("key".to_string()));
    }

    #[test]
    fn test_mask_secret() {
        assert_eq!(mask_secret(&None), "");
        assert_eq!(mask_secret(&Some("abc".to_string())), "***");
        assert_eq!(mask_secret(&Some("日本語の秘密".to_string())), "***語の秘密");
        assert_eq!(
            mask_secret(&Some("abcdefghijklmnop".to_string())),
            "***mnop"
        );
    }

    #[test]
    fn test_secrets_roundtrip() {
        let secrets = Secrets {
            discord_bot_token: Some("token123".to_string()),
            minimax_api_key: Some("key456".to_string()),
            mimo_api_key: Some("mimo789".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_string(&secrets).unwrap();
        let loaded: Secrets = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.discord_bot_token, secrets.discord_bot_token);
        assert_eq!(loaded.minimax_api_key, secrets.minimax_api_key);
        assert_eq!(loaded.mimo_api_key, secrets.mimo_api_key);
    }

    #[test]
    fn test_secrets_partial() {
        let json = r#"{"discord_bot_token":"token"}"#;
        let secrets: Secrets = serde_json::from_str(json).unwrap();
        assert_eq!(secrets.discord_bot_token, Some("token".to_string()));
        assert!(secrets.minimax_api_key.is_none());
        assert!(secrets.mimo_api_key.is_none());
    }
}
