//! Declarative source builds for `praxis plugin install --build`.
//!
//! One command installs every package: plugins that ship code are built first,
//! plugins that ship scripts or assets skip the step. The build is deliberately
//! not a command list — the host runs `cargo build` for the declared crate and
//! copies the declared outputs into the package, and nothing else. A manifest
//! can never make the installer execute arbitrary commands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// How to build this package from source before installing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageBuild {
    /// Workspace crate to build: `cargo build --locked -p <crate>`.
    #[serde(rename = "crate")]
    pub crate_name: String,
    /// Binaries produced in the cargo target directory; each is staged as
    /// `bin/<name>` inside the package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bins: Vec<String>,
    /// `dest-inside-package -> source-relative-to-the-workspace-root` directories
    /// to stage (for example a web frontend's `static/`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dirs: BTreeMap<String, String>,
}

/// Build the package at `source` and stage its outputs inside it, so the
/// manifest's declared paths (`bin/…`, `static/…`) resolve before install
/// validation runs. Returns the staged paths relative to the package.
pub fn build_package(source: &Path, build: &PackageBuild) -> anyhow::Result<Vec<String>> {
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".to_string());
    anyhow::ensure!(
        profile == "release" || profile == "debug",
        "PROFILE must be release or debug (got '{profile}')"
    );
    let mut command = std::process::Command::new("cargo");
    command
        .current_dir(source)
        .args(["build", "--locked", "-p", &build.crate_name]);
    if profile == "release" {
        command.arg("--release");
    }
    let status = command.status()?;
    anyhow::ensure!(
        status.success(),
        "cargo build -p {} failed ({status})",
        build.crate_name
    );

    let metadata_output = std::process::Command::new("cargo")
        .current_dir(source)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()?;
    anyhow::ensure!(metadata_output.status.success(), "cargo metadata failed");
    let metadata: serde_json::Value = serde_json::from_slice(&metadata_output.stdout)?;
    let target_dir = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .context("cargo metadata returned no target_directory")?,
    );
    let workspace_root = PathBuf::from(
        metadata["workspace_root"]
            .as_str()
            .context("cargo metadata returned no workspace_root")?,
    );

    let mut staged = Vec::new();
    for bin in &build.bins {
        let built = target_dir.join(&profile).join(bin);
        anyhow::ensure!(built.is_file(), "build produced no '{}'", built.display());
        let dest = source.join("bin").join(bin);
        std::fs::create_dir_all(dest.parent().context("bin/")?)?;
        std::fs::copy(&built, &dest)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
        }
        staged.push(format!("bin/{bin}"));
    }
    for (dest, from) in &build.dirs {
        let from_dir = workspace_root.join(from);
        anyhow::ensure!(
            from_dir.is_dir(),
            "declared build directory '{}' does not exist",
            from_dir.display()
        );
        let dest_dir = source.join(dest);
        if dest_dir.exists() {
            std::fs::remove_dir_all(&dest_dir)?;
        }
        crate::plugins::lifecycle::copy_dir_recursive(&from_dir, &dest_dir)?;
        staged.push(dest.clone());
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_declarations_are_strict() {
        let build: PackageBuild =
            serde_json::from_str(r#"{"crate":"xis","bins":["xis"],"dirs":{"static":"static"}}"#)
                .unwrap();
        assert_eq!(build.crate_name, "xis");
        assert_eq!(build.bins, vec!["xis".to_string()]);
        assert!(serde_json::from_str::<PackageBuild>(r#"{"crate":"x","command":"rm -rf /"}"#).is_err(),
            "a manifest can never declare commands to run");
        assert!(serde_json::from_str::<PackageBuild>(r#"{"bins":["x"]}"#).is_err(),
            "the crate to build is required");
    }

    #[test]
    fn missing_build_is_not_a_command() {
        let plugin: crate::plugins::Plugin =
            serde_json::from_str(r#"{"name":"x","description":"d","version":"1","tools":[]}"#).unwrap();
        assert!(plugin.build.is_none());
    }
}
