#[cfg(test)]
mod action_contract_tests;
pub mod action_contracts;
pub mod agent_loop;
pub mod auth;
pub mod client_api;
#[cfg(test)]
mod coding_profile_tests;
pub mod compaction;
pub mod cron_scheduler;
pub mod decision_client;
pub mod decision_ir;
pub mod decision_profiles;
pub mod decision_routing;
#[cfg(test)]
mod decision_tests;
pub mod delegation;
mod feedback;
#[cfg(test)]
mod foundation_tests;
pub mod http_handler;
pub mod inference;
#[cfg(test)]
mod learning_flow_tests;
pub mod llm;
pub mod message_handler;
pub mod poml;
pub mod prompt;
pub mod prompt_change;
pub mod providers;
pub mod rate_limiter;
#[cfg(test)]
mod resource_contract_tests;
pub(crate) mod resource_snapshots;
#[cfg(test)]
mod state_machine_tests;
pub mod task_control;
pub mod telemetry;
#[cfg(test)]
mod template_render_tests;
pub mod templates;
pub(crate) mod tool_dispatch;
pub mod tool_results;
#[cfg(test)]
mod workflow_action_tests;
pub mod workflow_actions;
pub mod workflow_graph;
pub mod workflow_preflight;
#[cfg(test)]
mod workspace_tests;
pub mod ws_handler;

use std::sync::Arc;

/// Hot-swappable LLM router: `/login` and provider changes replace the router
/// without restarting the gateway. Management reads configuration metadata;
/// `get()` lazily constructs clients once for that router generation.
#[derive(Clone)]
pub struct LlmHandle(Arc<std::sync::RwLock<Arc<RouterGeneration>>>);

enum RouterGeneration {
    Ready(Arc<llm::LLMRouter>),
    Configured {
        config: crate::config::Config,
        secrets: crate::db::secrets::Secrets,
        providers: Vec<String>,
        router: std::sync::OnceLock<Arc<llm::LLMRouter>>,
    },
}

impl LlmHandle {
    pub fn new(router: llm::LLMRouter) -> Self {
        Self(Arc::new(std::sync::RwLock::new(Arc::new(
            RouterGeneration::Ready(Arc::new(router)),
        ))))
    }
    pub fn configured(config: crate::config::Config, secrets: crate::db::secrets::Secrets) -> Self {
        let providers = llm::LLMRouter::configured_provider_names(&secrets);
        Self(Arc::new(std::sync::RwLock::new(Arc::new(
            RouterGeneration::Configured {
                config,
                secrets,
                providers,
                router: std::sync::OnceLock::new(),
            },
        ))))
    }
    fn snapshot(&self) -> Arc<RouterGeneration> {
        self.0
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }
    pub fn get(&self) -> Arc<llm::LLMRouter> {
        match &*self.snapshot() {
            RouterGeneration::Ready(router) => router.clone(),
            RouterGeneration::Configured {
                config,
                secrets,
                router,
                ..
            } => router
                .get_or_init(|| Arc::new(llm::LLMRouter::new(config, secrets)))
                .clone(),
        }
    }
    pub fn provider_names(&self) -> Vec<String> {
        match &*self.snapshot() {
            RouterGeneration::Ready(router) => router.provider_names(),
            RouterGeneration::Configured { providers, .. } => providers.clone(),
        }
    }
    pub fn default_provider(&self) -> String {
        match &*self.snapshot() {
            RouterGeneration::Ready(router) => router.default_provider().into(),
            RouterGeneration::Configured { config, .. } => config.use_provider.clone(),
        }
    }
    pub fn validate_request_configuration(&self, provider: Option<&str>) -> anyhow::Result<()> {
        match &*self.snapshot() {
            RouterGeneration::Ready(router) => router.validate_request_configuration(provider),
            RouterGeneration::Configured {
                config, providers, ..
            } => llm::LLMRouter::validate_provider_inventory(
                &config.llm_resilience,
                &config.use_provider,
                &config.llm_fallback_providers,
                providers,
                provider,
            ),
        }
    }
    pub fn is_initialized(&self) -> bool {
        match &*self.snapshot() {
            RouterGeneration::Ready(_) => true,
            RouterGeneration::Configured { router, .. } => router.get().is_some(),
        }
    }
    pub fn swap(&self, router: llm::LLMRouter) {
        match self.0.write() {
            Ok(mut guard) => *guard = Arc::new(RouterGeneration::Ready(Arc::new(router))),
            Err(poisoned) => {
                *poisoned.into_inner() = Arc::new(RouterGeneration::Ready(Arc::new(router)))
            }
        }
    }
}

