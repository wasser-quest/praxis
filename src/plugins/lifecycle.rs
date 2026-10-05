//! Plugin lifecycle: operator-approved install/uninstall hooks, staged
//! publication, script hashing and an install record. See
//! docs/PLUGIN_LIFECYCLE.md.
//!
//! Hooks are operator-trusted code, not a sandbox. The safety properties are:
//! the configured `allow|ask|deny` policy, bounded execution, a clean
//! environment, atomic publication and a recorded hash so a swapped script
//! cannot silently run later.
use crate::plugins::{validate_hooks, Plugin};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

/// Install hooks may take a while; uninstall hooks are expected to be quick.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const UNINSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_HOOK_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookPolicy {
    Allow,
    Ask,
    Deny,
}

impl HookPolicy {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            other => anyhow::bail!("PLUGIN_HOOKS must be allow, ask or deny (got '{other}')"),
        }
    }
    pub fn from_env() -> anyhow::Result<Self> {
        match std::env::var("PLUGIN_HOOKS") {
            Ok(value) if !value.trim().is_empty() => Self::parse(&value),
            _ => Ok(Self::Ask),
        }
    }
}

/// What the CLI should do about hooks for one command, before prompting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPlan {
    Run,
    Skip(String),
    Ask,
}

pub fn plan_hooks(policy: HookPolicy, allow_flag: bool, no_scripts: bool) -> HookPlan {
    if no_scripts {
        return HookPlan::Skip("--no-scripts".into());
    }
    if allow_flag {
        return HookPlan::Run;
    }
    match policy {
        HookPolicy::Allow => HookPlan::Run,
        HookPolicy::Deny => HookPlan::Skip("hooks policy is 'deny'".into()),
        HookPolicy::Ask => HookPlan::Ask,
    }
}

