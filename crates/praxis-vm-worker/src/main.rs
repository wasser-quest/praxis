//! Installed VM worker. Stdout contains protocol frames only; credentials and
//! implementation errors are never written to either output stream.
use praxis_plugin_api::{CallContext, Service, ServiceInfo};
use praxis_vm::guests::Principal;
use praxis_vm::runtime::{VmCaller, VmPreferences, VmRuntime, VmSettings};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

const OPERATIONS: &[&str] = &[
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

struct Worker {
    runtime: Option<Arc<VmRuntime>>,
    web: Option<praxis_vm_web::WebServer>,
}
impl Worker {
    fn runtime(&self) -> anyhow::Result<&VmRuntime> {
        self.runtime
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Worker is not initialized"))
    }
}

#[async_trait::async_trait]
impl Service for Worker {
    fn info(&self) -> ServiceInfo {
        ServiceInfo {
            owner: "vm".into(),
            service: "vm".into(),
            operations: OPERATIONS.iter().map(|op| (*op).into()).collect(),
            controls: vec![
                "autostart".into(),
                "web_info".into(),
                "capture".into(),
                "recover".into(),
                "guests".into(),
                "share".into(),
                "transfer".into(),
            ],
        }
    }
    async fn initialize(&mut self, mut initialization: Value) -> anyhow::Result<()> {
        let key = initialization
            .as_object_mut()
            .and_then(|input| input.remove("web_token"))
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| anyhow::anyhow!("Missing host web nonce"))?;
        let settings: VmSettings = serde_json::from_value(initialization)?;
        anyhow::ensure!(
            std::path::Path::new(&settings.data_dir).is_absolute(),
            "VM data directory must be host resolved"
        );
        let runtime = Arc::new(VmRuntime::new(settings)?);
        // Reattach surviving guests before serving tools or routes.
        let _ = runtime.recover().await;
        self.web = Some(praxis_vm_web::WebServer::start(runtime.clone(), key).await?);
        self.runtime = Some(runtime);
        Ok(())
    }
    async fn invoke(
        &self,
        context: CallContext,
        operation: &str,
        input: Value,
    ) -> anyhow::Result<Value> {
        let preferences: VmPreferences = serde_json::from_value(
            context
                .attributes
                .get("preferences")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Missing host preferences"))?,
        )?;
        let grants: BTreeMap<String, String> = serde_json::from_value(
            context
                .attributes
                .get("grants")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Missing host grants"))?,
        )?;
        praxis_vm::secrets_inject::validate_grants(&grants)?;
        self.runtime()?
            .execute(
                VmCaller {
                    user: &context.user,
                    task: &context.task_id,
                },
                operation,
                &input,
                &preferences,
                &grants,
            )
            .await
    }
    async fn control(&self, operation: &str, input: Value) -> anyhow::Result<Value> {
        if operation == "capture" {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Capture {
                name: String,
                user: String,
                preferences: VmPreferences,
            }
            let input: Capture = serde_json::from_value(input)?;
            praxis_vm::validate_vm_name(&input.name)?;
            return Ok(serde_json::to_value(
                self.runtime()?
                    .capture_as(
                        &Principal::user(&input.user),
                        &input.name,
                        input.preferences.screenshot_limit,
                    )
                    .await,
            )?);
        }
        if operation == "recover" {
            return Ok(self.runtime()?.recover().await);
        }
        if matches!(operation, "guests" | "share" | "transfer") {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Guests {
                /// Host-authenticated identity; `operator` only for operator surfaces.
                user: String,
                #[serde(default)]
                operator: bool,
                #[serde(default)]
                name: Option<String>,
                #[serde(default)]
                target: Option<String>,
                #[serde(default)]
                grant: Option<bool>,
            }
            let input: Guests = serde_json::from_value(input)?;
            let principal = if input.operator {
                Principal::operator()
            } else {
                Principal::user(&input.user)
            };
            let runtime = self.runtime()?;
            if operation == "guests" {
                return Ok(serde_json::json!({"vms": runtime.list_for(&principal).await}));
            }
            let name = input.name.ok_or_else(|| anyhow::anyhow!("Missing guest"))?;
            let target = input
                .target
                .ok_or_else(|| anyhow::anyhow!("Missing target"))?;
            let record = if operation == "share" {
                runtime
                    .guests()
                    .share(&principal, &name, &target, input.grant.unwrap_or(true))
                    .await?
            } else {
                runtime
                    .guests()
                    .transfer(&principal, &name, &target)
                    .await?
            };
            return Ok(record.public());
        }
        if operation == "web_info" {
            return Ok(serde_json::to_value(
                self.web
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("VM web service unavailable"))?
                    .info(),
            )?);
        }
        anyhow::ensure!(operation == "autostart", "Unsupported VM control");
        let grants: BTreeMap<String, String> = serde_json::from_value(input)?;
        praxis_vm::secrets_inject::validate_grants(&grants)?;
        self.runtime()?.autostart(&grants).await?;
        Ok(serde_json::json!({"started":true}))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--cli") {
        return praxis_vm::cli::from_host_stdin(&args[1..]).await;
    }
    if args != ["--stdio"] {
        eprintln!("praxis-vm-service requires --stdio or --cli");
        return std::process::ExitCode::FAILURE;
    }
    match praxis_plugin_api::serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        Worker {
            runtime: None,
            web: None,
        },
    )
    .await
    {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("VM worker connection ended");
            std::process::ExitCode::FAILURE
        }
    }
}
