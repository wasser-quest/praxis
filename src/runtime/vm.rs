//! VM feature registration. The optional installed worker needs no QEMU imports
//! in the host. POML/SM/IR never depend on this package.
use crate::{config::Config, db::Database, plugins::PluginRegistry};
use std::sync::Arc;

#[cfg(feature = "vm")]
pub mod cli;
mod process;

pub const TOOL_NAMES: &[&str] = &[
    "vm_start",
    "vm_stop",
    "vm_shell",
    "vm_keys",
    "vm_input",
    "vm_screenshot",
    "vm_file_transfer",
    "vm_snapshot",
    "vm_shared_folder",
    "vm_mouse",
    "vm_look_screenshot",
    "vm_install",
    "vm_process_list",
    "vm_file_read",
    "vm_network_test",
    "vm_service_list",
    "vm_package_install",
    "vm_snapshot_list",
    "vm_snapshot_restore",
    "vm_snapshot_delete",
    "vm_wait_for_text",
    "vm_window_list",
    "vm_window_focus",
    "vm_clipboard_set",
    "vm_clipboard_get",
];
pub fn is_vm_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// A temporary host-side bridge for dashboard and delivery adapters. It cannot
/// initialize a service. Independent service/UI packaging replaces it in PR 3.
pub struct VmAccess {
    #[cfg(feature = "vm")]
    pub inner: Arc<praxis_vm::runtime::VmRuntime>,
    #[cfg(feature = "vm")]
    preferences: Arc<dyn Fn(&str) -> praxis_vm::runtime::VmPreferences + Send + Sync>,
}

impl VmAccess {
    pub fn data_dir(&self) -> &str {
        #[cfg(feature = "vm")]
        {
            &self.inner.settings().data_dir
        }
        #[cfg(not(feature = "vm"))]
        {
            ""
        }
    }
    pub async fn capture(&self, user: &str, name: &str) -> Option<String> {
        #[cfg(feature = "vm")]
        {
            let preferences = (self.preferences)(user);
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.inner.capture(name, preferences.screenshot_limit),
            )
            .await
            .ok()
            .flatten()
        }
        #[cfg(not(feature = "vm"))]
        {
            let _ = (user, name);
            None
        }
    }
}

pub fn runtime(plugins: &PluginRegistry) -> Option<Arc<VmAccess>> {
    #[cfg(feature = "vm")]
    {
        if !plugins.get("vm")?.enabled {
            return None;
        }
        plugins
            .service_handle("vm", "vm")?
            .service_as::<VmAdapter>()
            .map(|adapter| adapter.access.clone())
    }
    #[cfg(not(feature = "vm"))]
    {
        let _ = plugins;
        None
    }
}

/// The selected execution backend is host-owned registry metadata, fixed at
/// startup. Disabling a binding must fail the guest call, never execute on host.
pub fn guest_backend(plugins: &PluginRegistry) -> bool {
    plugins.service_handle("vm", "vm").is_some()
        && plugins.get("vm").is_some_and(|p| {
            p.context
                .get("execution_backend")
                .and_then(serde_json::Value::as_str)
                == Some("guest")
        })
}

