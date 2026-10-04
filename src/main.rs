use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "praxis")]
#[command(about = "AI Agent Platform — Praxis")]
#[command(subcommand_required = false)]
enum Cli {
    /// Start all services
    Run {
        /// Master key password for encrypted secrets (non-interactive mode)
        #[arg(long)]
        password: Option<String>,
        /// Disable Discord bot
        #[arg(long)]
        no_discord: bool,
        /// Disable dashboard
        #[arg(long)]
        no_dashboard: bool,
        /// Prepared project root for verified actions (overrides WORKSPACE_DIR).
        /// Relative paths resolve from ROOT_DIR, not the executable directory.
        #[arg(long, value_name = "PROJECT")]
        workspace_dir: Option<PathBuf>,
    },
    /// Pair a Discord user with a pairing code
    Pair {
        #[arg(value_name = "CODE")]
        code: String,
    },
    /// Interactive onboarding setup
    Onboard {
        #[arg(long)]
        interactive: bool,
    },
    /// Restore missing bundled assets without reading/changing configuration or databases
    RepairAssets {
        /// Installation working directory (the directory used by praxis run)
        #[arg(long, default_value = ".")]
        directory: std::path::PathBuf,
        /// Also update bundled dashboard files, backing up changed files first
        #[arg(long)]
        update_dashboard: bool,
        /// Overwrite ALL existing bundled assets (not just dashboard), use with caution
        #[arg(long)]
        overwrite: bool,
    },
    /// Recover interrupted host file patches offline, preserving conflicting files
    RecoverPatches {
        #[arg(long, default_value = ".")]
        directory: PathBuf,
    },
    /// Manage the Praxis system service
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Manage the offline skill metadata index (no secrets, providers or services)
    Skill {
        #[arg(long, default_value = ".", global = true)]
        directory: std::path::PathBuf,
        /// Index storage directory (default: DIRECTORY/data); match DATA_DIR for runtime
        #[arg(long, global = true)]
        data_dir: Option<std::path::PathBuf>,
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Manage plugins
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Manage QEMU virtual machines
    Vm {
        #[command(subcommand)]
        action: VmAction,
    },
    /// Create a backup of all Praxis data
    Backup {
        /// Output file path (default: praxis-backup-YYYY-MM-DD.tar.gz)
        #[arg(long, short)]
        output: Option<String>,
        /// Exclude VM disk images (makes backup much smaller)
        #[arg(long)]
        no_disks: bool,
        /// Exclude ISO files
        #[arg(long)]
        no_isos: bool,
    },
    /// Restore Praxis from a backup file
    Restore {
        /// Path to backup file
        file: String,
        /// Skip confirmation prompt
        #[arg(long, short)]
        yes: bool,
    },
    /// Open the terminal chat UI. Connects to a running `praxis run` instance
    /// to send messages, but reads history directly from the local database
    /// so previous conversations are visible immediately on launch.
    Chat {
        /// Connect to a remote Praxis gateway (e.g. http://host:3537)
        #[arg(long)]
        gateway_url: Option<String>,
        /// Gateway API key (or set PRAXIS_GATEWAY_KEY to avoid shell history)
        #[arg(long)]
        gateway_key: Option<String>,
    },
}

#[derive(clap::Subcommand)]
enum ServiceAction {
    /// Install Praxis as a system service (systemd)
    Install,
    /// Stop the Praxis service
    Stop,
    /// Start the Praxis service
    Start,
    /// Show Praxis service logs
    Logs {
        /// Follow logs in real-time
        #[arg(long, short)]
        follow: bool,
        /// Number of lines to show
        #[arg(long, short = 'n', default_value = "100")]
        lines: usize,
    },
}

#[derive(clap::Subcommand)]
enum SkillAction {
    /// Rebuild the metadata index, or refresh only one folder relative to skills/
    Index {
        #[arg(long)]
        folder: Option<String>,
    },
    /// Search metadata as a human operator (includes hidden and user-only skills)
    Search {
        query: String,
        #[arg(long, default_value = "5")]
        limit: usize,
    },
    /// Browse a bounded page; pass the returned next_after to continue
    List {
        #[arg(long, default_value = "")]
        after: String,
        #[arg(long, default_value = "10")]
        limit: usize,
    },
}

#[derive(clap::Subcommand)]
enum PluginAction {
    /// Install a plugin from a local directory
    Install {
        /// Path to the plugin directory (must contain plugin.json)
        #[arg(value_name = "PLUGIN_PATH")]
        path: String,
    },
    /// Uninstall a plugin and clean up its context and secrets
    Uninstall {
        /// Name of the plugin to uninstall
        #[arg(value_name = "PLUGIN_NAME")]
        name: String,
    },
    /// List installed plugins
    List,
}

#[derive(clap::Subcommand)]
enum VmAction {
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
enum DiskAction {
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

fn initialize_tls_provider() {
    // Dependencies enable both ring and aws-lc-rs, making Rustls autodetection
    // ambiguous. Select the backend declared in Cargo.toml before creating any
    // TLS clients or servers. Err only means a provider is already installed;
    // leave that existing process-wide choice intact.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main]
async fn main() {
    initialize_tls_provider();

    if let Err(e) = run().await {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

fn load_dotenv() {
    if dotenvy::dotenv().is_err() {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let env_path = exe_dir.join(".env");
                if env_path.exists() {
                    let _ = dotenvy::from_path(&env_path);
                }
            }
        }
    }
    pin_install_root();
}

/// Praxis reads templates/, contexts/, plugins/ and skills/ relative to its
/// installation root. Resolve that root once and make it the process working
/// directory so `~/praxis/praxis` started from any cwd (or a service) uses
/// `~/praxis/templates`, never the directory the shell happened to be in.
/// Precedence: ROOT_DIR, then the cwd if it holds templates/, then the
/// executable's directory if it does, otherwise the cwd unchanged.
fn pin_install_root() {
    let root = praxis::config::resolve_install_root(
        std::env::var_os("ROOT_DIR").map(PathBuf::from),
        std::env::current_dir().ok(),
        std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
    );
    let Some(root) = root else { return };
    if std::env::set_current_dir(&root).is_ok() {
        std::env::set_var("ROOT_DIR", &root);
    } else {
        eprintln!("Warning: cannot use installation root {}", root.display());
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Cli::RecoverPatches { directory } = &cli {
        let report = praxis::tools::apply_patch::recover(directory).await?;
        println!("{report}");
        let report: serde_json::Value = serde_json::from_str(&report)?;
        anyhow::ensure!(report["outcome"] != "conflict", "Recovery conflicts require operator resolution");
        return Ok(());
    }
    // Offline repair must not load .env, unlock secrets, initialize a database
    // or start services. It only touches the explicit public-asset allow-list.
    if let Cli::RepairAssets { directory, update_dashboard, overwrite } = &cli {
        let report = praxis::assets::install(directory, *update_dashboard, *overwrite)?;
        println!("Assets in {}: {} created, {} preserved, {} updated.", directory.display(), report.created.len(), report.preserved.len(), report.updated.len());
        if let Some(backup) = &report.backup_dir { println!("Previous files backed up to: {}", backup.display()); }
        println!("Configuration, secrets, databases and service state were not changed.");
        return Ok(());
    }
    if let Cli::Skill { directory, data_dir, action } = &cli {
        let data = data_dir.clone().unwrap_or_else(|| directory.join("data"));
        let mut index = praxis::skills::SkillIndex::open(&data, &directory.join("skills"))?;
        let result = match action {
            SkillAction::Index { folder: Some(folder) } => serde_json::to_value(index.refresh(folder)?)?,
            SkillAction::Index { folder: None } => serde_json::to_value(index.rebuild()?)?,
            SkillAction::Search { query, limit } => {
                index.ensure_indexed()?;
                serde_json::to_value(index.search(query, *limit, true)?)?
            }
            SkillAction::List { after, limit } => {
                index.ensure_indexed()?;
                serde_json::to_value(index.browse(after, *limit, true)?)?
            }
        };
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    load_dotenv();

    // Service and Plugin commands don't need logging setup
    if let Cli::Service { action } = &cli {
        return handle_service_action(action).await;
    }
    if let Cli::Plugin { action } = &cli {
        return handle_plugin_action(action).await;
    }

    // The TUI takes over stdout (alternate screen) so we mustn't write
    // tracing logs there. Initialise logging with file-only output and
    // jump straight to the chat module.
    if let Cli::Chat { gateway_url, gateway_key } = &cli {
        let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "./logs".to_string());
        let _ = std::fs::create_dir_all(&log_dir);
        let file_appender = tracing_appender::rolling::daily(&log_dir, "praxis-tui.log");
        let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
        use tracing_subscriber::prelude::*;
        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(file_writer)
                    .with_ansi(false),
            )
            .try_init();
        // Keep the guard alive for the duration of the run so logs flush.
        let _keep = _guard;
        return praxis::tui::run_chat(gateway_url.clone(), gateway_key.clone()).await;
    }

    // Set up logging with file rotation
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "./logs".to_string());
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "praxis.log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let stdout_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false);

    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    match cli {
        Cli::Run {
            password,
            no_discord,
            no_dashboard,
            workspace_dir,
        } => run_services(password, !no_discord, !no_dashboard, workspace_dir).await,
        Cli::Pair { code } => pair_command(&code).await,
        Cli::Onboard { interactive: true } => praxis::onboard::run_interactive_onboard(),
        Cli::Onboard { interactive: false } => {
            anyhow::bail!("Onboard requires --interactive flag");
        }
        Cli::Vm { action } => return handle_vm_action(action).await,
        Cli::Backup {
            output,
            no_disks,
            no_isos,
        } => return handle_backup(output, no_disks, no_isos).await,
        Cli::Restore { file, yes } => return handle_restore(&file, yes).await,
        Cli::Chat { .. } => unreachable!(),
        Cli::RepairAssets { .. } => unreachable!(),
        Cli::RecoverPatches { .. } => unreachable!(),
        Cli::Service { .. } => unreachable!(),
        Cli::Plugin { .. } => unreachable!(),
        Cli::Skill { .. } => unreachable!(),
    }
}

async fn run_services(
    cli_password: Option<String>,
    enable_discord: bool,
    enable_dashboard: bool,
    workspace_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Master-Key-Zustellung. Reihenfolge = Expositionsrisiko aufsteigend:
    //   1. MASTER_KEY_FILE (Container-Standard: Root-Entrypoint kopiert den
    //      Key auf ein tmpfs-File 0400, Praxis liest+LÖSCHT es — der Key ist
    //      nie in argv/env, und die Datei existiert nur im Millisekunden-
    //      Fenster des Starts, bevor der Agent überhaupt Befehle ausführen
    //      kann. `sh -c env` und /proc/<pid>/environ bleiben sauber.)
    //   2. MASTER_KEY-Env (einfach, aber vom Agenten lesbar — nur für
    //      Umgebungen ohne Agent-Shell-Zugang)
    //   3. --password argv (nur interaktiv/legacy; sichtbar in /proc/cmdline)
    // Nach dem Lesen werden die Env-Variablen entfernt, damit kind-Prozesse
    // (Agent-Terminal) sie nicht erben.
    let delivered_master_key = || -> Option<String> {
        let path = std::env::var("MASTER_KEY_FILE").ok()?;
        std::env::remove_var("MASTER_KEY_FILE");
        let key = std::fs::read_to_string(&path).ok()?.trim().to_string();
        if key.is_empty() {
            return None;
        }
        // Einmal-Zustellung: Datei löschen. Bei ro-Mounts nur loggen —
        // dann mindestens mode 0400 + tmpfs sicherstellen (Deployment).
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::debug!(%e, "MASTER_KEY_FILE nicht löschbar (ro-Mount?) — Agent-Schutz auf VM/mode beruht");
        }
        Some(key)
    };
    let env_master_key = || -> Option<String> {
        let key = std::env::var("MASTER_KEY").ok()?;
        std::env::remove_var("MASTER_KEY");
        Some(key)
    };

