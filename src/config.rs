use std::env;

#[derive(Clone, Debug, PartialEq)]
pub enum ApiMode {
    OpenAI,
    Anthropic,
}

impl ApiMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "anthropic" => ApiMode::Anthropic,
            _ => ApiMode::OpenAI,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub poml_cli: String,
    pub use_provider: String,
    /// Explicit opt-in only: no automatic cross-provider data/cost fallback.
    pub llm_fallback_providers: Vec<String>,
    pub llm_resilience: crate::gateway::llm::resilience::ResilienceConfig,
    pub openai_api_key: Option<String>,
    pub openai_model: String,
    /// Model for the Codex (ChatGPT subscription) provider.
    pub codex_model: String,
    pub openai_api_base: String,
    pub anthropic_api_key: Option<String>,
    pub anthropic_model: String,
    pub anthropic_api_base: String,
    pub ollama_api_base: String,
    pub ollama_model: String,
    pub llamacpp_api_base: String,
    pub llamacpp_model: String,
    pub minimax_api_key: Option<String>,
    pub minimax_model: String,
    pub minimax_api_base: String,
    pub minimax_api_mode: ApiMode,
    pub mimo_api_key: Option<String>,
    pub mimo_model: String,
    pub mimo_api_base: String,
    pub mimo_api_mode: ApiMode,
    pub openrouter_api_key: Option<String>,
    pub openrouter_model: String,
    pub openrouter_api_base: String,
    pub vision_provider: Option<String>,
    pub vision_model: Option<String>,
    pub gateway_port: u16,
    pub gateway_api_key: String,
    pub dashboard_port: u16,
    pub dashboard_tls: bool,
    pub dashboard_admin_password: String,
    pub data_dir: String,
    /// Installation root directory (where templates/, contexts/ are located)
    pub root_dir: String,
    /// Optional operator-selected root for verified host actions, separate from assets.
    pub workspace_dir: Option<String>,
    pub rust_log: String,
    pub vm_enabled: bool,
    /// Optional installed worker; unset preserves the native compatibility backend.
    pub vm_service_executable: Option<String>,
    pub vm_cpu_cores: u32,
    pub vm_ram_mb: u32,
    pub vm_disk_size: String,
    pub vm_arch: String,
    pub vm_mode: String,        // "shared" or "vm"
    pub vm_socket_mode: String, // "unix" or "tcp" (auto-detected per OS if empty)
}

impl Config {
    /// Resolve verified host actions relative to installation assets, without
    /// changing the process cwd or accepting a model-selected project root.
    pub fn workspace_root(&self) -> anyhow::Result<std::path::PathBuf> {
        let install = std::path::Path::new(&self.root_dir);
        let selected = match self.workspace_dir.as_deref() {
            None => install.to_path_buf(),
            Some(value) => {
                anyhow::ensure!(!value.trim().is_empty(), "WORKSPACE_DIR must name an existing project directory");
                let path = std::path::Path::new(value);
                if path.is_absolute() { path.to_path_buf() } else { install.join(path) }
            }
        };
        let root = selected.canonicalize().map_err(|error| anyhow::anyhow!(
            "Cannot resolve verified workspace '{}': {error}. Set WORKSPACE_DIR to the prepared project directory; ROOT_DIR selects Praxis assets.", selected.display()
        ))?;
        anyhow::ensure!(root.is_dir(), "WORKSPACE_DIR must be a directory: {}", root.display());
        Ok(root)
    }