pub fn configure(
    db: &Database,
    config: &Config,
    plugins: &mut PluginRegistry,
) -> anyhow::Result<()> {
    if !config.vm_enabled {
        if let Some(plugin) = plugins.get("vm") {
            let mut plugin = plugin.clone();
            plugin.enabled = false;
            plugins.register(plugin);
        }
        return Ok(());
    }
    use crate::plugins::{PluginHandler, ServiceAdapter};
    let mut plugin = plugins.get("vm").cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "VM plugin missing: run praxis install-preset compatibility for this installation"
        )
    })?;
    anyhow::ensure!(
        plugin.enabled,
        "VM plugin is disabled; disable VM_ENABLED or enable the installed VM package"
    );
    anyhow::ensure!(
        plugin.tools.len() == TOOL_NAMES.len(),
        "VM manifest must declare all 25 native tools"
    );
    let mut names = std::collections::HashSet::new();
    for tool in &plugin.tools {
        anyhow::ensure!(
            is_vm_tool(&tool.name) && names.insert(tool.name.as_str()),
            "Invalid VM tool declaration"
        );
        anyhow::ensure!(
            matches!(&tool.handler, PluginHandler::Service(ServiceAdapter { service, operation, api_version: 1, .. }) if service == "vm" && operation == &tool.name),
            "VM tools require the native vm service binding"
        );
    }
    anyhow::ensure!(
        matches!(config.vm_mode.as_str(), "shared" | "vm"),
        "VM_MODE must be shared or vm"
    );
    anyhow::ensure!(
        matches!(config.vm_arch.as_str(), "x86_64" | "aarch64" | "arm64"),
        "Invalid VM architecture"
    );
    anyhow::ensure!(
        matches!(config.vm_socket_mode.as_str(), "unix" | "tcp"),
        "Invalid VM socket mode"
    );
    let grants = parse_grants(&plugin)?;
    plugin.context.insert(
        "execution_backend".into(),
        serde_json::json!(if config.vm_mode == "vm" {
            "guest"
        } else {
            "host"
        }),
    );
    if let Some(executable) = config.vm_service_executable.as_deref() {
        let adapter = process::VmProcessAdapter::new(db, config, executable, grants)?;
        plugins.register(plugin);
        plugins.register_service(
            "vm",
            "vm",
            crate::runtime::features::INVOCATION_API_VERSION,
            TOOL_NAMES,
            Arc::new(adapter),
        )?;
        return Ok(());
    }
    #[cfg(not(feature = "vm"))]
    {
        let _ = (db, grants);
        anyhow::bail!("VM_ENABLED requires VM_SERVICE_EXECUTABLE pointing to an installed worker, or a binary built with --features vm (or compatibility)");
    }
    #[cfg(feature = "vm")]
    {
        let settings = praxis_vm::runtime::VmSettings {
            data_dir: config.data_dir.clone(),
            arch: config.vm_arch.clone(),
            socket_mode: config.vm_socket_mode.clone(),
            cpu_cores: config.vm_cpu_cores,
            ram_mb: config.vm_ram_mb,
            disk_size: config.vm_disk_size.clone(),
        };
        let database = db.clone();
        let access = Arc::new(VmAccess {
            inner: Arc::new(praxis_vm::runtime::VmRuntime::new(settings)?),
            preferences: Arc::new(move |user| {
                let settings = database
                    .load_context(user)
                    .map(|ctx| ctx.settings)
                    .unwrap_or_default();
                praxis_vm::runtime::VmPreferences {
                    keyboard_layout: settings.vm_keyboard_layout,
                    screenshot_enabled: settings.vm_screenshot_enabled,
                    screenshot_limit: settings.vm_screenshot_limit,
                }
            }),
        });
        plugins.register(plugin);
        plugins.register_service(
            "vm",
            "vm",
            crate::runtime::features::INVOCATION_API_VERSION,
            TOOL_NAMES,
            Arc::new(VmAdapter { access, grants }),
        )?;
        Ok(())
    }
}

type Grants = std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>;

fn parse_grants(plugin: &crate::plugins::Plugin) -> anyhow::Result<Grants> {
    let grants: Grants = serde_json::from_value(
        plugin
            .context
            .get("credential_grants")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )?;
    for (name, values) in &grants {
        validate_vm_name(name)?;
        anyhow::ensure!(
            values.keys().all(|key| !key.is_empty()
                && key.len() <= 128
                && key
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                && !key.as_bytes()[0].is_ascii_digit()),
            "Invalid VM credential name"
        );
        anyhow::ensure!(
            values.values().all(|key| plugin.secrets.contains(key)),
            "VM credential grant references an undeclared plugin secret"
        );
    }
    Ok(grants)
}

// Host preflight must validate guest selection even in a QEMU-free build.
fn validate_vm_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name.len() <= 80
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            && !matches!(name, "." | ".." | "isos" | "disks" | "shared" | "secrets"),
        "Invalid VM name"
    );
    Ok(())
}
fn target_vm<'a>(operation: &str, args: &'a serde_json::Value) -> anyhow::Result<&'a str> {
    let key = if operation == "vm_install" {
        "vm_name"
    } else {
        "name"
    };
    let name = match args.get(key) {
        Some(value) => value
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("VM name must be a string"))?,
        None => "praxis-vm",
    };
    validate_vm_name(name)?;
    Ok(name)
}