#[derive(Clone)]
pub struct GatewayState {
    pub db: crate::db::Database,
    pub config: crate::config::Config,
    pub secrets: crate::db::secrets::Secrets,
    pub llm: LlmHandle,
    pub plugins: Arc<crate::plugins::PluginRegistry>,
    pub event_tx: tokio::sync::broadcast::Sender<crate::event_channel::GatewayEvent>,
    pub start_time: std::time::Instant,
}

/// Global reference to the running gateway state, set in `start`. Tools that
/// need to spawn sub-agent loops (e.g. delegation) access it from here.
static GATEWAY_STATE: once_cell::sync::OnceCell<GatewayState> = once_cell::sync::OnceCell::new();

pub fn state_ref() -> Option<&'static GatewayState> {
    GATEWAY_STATE.get()
}

/// Construct the management plane without requiring an inference provider.
/// Host authentication, budgets, workspace and catalog validity still apply.
pub fn management_state(
    db: crate::db::Database,
    config: crate::config::Config,
    secrets: crate::db::secrets::Secrets,
    plugins: crate::plugins::PluginRegistry,
) -> anyhow::Result<GatewayState> {
    config.validate()?;
    config.workspace_root()?;
    crate::tools::catalog::validate(&plugins)?;
    let llm = LlmHandle::configured(
        providers::effective_config(&config, &secrets),
        secrets.clone(),
    );
    Ok(GatewayState {
        db,
        config,
        secrets,
        llm,
        plugins: Arc::new(plugins),
        event_tx: crate::event_channel::get_event_tx().unwrap_or_else(crate::event_channel::init),
        start_time: std::time::Instant::now(),
    })
}

pub async fn start(db: crate::db::Database, config: crate::config::Config) -> anyhow::Result<()> {
    let _workspace = config.workspace_root()?;
    let secrets = crate::db::secrets::get_secrets();

    let plugins_dir = std::env::var("PLUGINS_DIR").unwrap_or_else(|_| "./plugins".to_string());
    let plugins = crate::plugins::load_all_plugins(std::path::Path::new(&plugins_dir));

    let state = management_state(db.clone(), config.clone(), secrets, plugins)?;
    let _ = GATEWAY_STATE.set(state.clone());

    if let Some(error) = inference::readiness(&state).error {
        tracing::warn!(%error, "Management ready; inference needs operator setup");
    }
    let feature_services = state.plugins.clone();
    let app = routes(state);

    let addr = format!("0.0.0.0:{}", config.gateway_port);
    tracing::info!("Gateway listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let mut services = crate::runtime::services::ServiceHost::new();
    crate::runtime::retention::register(&mut services, db.clone())?;
    cron_scheduler::register_service(&mut services, db)?;
    crate::tools::execute_terminal::register_maintenance(&mut services)?;
    let result = axum::serve(listener, app).await;
    services.shutdown(std::time::Duration::from_secs(2)).await;
    feature_services.shutdown_services(std::time::Duration::from_secs(2)).await;
    result?;

    Ok(())
}

/// The same authenticated routes serve configured and setup-only installations.
pub fn routes(state: GatewayState) -> axum::Router {
    let rate_limiter = Arc::new(rate_limiter::UserRateLimiter::new(60));

    let app = axum::Router::new()
        .route("/health", axum::routing::get(http_handler::health_check))
        .route("/api/auth/login", axum::routing::post(auth::login_handler))
        .with_state(state.clone());

    let protected = axum::Router::new()
        .merge(client_api::routes())
        .route("/v1/chat", axum::routing::post(http_handler::chat_handler))
        .route("/ws", axum::routing::get(ws_handler::ws_handler))
        .route("/api/status", axum::routing::get(http_handler::status))
        .route("/v1/events/:user", axum::routing::get(http_handler::events))
        .route("/v1/stop/:user", axum::routing::post(http_handler::stop))
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware_fn,
        ))
        .layer(axum::middleware::from_fn_with_state(
            rate_limiter.clone(),
            rate_limit_middleware,
        ));

    app.merge(protected)
}

async fn rate_limit_middleware(
    axum::extract::State(limiter): axum::extract::State<Arc<rate_limiter::UserRateLimiter>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    // Extract user ID from auth header or use IP as fallback
    let user_id = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();

    match limiter.check(&user_id) {
        Ok(()) => Ok(next.run(req).await),
        Err(_) => Err(axum::http::StatusCode::TOO_MANY_REQUESTS),
    }
}

#[cfg(test)]
mod gateway_tests {
    #[test]
    fn test_gateway_compiles() {
        assert!(true);
    }
}
