//! Provider login/status for `/login` (TUI, local or remote) and the
//! `/v1/providers` API. Credentials go into the live secret store, the LLM
//! router is rebuilt in place, and the store is persisted when the gateway
//! retained the master key. Everything runs on the gateway machine, so a remote
//! TUI logs the *backend* in — which is where the model calls happen.
use super::GatewayState;
use super::llm::codex::{self, CodexAuth};
use serde::{Deserialize, Serialize};
#[path = "codex_login.rs"]
mod codex_device;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    /// `/login <name> <api_key>`
    ApiKey,
    /// `/login <name> [api_base]` — local server, key optional
    Endpoint,
    /// `/login codex` — ChatGPT OAuth via the Codex CLI
    CodexOAuth,
    /// Configured through environment (GPU_ROUTER_URL/TOKEN)
    Environment,
}

pub struct ProviderSpec {
    pub name: &'static str,
    pub kind: AuthKind,
    pub label: &'static str,
    pub models_hint: &'static str,
}

pub const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec { name: "codex", kind: AuthKind::CodexOAuth, label: "OpenAI Codex (ChatGPT subscription)", models_hint: "gpt-5-codex, gpt-5" },
    ProviderSpec { name: "openai", kind: AuthKind::ApiKey, label: "OpenAI API", models_hint: "gpt-4o, gpt-4.1, o3" },
    ProviderSpec { name: "anthropic", kind: AuthKind::ApiKey, label: "Anthropic", models_hint: "claude-sonnet-4-5, claude-opus-4-1" },
    ProviderSpec { name: "openrouter", kind: AuthKind::ApiKey, label: "OpenRouter", models_hint: "any openrouter model id" },
    ProviderSpec { name: "minimax", kind: AuthKind::ApiKey, label: "MiniMax", models_hint: "MiniMax-M2" },
    ProviderSpec { name: "mimo", kind: AuthKind::ApiKey, label: "Xiaomi MiMo", models_hint: "mimo-v2-flash" },
    ProviderSpec { name: "ollama", kind: AuthKind::Endpoint, label: "Ollama (local)", models_hint: "any pulled model, e.g. qwen3:8b" },
    ProviderSpec { name: "llamacpp", kind: AuthKind::Endpoint, label: "llama.cpp server (local)", models_hint: "the model the server loaded" },
    ProviderSpec { name: "free_router", kind: AuthKind::Environment, label: "pgpu free router", models_hint: "configured in pgpu" },
];

pub fn spec(name: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|p| p.name == name)
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LoginRequest {
    pub provider: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Codex: paste the content of a `~/.codex/auth.json` from another machine.
    #[serde(default)]
    pub auth_json: Option<String>,
    /// Start a fresh CLI device login, bypassing stored/CLI credentials.
    #[serde(default)]
    pub device_auth: bool,
    /// Remove the stored credential instead of adding one.
    #[serde(default)]
    pub logout: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    pub name: String,
    pub label: String,
    pub auth: String,
    pub configured: bool,
    pub active: bool,
    pub default: bool,
    pub detail: String,
    pub models_hint: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginOutcome {
    pub provider: String,
    pub status: String,
    pub message: String,
    pub persisted: bool,
    pub setup: Vec<String>,
    pub providers: Vec<ProviderStatus>,
}

fn secret_slot<'a>(secrets: &'a mut crate::db::secrets::Secrets, provider: &str) -> Option<&'a mut Option<String>> {
    Some(match provider {
        "openai" => &mut secrets.openai_api_key,
        "anthropic" => &mut secrets.anthropic_api_key,
        "openrouter" => &mut secrets.openrouter_api_key,
        "minimax" => &mut secrets.minimax_api_key,
        "mimo" => &mut secrets.mimo_api_key,
        "ollama" => &mut secrets.ollama_api_key,
        "llamacpp" => &mut secrets.llamacpp_api_key,
        _ => return None,
    })
}