    let master_password = if praxis::db::secrets::has_secrets() {
        let stored_hash = std::env::var("PRAXIS_MASTER_KEY_HASH").ok();

        let password = if let Some(pass) = cli_password {
            pass
        } else if let Some(pass) = delivered_master_key().or_else(env_master_key) {
            pass
        } else {
            // Container-/Daemon-Betrieb ohne TTY: klare, handlungsfähige
            // Meldung statt eines Prompts, der im Container hängt bzw.
            // kryptisch abstirbt („alles per Env“, VPS-Deploy).
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                anyhow::bail!(
                    "Encrypted secrets (secrets.enc2) vorhanden, aber kein MASTER_KEY geliefert. \
                     Für Container-Betrieb MASTER_KEY_FILE setzen (Compose: secrets + Root-Entrypoint) \
                     und neu starten."
                );
            }
            rpassword::prompt_password("Enter MASTER_KEY to unlock secrets: ")
                .map_err(|e| anyhow::anyhow!("Failed to read password: {}", e))?
        };

        // If running as service with stored Argon2 hash, verify against hash first
        if let Some(ref hash) = stored_hash {
            if !praxis::db::enc2::verify_master_key_hash(&password, hash) {
                anyhow::bail!("Invalid MASTER_KEY (hash mismatch)");
            }
        } else {
            // Interactive mode: verify by attempting decryption
            if !praxis::db::enc2::verify_password(&password) {
                anyhow::bail!("Invalid MASTER_KEY");
            }
        }

