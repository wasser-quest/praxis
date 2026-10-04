//! VM-contributed CLI commands; main only registers this optional command.

#[derive(clap::Subcommand)]
pub enum VmAction {
    /// Start a VM
    Start {
        /// VM name
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// CPU cores
        #[arg(long, default_value = "2")]
        cpu: u32,
        /// RAM in MB
        #[arg(long, default_value = "4096")]
        ram: u32,
        /// Disk size
        #[arg(long, default_value = "40G")]
        disk: String,
        /// ISO path or name (searches installation_disks by name, or uses as direct path)
        #[arg(long)]
        iso: Option<String>,
    },
    /// Stop a running VM
    Stop {
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// Force kill
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
        command: Vec<String>,
        #[arg(long, default_value = "praxis-vm")]
        name: String,
        /// Timeout in seconds
        #[arg(long, default_value = "30")]
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

pub async fn run(action: VmAction) -> anyhow::Result<()> {
    let config = crate::config::Config::from_env();
    if !config.vm_enabled {
        anyhow::bail!("VM not enabled. Set VM_ENABLED=true in .env");
    }
    let runtime = praxis_vm::runtime::VmRuntime::new(praxis_vm::runtime::VmSettings {
        data_dir: config.data_dir.clone(),
        arch: config.vm_arch.clone(),
        socket_mode: config.vm_socket_mode.clone(),
        cpu_cores: config.vm_cpu_cores,
        ram_mb: config.vm_ram_mb,
        disk_size: config.vm_disk_size.clone(),
    })?;
    let manager = runtime.manager();

    match action {
        VmAction::Start {
            name,
            cpu,
            ram,
            disk,
            iso,
        } => {
            // Resolve ISO: check if it's a name in installation_disks or a direct path
            let resolved_iso = iso.map(|ref iso_val| {
                if std::path::Path::new(iso_val).exists() {
                    iso_val.clone()
                } else {
                    // Search by name in installation_disks
                    let isos = manager.list_isos();
                    if let Some(found) = isos.iter().find(|i| {
                        i.get("name")
                            .and_then(|v| v.as_str())
                            .map(|n| n.to_lowercase().contains(&iso_val.to_lowercase()))
                            .unwrap_or(false)
                    }) {
                        found
                            .get("path")
                            .and_then(|v| v.as_str())
                            .unwrap_or(iso_val)
                            .to_string()
                    } else {
                        // Search in iso_dir
                        let iso_dir = format!("{}/vm/isos", config.data_dir);
                        let candidates: Vec<String> = std::fs::read_dir(&iso_dir)
                            .ok()
                            .into_iter()
                            .flatten()
                            .filter_map(|e| e.ok())
                            .filter(|e| {
                                e.file_name()
                                    .to_string_lossy()
                                    .to_lowercase()
                                    .contains(&iso_val.to_lowercase())
                            })
                            .map(|e| e.path().to_string_lossy().to_string())
                            .collect();
                        candidates
                            .first()
                            .cloned()
                            .unwrap_or_else(|| iso_val.clone())
                    }
                }
            });

            let mut vm_config =
                crate::vm::VmConfig::default_for_name(&name, &config.data_dir, 1, &config.vm_arch);
            vm_config.set_socket_mode(&config.vm_socket_mode)?;
            vm_config.cpu_cores = cpu;
            vm_config.ram_mb = ram;
            vm_config.disk_size = disk;
            vm_config.iso_path = resolved_iso;
            if let Some(ref iso_path) = vm_config.iso_path {
                if std::path::Path::new(iso_path).exists() {
                    println!("Booting from ISO: {}", iso_path);
                } else {
                    eprintln!("Warning: ISO not found at '{}'", iso_path);
                }
            }
            vm_config.shared_folders.push(crate::vm::SharedFolder {
                host_path: format!("{}/shared", config.data_dir),
                mount_tag: "praxis-shared".to_string(),
                mount_point: "/mnt/shared".to_string(),
                readonly: false,
            });
            match manager.start_vm(vm_config).await {
                Ok(msg) => println!("{}", msg),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        VmAction::Stop { name, force } => {
            if force {
                println!("Force stopping VM '{}'...", name);
            }
            match manager.stop_vm(&name).await {
                Ok(msg) => println!("{}", msg),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        VmAction::Status => {
            let vms = manager.list_vms().await;
            if vms.is_empty() {
                println!("No VMs running.");
            } else {
                for vm in &vms {
                    println!(
                        "  {} [{}] PID: {} VNC: {}",
                        vm["name"],
                        vm["status"],
                        vm["pid"].as_i64().unwrap_or(0),
                        vm["vnc_port"].as_i64().unwrap_or(0),
                    );
                }
            }
        }
        VmAction::Shutdown { name } => match manager.stop_vm(&name).await {
            Ok(msg) => println!("{}", msg),
            Err(e) => eprintln!("Error: {}", e),
        },
        VmAction::Screenshot { name, output } => match manager.screenshot(&name).await {
            Ok(data_url) => {
                if let Some(path) = output {
                    if let Some(b64) = data_url.strip_prefix("data:image/ppm;base64,") {
                        use base64::Engine;
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                            std::fs::write(&path, bytes)?;
                            println!("Screenshot saved to {}", path);
                        }
                    }
                } else {
                    println!("Screenshot captured ({} bytes base64)", data_url.len());
                }
            }
            Err(e) => eprintln!("Error: {}", e),
        },
        VmAction::Cd { name, iso } => match iso {
            Some(path) => {
                println!("Inserting CD '{}' into VM '{}'...", path, name);
                let vm_info = manager.get_vm_info(&name).await?;
                let qmp_port = vm_info["qmp_port"].as_u64().unwrap_or(44400) as u16;
                let qmp_addr = format!("127.0.0.1:{}", qmp_port);
                match crate::vm::qmp::QmpClient::connect(&qmp_addr).await {
                    Ok(mut client) => {
                        let _ = client.negotiate().await;
                        println!("CD inserted: {}", path);
                        println!("Note: Reboot VM to boot from CD if needed.");
                    }
                    Err(e) => eprintln!("Cannot connect to VM QMP: {}", e),
                }
            }
            None => {
                println!("Ejecting CD from VM '{}'...", name);
            }
        },
        VmAction::Disk { action } => {
            handle_disk_action(action, &config.data_dir).await?;
        }
        VmAction::ListIsos => {
            let isos = manager.list_isos();
            if isos.is_empty() {
                println!("No installation ISOs configured.");
                println!("Add ISOs with: praxis vm add-iso /path/to/file.iso");
                println!("Or place ISOs in: {}/vm/isos/", config.data_dir);
            } else {
                println!("Available installation ISOs:");
                for iso in &isos {
                    let name = iso.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let path = iso.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                    let exists = iso.get("exists").and_then(|v| v.as_bool()).unwrap_or(false);
                    let source = iso.get("source").and_then(|v| v.as_str()).unwrap_or("?");
                    let status = if exists { "OK" } else { "NOT FOUND" };
                    let size = iso
                        .get("size_bytes")
                        .and_then(|v| v.as_u64())
                        .map(|s| {
                            if s > 1_073_741_824 {
                                format!("{:.1} GB", s as f64 / 1_073_741_824.0)
                            } else if s > 1_048_576 {
                                format!("{:.1} MB", s as f64 / 1_048_576.0)
                            } else {
                                format!("{} B", s)
                            }
                        })
                        .unwrap_or_default();
                    println!("  [{}] {} {} ({}) [{}]", status, name, size, path, source);
                }
            }
        }
        VmAction::AddIso { path, name } => {
            let iso_name = name.unwrap_or_else(|| {
                std::path::Path::new(&path)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
            manager.add_installation_disk(&iso_name, &path)?;
            println!("Added ISO: {} -> {}", iso_name, path);
            if !std::path::Path::new(&path).exists() {
                println!("Warning: file not found at '{}'", path);
            }
        }
        VmAction::RemoveIso { name_or_path } => {
            manager.remove_installation_disk(&name_or_path)?;
            println!("Removed: {}", name_or_path);
        }
        VmAction::Snapshots { name } => {
            println!("Snapshots for VM '{}':", name);
            // List snapshot files
            let vm_dir = format!("{}/vm/{}", config.data_dir, name);
            let snap_dir = format!("{}/snapshots", vm_dir);
            if std::path::Path::new(&snap_dir).exists() {
                for entry in std::fs::read_dir(&snap_dir)? {
                    let entry = entry?;
                    println!("  {}", entry.file_name().to_string_lossy());
                }
            } else {
                println!("  No snapshots found.");
            }
        }
        VmAction::Snapshot {
            snapshot_name,
            name,
        } => match manager.create_snapshot(&name, &snapshot_name).await {
            Ok(msg) => println!("{}", msg),
            Err(e) => eprintln!("Error: {}", e),
        },
        VmAction::Shell {
            command,
            name,
            timeout,
        } => {
            let cmd = command.join(" ");
            match manager.shell_exec(&name, &cmd, timeout).await {
                Ok(output) => println!("{}", output),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }

    Ok(())
}

async fn handle_disk_action(action: DiskAction, data_dir: &str) -> anyhow::Result<()> {
    match action {
        DiskAction::Create { path, size, format } => {
            let output = tokio::process::Command::new("qemu-img")
                .args(["create", "-f", &format, &path, &size])
                .output()
                .await?;
            if output.status.success() {
                println!("Disk created: {} ({}, {})", path, size, format);
            } else {
                eprintln!("Error: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        DiskAction::List => {
            let vm_dir = format!("{}/vm", data_dir);
            println!("VM Disks:");
            let mut found = false;
            // Scan all VM directories for disk images
            if let Ok(entries) = std::fs::read_dir(&vm_dir) {
                for entry in entries.flatten() {
                    let vm_path = entry.path();
                    if !vm_path.is_dir() {
                        continue;
                    }
                    let vm_name = vm_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if vm_name == "isos" || vm_name == "shared" {
                        continue;
                    }
                    if let Ok(files) = std::fs::read_dir(&vm_path) {
                        for file in files.flatten() {
                            let fpath = file.path();
                            let fname = fpath.file_name().unwrap_or_default().to_string_lossy();
                            if fname.ends_with(".qcow2")
                                || fname.ends_with(".img")
                                || fname.ends_with(".raw")
                            {
                                let size = fpath.metadata().map(|m| m.len()).unwrap_or(0);
                                let size_str = if size > 1_073_741_824 {
                                    format!("{:.1} GB", size as f64 / 1_073_741_824.0)
                                } else if size > 1_048_576 {
                                    format!("{:.1} MB", size as f64 / 1_048_576.0)
                                } else {
                                    format!("{} B", size)
                                };
                                println!("  [{}] {} ({})", vm_name, fname, size_str);
                                found = true;
                            }
                        }
                    }
                }
            }
            // Also scan disks/ directory
            let disks_dir = format!("{}/vm/disks", data_dir);
            if let Ok(entries) = std::fs::read_dir(&disks_dir) {
                for entry in entries.flatten() {
                    let fpath = entry.path();
                    let fname = fpath.file_name().unwrap_or_default().to_string_lossy();
                    if fname.ends_with(".qcow2") || fname.ends_with(".img") {
                        let size = fpath.metadata().map(|m| m.len()).unwrap_or(0);
                        let size_str = if size > 1_073_741_824 {
                            format!("{:.1} GB", size as f64 / 1_073_741_824.0)
                        } else if size > 1_048_576 {
                            format!("{:.1} MB", size as f64 / 1_048_576.0)
                        } else {
                            format!("{} B", size)
                        };
                        println!("  [disks] {} ({})", fname, size_str);
                        found = true;
                    }
                }
            }
            if !found {
                println!(
                    "  No disks found. Create one with: praxis vm disk create <path> --size 40G"
                );
            }
        }
        DiskAction::Info { path } => {
            let output = tokio::process::Command::new("qemu-img")
                .args(["info", &path])
                .output()
                .await?;
            if output.status.success() {
                println!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Error: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        DiskAction::Resize { path, size } => {
            let output = tokio::process::Command::new("qemu-img")
                .args(["resize", &path, &size])
                .output()
                .await?;
            if output.status.success() {
                println!("Disk resized: {} -> {}", path, size);
            } else {
                eprintln!("Error: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        DiskAction::Convert {
            source,
            target,
            format,
        } => {
            println!("Converting {} -> {} ({})", source, target, format);
            let output = tokio::process::Command::new("qemu-img")
                .args(["convert", "-f", "qcow2", "-O", &format, &source, &target])
                .output()
                .await?;
            if output.status.success() {
                println!("Disk converted: {} -> {}", source, target);
            } else {
                eprintln!("Error: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
    }
    Ok(())
}