pub(crate) fn has_credential(secrets: &crate::db::secrets::Secrets, provider: &str) -> bool {
    let mut s = secrets.clone();
    match provider {
        "codex" => CodexAuth::from_secrets(secrets).is_some(),
        "free_router" => crate::gpu_router::configured(),
        _ => secret_slot(&mut s, provider).is_some_and(|slot| slot.as_deref().is_some_and(|k| !k.trim().is_empty())),
    }
}

/// Effective config: `.env` values overridden by `/login`-provided endpoint and
/// model choices stored in `secrets.custom` as `<provider>_api_base|_model`.
pub fn effective_config(base: &crate::config::Config, secrets: &crate::db::secrets::Secrets) -> crate::config::Config {
    let mut config = base.clone();
    let get = |k: &str| secrets.custom.get(k).map(String::as_str).filter(|v| !v.trim().is_empty()).map(str::to_string);
    if let Some(v) = get("openai_api_base") { config.openai_api_base = v; }
    if let Some(v) = get("openai_model") { config.openai_model = v; }
    if let Some(v) = get("codex_model") { config.codex_model = v; }
    if let Some(v) = get("anthropic_api_base") { config.anthropic_api_base = v; }
    if let Some(v) = get("anthropic_model") { config.anthropic_model = v; }
    if let Some(v) = get("ollama_api_base") { config.ollama_api_base = v; }
    if let Some(v) = get("ollama_model") { config.ollama_model = v; }
    if let Some(v) = get("llamacpp_api_base") { config.llamacpp_api_base = v; }
    if let Some(v) = get("llamacpp_model") { config.llamacpp_model = v; }
    if let Some(v) = get("minimax_api_base") { config.minimax_api_base = v; }
    if let Some(v) = get("minimax_model") { config.minimax_model = v; }
    if let Some(v) = get("mimo_api_base") { config.mimo_api_base = v; }
    if let Some(v) = get("mimo_model") { config.mimo_model = v; }
    if let Some(v) = get("openrouter_api_base") { config.openrouter_api_base = v; }
    if let Some(v) = get("openrouter_model") { config.openrouter_model = v; }
    config
}

fn default_model(config: &crate::config::Config, provider: &str) -> String {
    match provider {
        "openai" => config.openai_model.clone(),
        "codex" => config.codex_model.clone(),
        "anthropic" => config.anthropic_model.clone(),
        "ollama" => config.ollama_model.clone(),
        "llamacpp" => config.llamacpp_model.clone(),
        "minimax" => config.minimax_model.clone(),
        "mimo" => config.mimo_model.clone(),
        "openrouter" => config.openrouter_model.clone(),
        _ => String::new(),
    }
}

fn api_base(config: &crate::config::Config, provider: &str) -> String {
    match provider {
        "openai" => config.openai_api_base.clone(),
        "anthropic" => config.anthropic_api_base.clone(),
        "ollama" => config.ollama_api_base.clone(),
        "llamacpp" => config.llamacpp_api_base.clone(),
        "minimax" => config.minimax_api_base.clone(),
        "mimo" => config.mimo_api_base.clone(),
        "openrouter" => config.openrouter_api_base.clone(),
        "codex" => codex::CODEX_RESPONSES_URL.into(),
        _ => String::new(),
    }
}

pub fn statuses(state: &GatewayState) -> Vec<ProviderStatus> {
    let secrets = crate::db::secrets::get_secrets();
    let config = effective_config(&state.config, &secrets);
    let active = state.llm.provider_names();
    PROVIDERS.iter().map(|p| {
        let configured = has_credential(&secrets, p.name);
        let detail = match p.kind {
            AuthKind::CodexOAuth => CodexAuth::from_secrets(&secrets).map(|a| a.describe()).unwrap_or_else(|| "not logged in".into()),
            AuthKind::Endpoint => format!("{} · model {}", api_base(&config, p.name), default_model(&config, p.name)),
            AuthKind::ApiKey => if configured { format!("key set · model {}", default_model(&config, p.name)) } else { "no key".into() },
            AuthKind::Environment => if configured { "configured".into() } else { "GPU_ROUTER_URL not set".into() },
        };
        ProviderStatus {
            name: p.name.into(), label: p.label.into(),
            auth: match p.kind { AuthKind::ApiKey => "api_key", AuthKind::Endpoint => "endpoint", AuthKind::CodexOAuth => "oauth", AuthKind::Environment => "env" }.into(),
            configured, active: active.iter().any(|n| n == p.name), default: config.use_provider == p.name,
            detail, models_hint: p.models_hint.into(),
        }
    }).collect()
}