/// Interactive confirmation for the `ask` policy. Reads one line from stdin.
pub fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Resolve the hook plan to a yes/no decision, prompting only for `ask` on a
/// terminal. Non-interactive `ask` skips hooks with a clear message.
pub fn resolve_consent(policy: HookPolicy, allow: bool, no_scripts: bool, prompt: &str) -> bool {
    use std::io::IsTerminal;
    match plan_hooks(policy, allow, no_scripts) {
        HookPlan::Run => true,
        HookPlan::Skip(reason) => {
            println!("Skipping hooks: {reason}");
            false
        }
        HookPlan::Ask => {
            if std::io::stdin().is_terminal() {
                confirm(prompt)
            } else {
                println!(
                    "Plugin hooks policy is 'ask' but stdin is not a terminal; skipping hooks (use --allow-scripts to run them)."
                );
                false
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    Install,
    Uninstall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookRecord {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct InstallRecord {
    pub version: String,
    pub manifest_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<HookRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uninstall: Option<HookRecord>,
    pub installed_at: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct HookOutcome {
    pub ran: bool,
    pub skipped_reason: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct InstallRequest<'a> {
    pub source: &'a Path,
    pub plugins_dir: &'a Path,
    pub data_dir: &'a Path,
    pub run_hooks: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub name: String,
    pub dest: PathBuf,
    pub plugin: Plugin,
    pub hook: HookOutcome,
}

#[derive(Debug, Clone)]
pub struct UninstallRequest<'a> {
    pub name: &'a str,
    pub plugins_dir: &'a Path,
    pub data_dir: &'a Path,
    pub run_hooks: bool,
    pub force: bool,
    pub purge: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct UninstallReport {
    pub name: String,
    pub context_keys: Vec<String>,
    pub secret_keys: Vec<String>,
    pub hook: HookOutcome,
    pub dir_removed: bool,
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    Ok(sha256_bytes(&std::fs::read(path)?))
}

/// The lockfile lives with the plugins so it travels with an installation.
fn record_path(plugins_dir: &Path) -> PathBuf {
    plugins_dir.join("praxis.lock.json")
}

fn load_records(plugins_dir: &Path) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    match std::fs::read_to_string(record_path(plugins_dir)) {
        Ok(data) => Ok(serde_json::from_str(&data)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::Map::new())
        }
        Err(e) => Err(e.into()),
    }
}

fn save_records(
    plugins_dir: &Path,
    records: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(plugins_dir)?;
    let path = record_path(plugins_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(records)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn hook_record(plugin_dir: &Path, path: &str) -> anyhow::Result<HookRecord> {
    Ok(HookRecord {
        path: path.to_string(),
        sha256: sha256_file(&plugin_dir.join(path))?,
    })
}

/// Copy a plugin tree. Symlinks are not followed, so a package cannot smuggle
/// bytes from outside itself into the install directory.
pub fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let meta = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            !meta.file_type().is_symlink(),
            "Plugin package must not contain symlinks: {}",
            path.display()
        );
        if meta.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else if meta.is_file() {
            std::fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

async fn run_hook(
    kind: HookKind,
    plugin_dir: &Path,
    script: &Path,
    data_dir: &Path,
    plugins_dir: &Path,
    plugin: &Plugin,
    purge: bool,
) -> anyhow::Result<HookOutcome> {
    let mut command = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C"]).arg(script);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg(script);
        c
    };
    command
        .current_dir(plugin_dir)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SystemRoot",
        "WINDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("PRAXIS_PLUGIN_ID", &plugin.name)
        .env("PRAXIS_PLUGIN_VERSION", &plugin.version)
        .env("PRAXIS_PLUGIN_DIR", plugin_dir)
        .env("PRAXIS_DATA_DIR", data_dir)
        .env("PRAXIS_PLUGINS_DIR", plugins_dir)
        .env("PRAXIS_HOOK", match kind {
            HookKind::Install => "install",
            HookKind::Uninstall => "uninstall",
        });
    if purge {
        command.env("PRAXIS_PURGE", "1");
    }
    let child = command.spawn()?;
    let timeout = match kind {
        HookKind::Install => INSTALL_TIMEOUT,
        HookKind::Uninstall => UNINSTALL_TIMEOUT,
    };
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "{} hook timed out after {}s and was stopped",
                match kind {
                    HookKind::Install => "install",
                    HookKind::Uninstall => "uninstall",
                },
                timeout.as_secs()
            )
        })??;
    let mut outcome = HookOutcome {
        ran: true,
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        ..Default::default()
    };
    truncate(&mut outcome.stdout);
    truncate(&mut outcome.stderr);
    Ok(outcome)
}

fn truncate(text: &mut String) {
    if text.len() > MAX_HOOK_OUTPUT {
        let mut end = MAX_HOOK_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… (output truncated)");
    }
}

fn report_hook(outcome: &HookOutcome, verb: &str) {
    match outcome.skipped_reason.as_deref() {
        Some(reason) => println!("  {verb} hook skipped: {reason}"),
        None => {
            let code = outcome.exit_code.unwrap_or(-1);
            println!("  {verb} hook exit code: {code}");
            if !outcome.stdout.trim().is_empty() {
                println!("  {verb} hook stdout:\n{}", outcome.stdout.trim_end());
            }
            if !outcome.stderr.trim().is_empty() {
                eprintln!("  {verb} hook stderr:\n{}", outcome.stderr.trim_end());
            }
        }
    }
}

/// Install a plugin from a local directory using staged, atomic publication.
/// If the install hook fails, the published directory is removed and no install
/// record or registry entry remains.
fn command_available(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            {
                return true;
            }
        }
        #[cfg(not(unix))]
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat", "com"] {
            if dir.join(format!("{name}.{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

/// Installed, enabled plugins that declare a dependency on `name`.
fn dependents(plugins_dir: &Path, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !dir.is_dir() || file_name.starts_with('.') {
            continue;
        }
        let manifest = dir.join("plugin.json");
        let Some(plugin) = std::fs::read(&manifest)
            .ok()
            .and_then(|data| serde_json::from_slice::<Plugin>(&data).ok())
        else {
            continue;
        };
        if plugin.name == name || !plugin.enabled {
            continue;
        }
        if crate::plugins::requires_declaration(&manifest)
            .is_ok_and(|requires| requires.plugins.iter().any(|id| id == name))
        {
            out.push(plugin.name);
        }
    }
    out.sort();
    out
}

fn preflight_requires(
    plugins_dir: &Path,
    requires: &crate::plugins::PluginRequires,
) -> anyhow::Result<()> {
    for id in &requires.plugins {
        let manifest = plugins_dir.join(id).join("plugin.json");
        anyhow::ensure!(
            manifest.is_file(),
            "Plugin requires '{id}', which is not installed; install it first"
        );
        let enabled = std::fs::read(&manifest)
            .ok()
            .and_then(|data| serde_json::from_slice::<Plugin>(&data).ok())
            .is_some_and(|plugin| plugin.enabled);
        anyhow::ensure!(enabled, "Plugin requires '{id}', which is installed but disabled");
    }
    for command in &requires.commands {
        anyhow::ensure!(
            command_available(command),
            "Plugin requires command '{command}' on PATH"
        );
    }
    Ok(())
}

fn build_record(
    source_root: &Path,
    plugin: &Plugin,
    hooks: &crate::plugins::PluginHooks,
    manifest_bytes: &[u8],
) -> anyhow::Result<InstallRecord> {
    Ok(InstallRecord {
        version: plugin.version.clone(),
        manifest_sha256: sha256_bytes(manifest_bytes),
        install: hooks
            .install
            .as_deref()
            .map(|path| hook_record(source_root, path))
            .transpose()?,
        uninstall: hooks
            .uninstall
            .as_deref()
            .map(|path| hook_record(source_root, path))
            .transpose()?,
        installed_at: chrono::Utc::now().to_rfc3339(),
        enabled: plugin.enabled,
        source: source_root.to_string_lossy().into_owned(),
    })
}

/// Install a plugin from a local directory using staged, atomic publication.
pub async fn install(request: &InstallRequest<'_>) -> anyhow::Result<InstallReport> {
    anyhow::ensure!(
        request.source.is_dir(),
        "Plugin path '{}' is not a directory",
        request.source.display()
    );
    let manifest_path = request.source.join("plugin.json");
    anyhow::ensure!(
        manifest_path.is_file(),
        "No plugin.json found in '{}'",
        request.source.display()
    );
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let plugin: Plugin = serde_json::from_slice(&manifest_bytes)?;
    // Parsing a manifest does not validate hook paths; do it before any copy.
    let source_root = request.source.canonicalize()?;
    let hooks = validate_hooks(&source_root, plugin.hooks.clone())?;
    preflight_requires(
        request.plugins_dir,
        &crate::plugins::requires_declaration(&manifest_path)?,
    )?;

    let dest = request.plugins_dir.join(&plugin.name);
    anyhow::ensure!(
        !dest.exists(),
        "Plugin '{}' already installed at '{}'",
        plugin.name,
        dest.display()
    );

    let record = build_record(&source_root, &plugin, &hooks, &manifest_bytes)?;

    if request.dry_run {
        println!("Dry run: would install '{}' to {}", plugin.name, dest.display());
        if let Some(path) = &hooks.install {
            println!("  install hook: {path}");
            if !request.run_hooks {
                println!("  install hook would be skipped");
            }
        }
        if let Some(path) = &hooks.uninstall {
            println!("  uninstall hook: {path}");
        }
        return Ok(InstallReport {
            name: plugin.name.clone(),
            dest,
            plugin,
            hook: HookOutcome {
                ran: false,
                skipped_reason: Some("dry run".into()),
                ..Default::default()
            },
        });
    }

    // Stage next to the destination so the final rename stays on one filesystem.
    std::fs::create_dir_all(request.plugins_dir)?;
    let stage = request
        .plugins_dir
        .join(format!(".staging-{}-{}", plugin.name, uuid::Uuid::new_v4().simple()));
    copy_dir_recursive(request.source, &stage)?;
    let mut hook = HookOutcome {
        ran: false,
        skipped_reason: Some("plugin declares no install hook".into()),
        ..Default::default()
    };
    let publish = || -> anyhow::Result<()> {
        std::fs::rename(&stage, &dest)?;
        Ok(())
    };
    if let Err(error) = publish() {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(error);
    }
    if let Some(script) = &hooks.install {
        // Provide the declared data directory even if the plugin has never run.
        std::fs::create_dir_all(request.data_dir)?;
        let script_path = dest.join(script);
        if request.run_hooks {
            match run_hook(
                HookKind::Install,
                &dest,
                &script_path,
                request.data_dir,
                request.plugins_dir,
                &plugin,
                false,
            )
            .await
            {
                Ok(outcome) if outcome.exit_code == Some(0) => {
                    report_hook(&outcome, "install");
                    hook = outcome;
                }
                Ok(outcome) => {
                    report_hook(&outcome, "install");
                    let _ = std::fs::remove_dir_all(&dest);
                    anyhow::bail!(
                        "Install hook for '{}' exited with {}. The plugin directory was removed.",
                        plugin.name,
                        outcome.exit_code.unwrap_or(-1)
                    );
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&dest);
                    return Err(error);
                }
            }
        } else {
            hook.skipped_reason = Some("hooks were not run (policy or --no-scripts)".into());
        }
    }
    let mut records = load_records(request.plugins_dir)?;
    records.insert(plugin.name.clone(), serde_json::to_value(&record)?);
    save_records(request.plugins_dir, &records)?;
    Ok(InstallReport {
        name: plugin.name.clone(),
        dest,
        plugin,
        hook,
    })
}

#[derive(Debug, Clone)]
pub struct UpgradeRequest<'a> {
    pub source: &'a Path,
    pub plugins_dir: &'a Path,
    pub data_dir: &'a Path,
    pub run_hooks: bool,
    pub dry_run: bool,
}

/// Replace an installed plugin with a new revision. The previous directory is
/// kept as a backup until the new install hook succeeds; a hook failure restores
/// it. Operator data under DATA_DIR is untouched.
pub async fn upgrade(request: &UpgradeRequest<'_>) -> anyhow::Result<InstallReport> {
    let manifest_path = request.source.join("plugin.json");
    anyhow::ensure!(
        request.source.is_dir() && manifest_path.is_file(),
        "No plugin.json found in '{}'",
        request.source.display()
    );
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let plugin: Plugin = serde_json::from_slice(&manifest_bytes)?;
    let source_root = request.source.canonicalize()?;
    let hooks = validate_hooks(&source_root, plugin.hooks.clone())?;
    preflight_requires(
        request.plugins_dir,
        &crate::plugins::requires_declaration(&manifest_path)?,
    )?;
    let dest = request.plugins_dir.join(&plugin.name);
    anyhow::ensure!(
        dest.exists(),
        "Plugin '{}' is not installed; use 'plugin install'",
        plugin.name
    );
    if request.dry_run {
        println!(
            "Dry run: would replace '{}' at {} with version {}",
            plugin.name,
            dest.display(),
            plugin.version
        );
        return Ok(InstallReport {
            name: plugin.name.clone(),
            dest,
            plugin,
            hook: HookOutcome {
                ran: false,
                skipped_reason: Some("dry run".into()),
                ..Default::default()
            },
        });
    }

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let stage = request.plugins_dir.join(format!(".upgrade-{}-{suffix}", plugin.name));
    let backup = request.plugins_dir.join(format!(".backup-{}-{suffix}", plugin.name));
    copy_dir_recursive(&source_root, &stage)?;
    std::fs::rename(&dest, &backup)?;
    if let Err(error) = std::fs::rename(&stage, &dest) {
        let _ = std::fs::rename(&backup, &dest);
        let _ = std::fs::remove_dir_all(&stage);
        return Err(error.into());
    }
    let mut hook = HookOutcome {
        ran: false,
        skipped_reason: Some("plugin declares no install hook".into()),
        ..Default::default()
    };
    if let Some(script) = &hooks.install {
        std::fs::create_dir_all(request.data_dir)?;
        let script_path = dest.join(script);
        if request.run_hooks {
            let outcome = run_hook(
                HookKind::Install,
                &dest,
                &script_path,
                request.data_dir,
                request.plugins_dir,
                &plugin,
                false,
            )
            .await;
            match outcome {
                Ok(outcome) if outcome.exit_code == Some(0) => {
                    report_hook(&outcome, "install");
                    hook = outcome;
                }
                Ok(outcome) => {
                    report_hook(&outcome, "install");
                    let _ = std::fs::remove_dir_all(&dest);
                    let _ = std::fs::rename(&backup, &dest);
                    anyhow::bail!(
                        "Upgrade hook for '{}' exited with {}. The previous version was restored.",
                        plugin.name,
                        outcome.exit_code.unwrap_or(-1)
                    );
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&dest);
                    let _ = std::fs::rename(&backup, &dest);
                    return Err(error);
                }
            }
        } else {
            hook.skipped_reason = Some("hooks were not run (policy or --no-scripts)".into());
        }
    }
    let _ = std::fs::remove_dir_all(&backup);
    let record = build_record(&source_root, &plugin, &hooks, &manifest_bytes)?;
    let mut records = load_records(request.plugins_dir)?;
    records.insert(plugin.name.clone(), serde_json::to_value(&record)?);
    save_records(request.plugins_dir, &records)?;
    Ok(InstallReport {
        name: plugin.name.clone(),
        dest,
        plugin,
        hook,
    })
}

