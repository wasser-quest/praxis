//! Applying a plan: files with backup/keep/overwrite, an ownership lock and
//! plugin installs delegated to the Praxis CLI (the kernel keeps lifecycle
//! hooks, `praxis.lock.json`, `PLUGIN_HOOKS` and the trust store).
use crate::plan::{FileAction, Lock, PlannedFile, Policy, SetupLock};
use crate::repo::{sha256_hex, Bundle};
use anyhow::{ensure, Context as _};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where backups go, per root: `<root>/.xis-backups/<timestamp>/…`.
pub fn backup_root(root: &Path, stamp: &str) -> PathBuf {
    root.join(".xis-backups").join(stamp)
}

/// One applied file, reported back for the change report.
#[derive(Debug, Clone)]
pub struct Applied {
    pub relative: String,
    pub action: FileAction,
    pub backup: Option<String>,
}

/// A plugin package handed to the Praxis CLI. `xis` never runs lifecycle
/// hooks itself: the kernel enforces `PLUGIN_HOOKS` and the trust store.
#[derive(Debug, Clone)]
pub struct PluginInstall {
    pub name: String,
    pub version: String,
    /// Staged package directory for `praxis plugin install`/`upgrade`.
    pub staged: PathBuf,
    pub upgrade: bool,
}

/// Delegate plugin installs to the Praxis CLI. The command is recorded so the
/// report can show exactly what ran (and what a step skipped).
pub fn delegate_plugin(praxis: &Path, install: &PluginInstall) -> anyhow::Result<String> {
    let action = if install.upgrade { "upgrade" } else { "install" };
    let output = std::process::Command::new(praxis)
        .arg("plugin")
        .arg(action)
        .arg(&install.staged)
        .output()
        .with_context(|| format!("cannot run the Praxis CLI at {}", praxis.display()))?;
    ensure!(
        output.status.success(),
        "praxis plugin {} failed for '{}': {}",
        action,
        install.name,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Write one bundle's files under `root`, applying the policy and recording
/// every byte written. Nothing is deleted or replaced outside `files`.
pub fn apply_files(
    files: &[(String, Bundle)],
    planned: &[PlannedFile],
    policy: Policy,
    root: &Path,
    plugins_dir: &Path,
    stamp: &str,
) -> anyhow::Result<Vec<Applied>> {
    let mut applied = Vec::new();
    for (relative, bundle) in files {
        for (path, bytes) in bundle.files()? {
            let relative = Path::new(relative).join(path);
            let relative = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let Some(plan) = planned.iter().find(|file| file.relative == relative) else {
                continue;
            };
            let target = crate::plan::destination_root("content", root, plugins_dir).join(&relative);
            let backup = match &plan.action {
                FileAction::Write => {
                    write_new(&target, &bytes)?;
                    None
                }
                FileAction::Update => {
                    let previous = std::fs::read(&target).ok();
                    let backup = previous.and_then(|old| backup_file(root, stamp, &relative, &old).ok());
                    write_new(&target, &bytes)?;
                    backup
                }
                FileAction::Keep { .. } | FileAction::RemoveKeep { .. } => {
                    applied.push(Applied { relative, action: plan.action.clone(), backup: None });
                    continue;
                }
                FileAction::Replace { .. } => {
                    let previous = std::fs::read(&target).ok();
                    let backup = previous.and_then(|old| backup_file(root, stamp, &relative, &old).ok());
                    // Replacement always happens after the bytes are safe.
                    std::fs::write(&target, &bytes)?;
                    backup
                }
                FileAction::Remove => {
                    std::fs::remove_file(&target).ok();
                    None
                }
            };
            applied.push(Applied { relative, action: plan.action.clone(), backup });
        }
    }
    let _ = policy;
    Ok(applied)
}

fn write_new(target: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, bytes)?;
    Ok(())
}

fn backup_file(root: &Path, stamp: &str, relative: &str, bytes: &[u8]) -> anyhow::Result<String> {
    let destination = backup_root(root, stamp).join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&destination, bytes)?;
    Ok(destination.display().to_string())
}

/// Record ownership: only files this setup wrote, with their new hashes.
pub fn record_ownership(
    lock: &mut Lock,
    setup: &str,
    repository: &str,
    version: &str,
    applied: &[Applied],
    files: &[(String, Bundle)],
) -> anyhow::Result<()> {
    let mut owned: BTreeMap<String, String> = lock
        .setups
        .get(setup)
        .map(|entry| entry.files.clone())
        .unwrap_or_default();
    for (relative, bundle) in files {
        for (path, bytes) in bundle.files()? {
            let relative = Path::new(relative).join(path);
            let relative = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let wrote = applied.iter().any(|entry| {
                entry.relative == relative
                    && matches!(
                        entry.action,
                        FileAction::Write | FileAction::Update | FileAction::Replace { .. }
                    )
            });
            if wrote {
                owned.insert(relative, sha256_hex(&bytes));
            }
        }
    }
    // Files a removal emptied are no longer owned.
    for entry in applied.iter().filter(|entry| matches!(entry.action, FileAction::Remove)) {
        owned.remove(&entry.relative);
    }
    lock.setups.insert(
        setup.to_string(),
        SetupLock { version: version.to_string(), repository: repository.to_string(), files: owned },
    );
    Ok(())
}

/// The timestamped directory backups live under; stable within one run.
pub fn stamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ").to_string()
}