/// Lines the TUI prints after a successful login.
pub fn setup_hints(state: &GatewayState, provider: &str, model: &str) -> Vec<String> {
    let mut lines = vec![
        format!("Use it in this session:   /context set settings.provider={provider} settings.model={model}"),
    ];
    if let Some(p) = spec(provider) {
        lines.push(format!("Models: {}", p.models_hint));
    }
    if provider == "codex" {
        lines.push("Reasoning effort:         /thinking medium   (low|medium|high|xhigh; older Codex models cap xhigh at high and off at low)".into());
    }
    lines.push("Vision (optional):        /context set settings.vision_provider=<name> settings.vision_model=<model>".into());
    if state.config.use_provider != provider {
        lines.push(format!("Default for all sessions: USE_PROVIDER={provider} in .env (restart)"));
    }
    lines.push("Check:                    /login".into());
    lines
}

fn rebuild_router(state: &GatewayState, secrets: &crate::db::secrets::Secrets) -> Option<String> {
    let config = effective_config(&state.config, secrets);
    let router = super::llm::LLMRouter::new(&config, secrets);
    let warning = router.validate_configuration().err().map(|e| format!("Router warning: {e}"));
    state.llm.swap(router);
    warning
}

fn commit(state: &GatewayState, provider: &str, changes: crate::db::secrets::Secrets) -> anyhow::Result<(bool, Option<String>)> {
    // Probes/device auth can take time. Merge only this provider's fields into
    // the latest store, preserving other logins and rotated Codex tokens.
    let (secrets, persisted) = crate::db::secrets::update_secrets(|current| {
        let mut changes = changes;
        if let Some(value) = secret_slot(&mut changes, provider) {
            *secret_slot(current, provider).expect("known provider slot") = value.clone();
        }
        let mut keys = vec![format!("{provider}_model"), format!("{provider}_api_base")];
        if provider == "codex" { keys.push(codex::SECRET_KEY.into()); }
        for key in keys {
            if let Some(value) = changes.custom.get(&key) {
                current.custom.insert(key, value.clone());
            } else {
                current.custom.remove(&key);
            }
        }
        Ok(())
    })?;
    Ok((persisted, rebuild_router(state, &secrets)))
}