/// Uninstall a plugin. The removal hook runs first; data is preserved unless
/// `--purge` is given. A missing/forced hook failure keeps the directory.
pub async fn uninstall(request: &UninstallRequest<'_>) -> anyhow::Result<UninstallReport> {
    let plugin_dir = request.plugins_dir.join(request.name);
    anyhow::ensure!(
        plugin_dir.exists(),
        "Plugin '{}' not found at '{}'",
        request.name,
        plugin_dir.display()
    );
    let blocking = dependents(request.plugins_dir, request.name);
    anyhow::ensure!(
        blocking.is_empty() || request.force,
        "Plugin '{}' is required by: {}. Pass --force to remove it anyway.",
        request.name,
        blocking.join(", ")
    );
    if request.dry_run {
        println!("Dry run: would remove {}", plugin_dir.display());
        return Ok(UninstallReport {
            name: request.name.to_string(),
            context_keys: Vec::new(),
            secret_keys: Vec::new(),
            hook: HookOutcome {
                ran: false,
                skipped_reason: Some("dry run".into()),
                ..Default::default()
            },
            dir_removed: false,
        });
    }

    let manifest_path = plugin_dir.join("plugin.json");
    let plugin = if manifest_path.is_file() {
        Some(serde_json::from_slice::<Plugin>(&std::fs::read(&manifest_path)?)?)
    } else {
        None
    };
    let mut context_keys = Vec::new();
    let mut secret_keys = Vec::new();
    if let Some(plugin) = &plugin {
        context_keys = plugin.context.keys().cloned().collect();
        secret_keys = plugin.secrets.clone();
    }

    let mut hook = HookOutcome {
        ran: false,
        skipped_reason: Some("plugin declares no uninstall hook".into()),
        ..Default::default()
    };
    if let Some(plugin) = &plugin {
        let hooks = validate_hooks(&plugin_dir.canonicalize()?, plugin.hooks.clone())?;
        if let Some(script) = &hooks.uninstall {
            // A changed uninstall script cannot run without explicit consent.
            let records = load_records(request.plugins_dir)?;
            if !request.force {
                if let Some(recorded) = records
                    .get(request.name)
                    .and_then(|value| serde_json::from_value::<InstallRecord>(value.clone()).ok())
                    .and_then(|record| record.uninstall)
                {
                    let current = hook_record(&plugin_dir, script)?;
                    anyhow::ensure!(
                        recorded.path == current.path && recorded.sha256 == current.sha256,
                        "Uninstall hook for '{}' changed since install; pass --force to run it",
                        request.name
                    );
                }
            }
            if request.run_hooks {
                std::fs::create_dir_all(request.data_dir)?;
                let script_path = plugin_dir.join(script);
                match run_hook(
                    HookKind::Uninstall,
                    &plugin_dir,
                    &script_path,
                    request.data_dir,
                    request.plugins_dir,
                    plugin,
                    request.purge,
                )
                .await
                {
                    Ok(outcome) if outcome.exit_code == Some(0) => {
                        report_hook(&outcome, "uninstall");
                        hook = outcome;
                    }
                    Ok(outcome) => {
                        report_hook(&outcome, "uninstall");
                        if !request.force {
                            anyhow::bail!(
                                "Uninstall hook for '{}' exited with {}. The plugin directory was kept; pass --force to remove it anyway.",
                                request.name,
                                outcome.exit_code.unwrap_or(-1)
                            );
                        }
                    }
                    Err(error) => {
                        if !request.force {
                            return Err(error);
                        }
                    }
                }
            } else {
                hook.skipped_reason =
                    Some("hooks were not run (policy or --no-scripts)".into());
            }
        }
    }

    std::fs::remove_dir_all(&plugin_dir)?;
    if request.purge {
        // Convention fallback for plugins that do not implement --purge in their
        // uninstall hook: remove DATA_DIR/<id>. Explicit operator opt-in only.
        let scoped = request.data_dir.join(request.name);
        if scoped.is_dir() {
            let _ = std::fs::remove_dir_all(&scoped);
        }
    }
    let mut records = load_records(request.plugins_dir)?;
    records.remove(request.name);
    save_records(request.plugins_dir, &records)?;
    Ok(UninstallReport {
        name: request.name.to_string(),
        context_keys,
        secret_keys,
        hook,
        dir_removed: true,
    })
}

