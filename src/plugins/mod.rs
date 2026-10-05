#[cfg(all(test, unix))]
mod contract_tests;
#[cfg(all(test, unix))]
mod source_contract_tests;
pub mod contracts;
pub mod minimax_image;
mod executable;

#[cfg(test)]
mod comfyui_tests;
#[cfg(test)]
mod media_tests;

#[cfg(all(test, unix))]
mod executable_tests;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub version: String,
    pub tools: Vec<PluginTool>,
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Builtin tool packages whose tool names this plugin implements
    /// instead of the native code (e.g. `["shell"]`). See docs/TOOL_PACKAGES.md.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaces: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub handler: PluginHandler,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<contracts::ActionContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PluginHandler {
    #[serde(rename = "builtin")]
    Builtin { name: String },
    #[serde(rename = "http")]
    Http { url: String, method: String },
    #[serde(rename = "script")]
    Script { path: String, interpreter: String },
    /// Installed executable, bounded versioned JSON over stdin/stdout. Results
    /// retain their exact text rather than the legacy script envelope.
    #[serde(rename = "executable")]
    Executable { path: String, timeout_secs: u64 },
    /// Native verification has no helper script, URL, or model-selected command.
    #[serde(rename = "verification")]
    Verification(VerificationAdapter),
    /// Scoped source replacement with the durable native patch journal.
    #[serde(rename = "source_edit")]
    SourceEdit(SourceEditAdapter),
    /// Owner-scoped native feature binding, resolved by the host at startup.
    #[serde(rename = "service")]
    Service(ServiceAdapter),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VerificationAdapter {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceEditAdapter {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServiceAdapter {
    pub service: String,
    pub operation: String,
    pub api_version: u32,
    #[serde(default)]
    pub timeout_secs: u64,
    /// Optional installed worker declaration. When present, the host can bind the
    /// process service from the manifest at startup instead of relying on a
    /// bespoke environment variable. The path is resolved inside the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Clone)]
pub struct PluginRegistry {
    plugins: HashMap<String, Plugin>,
    services: HashMap<String, crate::runtime::features::ServiceHandle>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            services: HashMap::new(),
        }
    }

    /// Identity of the resolved declarations, including contracts, defaults and
    /// credential grants (names only). Does not attest script bytes or binaries.
    pub fn revision(&self) -> anyhow::Result<String> {
        use sha2::{Digest, Sha256};
        fn canonical(value: serde_json::Value) -> serde_json::Value {
            match value {
                serde_json::Value::Object(map) => {
                    let sorted: std::collections::BTreeMap<_, _> = map.into_iter()
                        .map(|(key, value)| (key, canonical(value))).collect();
                    serde_json::Value::Object(sorted.into_iter().collect())
                }
                serde_json::Value::Array(values) => serde_json::Value::Array(values.into_iter().map(canonical).collect()),
                other => other,
            }
        }
        let declarations = canonical(serde_json::to_value(&self.plugins)?);
        let mut services: Vec<_> = self
            .services
            .values()
            .map(|handle| handle.descriptor())
            .collect();
        services.sort_by(|a, b| (&a.owner, &a.id).cmp(&(&b.owner, &b.id)));
        let bytes = serde_json::to_vec(&(
            "praxis.registry.v1", env!("CARGO_PKG_VERSION"),
            crate::runtime::services::SERVICE_API_VERSION,
            crate::runtime::features::INVOCATION_API_VERSION,
            declarations,
            services,
        ))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn register(&mut self, plugin: Plugin) {
        self.plugins.insert(plugin.name.clone(), plugin);
    }

    /// Activate a declaration only after checking the candidate owner catalog.
    /// Keep the previous registry unchanged if a package ID or tool conflicts.
    pub fn try_register(&mut self, plugin: Plugin) -> anyhow::Result<()> {
        anyhow::ensure!(!self.plugins.contains_key(&plugin.name), "Plugin owner_conflict: duplicate package '{}'", plugin.name);
        for tool in &plugin.tools { contracts::validate_tool(&plugin.name, tool)?; }
        let mut candidate = self.clone();
        candidate.register(plugin);
        crate::tools::catalog::validate(&candidate)?;
        self.plugins = candidate.plugins;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Plugin> {
        self.plugins.get(name)
    }

    pub fn list(&self) -> Vec<&Plugin> {
        let mut plugins: Vec<_> = self.plugins.values().collect();
        plugins.sort_by(|a, b| a.name.cmp(&b.name));
        plugins
    }

    pub fn enabled_tools(&self) -> Vec<&PluginTool> {
        self.plugins
            .values()
            .filter(|p| p.enabled)
            .flat_map(|p| &p.tools)
            .collect()
    }

    pub fn tool_definitions(&self) -> Vec<crate::gateway::llm::provider::ToolDefinition> {
        self.declared_tool_definitions()
            .into_iter()
            .filter(|definition| self.require_service(&definition.function.name).is_ok())
            .collect()
    }

    /// Declarations for setup validation, including unbound optional services.
    /// Discovery/inference must use tool_definitions instead.
    pub(crate) fn declared_tool_definitions(
        &self,
    ) -> Vec<crate::gateway::llm::provider::ToolDefinition> {
        self.enabled_tools()
            .iter()
            .map(|t| crate::gateway::llm::provider::ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::gateway::llm::provider::FunctionDefinition {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }

    pub fn register_service(
        &mut self,
        owner: &str,
        id: &str,
        api_version: u32,
        operations: &[&str],
        service: std::sync::Arc<dyn crate::runtime::features::NativeService>,
    ) -> anyhow::Result<()> {
        let plugin = self
            .get(owner)
            .ok_or_else(|| anyhow::anyhow!("Native service owner is not loaded"))?;
        let key = format!("{owner}/{id}");
        anyhow::ensure!(
            !self.services.contains_key(&key),
            "Native service owner conflict"
        );
        let candidate = crate::runtime::features::ServiceHandle::new(
            owner,
            id,
            api_version,
            operations,
            service,
        )?;
        let mut matched = false;
        for tool in &plugin.tools {
            if let PluginHandler::Service(adapter) = &tool.handler {
                if adapter.service == id {
                    candidate.validate_declaration(adapter)?;
                    matched = true;
                }
            }
        }
        anyhow::ensure!(matched, "Native service is not declared by its owner");
        if let Some(web) = &candidate.descriptor().web {
            anyhow::ensure!(!self.services.values().any(|handle| handle.descriptor().web.as_ref().is_some_and(|other| other.id == web.id)), "Dashboard contribution owner conflict");
        }
        for alias in candidate.web_aliases() {
            anyhow::ensure!(!self.services.values().any(|handle| handle.web_aliases().iter().any(|other| other.source == alias.source)), "Host web alias owner conflict");
        }
        self.services.insert(key, candidate);
        Ok(())
    }

    pub fn service_handle(
        &self,
        owner: &str,
        id: &str,
    ) -> Option<crate::runtime::features::ServiceHandle> {
        self.services.get(&format!("{owner}/{id}")).cloned()
    }
    pub(crate) fn web_services(&self) -> Vec<crate::runtime::features::ServiceHandle> {
        self.services.values().filter(|handle| handle.descriptor().web.is_some()
            && self.get(&handle.descriptor().owner).is_some_and(|plugin| plugin.enabled)).cloned().collect()
    }

    pub(crate) fn require_service(&self, name: &str) -> anyhow::Result<()> {
        let crate::tools::catalog::ToolOwner::Plugin { plugin, tool } =
            crate::tools::catalog::owner(self, name)?
        else {
            return Ok(());
        };
        if let PluginHandler::Service(adapter) = &tool.handler {
            self.service_handle(&plugin.name, &adapter.service)
                .ok_or_else(|| anyhow::anyhow!("Native service binding is missing"))?
                .require(adapter)?;
        }
        Ok(())
    }

    pub async fn shutdown_services(&self, grace: std::time::Duration) {
        let deadline = tokio::time::Instant::now() + grace;
        for handle in self.services.values() {
            handle.close();
        }
        for handle in self.services.values() {
            if !matches!(
                handle
                    .disable(deadline.saturating_duration_since(tokio::time::Instant::now()))
                    .await,
                Ok(true)
            ) {
                tracing::warn!(owner=%handle.descriptor().owner, service=%handle.descriptor().id, "Native feature shutdown was forced or cleanup incomplete");
            }
        }
    }

    pub fn context_defaults(&self) -> HashMap<String, serde_json::Value> {
        let mut defaults = HashMap::new();
        for plugin in self.list() {
            if !plugin.enabled {
                continue;
            }
            for (key, value) in &plugin.context {
                defaults.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        defaults
    }

    pub fn collect_secrets(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        for plugin in self.plugins.values() {
            if !plugin.enabled {
                continue;
            }
            for key in &plugin.secrets {
                if !secrets.contains(key) {
                    secrets.push(key.clone());
                }
            }
        }
        secrets
    }

    /// Only credentials declared by the plugin owning this enabled tool.
    pub fn secrets_for_tool(
        &self,
        tool_name: &str,
        secrets: &crate::db::secrets::Secrets,
    ) -> HashMap<String, String> {
        self.plugins
            .values()
            .find(|plugin| plugin.enabled && plugin.tools.iter().any(|tool| tool.name == tool_name))
            .map(|plugin| {
                plugin
                    .secrets
                    .iter()
                    .filter_map(|key| {
                        secrets
                            .plugin_secret(key)
                            .map(|value| (key.clone(), value.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn manages_contract(&self, name: &str) -> bool {
        matches!(crate::tools::catalog::owner(self, name),
            Ok(crate::tools::catalog::ToolOwner::Plugin { tool, .. }) if tool.contract.is_some())
    }

    pub async fn execute_tool_for_task(
        &self,
        user: &str,
        call: &str,
        name: &str,
        args: &serde_json::Value,
        context: Option<&serde_json::Value>,
        secrets: Option<&HashMap<String, String>>,
    ) -> anyhow::Result<String> {
        crate::gateway::task_control::check_registry(user, self)?;
        let crate::tools::catalog::ToolOwner::Plugin { plugin, tool } =
            crate::tools::catalog::owner(self, name)? else {
                anyhow::bail!("Capability cannot shadow a built-in tool");
            };
        anyhow::ensure!(
            !matches!(&tool.handler, PluginHandler::Service(_)),
            "Native services require a host-issued invocation context"
        );
        if tool.contract.is_some() {
            anyhow::ensure!(
                self.manages_contract(name),
                "Capability cannot shadow a built-in tool"
            );
            contracts::execute(plugin, tool, user, call, args, context, secrets).await
        } else {
            self.execute_tool(name, args, context, secrets).await
        }
    }

    /// Shared ingress supplies the authenticated database scope. Model operands
    /// cannot choose owner, task, session, registry revision or workspace root.
    pub async fn execute_tool_with_host(
        &self,
        db: &crate::db::Database,
        user: &str,
        call: &str,
        name: &str,
        args: &serde_json::Value,
        context: Option<&serde_json::Value>,
        secrets: Option<&HashMap<String, String>>,
    ) -> anyhow::Result<String> {
        crate::gateway::task_control::check_registry(user, self)?;
        let crate::tools::catalog::ToolOwner::Plugin { plugin, tool } =
            crate::tools::catalog::owner(self, name)?
        else {
            anyhow::bail!("Capability cannot shadow a built-in tool");
        };
        let PluginHandler::Service(adapter) = &tool.handler else {
            return self
                .execute_tool_for_task(user, call, name, args, context, secrets)
                .await;
        };
        contracts::validate_tool(&plugin.name, tool)?;
        self.require_service(name)?;
        let definitions = self.declared_tool_definitions();
        crate::gateway::agent_loop::validate_tool_params(name, args, &definitions)
            .map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            args.is_object() && serde_json::to_vec(args)?.len() <= 1024 * 1024,
            "Native service input must be a bounded object"
        );
        let scoped = plugin
            .secrets
            .iter()
            .filter_map(|key| {
                secrets
                    .and_then(|values| values.get(key))
                    .map(|value| (key.clone(), value.clone()))
            })
            .collect();
        let timeout = adapter.timeout_secs.min(
            tool.contract
                .as_ref()
                .map(|contract| contract.timeout_secs)
                .unwrap_or(adapter.timeout_secs),
        );
        let writable = tool.contract.as_ref().is_none_or(|contract| {
            matches!(
                contract.effect,
                contracts::EffectClass::WorkspaceWrite | contracts::EffectClass::ExternalWrite
            )
        });
        let handle = self
            .service_handle(&plugin.name, &adapter.service)
            .ok_or_else(|| anyhow::anyhow!("Native service binding is missing"))?;
        let invocation = crate::runtime::features::ServiceInvocation {
            context: crate::runtime::features::InvocationContext::issue(
                db,
                user,
                call,
                &plugin.name,
                name,
                scoped,
                std::time::Duration::from_secs(timeout),
                writable,
                handle.stop_token(),
            )?,
            handle,
            operation: adapter.operation.clone(),
        };
        if tool.contract.is_some() {
            contracts::execute_with_service(
                plugin,
                tool,
                user,
                call,
                args,
                context,
                secrets,
                Some(&invocation),
            )
            .await
        } else {
            crate::gateway::action_contracts::before_tool(user, name)?;
            let result = invocation.invoke(args).await?;
            // Native outputs cannot forge a top-level host receipt.
            let failed = result.get("outcome").and_then(serde_json::Value::as_str) == Some("failed");
            let envelope = serde_json::json!({"service":invocation.handle.descriptor(),"result":result}).to_string();
            Ok(if failed { format!("Error: {envelope}") } else { envelope })
        }
    }

    pub async fn execute_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        context: Option<&serde_json::Value>,
        secrets: Option<&HashMap<String, String>>,
    ) -> anyhow::Result<String> {
        let crate::tools::catalog::ToolOwner::Plugin { plugin, tool } =
            crate::tools::catalog::owner(self, tool_name)? else {
                anyhow::bail!("Capability cannot shadow a built-in tool");
            };
        anyhow::ensure!(tool.contract.is_none(), "Contracted tools require task-owned execution");
        match &tool.handler {
            PluginHandler::Service(_) => {
                anyhow::bail!("Native services require a host-issued invocation context")
            }
            PluginHandler::Verification(_) | PluginHandler::SourceEdit(_) => {
                anyhow::bail!("Native verification requires an action contract")
            }
            PluginHandler::Builtin { name } => minimax_image::execute_builtin(name, args).await,
            PluginHandler::Http { url, method } => execute_http_tool(url, method, args).await,
            PluginHandler::Script { path, interpreter } => {
                let scoped: HashMap<String, String> = plugin.secrets.iter().filter_map(|key| {
                    secrets.and_then(|values| values.get(key)).map(|value| (key.clone(), value.clone()))
                }).collect();
                execute_script(path, interpreter, args, context, Some(&scoped)).await
            }
            PluginHandler::Executable { path, timeout_secs } => {
                let scoped = plugin.secrets.iter().filter_map(|key| {
                    secrets.and_then(|values| values.get(key)).map(|value| (key.clone(), value.clone()))
                }).collect();
                executable::execute(path, *timeout_secs, tool_name, args, context, scoped).await
            }
        }
    }
}

async fn execute_http_tool(
    url: &str,
    method: &str,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    let client = crate::branding::client();
    let resp = match method.to_uppercase().as_str() {
        "POST" => client.post(url).json(args).send().await?,
        "GET" => {
            client
                .get(url)
                .query(&args.as_object().unwrap_or(&serde_json::Map::new()))
                .send()
                .await?
        }
        _ => anyhow::bail!("Unsupported HTTP method: {}", method),
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Plugin HTTP error {}: {}", status, text);
    }

    let data: serde_json::Value = resp.json().await?;
    Ok(serde_json::to_string_pretty(&data)?)
}

async fn execute_script(
    path: &str,
    interpreter: &str,
    args: &serde_json::Value,
    context: Option<&serde_json::Value>,
    secrets: Option<&HashMap<String, String>>,
) -> anyhow::Result<String> {
    let input = serde_json::to_string(args)?;

    let mut cmd = tokio::process::Command::new(interpreter);
    cmd.arg(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PLUGIN_ARGS", &input);

    // Always replace these envelopes, even when empty: never inherit another
    // plugin invocation's PLUGIN_CONTEXT / PLUGIN_SECRETS from the process env.
    cmd.env(
        "PLUGIN_CONTEXT",
        serde_json::to_string(context.unwrap_or(&serde_json::json!({})))?,
    );
    cmd.env(
        "PLUGIN_SECRETS",
        serde_json::to_string(&secrets.cloned().unwrap_or_default())?,
    );

    let output = cmd
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute script {}: {}", path, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            "(no output)".to_string()
        };
        anyhow::bail!(
            "Script {} exited with {}: {}",
            path,
            output.status.code().unwrap_or(-1),
            detail
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();

    if trimmed.is_empty() {
        return Ok("Script executed successfully (no output)".to_string());
    }

    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        Ok(trimmed.to_string())
    } else {
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "result": trimmed
        }))?)
    }
}

#[derive(Debug, Deserialize)]
struct PluginManifest {
    name: String,
    description: String,
    version: String,
    #[serde(default = "default_true")]
    enabled: bool,
    tools: Vec<PluginTool>,
    #[serde(default)]
    context: HashMap<String, serde_json::Value>,
    #[serde(default)]
    secrets: Vec<String>,
    #[serde(default)]
    replaces: Vec<String>,
}

pub fn load_plugins_from_dir(dir: &Path) -> anyhow::Result<Vec<Plugin>> {
    let mut plugins = Vec::new();
    if !dir.exists() {
        return Ok(plugins);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let plugin_dir = entry.path();

        if !plugin_dir.is_dir() {
            continue;
        }

        let manifest_path = plugin_dir.join("plugin.json");
        if !manifest_path.exists() {
            continue;
        }

        match load_plugin_from_manifest(&manifest_path, &plugin_dir) {
            Ok(plugin) => {
                tracing::info!(
                    name = %plugin.name,
                    version = %plugin.version,
                    tools = plugin.tools.len(),
                    "Loaded plugin"
                );
                plugins.push(plugin);
            }
            Err(e) => {
                tracing::warn!(
                    path = %manifest_path.display(),
                    error = %e,
                    "Failed to load plugin"
                );
            }
        }
    }

    Ok(plugins)
}

fn load_plugin_from_manifest(manifest_path: &Path, plugin_dir: &Path) -> anyhow::Result<Plugin> {
    let data = std::fs::read_to_string(manifest_path)?;
    let manifest: PluginManifest = serde_json::from_str(&data)?;

    let tools = manifest
        .tools
        .into_iter()
        .map(|mut tool| {
            contracts::validate_tool(&manifest.name, &tool)?;
            let folder = if tool.contract.is_some()
                || matches!(&tool.handler, PluginHandler::Executable { .. })
                || matches!(&tool.handler, PluginHandler::Service(adapter) if adapter.executable.is_some())
            {
                plugin_dir.canonicalize()?
            } else {
                plugin_dir.to_path_buf()
            };
            let resolve = |handler: &mut PluginHandler| -> anyhow::Result<()> {
                if let PluginHandler::Script { path, .. } = handler {
                    *path = folder.join(&*path).to_string_lossy().into_owned();
                }
                if let PluginHandler::Executable { path, .. } = handler {
                    let relative = Path::new(path);
                    anyhow::ensure!(relative.is_relative() && relative.components().all(|c| matches!(c, std::path::Component::Normal(_))),
                        "Executable must be a path inside its package");
                    let resolved = folder.join(relative).canonicalize()?;
                    anyhow::ensure!(resolved.starts_with(&folder) && resolved.is_file(), "Executable escapes its package or is not a regular file");
                    *path = resolved.to_string_lossy().into_owned();
                }
                if let PluginHandler::Service(adapter) = handler {
                    if let Some(path) = &adapter.executable {
                        let relative = Path::new(path);
                        anyhow::ensure!(relative.is_relative() && relative.components().all(|c| matches!(c, std::path::Component::Normal(_))),
                            "Service executable must be a path inside its package");
                        let resolved = folder.join(relative).canonicalize()?;
                        anyhow::ensure!(resolved.starts_with(&folder) && resolved.is_file(), "Service executable escapes its package or is not a regular file");
                        adapter.executable = Some(resolved.to_string_lossy().into_owned());
                    }
                }
                Ok(())
            };
            resolve(&mut tool.handler)?;
            if let Some(compensation) = tool.contract.as_mut().and_then(|c| c.compensation.as_mut())
            {
                resolve(&mut compensation.handler)?;
            }
            Ok(tool)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Plugin {
        name: manifest.name,
        description: manifest.description,
        version: manifest.version,
        tools,
        context: manifest.context,
        secrets: manifest.secrets,
        enabled: manifest.enabled,
        replaces: manifest.replaces,
    })
}

pub fn register_builtin_plugins(_registry: &mut PluginRegistry) {
    // No built-in plugins — install plugins via the plugins/ directory
}

pub fn load_all_plugins(plugins_dir: &Path) -> PluginRegistry {
    let mut registry = PluginRegistry::new();

    register_builtin_plugins(&mut registry);

    match load_plugins_from_dir(plugins_dir) {
        Ok(plugins) => {
            for plugin in plugins {
                if let Err(error) = registry.try_register(plugin) {
                    tracing::warn!(%error, "Plugin activation rejected; previous owners preserved");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to load plugins from directory");
        }
    }

    registry
}

#[cfg(test)]
mod plugin_tests {
    use super::*;

    #[test]
    fn test_plugin_registry() {
        let mut registry = PluginRegistry::new();
        registry.register(Plugin {
            name: "test".to_string(),
            description: "Test plugin".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![],
            context: HashMap::new(),
            secrets: Vec::new(),
            enabled: true,
            replaces: Vec::new(),
        });
        assert!(registry.get("test").is_some());
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn plugin_registration_rejects_conflicts_atomically() {
        let fixture = |owner: &str, name: &str| serde_json::from_value::<Plugin>(serde_json::json!({
            "name":owner,"description":"fixture","version":"1","tools":[{
                "name":name,"description":"fixture","parameters":{},
                "handler":{"type":"builtin","name":"fixture"}
            }]
        })).unwrap();
        let mut registry = PluginRegistry::new();
        registry.try_register(fixture("one", "probe")).unwrap();
        for candidate in [fixture("one", "other"), fixture("two", "probe"), fixture("two", "write_file")] {
            assert!(registry.try_register(candidate).is_err());
            assert_eq!(registry.list().len(), 1);
            assert_eq!(registry.get("one").unwrap().tools[0].name, "probe");
        }
        registry.try_register(fixture("two", "vm_probe")).unwrap();
        assert_eq!(registry.list().len(), 2);
    }

    #[test]
    fn test_plugin_enabled_tools() {
        let mut registry = PluginRegistry::new();
        registry.register(Plugin {
            name: "test".to_string(),
            description: "Test plugin".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![PluginTool {
                name: "tool1".to_string(),
                description: "A tool".to_string(),
                parameters: serde_json::json!({}),
                contract: None,
                handler: PluginHandler::Builtin {
                    name: "test".to_string(),
                },
            }],
            context: HashMap::new(),
            secrets: Vec::new(),
            enabled: true,
            replaces: Vec::new(),
        });
        assert_eq!(registry.enabled_tools().len(), 1);
    }

    #[test]
    fn test_plugin_disabled_tools() {
        let mut registry = PluginRegistry::new();
        registry.register(Plugin {
            name: "test".to_string(),
            description: "Test plugin".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![PluginTool {
                name: "tool1".to_string(),
                description: "A tool".to_string(),
                parameters: serde_json::json!({}),
                contract: None,
                handler: PluginHandler::Builtin {
                    name: "test".to_string(),
                },
            }],
            context: HashMap::new(),
            secrets: Vec::new(),
            enabled: false,
            replaces: Vec::new(),
        });
        assert_eq!(registry.enabled_tools().len(), 0);
    }

    #[test]
    fn test_builtin_plugins_registered() {
        let mut registry = PluginRegistry::new();
        register_builtin_plugins(&mut registry);
        // No built-in plugins — minimax_image is installed via plugins/ directory
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_load_folder_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my_plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        let manifest = serde_json::json!({
            "name": "my_plugin",
            "description": "A test plugin",
            "version": "0.1.0",
            "enabled": true,
            "tools": [{
                "name": "hello",
                "description": "Say hello",
                "parameters": {"type": "object", "properties": {"name": {"type": "string"}}},
                "handler": {"type": "script", "path": "hello.sh", "interpreter": "bash"}
            }]
        });
        std::fs::write(
            plugin_dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let plugins = load_plugins_from_dir(dir.path()).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "my_plugin");
        assert_eq!(plugins[0].tools.len(), 1);
        assert_eq!(plugins[0].tools[0].name, "hello");

        match &plugins[0].tools[0].handler {
            PluginHandler::Script { path, interpreter } => {
                assert!(path.ends_with("hello.sh"));
                assert_eq!(interpreter, "bash");
            }
            _ => panic!("Expected Script handler"),
        }
    }

    #[test]
    fn test_load_folder_skips_non_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not a plugin").unwrap();

        let plugins = load_plugins_from_dir(dir.path()).unwrap();
        assert_eq!(plugins.len(), 0);
    }

    #[test]
    fn test_manifest_handler_deserialization() {
        let script_json = r#"{"type": "script", "path": "run.py", "interpreter": "python3"}"#;
        let handler: PluginHandler = serde_json::from_str(script_json).unwrap();
        match handler {
            PluginHandler::Script { path, interpreter } => {
                assert_eq!(path, "run.py");
                assert_eq!(interpreter, "python3");
            }
            _ => panic!("Expected Script"),
        }

        let http_json = r#"{"type": "http", "url": "http://localhost:8080", "method": "POST"}"#;
        let handler: PluginHandler = serde_json::from_str(http_json).unwrap();
        match handler {
            PluginHandler::Http { url, method } => {
                assert_eq!(url, "http://localhost:8080");
                assert_eq!(method, "POST");
            }
            _ => panic!("Expected Http"),
        }

        let builtin_json = r#"{"type": "builtin", "name": "image_generate"}"#;
        let handler: PluginHandler = serde_json::from_str(builtin_json).unwrap();
        match handler {
            PluginHandler::Builtin { name } => {
                assert_eq!(name, "image_generate");
            }
            _ => panic!("Expected Builtin"),
        }
    }

    #[test]
    fn test_context_defaults_from_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("ctx_plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        let manifest = serde_json::json!({
            "name": "ctx_plugin",
            "description": "Plugin with context",
            "version": "1.0.0",
            "enabled": true,
            "tools": [],
            "context": {
                "api_url": "http://localhost:8080",
                "api_key": "",
                "max_results": 20
            }
        });
        std::fs::write(
            plugin_dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let plugins = load_plugins_from_dir(dir.path()).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].context.len(), 3);
        assert_eq!(plugins[0].context["api_url"], "http://localhost:8080");
        assert_eq!(plugins[0].context["max_results"], 20);

        let mut registry = PluginRegistry::new();
        for p in plugins {
            registry.register(p);
        }
        let defaults = registry.context_defaults();
        assert_eq!(defaults.len(), 3);
        assert_eq!(defaults["api_url"], "http://localhost:8080");
    }

    #[test]
    fn test_context_defaults_empty() {
        let mut registry = PluginRegistry::new();
        registry.register(Plugin {
            name: "no_ctx".to_string(),
            description: "No context".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![],
            context: HashMap::new(),
            secrets: Vec::new(),
            enabled: true,
            replaces: Vec::new(),
        });
        assert!(registry.context_defaults().is_empty());
    }

    #[test]
    fn test_collect_secrets() {
        let mut registry = PluginRegistry::new();
        registry.register(Plugin {
            name: "sec_plugin".to_string(),
            description: "Plugin with secrets".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![],
            context: HashMap::new(),
            secrets: vec!["api_key".to_string(), "api_secret".to_string()],
            enabled: true,
            replaces: Vec::new(),
        });
        let sec = registry.collect_secrets();
        assert_eq!(sec.len(), 2);
        assert!(sec.contains(&"api_key".to_string()));
        assert!(sec.contains(&"api_secret".to_string()));
    }
}
