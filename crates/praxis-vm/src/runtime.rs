//! Dependency-injected tool runtime, usable without Praxis DB, UI or models.
use crate::{tools, VmConfig, VmManager};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct VmSettings {
    pub data_dir: String,
    pub arch: String,
    pub socket_mode: String,
    pub cpu_cores: u32,
    pub ram_mb: u32,
    pub disk_size: String,
}

#[derive(Clone)]
pub struct VmPreferences {
    pub keyboard_layout: String,
    pub screenshot_enabled: bool,
    pub screenshot_limit: usize,
}

/// Identity supplied by the host, independently of model input. No JSON
/// deserializer can promote an operand into an authenticated caller.
pub struct VmCaller<'a> {
    pub user: &'a str,
    pub task: &'a str,
}

/// Every handler and credential lookup must select the same guest. Install has
/// a historical vm_name operand; unrelated extra operands cannot redirect it.
pub fn target_vm<'a>(operation: &str, args: &'a Value) -> anyhow::Result<&'a str> {
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
    crate::validate_vm_name(name)?;
    Ok(name)
}
impl Default for VmPreferences {
    fn default() -> Self {
        Self {
            keyboard_layout: "us".into(),
            screenshot_enabled: false,
            screenshot_limit: 5000,
        }
    }
}

pub struct VmRuntime {
    manager: Arc<VmManager>,
    settings: VmSettings,
}

impl VmRuntime {
    pub fn new(settings: VmSettings) -> anyhow::Result<Self> {
        anyhow::ensure!(
            matches!(settings.socket_mode.as_str(), "unix" | "tcp"),
            "Invalid VM socket mode"
        );
        anyhow::ensure!(
            matches!(settings.arch.as_str(), "x86_64" | "aarch64" | "arm64"),
            "Invalid VM architecture"
        );
        Ok(Self {
            manager: Arc::new(VmManager::new(&settings.data_dir)),
            settings,
        })
    }
    pub fn manager(&self) -> Arc<VmManager> {
        self.manager.clone()
    }
    pub fn settings(&self) -> &VmSettings {
        &self.settings
    }
    pub fn default_config(&self, name: &str) -> anyhow::Result<VmConfig> {
        crate::validate_vm_name(name)?;
        let mut config =
            VmConfig::default_for_name(name, &self.settings.data_dir, 1, &self.settings.arch);
        config.set_socket_mode(&self.settings.socket_mode)?;
        config.cpu_cores = self.settings.cpu_cores;
        config.ram_mb = self.settings.ram_mb;
        config.disk_size = self.settings.disk_size.clone();
        config.shared_folders.push(crate::SharedFolder {
            host_path: format!("{}/shared", self.settings.data_dir),
            mount_tag: "praxis-shared".into(),
            mount_point: "/mnt/shared".into(),
            readonly: false,
        });
        Ok(config)
    }