/// Bundled plugins installed by `praxis plugin install-default` in a source
/// checkout. Paths resolve from the installation root; missing entries are
/// reported, not fatal.
pub const DEFAULT_PRESET_PLUGINS: &[&str] = &[
    "examples/plugins/hooks_demo",
    "examples/tool-packages/allowlist_shell",
];

#[derive(Debug, Clone)]
pub struct PresetRequest<'a> {
    pub preset_path: Option<&'a Path>,
    pub root: &'a Path,
    pub plugins_dir: &'a Path,
    pub data_dir: &'a Path,
    pub run_hooks: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PresetReport {
    pub installed: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Install every plugin named by a preset in dependency order. Existing plugins
/// are skipped. A custom preset is `{"plugins": ["<dir>", ...]}`; otherwise the
/// bundled default list is used. `requires.plugins` that name another preset
/// entry are installed first; a cycle is reported instead of guessed.
pub async fn install_default(request: &PresetRequest<'_>) -> anyhow::Result<PresetReport> {
    let sources: Vec<String> = match request.preset_path {
        Some(path) => {
            #[derive(Deserialize)]
            struct Raw {
                #[serde(default)]
                plugins: Vec<String>,
            }
            serde_json::from_slice::<Raw>(&std::fs::read(path)?)?.plugins
        }
        None => DEFAULT_PRESET_PLUGINS.iter().map(|s| (*s).to_string()).collect(),
    };

    struct Entry {
        source: PathBuf,
        requires: crate::plugins::PluginRequires,
    }
    let mut entries: std::collections::BTreeMap<String, Entry> = std::collections::BTreeMap::new();
    let mut report = PresetReport::default();
    for relative in &sources {
        let path = Path::new(relative);
        let source = if path.is_absolute() {
            path.to_path_buf()
        } else {
            request.root.join(path)
        };
        let manifest = source.join("plugin.json");
        if !manifest.is_file() {
            report
                .failed
                .push((relative.clone(), "no plugin.json".into()));
            continue;
        }
        let plugin: Plugin = serde_json::from_slice(&std::fs::read(&manifest)?)?;
        let requires = crate::plugins::requires_declaration(&manifest)?;
        entries.insert(plugin.name, Entry { source, requires });
    }

    // Kahn topological order: dependencies (in this preset) install first.
    let mut pending: std::collections::BTreeMap<String, usize> = entries
        .iter()
        .map(|(name, entry)| {
            let count = entry
                .requires
                .plugins
                .iter()
                .filter(|dep| entries.contains_key(*dep))
                .count();
            (name.clone(), count)
        })
        .collect();
    let mut order: Vec<String> = Vec::new();
    loop {
        let ready: Vec<String> = pending
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(name, _)| name.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for name in ready {
            pending.remove(&name);
            for candidate in pending.keys().cloned().collect::<Vec<_>>() {
                if entries[&candidate].requires.plugins.iter().any(|dep| dep == &name) {
                    if let Some(count) = pending.get_mut(&candidate) {
                        *count = count.saturating_sub(1);
                    }
                }
            }
            order.push(name);
        }
    }
    for name in pending.keys() {
        report
            .failed
            .push((name.clone(), "dependency cycle".into()));
    }

    for name in order {
        let entry = &entries[&name];
        if request.plugins_dir.join(&name).exists() {
            report.skipped.push(name);
            continue;
        }
        match install(&InstallRequest {
            source: &entry.source,
            plugins_dir: request.plugins_dir,
            data_dir: request.data_dir,
            run_hooks: request.run_hooks,
            dry_run: request.dry_run,
        })
        .await
        {
            Ok(installed) => report.installed.push(installed.name),
            Err(error) => report.failed.push((name, error.to_string())),
        }
    }
    Ok(report)
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub ok: Vec<String>,
    /// Plugins whose files no longer match the locked hashes.
    pub changed: Vec<(String, String)>,
    /// Locked plugins whose directory is gone.
    pub missing: Vec<String>,
    /// Directories on disk that were not installed through the lifecycle.
    pub unlocked: Vec<String>,
}

/// Check installed plugins against `praxis.lock.json`. A changed hook or
/// manifest means the package is no longer the revision the operator approved.
pub fn verify(plugins_dir: &Path) -> anyhow::Result<VerifyReport> {
    let records = load_records(plugins_dir)?;
    let mut report = VerifyReport::default();
    for (name, value) in &records {
        let record: InstallRecord = serde_json::from_value(value.clone())?;
        let dir = plugins_dir.join(name);
        if !dir.join("plugin.json").is_file() {
            report.missing.push(name.clone());
            continue;
        }
        let manifest_bytes = std::fs::read(dir.join("plugin.json"))?;
        if sha256_bytes(&manifest_bytes) != record.manifest_sha256 {
            report.changed.push((name.clone(), "manifest changed".into()));
            continue;
        }
        let mut mismatch = None;
        for (slot, hook) in [("install", &record.install), ("uninstall", &record.uninstall)] {
            if let Some(hook) = hook {
                match std::fs::read(dir.join(&hook.path)) {
                    Ok(bytes) if sha256_bytes(&bytes) == hook.sha256 => {}
                    Ok(_) => mismatch = Some(format!("{slot} hook changed")),
                    Err(_) => mismatch = Some(format!("{slot} hook missing")),
                }
                if mismatch.is_some() {
                    break;
                }
            }
        }
        match mismatch {
            Some(reason) => report.changed.push((name.clone(), reason)),
            None => report.ok.push(name.clone()),
        }
    }
    if let Ok(entries) = std::fs::read_dir(plugins_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().is_dir() || name.starts_with('.') {
                continue;
            }
            if entry.path().join("plugin.json").is_file() && !records.contains_key(&name) {
                report.unlocked.push(name);
            }
        }
    }
    report.ok.sort();
    report.missing.sort();
    report.unlocked.sort();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_parsing_and_planning() {
        assert_eq!(HookPolicy::parse("allow").unwrap(), HookPolicy::Allow);
        assert_eq!(HookPolicy::parse("ASK").unwrap(), HookPolicy::Ask);
        assert_eq!(HookPolicy::parse("deny").unwrap(), HookPolicy::Deny);
        assert!(HookPolicy::parse("maybe").is_err());
        assert_eq!(plan_hooks(HookPolicy::Allow, false, false), HookPlan::Run);
        assert_eq!(plan_hooks(HookPolicy::Ask, false, false), HookPlan::Ask);
        assert!(matches!(
            plan_hooks(HookPolicy::Deny, false, false),
            HookPlan::Skip(_)
        ));
        assert_eq!(plan_hooks(HookPolicy::Deny, true, false), HookPlan::Run);
        assert!(matches!(
            plan_hooks(HookPolicy::Allow, true, true),
            HookPlan::Skip(_)
        ));
    }
}