pub async fn login(state: &GatewayState, request: LoginRequest) -> anyhow::Result<LoginOutcome> {
    let name = request.provider.trim().to_ascii_lowercase();
    let spec = spec(&name).ok_or_else(|| anyhow::anyhow!(
        "Unknown provider '{}'. Known: {}", name.chars().take(40).collect::<String>(),
        PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")))?;
    let mut secrets = crate::db::secrets::get_secrets();

    if request.logout {
        match spec.kind {
            AuthKind::CodexOAuth => {
                codex_device::cancel();
                secrets.custom.remove(codex::SECRET_KEY);
            }
            AuthKind::Environment => anyhow::bail!("{} is configured through the environment", spec.label),
            _ => { if let Some(slot) = secret_slot(&mut secrets, &name) { *slot = None; } }
        }
        let (persisted, warning) = commit(state, &name, secrets)?;
        return Ok(LoginOutcome { provider: name.clone(), status: "logged_out".into(), persisted,
            message: format!("Logged out of {}.{}", spec.label, warning.map(|w| format!(" {w}")).unwrap_or_default()),
            setup: vec![], providers: statuses(state) });
    }

    // Optional endpoint/model overrides for any provider.
    if let Some(base) = request.api_base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        let url = reqwest::Url::parse(base).map_err(|_| anyhow::anyhow!("api_base must be an http(s) URL"))?;
        anyhow::ensure!(matches!(url.scheme(), "http" | "https") && url.username().is_empty() && url.password().is_none(), "api_base must be a plain http(s) URL without credentials");
        secrets.custom.insert(format!("{name}_api_base"), base.trim_end_matches('/').to_string());
    }
    if let Some(model) = request.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        anyhow::ensure!(model.len() <= 128 && model.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/".contains(c)), "Invalid model name");
        secrets.custom.insert(format!("{name}_model"), model.to_string());
    }

    let status = match spec.kind {
        AuthKind::ApiKey => {
            let key = request.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Usage: /login {name} <api_key> [model] [api_base]"))?;
            anyhow::ensure!(key.len() >= 8 && key.len() <= 512 && !key.chars().any(char::is_whitespace), "API key looks malformed");
            *secret_slot(&mut secrets, &name).expect("api-key provider") = Some(key.to_string());
            "logged_in"
        }
        AuthKind::Endpoint => {
            if let Some(key) = request.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
                *secret_slot(&mut secrets, &name).expect("endpoint provider") = Some(key.to_string());
            }
            let config = effective_config(&state.config, &secrets);
            let base = api_base(&config, &name);
            let key = match name.as_str() {
                "llamacpp" => secrets.llamacpp_api_key.as_deref(),
                _ => secrets.ollama_api_key.as_deref(),
            };
            let reachable = probe_endpoint(&name, &base, key).await;
            anyhow::ensure!(reachable, "{} is not reachable at {base}. Start the server or pass its URL: /login {name} {base}", spec.label);
            "logged_in"
        }
        AuthKind::CodexOAuth => match codex_login(state, &mut secrets, request.auth_json.as_deref(), request.device_auth).await? {
            CodexStep::Ready => "logged_in",
            CodexStep::Pending(instructions) => {
                return Ok(LoginOutcome { provider: name.clone(), status: "pending".into(), persisted: false,
                    message: instructions, setup: vec!["When the browser step is done, run /login codex again to finish (it completes automatically in the background too).".into()],
                    providers: statuses(state) });
            }
        },
        AuthKind::Environment => anyhow::bail!("{} is configured with GPU_ROUTER_URL / GPU_ROUTER_TOKEN in .env", spec.label),
    };

    let (persisted, warning) = commit(state, &name, secrets.clone())?;
    let config = effective_config(&state.config, &secrets);
    let model = default_model(&config, &name);
    let detail = match spec.kind {
        AuthKind::CodexOAuth => CodexAuth::from_secrets(&secrets).map(|a| a.describe()).unwrap_or_default(),
        _ => api_base(&config, &name),
    };
    let mut message = format!("Logged in to {} ({detail}), model {model}.", spec.label);
    message.push_str(if persisted { " Saved to the encrypted secret store." } else { " Kept in memory until restart. To persist, save via dashboard Secrets with the master password, or explicitly opt in at gateway startup with PRAXIS_RETAIN_MASTER_KEY=1 and MASTER_KEY_FILE." });
    if let Some(w) = warning { message.push_str(&format!(" {w}")); }
    Ok(LoginOutcome { provider: name.clone(), status: status.into(), message, persisted,
        setup: setup_hints(state, &name, &model), providers: statuses(state) })
}

async fn probe_endpoint(provider: &str, base: &str, key: Option<&str>) -> bool {
    let client = super::llm::http::client();
    let base = base.trim_end_matches('/');
    let get = |url: String| {
        let mut req = client.get(url).timeout(std::time::Duration::from_secs(5));
        if let Some(key) = key.filter(|k| !k.is_empty()) { req = req.bearer_auth(key); }
        req
    };
    let probe = if provider == "ollama" { format!("{base}/api/tags") } else { format!("{base}/models") };
    let Ok(response) = get(probe).send().await else { return false };
    if response.status().is_success() {
        let Ok(data) = response.json::<serde_json::Value>().await else { return false };
        return if provider == "ollama" { data["models"].is_array() } else { data["data"].is_array() };
    }
    if provider != "llamacpp" || response.status().as_u16() != 404 { return false; }
    // Older llama.cpp builds lack /v1/models. Verify their actual health API,
    // rather than accepting a generic web server's 404 as a successful login.
    let health = format!("{}/health", base.strip_suffix("/v1").unwrap_or(base));
    let Ok(response) = get(health).send().await else { return false };
    response.status().is_success() && response.json::<serde_json::Value>().await
        .is_ok_and(|data| data["status"] == "ok")
}

