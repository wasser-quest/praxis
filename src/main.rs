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
    /// Install the bundled distribution without overwriting operator configuration
    InstallPreset {
        #[arg(value_enum)]
        preset: InstallationPreset,
        #[arg(long, default_value = ".")]
        directory: PathBuf,
        /// Existing DATA_DIR; relative paths resolve from --directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        update_dashboard: bool,
        /// Explicitly enable all VM model tools; does not set VM_ENABLED/start QEMU
        #[arg(long)]
        enable_vm_tools: bool,
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
    /// Manage QEMU virtual machines (optional native package)
    #[command(disable_help_flag = true)]
    Vm {
        /// Arguments interpreted by the installed VM package
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
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

#[derive(Clone, clap::ValueEnum)]
enum InstallationPreset { Compatibility }

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
        /// Run the install hook without asking
        #[arg(long)]
        allow_scripts: bool,
        /// Never run install/uninstall hooks
        #[arg(long)]
        no_scripts: bool,
        /// Assume yes to the interactive hook prompt
        #[arg(long)]
        yes: bool,
        /// Print what would happen without copying, publishing or running hooks
        #[arg(long)]
        dry_run: bool,
    },
    /// Uninstall a plugin and clean up its context and secrets
    Uninstall {
        /// Name of the plugin to uninstall
        #[arg(value_name = "PLUGIN_NAME")]
        name: String,
        /// Run the uninstall hook without asking
        #[arg(long)]
        allow_scripts: bool,
        /// Never run install/uninstall hooks
        #[arg(long)]
        no_scripts: bool,
        /// Assume yes to the interactive hook prompt
        #[arg(long)]
        yes: bool,
        /// Remove the plugin directory even if the uninstall hook fails or changed
        #[arg(long)]
        force: bool,
        /// Tell the uninstall hook to remove plugin data as well
        #[arg(long)]
        purge: bool,
        /// Print what would happen without running hooks or removing files
        #[arg(long)]
        dry_run: bool,
    },
    /// Replace an installed plugin with a new revision from a local directory
    Upgrade {
        /// Path to the new plugin directory (must contain plugin.json)
        #[arg(value_name = "PLUGIN_PATH")]
        path: String,
        /// Run the install hook without asking
        #[arg(long)]
        allow_scripts: bool,
        /// Never run install/uninstall hooks
        #[arg(long)]
        no_scripts: bool,
        /// Assume yes to the interactive hook prompt
        #[arg(long)]
        yes: bool,
        /// Print what would happen without replacing files or running hooks
        #[arg(long)]
        dry_run: bool,
    },
    /// Install the bundled default plugin set (or a custom preset)
    InstallDefault {
        /// Optional preset JSON: {"plugins": ["<dir>", ...]}
        #[arg(long)]
        preset: Option<String>,
        /// Run install hooks without asking
        #[arg(long)]
        allow_scripts: bool,
        /// Never run install/uninstall hooks
        #[arg(long)]
        no_scripts: bool,
        /// Assume yes to the interactive hook prompt
        #[arg(long)]
        yes: bool,
        /// Print what would happen without copying or running hooks
        #[arg(long)]
        dry_run: bool,
    },
    /// Verify installed plugins against praxis.lock.json
    Verify,
    /// Apply a package's declared migrations to its own database (or revert)
    Migrate {
        #[arg(value_name = "PLUGIN_NAME")]
        name: String,
        /// Revert applied migrations in reverse order instead of applying
        #[arg(long)]
        down: bool,
        /// Print the plan without changing the package database
        #[arg(long)]
        dry_run: bool,
    },
    /// Enable an installed plugin
    Enable {
        #[arg(value_name = "PLUGIN_NAME")]
        name: String,
    },
    /// Disable an installed plugin (its tools leave the catalog; files stay)
    Disable {
        #[arg(value_name = "PLUGIN_NAME")]
        name: String,
    },
    /// Grant a plugin a non-tool trust role (tool|data|channel|ui|authority|runtime)
    Trust {
        #[arg(value_name = "PLUGIN_NAME")]
        name: String,
        #[arg(long, default_value = "tool")]
        role: String,
    },
    /// Revoke a plugin's trust grant
    Untrust {
        #[arg(value_name = "PLUGIN_NAME")]
        name: String,
    },
    /// List installed plugins
    List,
    /// List builtin tool packages (runtime_control, shell, memory, …)
    Builtins,
    /// Enable a builtin tool package
    EnableBuiltin {
        #[arg(value_name = "PACKAGE")]
        id: String,
    },
    /// Disable a builtin tool package (its tools leave the catalog; per-tool flags are kept)
    DisableBuiltin {
        #[arg(value_name = "PACKAGE")]
        id: String,
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
    if let Cli::InstallPreset { preset: InstallationPreset::Compatibility, directory, data_dir, update_dashboard, enable_vm_tools } = &cli {
        let report = praxis::assets::install_compatibility(directory, *update_dashboard)?;
        println!("Compatibility assets in {}: {} created, {} preserved, {} updated.", directory.display(), report.created.len(), report.preserved.len(), report.updated.len());
        if let Some(backup) = report.backup_dir { println!("Previous dashboard files backed up to: {}", backup.display()); }
        if *enable_vm_tools {
            let data = data_dir.clone().map(|path| if path.is_absolute() { path } else { directory.join(path) }).unwrap_or_else(|| directory.join("data"));
            let db = praxis::db::Database::new(&data)?;
            praxis::db::tools::init_default_tools(&db)?;
            praxis::db::tools::enable_vm_compatibility(&db)?;
            println!("Enabled 25 VM tool flags in {}. VM_ENABLED remains an explicit operator setting.", data.display());
        } else {
            println!("Tool flags and data were preserved. Use --enable-vm-tools only if you want VM model access.");
        }
        println!("Existing prompts, manifests, .env and credentials were preserved. Restart Praxis to load installed packages.");
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
    if let Cli::Vm { args } = &cli {
        return praxis::runtime::vm::cli::run(args).await;
    }

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
        return launch_tui(gateway_url.clone(), gateway_key.clone());
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
        Cli::Vm { .. } => unreachable!(),
        Cli::Backup {
            output,
            no_disks,
            no_isos,
        } => return handle_backup(output, no_disks, no_isos).await,
        Cli::Restore { file, yes } => return handle_restore(&file, yes).await,
        Cli::Chat { .. } => unreachable!(),
        Cli::RepairAssets { .. } => unreachable!(),
        Cli::InstallPreset { .. } => unreachable!(),
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
    let trust_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let trust = praxis::plugins::trust::load(std::path::Path::new(&trust_dir))?;
    let mut plugin_registry = praxis::plugins::load_all_plugins_with_trust(
        std::path::Path::new(&plugins_dir),
        &trust,
    );
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
    // Management starts with incomplete provider setup. Actual chat/agent entry
    // validates its routed provider before changing workflow state or history.

    // Feature registration does not enable tools or initialize guests by itself.
    praxis::runtime::vm::configure(&db, &config, &mut plugin_registry)?;
    praxis::runtime::vm::initialize_service(&config, &plugin_registry).await?;
    praxis::runtime::shell::configure(&config, &mut plugin_registry)?;
    praxis::runtime::shell::initialize_service(&config, &plugin_registry).await?;
    praxis::runtime::process_service::configure(&config, &mut plugin_registry)?;
    praxis::runtime::process_service::initialize_service(&config, &plugin_registry).await?;
    let _engine_binding =
        praxis::runtime::engine_bridge::configure(&config, &plugin_registry).await?;
    if let Err(error) = praxis::runtime::vm::autostart(&config, &plugin_registry, &secrets).await {
        tracing::warn!(%error, "Configured VM autostart failed (non-fatal)");
    }
    let feature_plugins = std::sync::Arc::new(plugin_registry);

    // Gateway
    let gateway_db = db.clone();
    let gateway_config = config.clone();
    let gateway_plugins = (*feature_plugins).clone();
    let gateway_handle = tokio::spawn(async move {
        if let Err(e) = praxis::gateway::start_with_plugins(gateway_db, gateway_config, gateway_plugins).await {
            tracing::error!("Gateway error: {}", e);
        }
    });

    // Dashboard (optional). DASHBOARD_PACKAGE selects an installed dashboard
    // package instead of the built-in one; only one dashboard is active.
    let dashboard_package = std::env::var("DASHBOARD_PACKAGE")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if enable_dashboard {
        if let Some(name) = dashboard_package.clone() {
            let options = praxis::runtime::dashboard_package::Launch {
                db: db.clone(),
                plugins: feature_plugins.clone(),
                plugins_dir: std::path::Path::new(&plugins_dir),
                name: &name,
                listen: format!("0.0.0.0:{}", config.dashboard_port),
                tls: config.dashboard_tls,
                data_dir: std::path::Path::new(&config.data_dir),
                gateway_port: config.gateway_port,
            };
            match praxis::runtime::dashboard_package::launch(options).await {
                Ok(package) => {
                    tokio::spawn(praxis::runtime::dashboard_package::supervise(package));
                }
                Err(error) => tracing::error!(
                    %error, package = %name,
                    "Dashboard package failed to start; gateway and workflows continue"
                ),
            }
        }
    }
    #[cfg(feature = "dashboard")]
    if enable_dashboard && dashboard_package.is_none() {
        let dashboard_db = db.clone();
        let dashboard_port = config.dashboard_port;
        let dashboard_plugins = feature_plugins.clone();
        tokio::spawn(async move {
            let server = praxis::dashboard::DashboardServer::new(dashboard_port, config.dashboard_tls, dashboard_db, &config.data_dir).with_plugins(dashboard_plugins);
            if let Err(e) = server.start().await {
                tracing::error!("Dashboard error: {}", e);
            }
        });
        tracing::info!("Dashboard starting on port {}", config.dashboard_port);
    }

    #[cfg(not(feature = "dashboard"))]
    if enable_dashboard && dashboard_package.is_none() {
        tracing::warn!(
            "Built-in dashboard not compiled; set DASHBOARD_PACKAGE to an installed \
             dashboard package. Gateway, CLI and workflows continue without it"
        );
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

    let data_dir = std::path::PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string()));
    // Declared assets mirror their package-relative path under ROOT_DIR.
    let assets_root = std::env::var("ROOT_DIR").unwrap_or_else(|_| ".".to_string());
    let assets_root = Path::new(&assets_root);
    match action {
        PluginAction::Builtins => {
            let state = praxis::tools::packages::load(&data_dir)?;
            for package in praxis::tools::packages::PACKAGES {
                let on = package.required || state.get(package.id).copied().unwrap_or(true);
                let available = package.tools.iter().all(|tool| praxis::tools::packages::native_available(tool));
                let status = if package.required { "core" } else if !available { "not linked; install package" } else if on { "enabled" } else { "disabled" };
                println!("  {} [{}] — {} ({})", package.id, status, package.description, package.tools.join(", "));
            }
            return Ok(());
        }
        PluginAction::EnableBuiltin { id } | PluginAction::DisableBuiltin { id } => {
            let on = matches!(action, PluginAction::EnableBuiltin { .. });
            praxis::tools::packages::set(&data_dir, id, on)?;
            if !praxis::tools::packages::get(id).is_some_and(|package| package.tools.iter().all(|tool| praxis::tools::packages::native_available(tool))) {
                println!("Native package '{id}' preference saved; its implementation is not linked. Install its separate package or rebuild with its Cargo feature.");
                return Ok(());
            }
            println!("Tool package '{id}' {}. Running instances apply it on the next model turn.", if on { "enabled" } else { "disabled" });
            return Ok(());
        }
        PluginAction::Install {
            path,
            allow_scripts,
            no_scripts,
            yes,
            dry_run,
        } => {
            let policy = praxis::plugins::lifecycle::HookPolicy::from_env()?;
            let run_hooks = praxis::plugins::lifecycle::resolve_consent(
                policy,
                *allow_scripts || *yes,
                *no_scripts,
                "Run plugin install hooks?",
            );
            let report = praxis::plugins::lifecycle::install(
                &praxis::plugins::lifecycle::InstallRequest {
                    source: Path::new(path),
                    plugins_dir: plugins_path,
                    data_dir: &data_dir,
                    run_hooks,
                    dry_run: *dry_run,
                },
            )
            .await?;
            if *dry_run {
                return Ok(());
            }
            println!("Plugin '{}' installed to {}", report.name, report.dest.display());
            let assets = praxis::plugins::lifecycle::apply_assets(
                &report.dest,
                &report.plugin,
                &data_dir,
                assets_root,
            )?;
            for asset in &assets.written {
                println!("  asset installed {asset}");
            }
            for asset in &assets.kept {
                println!("  asset kept (existing file) {asset}");
            }
            println!("  Tools: {}", report.plugin.tools.len());
            println!("  Context vars: {}", report.plugin.context.len());
            println!("  Secrets: {}", report.plugin.secrets.len());
            if !report.plugin.secrets.is_empty() {
                println!("  Configure secrets via dashboard or API before use.");
            }
        }
        PluginAction::Uninstall {
            name,
            allow_scripts,
            no_scripts,
            yes,
            force,
            purge,
            dry_run,
        } => {
            let policy = praxis::plugins::lifecycle::HookPolicy::from_env()?;
            let run_hooks = praxis::plugins::lifecycle::resolve_consent(
                policy,
                *allow_scripts || *yes,
                *no_scripts,
                "Run plugin uninstall hooks?",
            );
            let report = praxis::plugins::lifecycle::uninstall(
                &praxis::plugins::lifecycle::UninstallRequest {
                    name,
                    plugins_dir: plugins_path,
                    data_dir: &data_dir,
                    run_hooks,
                    force: *force,
                    purge: *purge,
                    dry_run: *dry_run,
                },
            )
            .await?;
            if *dry_run {
                return Ok(());
            }
            println!("Removed plugin directory: {}", plugins_path.join(name).display());
            let assets =
                praxis::plugins::lifecycle::remove_assets(&data_dir, assets_root, name)?;
            for asset in &assets.removed {
                println!("  asset removed {asset}");
            }
            for asset in &assets.kept {
                println!("  asset kept (operator edit) {asset}");
            }
            let context_keys = report.context_keys;
            let secret_keys = report.secret_keys;

            if data_dir.exists() {
                if let Ok(db) = praxis::db::Database::new(&data_dir) {
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
        PluginAction::Upgrade {
            path,
            allow_scripts,
            no_scripts,
            yes,
            dry_run,
        } => {
            let policy = praxis::plugins::lifecycle::HookPolicy::from_env()?;
            let run_hooks = praxis::plugins::lifecycle::resolve_consent(
                policy,
                *allow_scripts || *yes,
                *no_scripts,
                "Run plugin upgrade hooks?",
            );
            let report = praxis::plugins::lifecycle::upgrade(
                &praxis::plugins::lifecycle::UpgradeRequest {
                    source: Path::new(path),
                    plugins_dir: plugins_path,
                    data_dir: &data_dir,
                    root: assets_root,
                    run_hooks,
                    dry_run: *dry_run,
                },
            )
            .await?;
            if *dry_run {
                return Ok(());
            }
            println!("Plugin '{}' upgraded at {}", report.name, report.dest.display());
            // The upgrade applies its manifest change (including placed
            // assets) atomically and rolls both back together on failure.
            let assets = &report.assets;
            for asset in &assets.written {
                println!("  asset installed {asset}");
            }
            for asset in &assets.kept {
                println!("  asset kept (existing file) {asset}");
            }
        }
        PluginAction::InstallDefault {
            preset,
            allow_scripts,
            no_scripts,
            yes,
            dry_run,
        } => {
            let policy = praxis::plugins::lifecycle::HookPolicy::from_env()?;
            let run_hooks = praxis::plugins::lifecycle::resolve_consent(
                policy,
                *allow_scripts || *yes,
                *no_scripts,
                "Run plugin install hooks?",
            );
            let root = std::env::var("ROOT_DIR").unwrap_or_else(|_| ".".to_string());
            let report = praxis::plugins::lifecycle::install_default(
                &praxis::plugins::lifecycle::PresetRequest {
                    preset_path: preset.as_deref().map(Path::new),
                    root: Path::new(&root),
                    plugins_dir: plugins_path,
                    data_dir: &data_dir,
                    run_hooks,
                    dry_run: *dry_run,
                },
            )
            .await?;
            for name in &report.installed {
                println!("  installed {name}");
            }
            for name in &report.skipped {
                println!("  already installed {name}");
            }
            for (name, error) in &report.failed {
                eprintln!("  failed {name}: {error}");
            }
            if !report.failed.is_empty() {
                anyhow::bail!("{} default plugin(s) failed to install", report.failed.len());
            }
        }
        PluginAction::Enable { name } | PluginAction::Disable { name } => {
            let enabled = matches!(action, PluginAction::Enable { .. });
            praxis::plugins::lifecycle::set_enabled(plugins_path, name, enabled)?;
            println!(
                "Plugin '{}' {}. Running instances apply it on the next model turn.",
                name,
                if enabled { "enabled" } else { "disabled" }
            );
        }
        PluginAction::Trust { name, role } => {
            let role = praxis::plugins::TrustRole::parse(role)?;
            praxis::plugins::trust::set(&data_dir, name, Some(role))?;
            println!("Plugin '{name}' granted role '{role:?}'. Restart Praxis to apply.");
        }
        PluginAction::Untrust { name } => {
            praxis::plugins::trust::set(&data_dir, name, None)?;
            println!("Plugin '{name}' trust grant removed. Restart Praxis to apply.");
        }
        PluginAction::Verify => {
            let report = praxis::plugins::lifecycle::verify(plugins_path)?;
            for name in &report.ok {
                println!("  ok {name}");
            }
            for name in &report.unlocked {
                println!("  unlocked {name} (not recorded; reinstall to lock)");
            }
            for (name, reason) in &report.changed {
                println!("  changed {name}: {reason}");
            }
            for name in &report.missing {
                println!("  missing {name}");
            }
            if !report.changed.is_empty() || !report.missing.is_empty() {
                anyhow::bail!(
                    "{} plugin(s) changed and {} missing",
                    report.changed.len(),
                    report.missing.len()
                );
            }
        }
        PluginAction::Migrate {
            name,
            down,
            dry_run,
        } => {
            let plugin_dir = plugins_path.join(name);
            let plugin = praxis::plugins::load_installed_plugin(&plugin_dir)
                .map_err(|error| anyhow::anyhow!("Cannot load plugin '{name}': {error}"))?;
            if plugin.provides.migrations.is_empty() {
                println!("Plugin '{name}' declares no migrations.");
                return Ok(());
            }
            let migrations =
                praxis::plugins::migrations::load(&plugin_dir, &plugin.provides.migrations)?;
            if *dry_run {
                let applied = praxis::plugins::migrations::history(&data_dir, name)?
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<std::collections::HashSet<_>>();
                for migration in &migrations {
                    let state = if applied.contains(&migration.id) {
                        "applied"
                    } else {
                        "pending"
                    };
                    println!(
                        "  {state} {}{}",
                        migration.id,
                        if migration.down.is_some() { "" } else { " (irreversible)" }
                    );
                }
                return Ok(());
            }
            if *down {
                let reverted = praxis::plugins::migrations::revert(&data_dir, name, &migrations)?;
                if reverted.is_empty() {
                    println!("Plugin '{name}': no applied migrations to revert.");
                } else {
                    println!("Plugin '{name}' reverted: {}", reverted.join(", "));
                }
            } else {
                let applied = praxis::plugins::migrations::apply(&data_dir, name, &migrations)?;
                if applied.is_empty() {
                    println!("Plugin '{name}': migrations already up to date.");
                } else {
                    println!("Plugin '{name}' applied: {}", applied.join(", "));
                }
            }
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
                match praxis::plugins::load_installed_plugin(&dir) {
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
                        if let Some(frontend) = &plugin.frontend {
                            println!(
                                "      frontend: {} {}",
                                frontend.executable,
                                frontend.args.join(" ")
                            );
                        }
                        if !plugin.provides.is_empty() {
                            println!(
                                "      provides: routes={} ui={} assets={} migrations={}",
                                plugin.provides.routes.len(),
                                plugin.provides.ui.len(),
                                plugin.provides.assets.len(),
                                plugin.provides.migrations.len()
                            );
                        }
                        if !plugin.role.is_tool() {
                            println!("      role: {:?}", plugin.role);
                        }
                        found = true;
                    }
                    Err(e) => {
                        println!("  {} — invalid manifest: {}", dir.display(), e);
                        found = true;
                    }
                }
            }
            if !found {
                println!("No plugins installed.");
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


/// `praxis chat` launches the standalone terminal frontend. The kernel does not
/// link it: the `praxis-tui` binary comes from its own crate/package.
fn launch_tui(gateway_url: Option<String>, gateway_key: Option<String>) -> anyhow::Result<()> {
    let explicit = std::env::var_os("PRAXIS_TUI_EXECUTABLE").map(std::path::PathBuf::from);
    let beside = std::env::current_exe().ok().map(|exe| exe.with_file_name("praxis-tui"));
    let program = explicit
        .or_else(|| beside.filter(|path| path.is_file()))
        .unwrap_or_else(|| "praxis-tui".into());
    let mut command = std::process::Command::new(program);
    if let Some(url) = gateway_url {
        command.arg("--gateway-url").arg(url);
    }
    if let Some(key) = gateway_key {
        command.arg("--gateway-key").arg(key);
    }
    let status = command.status().map_err(|error| {
        anyhow::anyhow!("Cannot start the praxis-tui frontend ({error}); install the TUI package or set PRAXIS_TUI_EXECUTABLE")
    })?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
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
    fn test_cli_forwards_vm_arguments_to_the_package_in_every_build() {
        for args in [vec!["status"], vec!["--help"], vec!["start", "--iso", "/a path/to/install.iso"], vec!["shell", "--name", "desktop", "printf", "a b"]] {
            let mut command = vec!["praxis", "vm"];
            command.extend(&args);
            let cli = Cli::try_parse_from(command).unwrap();
            assert!(matches!(cli, Cli::Vm { args: forwarded } if forwarded == args.iter().map(std::ffi::OsString::from).collect::<Vec<_>>()));
        }
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
    fn test_cli_parsing_compatibility_preset_with_explicit_vm_flags() {
        let cli = Cli::try_parse_from(["praxis", "install-preset", "compatibility", "--directory", "/install", "--data-dir", "state", "--enable-vm-tools", "--update-dashboard"]).unwrap();
        assert!(matches!(cli, Cli::InstallPreset { preset: InstallationPreset::Compatibility, directory, data_dir: Some(data), enable_vm_tools: true, update_dashboard: true } if directory == PathBuf::from("/install") && data == PathBuf::from("state")));
        assert!(Cli::try_parse_from(["praxis", "install-preset", "unknown"]).is_err());
        assert!(Cli::try_parse_from(["praxis", "install-preset", "compatibility", "--password", "ignored"]).is_err());
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

    #[test]
    fn test_cli_parsing_plugin_lifecycle_flags() {
        let cli = Cli::try_parse_from([
            "praxis",
            "plugin",
            "install",
            "./plugins/demo",
            "--allow-scripts",
            "--dry-run",
        ])
        .unwrap();
        match cli {
            Cli::Plugin { action } => match action {
                PluginAction::Install {
                    path,
                    allow_scripts,
                    no_scripts,
                    yes,
                    dry_run,
                } => {
                    assert_eq!(path, "./plugins/demo");
                    assert!(allow_scripts && dry_run && !no_scripts && !yes);
                }
                _ => panic!("Expected Install variant"),
            },
            _ => panic!("Expected Plugin variant"),
        }
        let cli = Cli::try_parse_from([
            "praxis", "plugin", "uninstall", "demo", "--force", "--purge",
        ])
        .unwrap();
        match cli {
            Cli::Plugin { action } => match action {
                PluginAction::Uninstall {
                    name,
                    force,
                    purge,
                    ..
                } => {
                    assert_eq!(name, "demo");
                    assert!(force && purge);
                }
                _ => panic!("Expected Uninstall variant"),
            },
            _ => panic!("Expected Plugin variant"),
        }
    }

    #[test]
    fn test_cli_parsing_plugin_install_default() {
        let cli = Cli::try_parse_from([
            "praxis",
            "plugin",
            "install-default",
            "--preset",
            "./preset.json",
            "--allow-scripts",
        ])
        .unwrap();
        match cli {
            Cli::Plugin { action } => match action {
                PluginAction::InstallDefault {
                    preset,
                    allow_scripts,
                    ..
                } => {
                    assert_eq!(preset.as_deref(), Some("./preset.json"));
                    assert!(allow_scripts);
                }
                _ => panic!("Expected InstallDefault variant"),
            },
            _ => panic!("Expected Plugin variant"),
        }
    }

    #[test]
    fn test_cli_parsing_plugin_enable_disable() {
        let cli = Cli::try_parse_from(["praxis", "plugin", "disable", "demo"]).unwrap();
        assert!(matches!(
            cli,
            Cli::Plugin {
                action: PluginAction::Disable { name }
            } if name == "demo"
        ));
        let cli = Cli::try_parse_from(["praxis", "plugin", "enable", "demo"]).unwrap();
        assert!(matches!(
            cli,
            Cli::Plugin {
                action: PluginAction::Enable { name }
            } if name == "demo"
        ));
    }

    #[test]
    fn test_cli_parsing_plugin_trust() {
        let cli = Cli::try_parse_from([
            "praxis",
            "plugin",
            "trust",
            "engine",
            "--role",
            "runtime",
        ])
        .unwrap();
        assert!(matches!(
            cli,
            Cli::Plugin {
                action: PluginAction::Trust { name, role }
            } if name == "engine" && role == "runtime"
        ));
        let cli = Cli::try_parse_from(["praxis", "plugin", "untrust", "engine"]).unwrap();
        assert!(matches!(
            cli,
            Cli::Plugin {
                action: PluginAction::Untrust { name }
            } if name == "engine"
        ));
    }

    #[test]
    fn test_cli_parsing_plugin_migrate() {
        let cli = Cli::try_parse_from(["praxis", "plugin", "migrate", "demo", "--down"])
            .unwrap();
        assert!(matches!(
            cli,
            Cli::Plugin {
                action: PluginAction::Migrate { name, down: true, dry_run: false }
            } if name == "demo"
        ));
        let cli = Cli::try_parse_from(["praxis", "plugin", "migrate", "demo", "--dry-run"])
            .unwrap();
        assert!(matches!(
            cli,
            Cli::Plugin {
                action: PluginAction::Migrate { name, down: false, dry_run: true }
            } if name == "demo"
        ));
    }
}
