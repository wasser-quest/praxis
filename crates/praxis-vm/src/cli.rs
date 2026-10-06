//! Operator commands owned by the VM package. No host DB, providers or UI required.

#[derive(clap::Subcommand)]
pub enum VmAction {
    /// Start a VM
    Start {
        /// VM name
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// CPU cores
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=128))]
        cpu: Option<u32>,
        /// RAM in MB
        #[arg(long, value_parser = clap::value_parser!(u32).range(128..))]
        ram: Option<u32>,
        /// Disk size
        #[arg(long)]
        disk: Option<String>,
        /// ISO path or name (searches installation_disks by name, or uses as direct path)
        #[arg(long)]
        iso: Option<String>,
    },
    /// Stop a running VM
    Stop {
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// Quit through QMP immediately instead of requesting ACPI powerdown
        #[arg(long)]
        force: bool,
    },
    /// Show VM status
    Status,
    /// Gracefully shutdown VM
    Shutdown {
        #[arg(long, default_value = "praxis-vm")]
        name: String,
    },
    /// Take a screenshot
    Screenshot {
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// Output file path
        #[arg(long, short)]
        output: Option<String>,
    },
    /// Insert/remove ISO CD
    Cd {
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// ISO path (omit to eject)
        iso: Option<String>,
    },
    /// Create a disk image
    Disk {
        #[command(subcommand)]
        action: DiskAction,
    },
    /// List available installation ISOs
    ListIsos,
    /// Add an ISO path to the installation disks registry
    AddIso {
        /// ISO file path
        path: String,
        /// Display name (optional, derived from filename if omitted)
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove an ISO from the installation disks registry
    RemoveIso {
        /// ISO name or path
        name_or_path: String,
    },
    /// List snapshots
    Snapshots {
        #[arg(long, default_value = "praxis-vm")]
        name: String,
    },
    /// Create a snapshot
    Snapshot {
        /// Snapshot name
        snapshot_name: String,
        #[arg(long, default_value = "praxis-vm")]
        name: String,
    },
    /// Run a shell command in the VM
    Shell {
        /// Command to execute
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// Timeout in seconds
        #[arg(long, default_value = "30", value_parser = clap::value_parser!(u64).range(1..=300))]
        timeout: u64,
    },
}

#[derive(clap::Subcommand)]
pub enum DiskAction {
    /// Create a new disk image
    Create {
        /// Disk path
        path: String,
        /// Disk size (e.g. 40G, 100G)
        #[arg(long, default_value = "40G")]
        size: String,
        /// Disk format (qcow2, raw, vdi, vmdk)
        #[arg(long, default_value = "qcow2")]
        format: String,
    },
    /// List all VM disks
    List,
    /// Show disk info
    Info {
        /// Disk path
        path: String,
    },
    /// Resize a disk image
    Resize {
        /// Disk path
        path: String,
        /// New size (e.g. 100G)
        size: String,
    },
    /// Convert disk format
    Convert {
        /// Source disk path
        source: String,
        /// Target disk path
        target: String,
        /// Target format (qcow2, raw, vdi, vmdk)
        #[arg(long)]
        format: String,
    },
}

use crate::runtime::{VmRuntime, VmSettings};
use clap::Parser;
use std::{ffi::OsString, path::Path, process::ExitCode};
use tokio::io::AsyncReadExt;

#[derive(clap::Parser)]
#[command(name = "praxis vm", about = "Manage the installed VM package")]
struct Command {
    #[command(subcommand)]
    action: VmAction,
}

