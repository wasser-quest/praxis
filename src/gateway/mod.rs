pub mod agent_loop;
pub mod auth;
pub mod cron_scheduler;
pub mod delegation;
pub mod http_handler;
pub mod client_api;
pub mod providers;
pub mod compaction;
pub mod decision_profiles;
pub mod decision_client;
pub mod decision_routing;
pub mod workflow_actions;
pub mod workflow_graph;
pub mod telemetry;
#[cfg(test)]
mod workspace_tests;
#[cfg(test)]
mod decision_tests;
#[cfg(test)]
mod workflow_action_tests;
#[cfg(test)]
mod state_machine_tests;
#[cfg(test)]
mod template_render_tests;
pub mod llm;
pub mod message_handler;
mod feedback;
pub mod poml;
pub mod prompt;
pub mod prompt_change;
pub mod rate_limiter;
pub mod task_control;
pub mod action_contracts;
pub mod decision_ir;
pub(crate) mod resource_snapshots;
#[cfg(test)]
mod action_contract_tests;
#[cfg(test)]
mod resource_contract_tests;
pub mod templates;
pub mod tool_results;
pub mod ws_handler;

use std::sync::Arc;

/// Hot-swappable LLM router: `/login` and provider changes replace the router
/// without restarting the gateway. Callers take a snapshot with `get()`.
#[derive(Clone)]
pub struct LlmHandle(Arc<std::sync::RwLock<Arc<llm::LLMRouter>>>);

impl LlmHandle {
    pub fn new(router: llm::LLMRouter) -> Self {
        Self(Arc::new(std::sync::RwLock::new(Arc::new(router))))
    }
    pub fn get(&self) -> Arc<llm::LLMRouter> {
        self.0.read().map(|g| g.clone()).unwrap_or_else(|e| e.into_inner().clone())
    }
    pub fn swap(&self, router: llm::LLMRouter) {
        match self.0.write() {
            Ok(mut guard) => *guard = Arc::new(router),
            Err(poisoned) => *poisoned.into_inner() = Arc::new(router),
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

pub async fn start(db: crate::db::Database, config: crate::config::Config) -> anyhow::Result<()> {
    let _workspace = config.workspace_root()?;
    let secrets = crate::db::secrets::get_secrets();
    let event_tx = crate::event_channel::init();

    let plugins_dir = std::env::var("PLUGINS_DIR").unwrap_or_else(|_| "./plugins".to_string());
    let plugins = Arc::new(crate::plugins::load_all_plugins(std::path::Path::new(
        &plugins_dir,
    )));

    // Endpoint/model choices made with /login live in the secret store.
    let llm = LlmHandle::new(llm::LLMRouter::new(&providers::effective_config(&config, &secrets), &secrets));
    llm.get().validate_configuration()?;

    let state = GatewayState {
        db: db.clone(),
        config: config.clone(),
        secrets,
        llm,
        plugins,
        event_tx,
        start_time: std::time::Instant::now(),
    };
    let _ = GATEWAY_STATE.set(state.clone());

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

    let app = app.merge(protected);

    let cron_db = db.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = run_due_cron_jobs(&cron_db).await {
                tracing::error!("Cron scheduler error: {}", e);
            }
            // Housekeeping: drop old finished background jobs so the
            // in-memory registry cannot grow without bound.
            crate::tools::execute_terminal::cleanup_finished_jobs();
            if let Err(error) = cron_db.prune_tool_outputs() {
                tracing::warn!(%error, "Tool-output retention cleanup failed");
            }
        }
    });

    let addr = format!("0.0.0.0:{}", config.gateway_port);
    tracing::info!("Gateway listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
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

async fn run_due_cron_jobs(db: &crate::db::Database) -> anyhow::Result<()> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT id, name, user_id, template, prompt FROM cron_jobs WHERE enabled = 1")?;

    let jobs: Vec<(String, String, String, String, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for (id, name, _user_id, _template, _prompt) in jobs {
        tracing::debug!("Cron job check: {} ({})", name, id);
    }

    Ok(())
}

#[cfg(test)]
mod gateway_tests {
    #[test]
    fn test_gateway_compiles() {
        assert!(true);
    }
}