        Some(password)
    } else if let Some(password) = delivered_master_key().or_else(env_master_key) {
        // Erststart ohne Store, aber mit geliefertem Master-Key: leeren
        // verschlüsselten Store anlegen — headless, ohne Onboarding-Prompt.
        // Die eigentlichen Secrets (Discord-Token, Provider-Keys) trägt man
        // danach über das Dashboard (Settings) ein; sie landen verschlüsselt
        // im Store und sind für die Agent-Shell nie lesbar.
        praxis::db::secrets::save_secrets(&praxis::db::secrets::Secrets::default(), &password)?;
        tracing::info!(
            "Erststart: leerer verschlüsselter Secret-Store angelegt — Secrets über das Dashboard (Settings) befüllen"
        );
        Some(password)
    } else {
        None
    };

    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let db = praxis::db::Database::new(Path::new(&data_dir))?;

    tracing::info!("Starting Praxis v{}...", env!("CARGO_PKG_VERSION"));

    // Initialize default tools if needed
    if let Err(e) = praxis::db::tools::init_default_tools(&db) {
        tracing::warn!("Failed to init default tools: {}", e);
    }

    // Ensure the delegations table exists before the first delegate_task call.
    praxis::gateway::delegation::ensure_delegations_table(&db);

    // Sync templates from disk to database
    if let Err(error) = praxis::runtime::templates::sync_from_disk(&db, Path::new("templates")) {
        tracing::warn!(%error, "Template catalog synchronization failed");
    }