enum CodexStep { Ready, Pending(String) }

async fn codex_login(state: &GatewayState, secrets: &mut crate::db::secrets::Secrets, auth_json: Option<&str>, device_auth: bool) -> anyhow::Result<CodexStep> {
    anyhow::ensure!(!device_auth || auth_json.is_none(), "Choose --device-auth OR --auth-json, not both");
    // 1. Pasted auth.json from another machine.
    if let Some(text) = auth_json.filter(|t| !t.trim().is_empty()) {
        CodexAuth::from_json(text)?.store(secrets);
        codex_device::cancel();
        return Ok(CodexStep::Ready);
    }
    if let Some(instructions) = codex_device::status()? {
        return Ok(CodexStep::Pending(instructions));
    }
    if device_auth {
        return Ok(CodexStep::Pending(codex_device::start(state.clone(), secrets.custom.get("codex_model").cloned()).await?));
    }
    // Do not replace rotated Praxis tokens with a stale CLI auth.json.
    if CodexAuth::from_secrets(secrets).is_some() {
        return Ok(CodexStep::Ready);
    }
    // 2. The Codex CLI is already logged in on this machine.
    if let Some(path) = CodexAuth::cli_auth_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(auth) = CodexAuth::from_cli_file(&text) {
                auth.store(secrets);
                codex_device::cancel();
                return Ok(CodexStep::Ready);
            }
        }
    }
    let instructions = codex_device::start(state.clone(), secrets.custom.get("codex_model").cloned()).await?;
    Ok(CodexStep::Pending(instructions))
}

/// Human-readable status block for `/login` without arguments.
pub fn render_status(statuses: &[ProviderStatus], config: &crate::config::Config) -> String {
    let mut out = String::from("Providers (login happens on the gateway machine):\n");
    for s in statuses {
        let mark = if s.configured { "●" } else { "○" };
        let flags = [s.default.then_some("default"), s.active.then_some("active")].into_iter().flatten().collect::<Vec<_>>().join(", ");
        out.push_str(&format!("  {mark} {:<11} {:<36} {}{}\n", s.name, s.label, s.detail, if flags.is_empty() { String::new() } else { format!("  [{flags}]") }));
    }
    out.push_str(&format!("\nDefault provider: {}. Per session: /context set settings.provider=<name> settings.model=<model>\n", config.use_provider));
    out.push_str("Log in:  /login codex | /login <openai|anthropic|openrouter|minimax|mimo> <api_key> [model] [api_base]\n         /login <ollama|llamacpp> [api_base] [model]   ·   /logout <name>");
    out
}