/// Explicit host startup, before inference; registration and tool calls never
/// start a missing/stopped worker or replay a prior action.
pub async fn initialize_service(config: &Config, plugins: &PluginRegistry) -> anyhow::Result<()> {
    if config.vm_enabled {
        plugins
            .service_handle("vm", "vm")
            .ok_or_else(|| anyhow::anyhow!("VM service binding missing"))?
            .initialize()
            .await?;
    }
    Ok(())
}

#[cfg(feature = "vm")]
struct VmAdapter {
    access: Arc<VmAccess>,
    grants: Grants,
}

#[cfg(feature = "vm")]
#[async_trait::async_trait]
impl crate::runtime::features::NativeService for VmAdapter {
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    async fn invoke(
        &self,
        context: crate::runtime::features::InvocationContext,
        operation: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let name = praxis_vm::runtime::target_vm(operation, &args)?;
        let mut grants = std::collections::BTreeMap::new();
        if let Some(keys) = self
            .grants
            .get(name)
            .filter(|_| matches!(operation, "vm_start" | "vm_install" | "vm_snapshot_restore"))
        {
            for (file, key) in keys {
                let value = context
                    .secret(key)
                    .ok_or_else(|| anyhow::anyhow!("VM credential is unavailable"))?;
                grants.insert(file.clone(), value.to_string());
            }
        }
        let preferences = (self.access.preferences)(context.user());
        let caller = praxis_vm::runtime::VmCaller {
            user: context.user(),
            task: context.task_id(),
        };
        self.access
            .inner
            .execute(caller, operation, &args, &preferences, &grants)
            .await
    }
    // Draining stops new invocations. Guests/disks survive host shutdown.
}

pub async fn autostart(
    config: &Config,
    plugins: &PluginRegistry,
    secrets: &crate::db::secrets::Secrets,
) -> anyhow::Result<()> {
    if !config.vm_enabled {
        return Ok(());
    }
    if config.vm_service_executable.is_some() {
        let handle = plugins
            .service_handle("vm", "vm")
            .ok_or_else(|| anyhow::anyhow!("VM feature is unavailable"))?;
        let adapter = handle
            .service_as::<process::VmProcessAdapter>()
            .ok_or_else(|| anyhow::anyhow!("VM worker is unavailable"))?;
        let plugin = plugins
            .get("vm")
            .ok_or_else(|| anyhow::anyhow!("VM package missing"))?;
        let names = parse_grants(plugin)?;
        let mut grants = std::collections::BTreeMap::new();
        if let Some(keys) = names.get("praxis-vm") {
            for (file, key) in keys {
                grants.insert(
                    file.clone(),
                    secrets
                        .plugin_secret(key)
                        .ok_or_else(|| anyhow::anyhow!("VM credential is unavailable"))?
                        .to_owned(),
                );
            }
        }
        return adapter.autostart(grants).await;
    }
    #[cfg(feature = "vm")]
    {
        let access =
            runtime(plugins).ok_or_else(|| anyhow::anyhow!("VM feature is unavailable"))?;
        let plugin = plugins
            .get("vm")
            .ok_or_else(|| anyhow::anyhow!("VM package missing"))?;
        let names = parse_grants(plugin)?;
        let mut grants = std::collections::BTreeMap::new();
        if let Some(keys) = names.get("praxis-vm") {
            for (file, key) in keys {
                let value = secrets
                    .plugin_secret(key)
                    .ok_or_else(|| anyhow::anyhow!("VM credential is unavailable"))?;
                grants.insert(file.clone(), value.to_string());
            }
        }
        access
            .inner
            .manager()
            .start_vm_with_grants(access.inner.default_config("praxis-vm")?, &grants)
            .await?;
        Ok(())
    }
    #[cfg(not(feature = "vm"))]
    {
        let _ = (plugins, secrets);
        anyhow::bail!("VM package not compiled")
    }
}