    // Load secrets (encrypted or plaintext)
    let mut secrets = if let Some(ref password) = master_password {
        praxis::db::secrets::load_secrets_with_password(password)?
    } else {
        praxis::db::secrets::Secrets {
            discord_bot_token: std::env::var("DISCORD_BOT_TOKEN").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openrouter_api_key: std::env::var("OPENROUTER_API_KEY").ok(),
            ollama_api_key: std::env::var("OLLAMA_API_KEY").ok(),
            llamacpp_api_key: std::env::var("LLAMACPP_API_KEY").ok(),
            minimax_api_key: std::env::var("MINIMAX_API_KEY").ok(),
            mimo_api_key: std::env::var("MIMO_API_KEY").ok(),
            elevenlabs_api_key: std::env::var("ELEVENLABS_API_KEY").ok(),
            gateway_api_key: std::env::var("GATEWAY_API_KEY").ok(),
            dashboard_admin_password: std::env::var("DASHBOARD_ADMIN_PASSWORD").ok(),
            ..Default::default()
        }
    };

    // Create placeholder secrets for plugins
    let plugins_dir = std::env::var("PLUGINS_DIR").unwrap_or_else(|_| "./plugins".to_string());
    let plugin_registry = praxis::plugins::load_all_plugins(std::path::Path::new(&plugins_dir));
    for key in plugin_registry.collect_secrets() {
        if !secrets.custom.contains_key(&key) && secrets.plugin_secret(&key).is_none() {
            tracing::info!(key = %key, "Creating placeholder secret for plugin");
            secrets.custom.insert(key, "CHANGE_ME".to_string());
        }
    }

    // Initialize global secrets
    praxis::db::secrets::init_secrets(secrets.clone());
    if let Some(password) = master_password.as_deref() {
        praxis::db::secrets::retain_master_password(password);
    }

    // Do not leave a second, unprotected password allocation alive in this
    // long-running function after optional retention has been decided.
    if let Some(mut password) = master_password {
        zeroize::Zeroize::zeroize(&mut password);
    }

    // Build config, overriding sensitive fields from secrets if available
    let mut config = praxis::config::Config::from_env();
    if let Some(project) = workspace_dir {
        config.workspace_dir = Some(project.into_os_string().into_string()
            .map_err(|_| anyhow::anyhow!("--workspace-dir must be a UTF-8 project path"))?);
    }
    config.apply_secrets(&secrets);
    config.ensure_generated();
    config.validate()?;
    let workspace = config.workspace_root()?;
    tracing::info!(root = %workspace.display(), "Verified action workspace selected");
    // Fail before starting VMs/Discord when the selected provider is missing.
    praxis::gateway::llm::LLMRouter::new(&config, &secrets).validate_configuration()?;

    // Enable VM tools if VM=true in config
    if config.vm_enabled {
        tracing::info!("VM mode enabled — activating VM tools");
        for tool_name in &[
            "vm_start",
            "vm_stop",
            "vm_shell",
            "vm_keys",
            "vm_screenshot",
            "vm_file_transfer",
            "vm_snapshot",
            "vm_shared_folder",
            "vm_mouse",
            "vm_look_screenshot",
            "vm_install",
        ] {
            let _ = praxis::db::tools::set_enabled(&db, tool_name, true);
        }
        praxis::tools::vm_tools::init_vm_manager(&config.data_dir);
        let _ = std::fs::create_dir_all(format!("{}/shared", config.data_dir));
        tracing::info!(
            "VM tools enabled, shared folder at {}/shared, mode: {}",
            config.data_dir,
            config.vm_mode
        );
        // Auto-start default VM
        let vm_manager = praxis::tools::vm_tools::get_vm_manager().await;
        if let Some(manager) = vm_manager {
            let vm_name = "praxis-vm";
            let mut vm_config = praxis::vm::VmConfig::default_for_name(
                vm_name,
                &config.data_dir,
                1,
                &config.vm_arch,
            );
            vm_config.cpu_cores = config.vm_cpu_cores;
            vm_config.ram_mb = config.vm_ram_mb;
            vm_config.disk_size = config.vm_disk_size.clone();
            vm_config.shared_folders.push(praxis::vm::SharedFolder {
                host_path: format!("{}/shared", config.data_dir),
                mount_tag: "praxis-shared".to_string(),
                mount_point: "/mnt/shared".to_string(),
                readonly: false,
            });
            match manager.start_vm(vm_config).await {
                Ok(msg) => tracing::info!("{}", msg),
                Err(e) => tracing::warn!("Auto-start VM failed (non-fatal): {}", e),
            }
        }
    }

    // Gateway
    let gateway_db = db.clone();
    let gateway_config = config.clone();
    let gateway_handle = tokio::spawn(async move {
        if let Err(e) = praxis::gateway::start(gateway_db, gateway_config).await {
            tracing::error!("Gateway error: {}", e);
        }
    });

