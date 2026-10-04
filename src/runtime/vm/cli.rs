//! Thin operator CLI contribution. The VM package owns parsing and effects;
//! no core DB, inference clients, dashboard or credential grants are started.
use crate::config::Config;
use std::{ffi::OsString, process::Stdio};
use tokio::io::AsyncWriteExt;

pub async fn run(args: &[OsString]) -> anyhow::Result<()> {
    run_with_config(&Config::from_env(), args).await
}

pub async fn run_with_config(config: &Config, args: &[OsString]) -> anyhow::Result<()> {
    let help = args.iter().any(|arg| arg == "--help" || arg == "-h")
        || args.first().is_some_and(|arg| arg == "help");
    anyhow::ensure!(
        config.vm_enabled || help,
        "VM not enabled. Set VM_ENABLED=true in .env"
    );
    if let Some(executable) = config.vm_service_executable.as_deref() {
        let (spec, mut settings) = super::process::launch_configuration(config, executable)?;
        settings
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Invalid VM CLI settings"))?
            .remove("web_token");
        let mut child = tokio::process::Command::new(spec.program)
            .arg("--cli")
            .args(args)
            .current_dir(spec.cwd)
            .env_clear()
            .envs(spec.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                anyhow::anyhow!("Cannot launch installed VM CLI; check VM_SERVICE_EXECUTABLE")
            })?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("VM CLI input unavailable"))?;
        let written = stdin.write_all(&serde_json::to_vec(&settings)?).await;
        drop(stdin);
        let status = child.wait().await?;
        anyhow::ensure!(status.success(), "VM CLI failed ({status})");
        // Help can close its input before configuration is sent.
        if !help {
            written?;
        }
        return Ok(());
    }
    #[cfg(feature = "vm")]
    {
        let data = std::path::Path::new(&config.data_dir);
        let data = if data.is_absolute() {
            data.to_path_buf()
        } else {
            std::env::current_dir()?.join(data)
        };
        praxis_vm::cli::run(
            praxis_vm::runtime::VmSettings {
                data_dir: data.to_string_lossy().into(),
                arch: config.vm_arch.clone(),
                socket_mode: config.vm_socket_mode.clone(),
                cpu_cores: config.vm_cpu_cores,
                ram_mb: config.vm_ram_mb,
                disk_size: config.vm_disk_size.clone(),
            },
            args,
        )
        .await
    }
    #[cfg(not(feature = "vm"))]
    anyhow::bail!(
        "Install the VM worker and set VM_SERVICE_EXECUTABLE, or use a build with --features vm"
    )
}