fn parse(args: &[OsString]) -> anyhow::Result<Option<VmAction>> {
    match Command::try_parse_from(
        std::iter::once(OsString::from("praxis vm")).chain(args.iter().cloned()),
    ) {
        Ok(command) => Ok(Some(command.action)),
        Err(help)
            if matches!(
                help.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            help.print()?;
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn run(settings: VmSettings, args: &[OsString]) -> anyhow::Result<()> {
    if let Some(action) = parse(args)? {
        execute(&VmRuntime::new(settings)?, action).await?;
    }
    Ok(())
}

/// A standalone operator invocation receives only public, host-resolved VM
/// settings on stdin. Parse help before reading stdin; stdout is ordinary CLI
/// output here, and remains framed-only in --stdio mode.
pub async fn from_host_stdin(args: &[OsString]) -> ExitCode {
    let result = async {
        let Some(action) = parse(args)? else {
            return Ok(());
        };
        let mut bytes = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::io::stdin().take(65537).read_to_end(&mut bytes),
        )
        .await??;
        anyhow::ensure!(bytes.len() <= 65536, "VM CLI settings exceed the limit");
        let settings: VmSettings = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            Path::new(&settings.data_dir).is_absolute(),
            "VM data directory must be host resolved"
        );
        execute(&VmRuntime::new(settings)?, action).await
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn attach(runtime: &VmRuntime, name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        runtime
            .manager()
            .attach_existing(runtime.default_config(name)?)
            .await?,
        "VM '{name}' is not running or its configured QMP endpoint is unavailable"
    );
    Ok(())
}

async fn execute(runtime: &VmRuntime, action: VmAction) -> anyhow::Result<()> {
    let manager = runtime.manager();
    let data = &runtime.settings().data_dir;
    match action {
        VmAction::Start {
            name,
            cpu,
            ram,
            disk,
            iso,
        } => {
            let mut config = runtime.default_config(&name)?;
            if let Some(cpu) = cpu {
                config.cpu_cores = cpu;
            }
            if let Some(ram) = ram {
                config.ram_mb = ram;
            }
            if let Some(disk) = disk {
                validate_size(&disk)?;
                config.disk_size = disk;
            }
            validate_size(&config.disk_size)?;
            config.iso_path = iso.map(|iso| resolve_iso(runtime, &iso)).transpose()?;
            // Manual operator starts never receive model/plugin credential grants.
            println!("{}", manager.start_vm(config).await?);
        }
        VmAction::Stop { name, force } => {
            attach(runtime, &name).await?;
            println!(
                "{}",
                if force {
                    manager.force_stop_vm(&name).await?
                } else {
                    manager.request_shutdown(&name).await?
                }
            );
        }
        VmAction::Shutdown { name } => {
            attach(runtime, &name).await?;
            println!("{}", manager.request_shutdown(&name).await?);
        }
        VmAction::Status => {
            // Attach through each guest's persisted metadata: records carry
            // the per-guest endpoints (Unix sockets or TCP ports) recorded at
            // creation, claim or recovery, in either socket mode.
            let mut seen = std::collections::HashSet::new();
            for record in runtime.guests().records() {
                seen.insert(record.name.clone());
                let _ = manager.attach_existing(record.config).await;
            }
            // Directory discovery covers guests that predate records; only
            // Unix mode derives their per-guest sockets without one.
            if runtime.settings().socket_mode != "tcp" {
                if let Ok(entries) = std::fs::read_dir(Path::new(data).join("vm")) {
                    for entry in entries {
                        let entry = entry?;
                        if !entry.file_type()?.is_dir() {
                            continue;
                        }
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if !seen.contains(&name) && crate::validate_vm_name(&name).is_ok() {
                            manager
                                .attach_existing(runtime.default_config(&name)?)
                                .await?;
                        }
                    }
                }
            }
            let vms = manager.list_vms().await;
            if vms.is_empty() {
                println!("No VMs running.");
            }
            for vm in vms {
                println!(
                    "  {} [{}] VNC: {}",
                    vm["name"], vm["status"], vm["vnc_port"]
                );
            }
        }
        VmAction::Screenshot { name, output } => {
            attach(runtime, &name).await?;
            let path = runtime
                .capture(&name, 5000)
                .await
                .ok_or_else(|| anyhow::anyhow!("Unable to capture VM screenshot"))?;
            if let Some(output) = output {
                if Path::new(&path) != Path::new(&output) {
                    std::fs::copy(&path, &output)?;
                }
                println!("Screenshot saved to {output}");
            } else {
                println!("Screenshot saved to {path}");
            }
        }
        VmAction::Cd { name, iso } => {
            attach(runtime, &name).await?;
            let iso = iso.map(|value| resolve_iso(runtime, &value)).transpose()?;
            println!("{}", manager.change_cd(&name, iso.as_deref()).await?);
        }
        VmAction::Disk { action } => disk_action(action, data).await?,
        VmAction::ListIsos => {
            let isos = manager.list_isos();
            if isos.is_empty() {
                println!("No installation ISOs configured. Add one with: praxis vm add-iso /path/to/file.iso");
            }
            for iso in isos {
                println!(
                    "  {} -> {} [{}]",
                    iso["name"],
                    iso["path"],
                    if iso["exists"].as_bool().unwrap_or(false) {
                        "OK"
                    } else {
                        "NOT FOUND"
                    }
                );
            }
        }
        VmAction::AddIso { path, name } => {
            let path = Path::new(&path).canonicalize()?;
            anyhow::ensure!(path.is_file(), "ISO must be a regular file");
            let name = name.unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            });
            manager.add_installation_disk(&name, &path.to_string_lossy())?;
            println!("Added ISO: {name} -> {}", path.display());
        }
        VmAction::RemoveIso { name_or_path } => {
            manager.remove_installation_disk(&name_or_path)?;
            println!("Removed: {name_or_path}");
        }
        VmAction::Snapshots { name } => {
            let config = runtime.default_config(&name)?;
            image_command(&["snapshot", "-l", &config.disk_path]).await?;
        }
        VmAction::Snapshot {
            snapshot_name,
            name,
        } => {
            crate::validate_vm_name(&snapshot_name)?;
            attach(runtime, &name).await?;
            println!("{}", manager.create_snapshot(&name, &snapshot_name).await?);
        }
        VmAction::Shell {
            command,
            name,
            timeout,
        } => {
            attach(runtime, &name).await?;
            println!(
                "{}",
                manager
                    .shell_exec(&name, &command.join(" "), timeout)
                    .await?
            );
        }
    }
    Ok(())
}

