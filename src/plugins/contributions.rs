//! Versioned non-tool package contributions. Declarations reserve namespaces;
//! they never launch a worker or grant host authority during manifest loading.
use super::TrustRole;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessContribution {
    pub id: String,
    pub executable: String,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Host-resolved implementation identity; never accepted from the manifest.
    #[serde(skip)]
    pub(crate) resolved: Option<PathBuf>,
    /// Exact loader provenance, not inferred from a package ID or executable.
    #[serde(skip)]
    pub(crate) source: Option<ContributionSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContributionSource {
    pub(crate) manifest: PathBuf,
    pub(crate) package_dir: PathBuf,
}

fn default_timeout() -> u64 {
    120
}

impl ProcessContribution {
    pub(crate) fn implementation_digest(&self) -> anyhow::Result<String> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let path = self
            .resolved
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Unresolved contribution implementation"))?;
        let mut bytes = Vec::new();
        let file = std::fs::File::open(path)?;
        const MAX: u64 = 128 * 1024 * 1024;
        file.take(MAX + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX,
            "Contribution implementation exceeds 128 MiB"
        );
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn validate(&self, kind: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            praxis_plugin_api::identifier(&self.id),
            "Invalid {kind} identifier"
        );
        let (protocol, allowed): (&str, &[&str]) = match kind {
            "provider" => (
                "praxis.provider.v1",
                &[
                    "chat",
                    "stream",
                    "models",
                    "embedding",
                    "decision",
                    "continuation",
                    "output_limit",
                ],
            ),
            "channel" => (
                "praxis.channel.v1",
                &["ingress", "delivery", "files", "voice"],
            ),
            "media" => (
                "praxis.media.v1",
                &[
                    "transcription",
                    "synthesis",
                    "image",
                    "video",
                    "gpu_routing",
                ],
            ),
            _ => anyhow::bail!("Unknown process contribution kind"),
        };
        anyhow::ensure!(self.protocol == protocol, "Unsupported {kind} protocol");
        let path = Path::new(&self.executable);
        anyhow::ensure!(
            path.is_relative()
                && path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                && !self.executable.is_empty(),
            "Contribution executable must be inside its package"
        );
        anyhow::ensure!(
            (1..=3600).contains(&self.timeout_secs),
            "Contribution timeout must be 1..3600 seconds"
        );
        anyhow::ensure!(
            self.args.len() <= 64
                && self
                    .args
                    .iter()
                    .all(|arg| arg.len() <= 8192 && !arg.contains('\0')),
            "Invalid contribution arguments"
        );
        anyhow::ensure!(
            !self.capabilities.is_empty(),
            "Contribution must declare capabilities"
        );
        let mut seen = std::collections::HashSet::new();
        for capability in &self.capabilities {
            anyhow::ensure!(
                allowed.contains(&capability.as_str()) && seen.insert(capability),
                "Unknown or duplicate {kind} capability"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for secret in &self.secrets {
            anyhow::ensure!(
                praxis_plugin_api::identifier(secret) && seen.insert(secret),
                "Invalid or duplicate contribution secret"
            );
        }
        if kind == "provider" {
            anyhow::ensure!(
                self.capabilities
                    .iter()
                    .any(|c| c == "chat" || c == "embedding"),
                "Provider needs chat or embedding capability"
            );
        }
        Ok(())
    }

    fn validate_grants(
        &self,
        role: TrustRole,
        declared_secrets: &[String],
        kind: &str,
    ) -> anyhow::Result<()> {
        self.validate(kind)?;
        let required = if kind == "channel" {
            TrustRole::Channel
        } else {
            TrustRole::Runtime
        };
        anyhow::ensure!(
            role >= required,
            "{kind} contribution requires explicit {required:?} trust role"
        );
        anyhow::ensure!(
            self.secrets
                .iter()
                .all(|key| declared_secrets.contains(key)),
            "Contribution requests undeclared package secrets"
        );
        Ok(())
    }

    pub(crate) fn validate_registration(
        &self,
        role: TrustRole,
        declared_secrets: &[String],
        kind: &str,
    ) -> anyhow::Result<()> {
        self.validate_grants(role, declared_secrets, kind)?;
        let path = self
            .resolved
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Unresolved {kind} implementation"))?;
        anyhow::ensure!(
            path.canonicalize()? == path && path.is_file(),
            "Contribution implementation changed or disappeared"
        );
        Ok(())
    }

    pub(crate) fn resolve(
        &mut self,
        root: &Path,
        role: TrustRole,
        declared_secrets: &[String],
        kind: &str,
    ) -> anyhow::Result<()> {
        self.validate_grants(role, declared_secrets, kind)?;
        let root = root.canonicalize()?;
        let path = root.join(&self.executable);
        anyhow::ensure!(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "Contribution executable cannot be a symlink"
        );
        let resolved = path.canonicalize()?;
        anyhow::ensure!(
            resolved.starts_with(&root) && resolved.is_file(),
            "Contribution executable escapes its package or is not a regular file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                std::fs::metadata(&resolved)?.permissions().mode() & 0o111 != 0,
                "Contribution worker is not executable"
            );
        }
        self.resolved = Some(resolved);
        Ok(())
    }
}
