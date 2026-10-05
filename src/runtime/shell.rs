//! Installed shell worker binding (process protocol v1). This module has no
//! dependency on the optional `praxis-shell` crate, so a core-only host can
//! still talk to a separately installed worker. Background jobs live in the
//! worker process, which owns them for the lifetime of the binding; the host
//! supplies the authenticated caller and drains completion announcements.
use crate::{
    config::Config,
    plugins::{PluginHandler, PluginRegistry, ServiceAdapter},
    runtime::features::{InvocationContext, NativeService},
};
use praxis_plugin_api::{CallContext, Client, LaunchSpec};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::OnceCell;

/// Operations served by the long-lived worker. Foreground `execute_terminal`
/// stays on the one-shot executable transport so its result text is unchanged.
pub const SERVICE_TOOLS: &[&str] = &["run_background", "background_status"];
pub const CONTROLS: &[&str] = &["cleanup", "drain_completions"];

/// Bind the installed `shell` package from an operator override or a
/// manifest-declared worker. A missing or malformed declaration is a setup
/// error, not a silent fallback.
pub fn configure(config: &Config, plugins: &mut PluginRegistry) -> anyhow::Result<()> {
    let declared = plugins.get("shell").and_then(|plugin| {
        plugin.tools.iter().find_map(|tool| match &tool.handler {
            PluginHandler::Service(adapter)
                if adapter.service == "shell" && adapter.api_version == 1 =>
            {
                adapter.executable.clone()
            }
            _ => None,
        })
    });
    // The explicit environment override wins so an operator can retarget a
    // customized installation without editing the manifest.
    let Some(executable) = config.shell_service_executable.clone().or(declared) else {
        return Ok(());
    };
    let plugin = plugins.get("shell").cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Shell worker selected but the installed shell package is missing"
        )
    })?;
    anyhow::ensure!(
        plugin.enabled,
        "shell package is disabled; enable it or unset SHELL_SERVICE_EXECUTABLE"
    );
    for tool in SERVICE_TOOLS {
        let declared = plugin
            .tools
            .iter()
            .find(|candidate| candidate.name == *tool)
            .ok_or_else(|| anyhow::anyhow!("shell package must declare {tool}"))?;
        anyhow::ensure!(
            matches!(
                &declared.handler,
                PluginHandler::Service(ServiceAdapter { service, operation, api_version: 1, .. })
                    if service == "shell" && operation == tool
            ),
            "shell tool {tool} requires the process service binding"
        );
    }
    let adapter = ShellProcessAdapter::new(config, &executable)?;
    plugins.register_service(
        "shell",
        "shell",
        crate::runtime::features::INVOCATION_API_VERSION,
        SERVICE_TOOLS,
        Arc::new(adapter),
    )?;
    Ok(())
}

/// Host startup, before inference. Registration alone never spawns a worker.
pub async fn initialize_service(_config: &Config, plugins: &PluginRegistry) -> anyhow::Result<()> {
    // `configure` only registers when an operator override or manifest worker is
    // available; if a binding exists, initialize it before inference.
    if let Some(handle) = plugins.service_handle("shell", "shell") {
        handle.initialize().await?;
    }
    Ok(())
}

fn launch_configuration(config: &Config, executable: &str) -> anyhow::Result<LaunchSpec> {
    let cwd = Path::new(&config.root_dir).canonicalize()?;
    let path = Path::new(executable);
    let program = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
    .canonicalize()
    .map_err(|_| anyhow::anyhow!("SHELL_SERVICE_EXECUTABLE must name an installed worker"))?;
    let mut environment = BTreeMap::new();
    // The shell needs PATH to find operator-installed commands. Never pass the
    // provider/admin environment or the host credential store.
    if let Ok(path) = std::env::var("PATH") {
        environment.insert("PATH".into(), path);
    }
    #[cfg(windows)]
    for key in ["SystemRoot", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(key) {
            environment.insert(key.into(), value);
        }
    }
    let spec = LaunchSpec {
        program,
        args: vec!["--stdio".into()],
        cwd,
        owner: "shell".into(),
        service: "shell".into(),
        operations: SERVICE_TOOLS.iter().map(|op| (*op).into()).collect(),
        controls: CONTROLS.iter().map(|op| (*op).into()).collect(),
        environment,
    };
    spec.validate()?;
    Ok(spec)
}

pub struct ShellProcessAdapter {
    spec: LaunchSpec,
    client: OnceCell<Arc<Client>>,
}

impl ShellProcessAdapter {
    pub fn new(config: &Config, executable: &str) -> anyhow::Result<Self> {
        Ok(Self {
            spec: launch_configuration(config, executable)?,
            client: OnceCell::new(),
        })
    }

    fn client(&self) -> anyhow::Result<&Arc<Client>> {
        self.client
            .get()
            .filter(|client| client.available())
            .ok_or_else(|| anyhow::anyhow!("Shell worker unavailable; bind a new instance"))
    }
}

#[async_trait::async_trait]
impl NativeService for ShellProcessAdapter {
    fn available(&self) -> bool {
        self.client
            .get()
            .is_some_and(|client| client.available())
    }

    async fn initialize(&self) -> anyhow::Result<()> {
        let client = Arc::new(Client::launch(&self.spec, json!({})).await?);
        if let Err(error) = client.health().await {
            client.force_stop();
            return Err(error);
        }
        self.client
            .set(client.clone())
            .map_err(|_| anyhow::anyhow!("Shell binding is already initialized"))?;
        // The worker queues completion notices. Announce them on the owner's
        // stream; the task ends when the client stops.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            while client.available() {
                if let Ok(value) = client.control("drain_completions", json!({})).await {
                    if let Some(events) = value.as_array() {
                        for event in events {
                            if let Some(owner) = event["owner_user_id"].as_str() {
                                let _ = crate::runtime::events::send(
                                    owner,
                                    "background_job",
                                    &serde_json::to_string(event).unwrap_or_default(),
                                );
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
        Ok(())
    }

    async fn invoke(
        &self,
        context: InvocationContext,
        operation: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        context.require_live()?;
        let timeout_ms = context
            .deadline()
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis()
            .min(300_000) as u64;
        anyhow::ensure!(timeout_ms > 0, "Shell invocation expired");
        let caller = CallContext {
            user: context.user().into(),
            session: context.session().into(),
            task_id: context.task_id().into(),
            call_id: context.call_id().into(),
            owner: context.owner().into(),
            registry_revision: context.registry_revision().into(),
            workspace: context.workspace().to_string_lossy().into(),
            active_state: context.active_state().map(str::to_owned),
            timeout_ms,
            attributes: json!({}),
            secrets: BTreeMap::new(),
        };
        self.client()?.invoke(caller, operation, args).await
    }

    fn force_stop(&self) {
        if let Some(client) = self.client.get() {
            client.force_stop();
        }
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        if let Some(client) = self.client.get() {
            client.shutdown().await?;
        }
        Ok(())
    }
}
