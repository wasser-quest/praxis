//! Manifest-declared process services. A package may declare an installed
//! worker directly in its `service` handlers so a compatible host can bind it
//! at startup without a bespoke environment variable. This is the v1 bridge on
//! the path to generic v2 declarations; explicit bindings (e.g. VM) still take
//! precedence because their manifests declare no executable.
use crate::{
    config::Config,
    plugins::{PluginHandler, PluginRegistry},
    runtime::features::{InvocationContext, NativeService},
};
use praxis_plugin_api::{CallContext, Client, LaunchSpec};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
};
use tokio::sync::OnceCell;

fn launch_spec(
    config: &Config,
    owner: &str,
    service: &str,
    executable: &str,
    args: Vec<String>,
    operations: &[String],
) -> anyhow::Result<LaunchSpec> {
    let cwd = Path::new(&config.root_dir).canonicalize()?;
    let program = Path::new(executable).to_path_buf();
    let mut environment = BTreeMap::new();
    // Installed workers need PATH to find operator tools; never pass provider
    // credentials, plugin envelopes or the host secret store.
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
        args,
        cwd,
        owner: owner.into(),
        service: service.into(),
        operations: operations.to_vec(),
        controls: Vec::new(),
        environment,
    };
    spec.validate()?;
    Ok(spec)
}

/// Gather the distinct `(owner, service)` executable declarations from enabled
/// plugins and validate that every tool of one service agrees on the worker.
fn declarations(
    plugins: &PluginRegistry,
) -> anyhow::Result<BTreeMap<(String, String), (String, Vec<String>, Vec<String>)>> {
    let mut bindings: BTreeMap<(String, String), (String, Vec<String>, Vec<String>)> =
        BTreeMap::new();
    for plugin in plugins.list().into_iter().filter(|plugin| plugin.enabled) {
        for tool in &plugin.tools {
            let PluginHandler::Service(adapter) = &tool.handler else {
                continue;
            };
            let Some(executable) = &adapter.executable else {
                continue;
            };
            let key = (plugin.name.clone(), adapter.service.clone());
            let entry = bindings.entry(key).or_insert_with(|| {
                (
                    executable.clone(),
                    adapter.args.clone(),
                    Vec::new(),
                )
            });
            anyhow::ensure!(
                &entry.0 == executable && entry.1 == adapter.args,
                "Conflicting worker declarations for native service '{}'",
                adapter.service
            );
            entry.2.push(adapter.operation.clone());
        }
    }
    Ok(bindings)
}

pub struct DeclaredServiceAdapter {
    spec: LaunchSpec,
    initialization: Value,
    client: OnceCell<Arc<Client>>,
}

impl DeclaredServiceAdapter {
    pub fn new(config: &Config, spec: LaunchSpec) -> anyhow::Result<Self> {
        let data = Path::new(&config.data_dir);
        let data = if data.is_absolute() {
            data.to_path_buf()
        } else {
            std::env::current_dir()?.join(data)
        };
        let initialization = json!({
            "data_dir": data.to_string_lossy(),
            "root_dir": spec.cwd.to_string_lossy(),
            "workspace": spec.cwd.to_string_lossy(),
        });
        Ok(Self {
            spec,
            initialization,
            client: OnceCell::new(),
        })
    }

    fn client(&self) -> anyhow::Result<&Arc<Client>> {
        self.client
            .get()
            .filter(|client| client.available())
            .ok_or_else(|| anyhow::anyhow!("Declared worker unavailable; bind a new instance"))
    }
}

#[async_trait::async_trait]
impl NativeService for DeclaredServiceAdapter {
    fn available(&self) -> bool {
        self.client.get().is_some_and(|client| client.available())
    }

    async fn initialize(&self) -> anyhow::Result<()> {
        let client = Arc::new(Client::launch(&self.spec, self.initialization.clone()).await?);
        if let Err(error) = client.health().await {
            client.force_stop();
            return Err(error);
        }
        self.client
            .set(client)
            .map_err(|_| anyhow::anyhow!("Declared service binding is already initialized"))?;
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
        anyhow::ensure!(timeout_ms > 0, "Declared service invocation expired");
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

/// Register every executable service declaration. Existing bindings (VM, shell)
/// are left untouched; an explicitly registered owner wins over a declaration.
pub fn configure(config: &Config, plugins: &mut PluginRegistry) -> anyhow::Result<()> {
    for ((owner, service), (executable, args, mut operations)) in declarations(plugins)? {
        if plugins.service_handle(&owner, &service).is_some() {
            continue;
        }
        operations.sort();
        operations.dedup();
        let spec = launch_spec(config, &owner, &service, &executable, args, &operations)?;
        plugins.register_service(
            &owner,
            &service,
            crate::runtime::features::INVOCATION_API_VERSION,
            &operations.iter().map(String::as_str).collect::<Vec<_>>(),
            Arc::new(DeclaredServiceAdapter::new(config, spec)?),
        )?;
    }
    Ok(())
}

/// Host startup, after registration. Only declared bindings are initialized.
pub async fn initialize_service(_config: &Config, plugins: &PluginRegistry) -> anyhow::Result<()> {
    for ((owner, service), _) in declarations(plugins)? {
        if let Some(handle) = plugins.service_handle(&owner, &service) {
            handle.initialize().await?;
        }
    }
    Ok(())
}
