//! Plans: what `xis` would write, keep, back up, remove or require —
//! computed completely and printed before anything touches disk.
use crate::repo::{sha256_hex, Bundle, ConfigChange, Setup};
use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Non-secret keys a config profile may write. Everything else — especially
/// anything that looks like a credential — is listed in the report and never
/// written.
pub const CONFIG_KEYS: &[&str] = &[
    "USE_PROVIDER",
    "POML_CLI",
    "SM_FILE",
    "GATEWAY_PORT",
    "DASHBOARD_PORT",
    "DASHBOARD_TLS",
    "OPENAI_MODEL",
    "ANTHROPIC_MODEL",
    "CODEX_MODEL",
    "OPENROUTER_MODEL",
    "OLLAMA_MODEL",
    "MIMO_MODEL",
    "TOOL_GROUPS",
];

pub fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    ["KEY", "TOKEN", "PASSWORD", "SECRET", "CREDENTIAL"]
        .iter()
        .any(|ending| key.ends_with(ending))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Existing files win; conflicts are reported.
    Keep,
    /// Conflicts are backed up, then written.
    Backup,
    /// Like backup, and operator edits are replaced too (needs `--force`).
    Overwrite,
}

impl Policy {
    pub fn new(backup: bool, force: bool) -> anyhow::Result<Self> {
        ensure!(!(backup && force), "choose --backup or --force, not both");
        Ok(if force {
            Self::Overwrite
        } else if backup {
            Self::Backup
        } else {
            Self::Keep
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum FileAction {
    /// The destination does not exist: write it and record ownership.
    Write,
    /// A file we own and did not edit: replace it (previous bytes backed up).
    Update,
    /// An unowned file or one we own but the operator edited: keep unless
    /// forced, and report the conflict either way.
    Keep { operator_edit: bool },
    /// Forced replacement of a conflict: the previous bytes are backed up.
    Replace { operator_edit: bool },
    /// A file a removed setup owns and no operator edited: delete it.
    Remove,
    /// A file a removed setup owns but the operator edited: leave it alone.
    RemoveKeep { operator_edit: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedFile {
    /// Path relative to the destination root.
    pub relative: String,
    pub action: FileAction,
    /// SHA-256 of the bytes on disk today, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigLine {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub reference: String,
    pub version: String,
    pub files: Vec<PlannedFile>,
    pub config: Vec<ConfigLine>,
    /// Environment the operator must set; never written by `xis`.
    pub required: Vec<ConfigChange>,
    /// Plugin packages delegated to the Praxis CLI.
    pub plugins: Vec<(String, String)>,
}

impl Plan {
    pub fn conflicts(&self) -> impl Iterator<Item = &PlannedFile> {
        self.files.iter().filter(|file| {
            matches!(
                file.action,
                FileAction::Keep { operator_edit: true } | FileAction::Replace { operator_edit: true }
            )
        })
    }
}

/// Files recorded for one installed setup (`xis.lock.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupLock {
    pub version: String,
    #[serde(default)]
    pub repository: String,
    /// Owned destination paths → SHA-256 at install time.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lock {
    pub schema: u32,
    #[serde(default)]
    pub setups: BTreeMap<String, SetupLock>,
}

impl Lock {
    pub fn path(root: &Path) -> PathBuf {
        root.join("xis.lock.json")
    }

    pub fn load(root: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(Self::path(root)) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self { schema: 1, setups: BTreeMap::new() })
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, root: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(root)?;
        std::fs::write(Self::path(root), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Destination roots for each artifact kind. Everything lands under the
/// Praxis installation root; plugins go to the plugins directory.
pub fn destination_root(kind: &str, root: &Path, plugins_dir: &Path) -> PathBuf {
    if kind == "plugin" {
        plugins_dir.to_path_buf()
    } else {
        root.to_path_buf()
    }
}

/// Compute everything an install or upgrade would do, without touching disk.
#[allow(clippy::too_many_arguments)]
pub fn plan_install(
    setup: &Setup,
    reference: &str,
    artifacts: &[(String, Bundle)],
    policy: Policy,
    root: &Path,
    plugins_dir: &Path,
    lock: &Lock,
    config: &BTreeMap<String, String>,
) -> anyhow::Result<Plan> {
    let mut files = Vec::new();
    for (relative, bundle) in artifacts {
        for (path, bytes) in bundle.files()? {
            let relative = Path::new(relative);
            let joined = relative.join(&path);
            let relative = joined
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let target = destination_root("content", root, plugins_dir).join(&relative);
            let current = std::fs::read(&target).ok();
            let current_sha256 = current.as_deref().map(sha256_hex);
            let owned = lock
                .setups
                .get(&setup.name)
                .and_then(|entry| entry.files.get(&relative))
                .cloned();
            let operator_edit = owned.as_ref().is_some_and(|hash| current_sha256.as_deref() != Some(hash.as_str()));
            let action = match (&current, &owned) {
                (None, _) => FileAction::Write,
                (Some(_), None) => conflict_action(policy, false),
                (Some(_), Some(_)) if operator_edit => conflict_action(policy, true),
                (Some(_), Some(_)) => {
                    if current_sha256 == Some(sha256_hex(&bytes)) {
                        // Already the target bytes: nothing to do but keep
                        // ownership for later upgrades and removals.
                        FileAction::Update
                    } else {
                        FileAction::Update
                    }
                }
            };
            files.push(PlannedFile { relative, action, current_sha256 });
        }
    }
    files.sort_by(|a, b| a.relative.cmp(&b.relative));

    // Config profiles touch documented non-secret keys only, and every change
    // is a diff the operator sees before anything is written.
    let mut config_lines = Vec::new();
    for (key, value) in &setup.config_clone() {
        ensure!(
            CONFIG_KEYS.contains(&key.as_str()) && !is_secret_key(key),
            "'{key}' is not a documented non-secret configuration key"
        );
        let old = config.get(key).cloned();
        if old.as_deref() != Some(value.as_str()) {
            config_lines.push(ConfigLine { key: key.clone(), old, new: Some(value.clone()) });
        }
    }

    let plugins = setup
        .items
        .iter()
        .filter(|item| item.kind == "plugin")
        .map(|item| {
            (
                item.name.clone().unwrap_or_default(),
                item.version.clone().unwrap_or_default(),
            )
        })
        .collect();

    Ok(Plan {
        reference: reference.to_string(),
        version: setup.version.clone(),
        files,
        config: config_lines,
        required: setup.config_changes.clone(),
        plugins,
    })
}

fn conflict_action(policy: Policy, operator_edit: bool) -> FileAction {
    match policy {
        Policy::Keep => FileAction::Keep { operator_edit },
        Policy::Backup | Policy::Overwrite => FileAction::Replace { operator_edit },
    }
}

/// Compute what removing a setup touches: only files it owns and nobody
/// edited. `keep_data` reports every owned file and removes none of them.
pub fn plan_remove(
    setup: &str,
    lock: &Lock,
    root: &Path,
    plugins_dir: &Path,
    keep_data: bool,
) -> anyhow::Result<(Vec<PlannedFile>, Vec<(String, String)>)> {
    let Some(entry) = lock.setups.get(setup) else {
        bail!("'{setup}' is not installed");
    };
    let mut files = Vec::new();
    for (relative, hash) in &entry.files {
        let target = destination_root("content", root, plugins_dir).join(relative);
        let current = std::fs::read(&target).ok().map(|bytes| sha256_hex(&bytes));
        let operator_edit = current.as_deref().is_some_and(|now| now != hash);
        let action = match (keep_data, operator_edit) {
            (true, _) => FileAction::RemoveKeep { operator_edit },
            (false, true) => FileAction::RemoveKeep { operator_edit: true },
            (false, false) => FileAction::Remove,
        };
        files.push(PlannedFile { relative: relative.clone(), action, current_sha256: current });
    }
    Ok((files, entry.files.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
}

/// Parse a `.env`-style file into a key/value map (last occurrence wins).
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    values
}

/// Render a `.env`-style file with updated keys, preserving comments and
/// unknown lines (the operator's file is never rewritten from scratch).
pub fn render_env(text: &str, changes: &[ConfigLine]) -> String {
    let mut out = String::new();
    let mut written: Vec<&str> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let key = trimmed
            .split_once('=')
            .map(|(key, _)| key.trim())
            .filter(|key| !key.is_empty() && !key.starts_with('#'));
        match key.and_then(|key| changes.iter().find(|change| change.key == key)) {
            Some(change) => {
                if let Some(new) = &change.new {
                    out.push_str(&format!("{}={}\n", change.key, new));
                    written.push(&change.key);
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for change in changes {
        if !written.contains(&&*change.key) {
            if let Some(new) = &change.new {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&format!("{}={}\n", change.key, new));
            }
        }
    }
    out
}

impl Setup {
    fn config_clone(&self) -> BTreeMap<String, String> {
        self.config.clone()
    }
}
