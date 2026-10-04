#[cfg(all(test, unix))]
mod contract_tests;
#[cfg(all(test, unix))]
mod source_contract_tests;
pub mod contracts;
pub mod minimax_image;

#[cfg(test)]
mod comfyui_tests;
#[cfg(test)]
mod media_tests;

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
    /// Native verification has no helper script, URL, or model-selected command.
    #[serde(rename = "verification")]
    Verification(VerificationAdapter),
    /// Scoped source replacement with the durable native patch journal.
    #[serde(rename = "source_edit")]
    SourceEdit(SourceEditAdapter),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VerificationAdapter {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceEditAdapter {}

pub struct PluginRegistry {
    plugins: HashMap<String, Plugin>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    pub fn register(&mut self, plugin: Plugin) {
        self.plugins.insert(plugin.name.clone(), plugin);
    }

    /// Activate a declaration only after checking the candidate owner catalog.
    /// Keep the previous registry unchanged if a package ID or tool conflicts.
    pub fn try_register(&mut self, plugin: Plugin) -> anyhow::Result<()> {
        anyhow::ensure!(!self.plugins.contains_key(&plugin.name), "Plugin owner_conflict: duplicate package '{}'", plugin.name);
        for tool in &plugin.tools { contracts::validate_tool(&plugin.name, tool)?; }
        let mut candidate = Self { plugins: self.plugins.clone() };
        candidate.register(plugin);
        crate::tools::catalog::validate(&candidate)?;
        self.plugins = candidate.plugins;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Plugin> {
        self.plugins.get(name)
    }

    pub fn list(&self) -> Vec<&Plugin> {
        self.plugins.values().collect()
    }

    pub fn enabled_tools(&self) -> Vec<&PluginTool> {
        self.plugins
            .values()
            .filter(|p| p.enabled)
            .flat_map(|p| &p.tools)
            .collect()
    }

    pub fn tool_definitions(&self) -> Vec<crate::gateway::llm::provider::ToolDefinition> {
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

    pub fn context_defaults(&self) -> HashMap<String, serde_json::Value> {
        let mut defaults = HashMap::new();
        for plugin in self.plugins.values() {
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
        let crate::tools::catalog::ToolOwner::Plugin { plugin, tool } =
            crate::tools::catalog::owner(self, name)? else {
                anyhow::bail!("Capability cannot shadow a built-in tool");
            };
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
            let folder = if tool.contract.is_some() {
                plugin_dir.canonicalize()?
            } else {
                plugin_dir.to_path_buf()
            };
            let resolve = |handler: &mut PluginHandler| {
                if let PluginHandler::Script { path, .. } = handler {
                    *path = folder.join(&*path).to_string_lossy().into_owned();
                }
            };
            resolve(&mut tool.handler);
            if let Some(compensation) = tool.contract.as_mut().and_then(|c| c.compensation.as_mut())
            {
                resolve(&mut compensation.handler);
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
        });
        let sec = registry.collect_secrets();
        assert_eq!(sec.len(), 2);
        assert!(sec.contains(&"api_key".to_string()));
        assert!(sec.contains(&"api_secret".to_string()));
    }
}
