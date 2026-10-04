//! Optional headless worker binding. This module has no praxis-vm dependency.
use crate::{
    config::Config,
    db::Database,
    runtime::features::{InvocationContext, NativeService},
};
use praxis_plugin_api::{CallContext, Client, LaunchSpec};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::OnceCell;

pub(super) fn launch_configuration(
    config: &Config,
    executable: &str,
) -> anyhow::Result<(LaunchSpec, Value)> {
    let cwd = Path::new(&config.root_dir).canonicalize()?;
    let path = Path::new(executable);
    let program = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
    .canonicalize()
    .map_err(|_| anyhow::anyhow!("VM_SERVICE_EXECUTABLE must name an installed VM worker"))?;
    let data = Path::new(&config.data_dir);
    // DATA_DIR keeps its existing host-cwd semantics; the worker receives
    // an absolute path so its installation cwd cannot retarget storage.
    let data = if data.is_absolute() {
        data.to_path_buf()
    } else {
        std::env::current_dir()?.join(data)
    };
    let mut environment = BTreeMap::new();
    // Needed to find operator-installed QEMU utilities. Never pass the
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
        owner: "vm".into(),
        service: "vm".into(),
        operations: super::TOOL_NAMES.iter().map(|op| (*op).into()).collect(),
        controls: ["autostart", "web_info", "capture", "recover", "guests", "share", "transfer"]
            .map(String::from)
            .to_vec(),
        environment,
    };
    spec.validate()?;
    let initialization = json!({"data_dir":data.to_string_lossy(), "arch":config.vm_arch, "socket_mode":config.vm_socket_mode, "cpu_cores":config.vm_cpu_cores, "ram_mb":config.vm_ram_mb, "disk_size":config.vm_disk_size, "web_token":uuid::Uuid::new_v4().simple().to_string()});
    Ok((spec, initialization))
}

pub(super) struct VmProcessAdapter {
    spec: LaunchSpec,
    initialization: Value,
    pub(super) data_dir: String,
    client: OnceCell<Client>,
    web: OnceCell<crate::runtime::web::WebEndpoint>,
    grants: super::Grants,
    preferences: Arc<dyn Fn(&str) -> anyhow::Result<Value> + Send + Sync>,
}

impl VmProcessAdapter {
    pub(super) fn new(
        db: &Database,
        config: &Config,
        executable: &str,
        grants: super::Grants,
    ) -> anyhow::Result<Self> {
        let (spec, initialization) = launch_configuration(config, executable)?;
        let data_dir = initialization["data_dir"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing VM data directory"))?
            .to_owned();
        let database = db.clone();
        Ok(Self {
            spec,
            data_dir,
            initialization,
            client: OnceCell::new(),
            web: OnceCell::new(),
            grants,
            preferences: Arc::new(move |user| {
                let context = database.load_context(user)?;
                anyhow::ensure!(context.user_id == user, "VM preference identity mismatch");
                Ok(
                    json!({"keyboard_layout":context.settings.vm_keyboard_layout, "screenshot_enabled":context.settings.vm_screenshot_enabled, "screenshot_limit":context.settings.vm_screenshot_limit}),
                )
            }),
        })
    }
    fn client(&self) -> anyhow::Result<&Client> {
        self.client
            .get()
            .filter(|client| client.available())
            .ok_or_else(|| anyhow::anyhow!("VM worker unavailable; bind a new instance"))
    }
    pub(super) async fn capture(&self, user: &str, name: &str) -> anyhow::Result<Option<String>> {
        let preferences = (self.preferences)(user)?;
        serde_json::from_value(
            self.client()?
                .control("capture", json!({"name":name,"user":user,"preferences":preferences}))
                .await?,
        )
        .map_err(Into::into)
    }
    pub(super) async fn autostart(&self, grants: BTreeMap<String, String>) -> anyhow::Result<()> {
        tokio::time::timeout(
            Duration::from_secs(300),
            self.client()?.control("autostart", json!(grants)),
        )
        .await??;
        Ok(())
    }
}

#[async_trait::async_trait]
impl NativeService for VmProcessAdapter {
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn available(&self) -> bool {
        self.client.get().is_some_and(Client::available)
    }
    fn web_descriptor(&self) -> Option<praxis_plugin_api::web::WebDescriptor> {
        Some(super::web_descriptor())
    }
    fn web_aliases(&self) -> Vec<crate::runtime::web::WebAlias> {
        super::legacy_web_aliases()
    }
    fn web_endpoint(&self) -> Option<crate::runtime::web::WebEndpoint> {
        if !self.available() {
            return None;
        }
        self.web.get().cloned()
    }
    async fn initialize(&self) -> anyhow::Result<()> {
        let client = Client::launch(&self.spec, self.initialization.clone()).await?;
        let info = async {
            let info: praxis_plugin_api::web::WebInfo =
                serde_json::from_value(client.control("web_info", serde_json::json!({})).await?)?;
            info.validate("vm")?;
            anyhow::ensure!(
                info.descriptor == super::web_descriptor(),
                "VM web contribution mismatch"
            );
            Ok::<_, anyhow::Error>(info)
        }
        .await;
        let info = match info {
            Ok(info) => info,
            Err(error) => {
                client.force_stop();
                return Err(error);
            }
        };
        let key = self.initialization["web_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing host web nonce"))?
            .to_owned();
        self.web
            .set(crate::runtime::web::WebEndpoint::new(info, key))
            .map_err(|_| anyhow::anyhow!("VM web binding is already initialized"))?;
        self.client
            .set(client)
            .map_err(|_| anyhow::anyhow!("VM binding is already initialized"))?;
        Ok(())
    }
    async fn invoke(
        &self,
        context: InvocationContext,
        operation: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        context.require_live()?;
        let name = super::target_vm(operation, &args)?;
        let mut grants = BTreeMap::new();
        if matches!(operation, "vm_start" | "vm_install" | "vm_snapshot_restore") {
            if let Some(keys) = self.grants.get(name) {
                for (file, key) in keys {
                    grants.insert(
                        file.clone(),
                        context
                            .secret(key)
                            .ok_or_else(|| anyhow::anyhow!("VM credential is unavailable"))?
                            .to_owned(),
                    );
                }
            }
        }
        let preferences = (self.preferences)(context.user())?;
        let timeout_ms = context
            .deadline()
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis()
            .min(300_000) as u64;
        anyhow::ensure!(timeout_ms > 0, "VM invocation expired");
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
            attributes: json!({"preferences":preferences,"grants":grants}),
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
