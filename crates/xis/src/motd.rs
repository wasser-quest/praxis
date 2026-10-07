//! The change report (MOTD) `xis` leaves behind and Praxis shows on
//! `praxis run` / `praxis motd`.
//!
//! Rules: never a secret value, never an enabled tool or trust role, and
//! never a claim that a skipped step succeeded. A setup that needs a
//! `runtime`/`authority` plugin lists the exact `praxis plugin trust …`
//! command for the operator.
use crate::plan::{ConfigLine, PlannedFile};
use crate::repo::ConfigChange;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub setup: String,
    pub version: String,
    #[serde(default)]
    pub required: Vec<ConfigChange>,
    #[serde(default)]
    pub backed_up: Vec<String>,
    #[serde(default)]
    pub kept: Vec<String>,
    #[serde(default)]
    pub operator_edits: Vec<String>,
    #[serde(default)]
    pub config: Vec<ConfigLine>,
    #[serde(default)]
    pub trust_commands: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Report {
    pub fn path(root: &Path) -> PathBuf {
        root.join("xis-motd.md")
    }

    pub fn render(&self) -> String {
        let mut out = format!("xis: {} {} installed\n", self.setup, self.version);
        if !self.required.is_empty() {
            out.push_str("\nRequired changes\n");
            for change in &self.required {
                out.push_str(&format!(
                    "  {:<16} {}{}\n",
                    change.key,
                    change.reason,
                    if change.required { " (required)" } else { "" }
                ));
            }
        }
        if !self.config.is_empty() {
            out.push_str("\nConfiguration applied\n");
            for line in &self.config {
                out.push_str(&format!(
                    "  {:<16} {} -> {}\n",
                    line.key,
                    line.old.as_deref().unwrap_or("(unset)"),
                    line.new.as_deref().unwrap_or("(unset)")
                ));
            }
        }
        if !self.backed_up.is_empty() {
            out.push_str(&format!("\nBacked up ({})\n", self.backed_up.len()));
            for file in &self.backed_up {
                out.push_str(&format!("  {file}\n"));
            }
        }
        if !self.kept.is_empty() {
            out.push_str("\nKept (existing files win)\n");
            for file in &self.kept {
                out.push_str(&format!("  {file}\n"));
            }
        }
        if !self.operator_edits.is_empty() {
            out.push_str("\nOperator edits left alone (use --force to replace, backed up)\n");
            for file in &self.operator_edits {
                out.push_str(&format!("  {file}\n"));
            }
        }
        if !self.trust_commands.is_empty() {
            out.push_str("\nTrust (explicit operator action; xis never runs it)\n");
            for command in &self.trust_commands {
                out.push_str(&format!("  {command}\n"));
            }
        }
        for note in &self.notes {
            out.push_str(&format!("\nNotes\n  {note}\n"));
        }
        out
    }

    pub fn write(&self, root: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(root)?;
        std::fs::write(Self::path(root), self.render())?;
        Ok(())
    }

    pub fn read(root: &Path) -> anyhow::Result<Option<String>> {
        match std::fs::read_to_string(Self::path(root)) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn acknowledge(root: &Path) -> anyhow::Result<bool> {
        Ok(std::fs::remove_file(Self::path(root)).is_ok())
    }
}

/// Build the report from what the plan said and what apply did. A step that
/// was skipped is reported as skipped, never as done.
pub fn build(
    setup: &str,
    version: &str,
    required: Vec<ConfigChange>,
    config: Vec<ConfigLine>,
    files: &[PlannedFile],
    backups: &[(String, Option<String>)],
    trust_commands: Vec<String>,
    notes: Vec<String>,
) -> Report {
    let mut report = Report {
        setup: setup.to_string(),
        version: version.to_string(),
        required,
        config,
        trust_commands,
        notes,
        ..Default::default()
    };
    for file in files {
        match &file.action {
            crate::plan::FileAction::Keep { operator_edit: false } => {
                report.kept.push(file.relative.clone())
            }
            crate::plan::FileAction::Keep { operator_edit: true }
            | crate::plan::FileAction::Replace { operator_edit: true }
            | crate::plan::FileAction::RemoveKeep { operator_edit: true } => {
                report.operator_edits.push(file.relative.clone())
            }
            crate::plan::FileAction::RemoveKeep { operator_edit: false } => {
                report.kept.push(file.relative.clone())
            }
            _ => {}
        }
    }
    for (relative, backup) in backups {
        if let Some(backup) = backup {
            report.backed_up.push(format!("{relative} -> {backup}"));
        }
    }
    report
}