    // Dashboard (optional)
    if enable_dashboard {
        let dashboard_db = db.clone();
        let dashboard_port = config.dashboard_port;
        tokio::spawn(async move {
            let server = praxis::dashboard::DashboardServer::new(dashboard_port, config.dashboard_tls, dashboard_db, &config.data_dir);
            if let Err(e) = server.start().await {
                tracing::error!("Dashboard error: {}", e);
            }
        });
        tracing::info!("Dashboard starting on port {}", config.dashboard_port);
    }

    // Discord (optional)
    if enable_discord {
        let discord_db = db.clone();
        let discord_secrets = secrets.clone();
        tokio::spawn(async move {
            if let Err(e) = praxis::discord::start_with_secrets(discord_db, discord_secrets).await {
                tracing::error!("Discord error: {}", e);
            }
        });
        tracing::info!("Discord bot starting...");
    }

    tracing::info!("Praxis started successfully");
    gateway_handle.await?;

    Ok(())
}

async fn pair_command(code: &str) -> anyhow::Result<()> {
    load_dotenv();

    let config = praxis::config::Config::from_env();
    let db = praxis::db::Database::new(Path::new(&config.data_dir))?;

    match db.get_pending_pairing(code) {
        Ok(Some(pending)) => {
            let user_id = uuid::Uuid::new_v4().to_string();
            db.create_pairing(&user_id, &pending.discord_user_id, None)?;
            db.delete_pending_pairing(code)?;
            println!("Pairing successful!");
            println!("User ID: {}", user_id);
        }
        Ok(None) => {
            println!("Invalid or expired pairing code");
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }

    Ok(())
}

async fn handle_service_action(action: &ServiceAction) -> anyhow::Result<()> {
    let binary_path = std::env::current_exe()
        .map_err(|_| anyhow::anyhow!("Could not determine binary path"))?
        .to_string_lossy()
        .to_string();

    let working_dir = std::env::current_dir()
        .map_err(|_| anyhow::anyhow!("Could not determine working directory"))?
        .to_string_lossy()
        .to_string();

    match action {
        ServiceAction::Install => {
            println!("Enter the MASTER_KEY password for the service:");
            let password = rpassword::prompt_password("MASTER_KEY: ")
                .map_err(|e| anyhow::anyhow!("Failed to read password: {}", e))?;

            if password.is_empty() {
                anyhow::bail!("MASTER_KEY cannot be empty");
            }

            let hash = praxis::db::enc2::hash_master_key(&password);

            let service_content = format!(
                r#"[Unit]
Description=Praxis AI Agent Platform
After=network.target

[Service]
Type=simple
User={user}
WorkingDirectory={working_dir}
ExecStart={binary_path} run
Restart=on-failure
RestartSec=5
Environment=PRAXIS_MASTER_KEY_HASH={hash}

[Install]
WantedBy=multi-user.target
"#,
                user = std::env::var("USER").unwrap_or_else(|_| "root".to_string()),
                working_dir = working_dir,
                binary_path = binary_path,
                hash = hash,
            );

            let service_path = "/etc/systemd/system/praxis.service";
            std::fs::write(service_path, service_content)?;

            std::process::Command::new("systemctl")
                .args(["daemon-reload"])
                .status()?;

            println!("Service installed at {}", service_path);
            println!("MASTER_KEY stored as Argon2 hash (not plaintext).");
            println!("To start: sudo systemctl enable praxis && sudo systemctl start praxis");
        }
        ServiceAction::Stop => {
            let status = std::process::Command::new("systemctl")
                .args(["stop", "praxis"])
                .status()?;
            if status.success() {
                println!("Praxis service stopped");
            } else {
                anyhow::bail!("Failed to stop service. Is it installed?");
            }
        }
        ServiceAction::Start => {
            let status = std::process::Command::new("systemctl")
                .args(["start", "praxis"])
                .status()?;
            if status.success() {
                println!("Praxis service started");
            } else {
                anyhow::bail!("Failed to start service. Is it installed?");
            }
        }
        ServiceAction::Logs { follow, lines } => {
            let mut args = vec!["-u", "praxis", "--no-pager"];
            if *follow {
                args.push("-f");
            }
            args.push("-n");
            let lines_str = lines.to_string();
            args.push(&lines_str);

            let status = std::process::Command::new("journalctl")
                .args(&args)
                .status()?;
            if !status.success() {
                anyhow::bail!("Failed to retrieve logs. Is the service installed?");
            }
        }
    }

    Ok(())
}

async fn handle_plugin_action(action: &PluginAction) -> anyhow::Result<()> {
    let plugins_dir = std::env::var("PLUGINS_DIR").unwrap_or_else(|_| "./plugins".to_string());
    let plugins_path = Path::new(&plugins_dir);

    match action {
        PluginAction::Install { path } => {
            let src = Path::new(path);
            if !src.is_dir() {
                anyhow::bail!("Plugin path '{}' is not a directory", path);
            }
            let manifest = src.join("plugin.json");
            if !manifest.exists() {
                anyhow::bail!("No plugin.json found in '{}'", path);
            }

            let data = std::fs::read_to_string(&manifest)?;
            let plugin: praxis::plugins::Plugin = serde_json::from_str(&data)?;

            let dest = plugins_path.join(&plugin.name);
            if dest.exists() {
                anyhow::bail!(
                    "Plugin '{}' already installed at '{}'",
                    plugin.name,
                    dest.display()
                );
            }

            std::fs::create_dir_all(plugins_path)?;
            copy_dir_recursive(src, &dest)?;

            println!("Plugin '{}' installed to {}", plugin.name, dest.display());
            println!("  Tools: {}", plugin.tools.len());
            println!("  Context vars: {}", plugin.context.len());
            println!("  Secrets: {}", plugin.secrets.len());
            if !plugin.secrets.is_empty() {
                println!("  Configure secrets via dashboard or API before use.");
            }
        }
        PluginAction::Uninstall { name } => {
            let plugin_dir = plugins_path.join(name);
            if !plugin_dir.exists() {
                anyhow::bail!("Plugin '{}' not found at '{}'", name, plugin_dir.display());
            }

            let manifest = plugin_dir.join("plugin.json");
            let (context_keys, secret_keys) = if manifest.exists() {
                let data = std::fs::read_to_string(&manifest)?;
                let plugin: praxis::plugins::Plugin = serde_json::from_str(&data)?;
                let ctx_keys: Vec<String> = plugin.context.keys().cloned().collect();
                (ctx_keys, plugin.secrets)
            } else {
                (vec![], vec![])
            };

            std::fs::remove_dir_all(&plugin_dir)?;
            println!("Removed plugin directory: {}", plugin_dir.display());

            let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
            if Path::new(&data_dir).exists() {
                if let Ok(db) = praxis::db::Database::new(Path::new(&data_dir)) {
                    if !secret_keys.is_empty() {
                        if praxis::db::secrets::has_secrets() {
                            println!("Note: Secrets ({}) are stored encrypted. Remove them manually via dashboard.", secret_keys.join(", "));
                        }
                    }

                    if !context_keys.is_empty() {
                        let conn = db.conn();
                        let mut stmt = conn.prepare("SELECT user_id, data FROM contexts")?;
                        let rows = stmt.query_map([], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?;

                        let mut cleaned = 0;
                        for row in rows {
                            let (user_id, data) = row?;
                            if let Ok(mut ctx) = serde_json::from_str::<serde_json::Value>(&data) {
                                if let Some(obj) =
                                    ctx.get_mut("custom_data").and_then(|v| v.as_object_mut())
                                {
                                    let before = obj.len();
                                    for key in &context_keys {
                                        obj.remove(key);
                                    }
                                    if obj.len() < before {
                                        conn.execute(
                                            "UPDATE contexts SET data = ?1, updated_at = datetime('now') WHERE user_id = ?2",
                                            rusqlite::params![serde_json::to_string(&ctx)?, user_id],
                                        )?;
                                        cleaned += 1;
                                    }
                                }
                            }
                        }
                        if cleaned > 0 {
                            println!(
                                "Cleaned context variables ({}) from {} user(s)",
                                context_keys.join(", "),
                                cleaned
                            );
                        }
                    }
                }
            }

            println!("Plugin '{}' uninstalled.", name);
        }
        PluginAction::List => {
            if !plugins_path.exists() {
                println!("No plugins directory found at '{}'", plugins_dir);
                return Ok(());
            }

            let mut found = false;
            for entry in std::fs::read_dir(plugins_path)? {
                let entry = entry?;
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let manifest = dir.join("plugin.json");
                if !manifest.exists() {
                    continue;
                }
                match std::fs::read_to_string(&manifest) {
                    Ok(data) => match serde_json::from_str::<praxis::plugins::Plugin>(&data) {
                        Ok(plugin) => {
                            let status = if plugin.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            };
                            println!(
                                "  {} v{} [{}] — {} tool(s), {} secret(s)",
                                plugin.name,
                                plugin.version,
                                status,
                                plugin.tools.len(),
                                plugin.secrets.len()
                            );
                            found = true;
                        }
                        Err(e) => {
                            println!("  {} — invalid manifest: {}", dir.display(), e);
                            found = true;
                        }
                    },
                    Err(_) => continue,
                }
            }
            if !found {
                println!("No plugins installed.");
            }
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            std::fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

async fn handle_vm_action(action: VmAction) -> anyhow::Result<()> {
    load_dotenv();
    let config = praxis::config::Config::from_env();
    if !config.vm_enabled {
        anyhow::bail!("VM not enabled. Set VM_ENABLED=true in .env");
    }
    let manager = praxis::tools::vm_tools::init_vm_manager(&config.data_dir);

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
                praxis::vm::VmConfig::default_for_name(&name, &config.data_dir, 1, &config.vm_arch);
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
            vm_config.shared_folders.push(praxis::vm::SharedFolder {
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
                match praxis::vm::qmp::QmpClient::connect(&qmp_addr).await {
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

async fn handle_backup(
    output: Option<String>,
    no_disks: bool,
    no_isos: bool,
) -> anyhow::Result<()> {
    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H%M%S");
    let default_name = format!("praxis-backup-{}.tar.gz", timestamp);
    let output_path = output.unwrap_or(default_name);

    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let plugins_dir = std::env::var("PLUGINS_DIR").unwrap_or_else(|_| "./plugins".to_string());

    println!("Creating backup: {}", output_path);
    println!("Data dir: {}", data_dir);

    // Build list of paths to include
    let mut includes: Vec<String> = Vec::new();

    // .env file
    if std::path::Path::new(".env").exists() {
        includes.push(".env".to_string());
    }

    // Templates
    if std::path::Path::new("templates").exists() {
        includes.push("templates".to_string());
    }

    // Skills
    if std::path::Path::new("skills").exists() {
        includes.push("skills".to_string());
    }

    // Plugins
    if std::path::Path::new(&plugins_dir).exists() {
        includes.push(plugins_dir.clone());
    }

    // Context language files
    // Preserve obsolete installations in backups; new workflows use contexts/.
    if std::path::Path::new("contextlanguage").exists() {
        includes.push("contextlanguage".to_string());
    }

    // Data directory (database, secrets, contexts, tools config)
    if std::path::Path::new(&data_dir).exists() {
        includes.push(format!("{}/praxis.db", data_dir));
        includes.push(format!("{}/tools.json", data_dir));

        // Secrets (encrypted)
        let secrets_path = format!("{}/secrets.enc2", data_dir);
        if std::path::Path::new(&secrets_path).exists() {
            includes.push(secrets_path);
        }
        let secrets_plain = format!("{}/secrets.json", data_dir);
        if std::path::Path::new(&secrets_plain).exists() {
            includes.push(secrets_plain);
        }

        // VM config
        let vm_dir = format!("{}/vm", data_dir);
        if std::path::Path::new(&vm_dir).exists() {
            // Always include VM configs and installation_disks.json
            includes.push(format!("{}/vm/installation_disks.json", data_dir));

            // Include per-VM configs (qmp.sock paths, etc are in the config)
            if let Ok(entries) = std::fs::read_dir(&vm_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        if name == "isos" || name == "shared" || name == "disks" {
                            continue;
                        }
                        // Include screenshots and secrets
                        includes.push(format!("{}/vm/{}/screenshots", data_dir, name));
                        includes.push(format!("{}/vm/{}/secrets", data_dir, name));
                    }
                }
            }

            // VM disks (optional, can be huge)
            if !no_disks {
                if let Ok(entries) = std::fs::read_dir(&vm_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            let name = path.file_name().unwrap_or_default().to_string_lossy();
                            if name == "isos" || name == "shared" || name == "disks" {
                                continue;
                            }
                            let disk = format!("{}/vm/{}/disk.qcow2", data_dir, name);
                            if std::path::Path::new(&disk).exists() {
                                includes.push(disk);
                            }
                        }
                    }
                }
            }

            // ISOs (optional)
            if !no_isos {
                let iso_dir = format!("{}/vm/isos", data_dir);
                if std::path::Path::new(&iso_dir).exists() {
                    includes.push(iso_dir);
                }
            }
        }

        // Shared folder
        let shared_dir = format!("{}/shared", data_dir);
        if std::path::Path::new(&shared_dir).exists() {
            includes.push(shared_dir);
        }
    }

    if includes.is_empty() {
        anyhow::bail!("Nothing to backup. No data found.");
    }

    // Build tar command
    let mut cmd = tokio::process::Command::new("tar");
    cmd.arg("czf").arg(&output_path);

    // Exclude patterns
    cmd.arg("--exclude").arg("*.log");
    cmd.arg("--exclude").arg("logs/");

    for path in &includes {
        if std::path::Path::new(path).exists() {
            cmd.arg(path);
        }
    }

    let output = cmd.output().await?;
    if output.status.success() {
        let size = std::fs::metadata(&output_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let size_str = if size > 1_073_741_824 {
            format!("{:.1} GB", size as f64 / 1_073_741_824.0)
        } else {
            format!("{:.1} MB", size as f64 / 1_048_576.0)
        };
        println!("Backup created: {} ({})", output_path, size_str);
        println!("Includes: {} paths", includes.len());
        if no_disks {
            println!("  (VM disks excluded)");
        }
        if no_isos {
            println!("  (ISOs excluded)");
        }
    } else {
        anyhow::bail!("tar failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

async fn handle_restore(file: &str, yes: bool) -> anyhow::Result<()> {
    if !std::path::Path::new(file).exists() {
        anyhow::bail!("Backup file not found: {}", file);
    }

    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    let size_str = if size > 1_073_741_824 {
        format!("{:.1} GB", size as f64 / 1_073_741_824.0)
    } else {
        format!("{:.1} MB", size as f64 / 1_048_576.0)
    };

    println!("Restoring from: {} ({})", file, size_str);
    println!();
    println!("This will overwrite:");
    println!("  - .env (if included)");
    println!("  - data/ (database, secrets, VM configs)");
    println!("  - templates/, skills/, plugins/");
    println!();

    if !yes {
        println!("Continue? (y/N)");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Extract with tar
    let output = tokio::process::Command::new("tar")
        .args(["xzf", file])
        .output()
        .await?;

    if output.status.success() {
        println!("Restore complete!");
        println!("Run ./praxis run to start with the restored data.");
    } else {
        anyhow::bail!("tar failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_provider_initialization_for_clients_and_servers() {
        initialize_tls_provider();

        // Construct TLS configurations only: no sockets, credentials or API calls.
        let _client = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        let _server = rustls::ServerConfig::builder().with_no_client_auth();
        reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .expect("HTTPS client construction should not panic with Discord voice enabled");
    }

    #[test]
    fn test_tls_provider_initialization_is_idempotent() {
        initialize_tls_provider();
        let original = rustls::crypto::CryptoProvider::get_default()
            .expect("startup must select a TLS provider")
            .clone();

        initialize_tls_provider();
        let current = rustls::crypto::CryptoProvider::get_default().unwrap();
        assert!(std::sync::Arc::ptr_eq(&original, current));
    }

    #[test]
    fn test_cli_parsing_recover_patches() {
        let cli = Cli::try_parse_from(["praxis", "recover-patches", "--directory", "/synthetic/workspace"]).unwrap();
        assert!(matches!(cli, Cli::RecoverPatches { directory } if directory == PathBuf::from("/synthetic/workspace")));
    }

    #[test]
    fn test_cli_parsing_repair_assets() {
        let cli = Cli::try_parse_from(["praxis", "repair-assets", "--directory", "/synthetic/install", "--update-dashboard"]).unwrap();
        match cli {
            Cli::RepairAssets { directory, update_dashboard, overwrite } => {
                assert_eq!(directory, std::path::PathBuf::from("/synthetic/install"));
                assert!(update_dashboard);
            }
            _ => panic!("Expected RepairAssets"),
        }
        assert!(matches!(Cli::try_parse_from(["praxis", "repair-assets"]).unwrap(), Cli::RepairAssets { update_dashboard: false, .. }));
    }

    #[test]
    fn test_cli_parsing_run() {
        let args = vec!["praxis", "run"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(cli, Cli::Run { .. }));
    }

    #[test]
    fn test_cli_parsing_run_with_explicit_workspace() {
        let cli = Cli::try_parse_from(["praxis", "run", "--workspace-dir", "/synthetic/ir-snake"]).unwrap();
        assert!(matches!(cli, Cli::Run { workspace_dir: Some(path), .. } if path == PathBuf::from("/synthetic/ir-snake")));
    }

    #[test]
    fn test_cli_parsing_run_with_flags() {
        let args = vec!["praxis", "run", "--no-discord", "--no-dashboard"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Run {
                no_discord,
                no_dashboard,
                ..
            } => {
                assert!(no_discord);
                assert!(no_dashboard);
            }
            _ => panic!("Expected Run variant"),
        }
    }

    #[test]
    fn test_cli_parsing_run_with_password() {
        let args = vec!["praxis", "run", "--password", "test123"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Run { password, .. } => {
                assert_eq!(password, Some("test123".to_string()));
            }
            _ => panic!("Expected Run variant"),
        }
    }

    #[test]
    fn test_cli_parsing_pair() {
        let args = vec!["praxis", "pair", "ABCD-1234"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Pair { code } => assert_eq!(code, "ABCD-1234"),
            _ => panic!("Expected Pair variant"),
        }
    }

    #[test]
    fn test_cli_parsing_onboard() {
        let args = vec!["praxis", "onboard", "--interactive"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Onboard { interactive } => assert!(interactive),
            _ => panic!("Expected Run variant"),
        }
    }

    #[test]
    fn test_cli_parsing_service_install() {
        let args = vec!["praxis", "service", "install"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Service { action } => assert!(matches!(action, ServiceAction::Install)),
            _ => panic!("Expected Service variant"),
        }
    }

    #[test]
    fn test_cli_parsing_service_stop() {
        let args = vec!["praxis", "service", "stop"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Service { action } => assert!(matches!(action, ServiceAction::Stop)),
            _ => panic!("Expected Service variant"),
        }
    }

    #[test]
    fn test_cli_parsing_service_start() {
        let args = vec!["praxis", "service", "start"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Service { action } => assert!(matches!(action, ServiceAction::Start)),
            _ => panic!("Expected Service variant"),
        }
    }

    #[test]
    fn test_cli_parsing_service_logs_default() {
        let args = vec!["praxis", "service", "logs"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Service { action } => match action {
                ServiceAction::Logs { follow, lines } => {
                    assert!(!follow);
                    assert_eq!(lines, 100);
                }
                _ => panic!("Expected Logs variant"),
            },
            _ => panic!("Expected Service variant"),
        }
    }

    #[test]
    fn test_cli_parsing_service_logs_follow() {
        let args = vec!["praxis", "service", "logs", "-f", "-n", "50"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli {
            Cli::Service { action } => match action {
                ServiceAction::Logs { follow, lines } => {
                    assert!(follow);
                    assert_eq!(lines, 50);
                }
                _ => panic!("Expected Logs variant"),
            },
            _ => panic!("Expected Service variant"),
        }
    }
}