    /// Host preferences are resolved for the authenticated user. An operand can
    /// override a keyboard layout but cannot choose the host user or data root.
    pub fn normalized_args(
        &self,
        args: &Value,
        preferences: &VmPreferences,
    ) -> anyhow::Result<Value> {
        let mut args = args
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("VM input must be an object"))?;
        for (key, value) in [
            (
                "keyboard_layout",
                Value::String(preferences.keyboard_layout.clone()),
            ),
            ("arch", Value::String(self.settings.arch.clone())),
            (
                "socket_mode",
                Value::String(self.settings.socket_mode.clone()),
            ),
            ("cpu_cores", self.settings.cpu_cores.into()),
            ("ram_mb", self.settings.ram_mb.into()),
            ("disk_size", Value::String(self.settings.disk_size.clone())),
            ("screenshot_limit", preferences.screenshot_limit.into()),
        ] {
            args.entry(key.to_string()).or_insert(value);
        }
        // These policy fields are host-owned even if an operand attempts to
        // smuggle in fields outside the public VM tool schema.
        args.insert(
            "socket_mode".into(),
            self.settings.socket_mode.clone().into(),
        );
        args.insert(
            "screenshot_limit".into(),
            preferences.screenshot_limit.into(),
        );
        let cpu = args
            .get("cpu_cores")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("Invalid VM CPU count"))?;
        let ram = args
            .get("ram_mb")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("Invalid VM RAM"))?;
        anyhow::ensure!(
            (1..=128).contains(&cpu) && (128..=u32::MAX as u64).contains(&ram),
            "Invalid VM resources"
        );
        Ok(Value::Object(args))
    }

    pub async fn execute(
        &self,
        caller: VmCaller<'_>,
        operation: &str,
        args: &Value,
        preferences: &VmPreferences,
        grants: &BTreeMap<String, String>,
    ) -> anyhow::Result<Value> {
        let args = self.normalized_args(args, preferences)?;
        let name = target_vm(operation, &args)?;
        let output = tools::dispatch_vm_tool(&self.manager, operation, &args, grants)
            .await
            .ok_or_else(|| anyhow::anyhow!("Unknown VM operation"))?;
        let failed = output.starts_with("Error")
            || output.starts_with("ISO ")
            || output.starts_with("Timeout:");
        let screenshot_path = if !failed && operation == "vm_screenshot" {
            output
                .strip_prefix("Screenshot saved to: ")
                .map(str::to_owned)
        } else if !failed && preferences.screenshot_enabled {
            self.capture(name, preferences.screenshot_limit).await
        } else {
            None
        };
        Ok(serde_json::json!({
            "scope":{"kind":"guest","vm":name,"user":caller.user,"task":caller.task}, "operation":operation,
            "outcome":if failed {"failed"} else {"succeeded"}, "output":output,
            "verified":false, "screenshot_path":screenshot_path,
        }))
    }

    pub async fn capture(&self, name: &str, limit: usize) -> Option<String> {
        crate::validate_vm_name(name).ok()?;
        tools::save_screenshot_to_disk(&self.manager, name, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vm_preferences_and_defaults_are_explicit() {
        let root = tempfile::tempdir().unwrap();
        let runtime = VmRuntime::new(VmSettings {
            data_dir: root.path().to_str().unwrap().into(),
            arch: "aarch64".into(),
            socket_mode: "tcp".into(),
            cpu_cores: 4,
            ram_mb: 8192,
            disk_size: "50G".into(),
        })
        .unwrap();
        let preferences = VmPreferences {
            keyboard_layout: "de".into(),
            ..Default::default()
        };
        let args = runtime
            .normalized_args(&serde_json::json!({}), &preferences)
            .unwrap();
        assert_eq!(args["arch"], "aarch64");
        assert_eq!(args["keyboard_layout"], "de");
        assert_eq!(args["cpu_cores"], 4);
        assert_eq!(
            runtime
                .normalized_args(&serde_json::json!({"keyboard_layout":"us"}), &preferences)
                .unwrap()["keyboard_layout"],
            "us"
        );
        assert!(!root.path().join("vm").exists());
        let spoofed = runtime
            .normalized_args(
                &serde_json::json!({"socket_mode":"unix", "screenshot_limit":0}),
                &preferences,
            )
            .unwrap();
        assert_eq!(spoofed["socket_mode"], "tcp");
        assert_eq!(spoofed["screenshot_limit"], 5000);
    }

    #[test]
    fn vm_credentials_and_scope_select_the_handler_guest() {
        let args = serde_json::json!({"name":"wrong", "vm_name":"installer"});
        assert_eq!(target_vm("vm_install", &args).unwrap(), "installer");
        assert_eq!(target_vm("vm_start", &args).unwrap(), "wrong");
        assert!(target_vm("vm_start", &serde_json::json!({"name":null})).is_err());
        assert!(target_vm("vm_install", &serde_json::json!({"vm_name":"../escape"})).is_err());
    }
}