fn validate_size(size: &str) -> anyhow::Result<()> {
    let digits = size.trim_end_matches(['M', 'G', 'T']);
    anyhow::ensure!(
        !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && digits.parse::<u64>().is_ok_and(|n| n > 0)
            && (digits == size || size.len() == digits.len() + 1),
        "Invalid disk size"
    );
    Ok(())
}

fn resolve_iso(runtime: &VmRuntime, value: &str) -> anyhow::Result<String> {
    let mut candidates = Vec::new();
    let direct = Path::new(value);
    if direct.is_file() {
        candidates.push(direct.to_path_buf());
    }
    for iso in runtime.manager().list_isos() {
        if iso["name"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(value))
        {
            if let Some(path) = iso["path"].as_str() {
                candidates.push(path.into());
            }
        }
    }
    let in_directory = Path::new(&runtime.settings().data_dir)
        .join("vm/isos")
        .join(value);
    if in_directory.is_file() {
        candidates.push(in_directory);
    }
    candidates.sort();
    candidates.dedup();
    anyhow::ensure!(
        candidates.len() == 1,
        "ISO must identify one existing file or exact registered name"
    );
    Ok(candidates[0].canonicalize()?.to_string_lossy().into())
}

async fn image_command(args: &[&str]) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("qemu-img")
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "qemu-img failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    Ok(())
}

async fn disk_action(action: DiskAction, data: &str) -> anyhow::Result<()> {
    let format_valid = |format: &str| matches!(format, "qcow2" | "raw" | "vdi" | "vmdk");
    let path_valid = |path: &str| !path.is_empty() && !path.starts_with('-');
    match action {
        DiskAction::Create { path, size, format } => {
            anyhow::ensure!(
                path_valid(&path) && format_valid(&format),
                "Invalid disk path or format"
            );
            validate_size(&size)?;
            image_command(&["create", "-f", &format, &path, &size]).await?;
        }
        DiskAction::Info { path } => {
            anyhow::ensure!(path_valid(&path), "Invalid disk path");
            image_command(&["info", &path]).await?;
        }
        DiskAction::Resize { path, size } => {
            anyhow::ensure!(path_valid(&path), "Invalid disk path");
            validate_size(&size)?;
            image_command(&["resize", &path, &size]).await?;
        }
        DiskAction::Convert {
            source,
            target,
            format,
        } => {
            anyhow::ensure!(
                path_valid(&source) && path_valid(&target) && format_valid(&format),
                "Invalid disk path or format"
            );
            image_command(&["convert", "-O", &format, &source, &target]).await?;
        }
        DiskAction::List => {
            if let Ok(directories) = std::fs::read_dir(Path::new(data).join("vm")) {
                for directory in directories {
                    let directory = directory?;
                    if !directory.file_type()?.is_dir() {
                        continue;
                    }
                    if let Ok(files) = std::fs::read_dir(directory.path()) {
                        for file in files {
                            let file = file?;
                            let path = file.path();
                            if file.file_type()?.is_file()
                                && matches!(
                                    path.extension().and_then(|s| s.to_str()),
                                    Some("qcow2" | "img" | "raw" | "vdi" | "vmdk")
                                )
                            {
                                println!("{} ({} bytes)", path.display(), file.metadata()?.len());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
