//! xis — a setup and package manager for Praxis distributions.
//!
//! A separate executable from the Praxis kernel that links nothing of it: it
//! fetches signed repository indexes, resolves a setup, prints a plan, places
//! files under a keep/backup/overwrite policy with an ownership lock,
//! delegates plugin installs to the Praxis CLI and leaves a change report.
//!
//! `xis` cannot bypass guards or grant itself a trust role: it is a signed
//! `curl | tar` with a plan, a diff and a lockfile
//! (`docs/XIS_PACKAGE_MANAGER.md`).
mod apply;
mod motd;
mod plan;
mod repo;
#[cfg(test)]
mod tests;

use anyhow::{ensure, Context as _};
use clap::{Parser, Subcommand};
use plan::{FileAction, Lock, Policy};
use repo::{RepositoryStore, VerifiedIndex};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "xis", version, about = "Setup and package manager for Praxis distributions")]
struct Cli {
    /// Praxis installation root (defaults to ROOT_DIR or the current directory)
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Praxis plugins directory (defaults to PLUGINS_DIR or <root>/plugins)
    #[arg(long, global = true)]
    plugins_dir: Option<PathBuf>,
    /// The `praxis` executable used for plugin installs (defaults to `praxis`)
    #[arg(long, global = true, default_value = "praxis")]
    praxis: PathBuf,
    /// Trust unsigned repositories this once (pinned keys always win)
    #[arg(long, global = true)]
    allow_unsigned: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage operator-pinned repositories
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },
    /// Search pinned repositories for a setup
    Search { query: String },
    /// Show what an install or upgrade would change (nothing is written)
    Plan {
        /// repo/name@version (or repo/name for the newest)
        reference: String,
        /// Back up conflicting files, then write
        #[arg(long)]
        backup: bool,
        /// Replace conflicts and operator edits (always backed up)
        #[arg(long)]
        force: bool,
    },
    /// Install a setup: prints the plan first, then applies it
    Install {
        reference: String,
        #[arg(long)]
        backup: bool,
        #[arg(long)]
        force: bool,
    },
    /// Upgrade an installed setup to the newest version
    Upgrade {
        name: String,
        #[arg(long)]
        backup: bool,
        #[arg(long)]
        force: bool,
    },
    /// Remove an installed setup, touching only what it owns
    Remove {
        name: String,
        /// Keep the setup's files and data; only its lock entry goes
        #[arg(long)]
        keep_data: bool,
    },
    /// Show (or acknowledge) the change report
    Motd {
        #[arg(long)]
        ack: bool,
    },
}

#[derive(Subcommand)]
enum RepoAction {
    /// Pin a repository (its signing key is the operator's decision)
    Add {
        name: String,
        url: String,
        #[arg(long)]
        key: Option<String>,
    },
    /// List pinned repositories
    List,
    /// Re-fetch and verify every pinned repository index
    Refresh,
    /// Remove a repository pin
    Remove { name: String },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(run(cli))
}