    pub fn from_env() -> Self {
        Self {
            poml_cli: env::var("POML_CLI").unwrap_or_else(|_| "./poml/js/cli.cjs".to_string()),
            use_provider: env::var("USE_PROVIDER").unwrap_or_else(|_| "openai".to_string()),
            llm_fallback_providers: env::var("LLM_FALLBACK_PROVIDERS").unwrap_or_default()
                .split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
            llm_resilience: crate::gateway::llm::resilience::ResilienceConfig::from_env(),
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            openai_model: env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
            codex_model: env::var("CODEX_MODEL").unwrap_or_else(|_| crate::gateway::llm::codex::DEFAULT_MODEL.to_string()),
            openai_api_base: env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            anthropic_api_key: env::var("ANTHROPIC_API_KEY").ok(),
            anthropic_model: env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| "claude-3-5-sonnet-20241022".to_string()),
            anthropic_api_base: env::var("ANTHROPIC_API_BASE")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            ollama_api_base: env::var("OLLAMA_API_BASE")
                .unwrap_or_else(|_| "http://localhost:11434".to_string()),
            ollama_model: env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3".to_string()),
            llamacpp_api_base: env::var("LLAMACPP_API_BASE")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            llamacpp_model: env::var("LLAMACPP_MODEL")
                .unwrap_or_else(|_| "llama.cpp".to_string()),
            minimax_api_key: env::var("MINIMAX_API_KEY").ok(),
            minimax_model: env::var("MINIMAX_MODEL")
                .unwrap_or_else(|_| "MiniMax-Text-01".to_string()),
            minimax_api_base: env::var("MINIMAX_API_BASE")
                .unwrap_or_else(|_| "https://api.minimax.chat/v1".to_string()),
            minimax_api_mode: ApiMode::from_str(
                &env::var("MINIMAX_API_MODE").unwrap_or_else(|_| "openai".to_string()),
            ),
            mimo_api_key: env::var("MIMO_API_KEY").ok(),
            mimo_model: env::var("MIMO_MODEL").unwrap_or_else(|_| "mimo".to_string()),
            mimo_api_base: env::var("MIMO_API_BASE")
                .unwrap_or_else(|_| "https://api.mimo.com/v1".to_string()),
            mimo_api_mode: ApiMode::from_str(
                &env::var("MIMO_API_MODE").unwrap_or_else(|_| "openai".to_string()),
            ),
            vision_provider: env::var("VISION_PROVIDER").ok(),
            vision_model: env::var("VISION_MODEL").ok(),
            openrouter_api_key: env::var("OPENROUTER_API_KEY").ok(),
            openrouter_model: env::var("OPENROUTER_MODEL")
                .unwrap_or_else(|_| "openai/gpt-4o".to_string()),
            openrouter_api_base: env::var("OPENROUTER_API_BASE")
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string()),
            gateway_port: env::var("GATEWAY_PORT")
                .unwrap_or_else(|_| "3537".to_string())
                .parse()
                .unwrap_or(3537),
            gateway_api_key: env::var("GATEWAY_API_KEY").unwrap_or_default(),
            dashboard_port: env::var("DASHBOARD_PORT")
                .unwrap_or_else(|_| "1337".to_string())
                .parse()
                .unwrap_or(1337),
            dashboard_tls: env::var("DASHBOARD_TLS")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            dashboard_admin_password: env::var("DASHBOARD_ADMIN_PASSWORD").unwrap_or_default(),
            data_dir: env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string()),
            root_dir: env::var("ROOT_DIR").unwrap_or_else(|_| ".".to_string()),
            workspace_dir: env::var("WORKSPACE_DIR").ok(),
            rust_log: env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
            vm_enabled: env::var("VM_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            vm_service_executable: env::var("VM_SERVICE_EXECUTABLE").ok()
                .map(|value| value.trim().to_owned()).filter(|value| !value.is_empty()),
            vm_cpu_cores: env::var("VM_CPU_CORES")
                .unwrap_or_else(|_| "2".to_string())
                .parse()
                .unwrap_or(2),
            vm_ram_mb: env::var("VM_RAM_MB")
                .unwrap_or_else(|_| "4096".to_string())
                .parse()
                .unwrap_or(4096),
            vm_disk_size: env::var("VM_DISK_SIZE").unwrap_or_else(|_| "40G".to_string()),
            vm_arch: env::var("VM_ARCH").unwrap_or_else(|_| "x86_64".to_string()),
            vm_mode: env::var("VM_MODE").unwrap_or_else(|_| "shared".to_string()),
            vm_socket_mode: env::var("VM_SOCKET_MODE").unwrap_or_else(|_| {
                if cfg!(target_os = "linux") {
                    "unix".to_string()
                } else {
                    "tcp".to_string()
                }
            }),
        }
    }

    pub fn apply_secrets(&mut self, secrets: &crate::db::secrets::Secrets) {
        if let Some(ref key) = secrets.gateway_api_key {
            if !key.is_empty() && self.gateway_api_key.is_empty() {
                self.gateway_api_key = key.clone();
            }
        }
        if let Some(ref pass) = secrets.dashboard_admin_password {
            if !pass.is_empty() && self.dashboard_admin_password.is_empty() {
                self.dashboard_admin_password = pass.clone();
            }
        }
    }

    pub fn ensure_generated(&mut self) {
        if self.gateway_api_key.is_empty() {
            let key = uuid::Uuid::new_v4().to_string();
            tracing::info!("GATEWAY_API_KEY not set, generated: {}", key);
            self.gateway_api_key = key;
        }
        if self.dashboard_admin_password.is_empty() {
            let pass = uuid::Uuid::new_v4().to_string()[..12].to_string();
            tracing::info!("DASHBOARD_ADMIN_PASSWORD not set, generated: {}", pass);
            self.dashboard_admin_password = pass;
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.llm_resilience.validate()?;
        if self.gateway_api_key.len() < 16 {
            anyhow::bail!("GATEWAY_API_KEY must be at least 16 characters");
        }
        if self.dashboard_admin_password.len() < 8 {
            anyhow::bail!("DASHBOARD_ADMIN_PASSWORD must be at least 8 characters");
        }
        Ok(())
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = Config::from_env();
        assert!(!config.data_dir.is_empty());
        assert!(!config.rust_log.is_empty());
    }

    #[test]
    fn test_validate_short_api_key() {
        let config = Config {
            gateway_api_key: "short".to_string(),
            dashboard_admin_password: "longpassword".to_string(),
            ..Config::from_env()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_short_password() {
        let config = Config {
            gateway_api_key: "a".repeat(20),
            dashboard_admin_password: "short".to_string(),
            ..Config::from_env()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_ok() {
        let config = Config {
            gateway_api_key: "a".repeat(20),
            dashboard_admin_password: "longpassword".to_string(),
            ..Config::from_env()
        };
        assert!(config.validate().is_ok());
    }
}

/// Choose the installation root holding templates/, contexts/, plugins/, skills/.
/// `ROOT_DIR` always wins (relative values resolve against the cwd). Without it,
/// prefer the cwd when it already contains templates/, then the executable's
/// directory when that does, else leave the process where it is.
pub fn resolve_install_root(
    env_root: Option<std::path::PathBuf>,
    cwd: Option<std::path::PathBuf>,
    exe_dir: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let has_assets = |dir: &std::path::Path| dir.join("templates").is_dir();
    if let Some(root) = env_root.filter(|r| !r.as_os_str().is_empty()) {
        let root = if root.is_absolute() { root } else { cwd.clone()?.join(root) };
        return root.canonicalize().ok().or(Some(root));
    }
    if let Some(cwd) = cwd.as_ref().filter(|d| has_assets(d)) {
        return cwd.canonicalize().ok();
    }
    if let Some(exe_dir) = exe_dir.filter(|d| has_assets(d)) {
        return exe_dir.canonicalize().ok();
    }
    None
}

#[cfg(test)]
mod install_root_tests {
    use super::resolve_install_root;

    #[test]
    fn install_root_prefers_env_then_cwd_assets_then_exe_dir() {
        let install = tempfile::tempdir().unwrap();
        std::fs::create_dir(install.path().join("templates")).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let canon = install.path().canonicalize().unwrap();

        // Started from an unrelated cwd: use the executable's directory.
        assert_eq!(resolve_install_root(None, Some(elsewhere.path().into()), Some(install.path().into())), Some(canon.clone()));
        // Started inside the install dir: keep it.
        assert_eq!(resolve_install_root(None, Some(install.path().into()), Some(elsewhere.path().into())), Some(canon.clone()));
        // ROOT_DIR wins even without assets present.
        assert_eq!(resolve_install_root(Some(elsewhere.path().into()), Some(install.path().into()), Some(install.path().into())), Some(elsewhere.path().canonicalize().unwrap()));
        // Nothing sensible: do not move.
        assert_eq!(resolve_install_root(None, Some(elsewhere.path().into()), Some(elsewhere.path().into())), None);
    }
}

#[cfg(test)]
mod workspace_root_tests {
    use super::Config;

    #[test]
    fn workspace_root_resolves_operator_selection_separately_from_assets() {
        let install = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::create_dir(install.path().join("project")).unwrap();
        let mut config = Config {
            root_dir: install.path().to_string_lossy().into_owned(),
            workspace_dir: None,
            ..Config::from_env()
        };
        assert_eq!(config.workspace_root().unwrap(), install.path().canonicalize().unwrap());
        config.workspace_dir = Some("project".into());
        assert_eq!(config.workspace_root().unwrap(), install.path().join("project").canonicalize().unwrap());
        config.workspace_dir = Some(external.path().to_string_lossy().into_owned());
        assert_eq!(config.workspace_root().unwrap(), external.path().canonicalize().unwrap());
        assert_eq!(config.root_dir, install.path().to_string_lossy());
    }

    #[test]
    fn workspace_root_rejects_missing_blank_and_file_roots() {
        let install = tempfile::tempdir().unwrap();
        std::fs::write(install.path().join("file"), "bytes").unwrap();
        for value in ["", "  ", "missing", "file"] {
            let config = Config {
                root_dir: install.path().to_string_lossy().into_owned(),
                workspace_dir: Some(value.into()),
                ..Config::from_env()
            };
            let error = config.workspace_root().unwrap_err().to_string();
            assert!(error.contains("WORKSPACE_DIR"), "{error}");
        }
        assert!(!install.path().join("missing").exists());
    }
}