#[cfg(test)]
#[path = "provider_login_tests.rs"]
mod regression_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_config_applies_login_overrides() {
        let base = crate::config::Config::from_env();
        let mut secrets = crate::db::secrets::Secrets::default();
        secrets.custom.insert("ollama_api_base".into(), "http://gpu-box:11434".into());
        secrets.custom.insert("codex_model".into(), "gpt-5".into());
        let cfg = effective_config(&base, &secrets);
        assert_eq!(cfg.ollama_api_base, "http://gpu-box:11434");
        assert_eq!(cfg.codex_model, "gpt-5");
        assert_eq!(cfg.openai_model, base.openai_model);
    }

    pub(super) fn state() -> GatewayState {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        std::mem::forget(dir);
        let mut config = crate::config::Config::from_env();
        config.use_provider = "ollama".into();
        GatewayState {
            db, config, secrets: Default::default(),
            llm: super::super::LlmHandle::new(super::super::llm::LLMRouter::with_providers(vec![], "ollama".into(), vec![], Default::default())),
            plugins: std::sync::Arc::new(crate::plugins::PluginRegistry::new()),
            event_tx: tokio::sync::broadcast::channel(16).0,
            start_time: std::time::Instant::now(),
        }
    }

    #[tokio::test]
    async fn login_rebuilds_router_and_logout_removes_credentials() {
        use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
        let _serial = crate::db::secrets::test_lock();
        crate::db::secrets::init_secrets(Default::default());
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/tags")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": []}))).mount(&server).await;
        let state = state();
        assert!(state.llm.get().provider_names().is_empty());

        assert!(login(&state, LoginRequest { provider: "nope".into(), ..Default::default() }).await.is_err());
        assert!(login(&state, LoginRequest { provider: "openai".into(), ..Default::default() }).await.is_err(), "key required");
        assert!(login(&state, LoginRequest { provider: "ollama".into(), api_base: Some("http://127.0.0.1:9".into()), ..Default::default() }).await.is_err(), "unreachable endpoint rejected");

        let outcome = login(&state, LoginRequest { provider: "ollama".into(), api_base: Some(server.uri()), model: Some("qwen3:8b".into()), ..Default::default() }).await.unwrap();
        assert_eq!(outcome.status, "logged_in");
        assert!(!outcome.persisted, "no master key in tests");
        assert!(outcome.setup.iter().any(|l| l.contains("settings.provider=ollama")));
        assert!(outcome.setup.iter().any(|l| l.contains("settings.model=qwen3:8b")));
        assert!(state.llm.get().provider_names().contains(&"ollama".to_string()), "router hot-swapped");
        assert_eq!(crate::db::secrets::get_secrets().custom.get("ollama_api_base").map(String::as_str), Some(server.uri().as_str()));

        let outcome = login(&state, LoginRequest { provider: "openai".into(), api_key: Some("sk-test-12345678".into()), model: Some("gpt-4.1".into()), ..Default::default() }).await.unwrap();
        assert!(outcome.message.contains("gpt-4.1"));
        assert!(state.llm.get().provider_names().contains(&"openai".to_string()));
        assert!(!outcome.message.contains("sk-test"), "keys are never echoed");

        let statuses = statuses(&state);
        assert!(statuses.iter().find(|s| s.name == "openai").unwrap().configured);
        assert!(render_status(&statuses, &state.config).contains("/login codex"));

        let outcome = login(&state, LoginRequest { provider: "openai".into(), logout: true, ..Default::default() }).await.unwrap();
        assert_eq!(outcome.status, "logged_out");
        assert!(!state.llm.get().provider_names().contains(&"openai".to_string()));

        // Codex: a pasted CLI auth file logs in without touching the network.
        let token = {
            use base64::Engine;
            let enc = |v: &serde_json::Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
            format!("{}.{}.sig", enc(&serde_json::json!({"alg":"none"})), enc(&serde_json::json!({"exp": 4102444800i64, "https://api.openai.com/auth": {"chatgpt_account_id": "acct_1"}})))
        };
        let auth_json = serde_json::json!({"tokens": {"access_token": token, "refresh_token": "r"}}).to_string();
        let outcome = login(&state, LoginRequest { provider: "codex".into(), auth_json: Some(auth_json), ..Default::default() }).await.unwrap();
        assert_eq!(outcome.status, "logged_in");
        assert!(state.llm.get().provider_names().contains(&"codex".to_string()));
        assert!(outcome.setup.iter().any(|l| l.contains("/thinking")));
        crate::db::secrets::init_secrets(Default::default());
    }

    #[test]
    fn provider_specs_cover_every_router_provider_name() {
        for name in ["openai", "anthropic", "ollama", "llamacpp", "free_router", "minimax", "mimo", "openrouter", "codex"] {
            assert!(spec(name).is_some(), "{name}");
        }
        assert!(spec("nope").is_none());
    }
}