fn roots(cli: &Cli) -> anyhow::Result<(PathBuf, PathBuf)> {
    let root = cli
        .root
        .clone()
        .or_else(|| std::env::var_os("ROOT_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let plugins_dir = cli
        .plugins_dir
        .clone()
        .or_else(|| std::env::var_os("PLUGINS_DIR").map(PathBuf::from))
        .unwrap_or_else(|| root.join("plugins"));
    Ok((root, plugins_dir))
}

fn indexes(allow_unsigned: bool) -> anyhow::Result<Vec<VerifiedIndex>> {
    let store = RepositoryStore::load()?;
    ensure!(
        !store.repositories.is_empty(),
        "no repositories pinned; use 'xis repo add'"
    );
    store
        .repositories
        .iter()
        .map(|repository| repo::fetch_index(repository, allow_unsigned))
        .collect()
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let (root, plugins_dir) = roots(&cli)?;
    match &cli.command {
        Command::Repo { action } => match action {
            RepoAction::Add { name, url, key } => {
                let mut store = RepositoryStore::load()?;
                store.pin(name, url, key.as_deref())?;
                store.save()?;
                println!("Pinned repository '{name}' at {url}");
            }
            RepoAction::List => {
                let store = RepositoryStore::load()?;
                if store.repositories.is_empty() {
                    println!("No repositories pinned.");
                }
                for repository in &store.repositories {
                    println!(
                        "{:<16} {}{}",
                        repository.name,
                        repository.url,
                        if repository.key.is_some() {
                            " (key pinned)"
                        } else {
                            " (unsigned)"
                        }
                    );
                }
            }
            RepoAction::Refresh => {
                for index in indexes(cli.allow_unsigned)? {
                    println!(
                        "{}: {} setup(s){}",
                        index.repository,
                        index.index.setups.len(),
                        if index.signed {
                            ", signature verified"
                        } else {
                            ", unsigned"
                        }
                    );
                }
            }
            RepoAction::Remove { name } => {
                let mut store = RepositoryStore::load()?;
                store.remove(name)?;
                store.save()?;
                println!("Removed repository pin '{name}'");
            }
        },
        Command::Search { query } => {
            let query = query.to_lowercase();
            for index in indexes(cli.allow_unsigned)? {
                for setup in &index.index.setups {
                    if setup.name.to_lowercase().contains(&query)
                        || setup.description.to_lowercase().contains(&query)
                    {
                        println!(
                            "{:<28} {:<10} {}",
                            setup.reference(&index.repository),
                            setup.version,
                            setup.description
                        );
                    }
                }
            }
        }
        Command::Plan {
            reference,
            backup,
            force,
        } => {
            let policy = Policy::new(*backup, *force)?;
            let (report, _) = stage(
                &cli.praxis,
                cli.allow_unsigned,
                reference,
                policy,
                &root,
                &plugins_dir,
                false,
            )?;
            println!("{report}\n(plan only: nothing was written)");
        }
        Command::Install {
            reference,
            backup,
            force,
        } => {
            let policy = Policy::new(*backup, *force)?;
            let (report, _) = stage(
                &cli.praxis,
                cli.allow_unsigned,
                reference,
                policy,
                &root,
                &plugins_dir,
                true,
            )?;
            println!("{report}");
        }
        Command::Upgrade {
            name,
            backup,
            force,
        } => {
            let policy = Policy::new(*backup, *force)?;
            let (report, _) = stage(
                &cli.praxis,
                cli.allow_unsigned,
                name,
                policy,
                &root,
                &plugins_dir,
                true,
            )?;
            println!("{report}");
        }
        Command::Remove { name, keep_data } => {
            println!("{}", remove(name, *keep_data, &root, &plugins_dir)?);
        }
        Command::Motd { ack } => {
            if *ack {
                if motd::Report::acknowledge(&root)? {
                    println!("Change report acknowledged.");
                } else {
                    println!("No change report to acknowledge.");
                }
            } else if let Some(text) = motd::Report::read(&root)? {
                print!("{text}");
            } else {
                println!("No change report.");
            }
        }
    }
    Ok(())
}

/// Resolve, verify and plan a setup; when `apply` is set, also apply it. The
/// plan text is built first and returned either way: it is what the operator
/// sees before anything touches disk.
#[allow(clippy::too_many_arguments)]
fn stage(
    praxis: &Path,
    allow_unsigned: bool,
    reference: &str,
    policy: Policy,
    root: &Path,
    plugins_dir: &Path,
    apply: bool,
) -> anyhow::Result<(String, bool)> {
    let store = RepositoryStore::load()?;
    let indexes = indexes(allow_unsigned)?;
    let (index, setup) = repo::resolve(&indexes, reference)?;
    let repository = store
        .get(&index.name)
        .cloned()
        .with_context(|| format!("repository '{}' disappeared from the pins", index.name))?;
    ensure!(
        setup
            .runtime_api
            .as_deref()
            .is_none_or(|api| matches!(api, "1" | "2")),
        "setup '{}' requires runtime API {}",
        setup.name,
        setup.runtime_api.clone().unwrap_or_default()
    );
    let mut artifacts = Vec::new();
    for item in &setup.items {
        let bundle = repo::fetch_artifact(&repository, item)?;
        artifacts.push((item.path.clone().unwrap_or_default(), bundle));
    }
    let lock = Lock::load(root)?;
    let env_path = root.join(".env");
    let config = plan::parse_env(&std::fs::read_to_string(&env_path).unwrap_or_default());
    let plan = plan::plan_install(
        setup,
        reference,
        &artifacts,
        policy,
        root,
        plugins_dir,
        &lock,
        &config,
    )?;

    let mut report = format!("xis: {} {}\n", plan.reference, plan.version);
    for file in &plan.files {
        report.push_str(&format!("  {:<8} {}\n", action_name(&file.action), file.relative));
    }
    for line in &plan.config {
        report.push_str(&format!(
            "  config   {} {} -> {}\n",
            line.key,
            line.old.as_deref().unwrap_or("(unset)"),
            line.new.as_deref().unwrap_or("(unset)")
        ));
    }
    for change in &plan.required {
        report.push_str(&format!(
            "  required {} — {}{}\n",
            change.key,
            change.reason,
            if change.required { " (required)" } else { "" }
        ));
    }
    if !plan.plugins.is_empty() {
        report.push_str(
            "  plugins  delegated to the Praxis CLI (its hooks policy and trust store apply)\n",
        );
    }
    if !apply {
        return Ok((report, false));
    }

    let stamp = apply::stamp();
    let mut applied = apply::apply_files(&artifacts, &plan.files, policy, root, plugins_dir, &stamp)?;
    applied.sort_by(|a, b| a.relative.cmp(&b.relative));

    // Config profiles: documented non-secret keys only, written diffably and
    // with a backup of the operator's file.
    let mut backups = Vec::new();
    if !plan.config.is_empty() {
        let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
        if !existing.is_empty() {
            let backup = apply::backup_root(root, &stamp).join(".env");
            std::fs::create_dir_all(backup.parent().unwrap())?;
            std::fs::write(&backup, &existing)?;
            backups.push((".env".to_string(), Some(backup.display().to_string())));
        }
        std::fs::write(&env_path, plan::render_env(&existing, &plan.config))?;
    }
    for file in &applied {
        backups.push((file.relative.clone(), file.backup.clone()));
    }

    // Plugin packages go through the Praxis CLI: the kernel runs its own
    // lifecycle hooks, writes praxis.lock.json and enforces PLUGIN_HOOKS and
    // the trust store. A failing delegation is reported, never hidden.
    let mut notes = Vec::new();
    for (name, version) in plan.plugins.iter().filter(|(name, _)| !name.is_empty()) {
        let staged = plugins_dir.join(name);
        let installed = Lock::load(root)
            .map(|lock| lock.setups.contains_key(name))
            .unwrap_or(false);
        let outcome = apply::delegate_plugin(
            praxis,
            &apply::PluginInstall {
                name: name.clone(),
                version: version.clone(),
                staged,
                upgrade: installed,
            },
        );
        match outcome {
            Ok(_) => notes.push(format!(
                "Plugin '{name}' {version} delegated to the Praxis CLI; restart Praxis to load it. No tools were enabled automatically."
            )),
            Err(error) => notes.push(format!(
                "Plugin '{name}' was NOT installed ({error}); the setup's files are in place and the change report says so."
            )),
        }
    }

    let mut lock = Lock::load(root)?;
    apply::record_ownership(
        &mut lock,
        &setup.name,
        &repository.name,
        &setup.version,
        &applied,
        &artifacts,
    )?;
    lock.save(root)?;

    let trust_commands = plan
        .plugins
        .iter()
        .map(|(name, _)| format!("praxis plugin trust {name}"))
        .collect();
    let change = motd::build(
        &setup.name,
        &setup.version,
        plan.required.clone(),
        plan.config.clone(),
        &plan.files,
        &backups,
        trust_commands,
        notes,
    );
    change.write(root)?;
    report.push_str("\nChange report written for Praxis ('praxis motd' or 'xis motd')\n");
    Ok((report, true))
}

fn remove(name: &str, keep_data: bool, root: &Path, plugins_dir: &Path) -> anyhow::Result<String> {
    let lock = Lock::load(root)?;
    let (files, _) = plan::plan_remove(name, &lock, root, plugins_dir, keep_data)?;
    let mut report = format!("xis: removing {name}\n");
    for file in &files {
        let target = plan::destination_root("content", root, plugins_dir).join(&file.relative);
        match &file.action {
            FileAction::Remove => {
                std::fs::remove_file(&target).ok();
                report.push_str(&format!("  removed  {}\n", file.relative));
            }
            FileAction::RemoveKeep { operator_edit: true } => {
                report.push_str(&format!("  kept     {} (operator edit)\n", file.relative));
            }
            FileAction::RemoveKeep { operator_edit: false } => {
                report.push_str(&format!("  kept     {} (--keep-data)\n", file.relative));
            }
            _ => {}
        }
    }
    let mut lock = lock;
    lock.setups.remove(name);
    lock.save(root)?;
    report.push_str(
        "\nNotes\n  Plugins are removed with 'praxis plugin uninstall <name>'; xis never removes trust roles.\n",
    );
    Ok(report)
}

fn action_name(action: &FileAction) -> &'static str {
    match action {
        FileAction::Write => "write",
        FileAction::Update => "update",
        FileAction::Keep { operator_edit: true } => "keep!",
        FileAction::Keep { operator_edit: false } => "keep",
        FileAction::Replace { operator_edit: true } => "replace!",
        FileAction::Replace { operator_edit: false } => "replace",
        FileAction::Remove => "remove",
        FileAction::RemoveKeep { .. } => "keep",
    }
}
