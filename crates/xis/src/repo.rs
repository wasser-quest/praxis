//! Repository indexes, operator-pinned keys and content-addressed artifacts.
//!
//! `xis` trusts nothing a repository says about itself: the signing key is
//! pinned by the operator, every artifact is addressed by its SHA-256, and a
//! version is immutable (republishing the same version with different bytes is
//! a verify failure). Repositories are operator-trusted code like Praxis
//! plugins: signatures make them tamper-evident, not sandboxed.
use anyhow::{bail, ensure, Context as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// One operator-pinned repository. Keys live here and nowhere else.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    pub url: String,
    /// `ed25519:<hex public key>`. Missing means unsigned-by-policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// The operator's repository pins (`~/.config/xis/repositories.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepositoryStore {
    #[serde(default)]
    pub repositories: Vec<Repository>,
}

impl RepositoryStore {
    pub fn config_dir() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("xis")
    }

    pub fn path() -> PathBuf {
        Self::config_dir().join("repositories.json")
    }

    pub fn load() -> anyhow::Result<Self> {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Repository> {
        self.repositories.iter().find(|repo| repo.name == name)
    }

    /// Add or replace a pin. The key always comes from the operator.
    pub fn pin(&mut self, name: &str, url: &str, key: Option<&str>) -> anyhow::Result<()> {
        let url = url.trim().trim_end_matches('/').to_string();
        ensure!(!url.is_empty(), "repository URL must not be empty");
        ensure!(
            url.starts_with("https://")
                || url.starts_with("http://")
                || url.starts_with("file://"),
            "repository URL must be http(s) or file://"
        );
        if let Some(key) = key {
            verify_key_shape(key)?;
        }
        self.repositories.retain(|repo| repo.name != name);
        self.repositories.push(Repository {
            name: name.to_string(),
            url,
            key: key.map(str::to_string),
        });
        self.repositories.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> anyhow::Result<()> {
        let before = self.repositories.len();
        self.repositories.retain(|repo| repo.name != name);
        ensure!(before != self.repositories.len(), "unknown repository '{name}'");
        Ok(())
    }
}

fn verify_key_shape(key: &str) -> anyhow::Result<()> {
    let hex_key = key
        .strip_prefix("ed25519:")
        .context("repository keys must look like ed25519:<hex public key>")?;
    ensure!(hex_key.len() == 64, "ed25519 keys are 64 hex characters");
    ensure!(hex_key.bytes().all(|b| b.is_ascii_hexdigit()), "ed25519 keys are hex");
    Ok(())
}

// ── the index ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryIndex {
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub generated_at: String,
    /// Informational only: the pinned key is what `xis` verifies against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key: Option<String>,
    pub setups: Vec<Setup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setup {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_api: Option<String>,
    #[serde(default)]
    pub items: Vec<Item>,
    /// Non-secret settings this setup applies (documented keys only).
    #[serde(default)]
    pub config: std::collections::BTreeMap<String, String>,
    /// Environment the operator must supply; listed, never written.
    #[serde(default)]
    pub config_changes: Vec<ConfigChange>,
}

impl Setup {
    /// `repo/name@version` — the reference operators type.
    pub fn reference(&self, repo: &str) -> String {
        format!("{repo}/{}@{}", self.name, self.version)
    }
}

/// One artifact of a setup: a content-addressed bundle unpacked into place.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// `templates`, `contexts`, `skills`, `plugin`, `assets`, `workflows`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Destination root for directory kinds (`templates/`) or a file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Plugin items name and version the package they install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// SHA-256 of the artifact bytes (`<repo>/artifacts/<sha256>`).
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChange {
    pub key: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub reason: String,
}

/// A file inside an artifact bundle. Bundles are JSON so `xis` needs no
/// archive tooling and every byte is verifiable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub files: Vec<BundleFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    pub content_base64: String,
}

impl Bundle {
    pub fn digest(&self) -> String {
        sha256_hex(&serde_json::to_vec(self).unwrap_or_default())
    }

    pub fn files(&self) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        self.files
            .iter()
            .map(|file| {
                let path = Path::new(&file.path);
                ensure!(
                    path.is_relative()
                        && path
                            .components()
                            .all(|c| matches!(c, std::path::Component::Normal(_))),
                    "bundle file '{}' must be a relative path inside its root",
                    file.path
                );
                let bytes = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &file.content_base64,
                )
                .context("bundle file is not valid base64")?;
                Ok((file.path.clone(), bytes))
            })
            .collect()
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// ── fetching and verification ────────────────────────────────────────────

/// A fetched index with its verification state.
#[derive(Debug, Clone)]
pub struct VerifiedIndex {
    pub index: RepositoryIndex,
    pub repository: String,
    pub signed: bool,
}

pub fn fetch_index(repo: &Repository, allow_unsigned: bool) -> anyhow::Result<VerifiedIndex> {
    let (bytes, signature) = fetch_index_bytes(repo)?;
    let index: RepositoryIndex = serde_json::from_slice(&bytes)
        .context("repository index is not valid JSON")?;
    ensure!(index.schema == 1, "unsupported repository index schema {}", index.schema);
    ensure!(
        index.name == repo.name,
        "repository index names '{}', but the pin is '{}'",
        index.name,
        repo.name
    );
    let signed = match (&repo.key, signature.as_deref()) {
        (Some(key), Some(signature)) => {
            verify_signature(&bytes, key, signature)?;
            if let Some(declared) = &index.signing_key {
                ensure!(
                    declared == key,
                    "repository declares a different signing key than the operator pinned"
                );
            }
            true
        }
        (Some(_), None) => bail!("repository '{repo}' has a pinned key but no index signature", repo = repo.name),
        (None, Some(_)) => bail!(
            "repository '{}' is signed but no key is pinned; add it with 'xis repo add --key'",
            repo.name
        ),
        (None, None) => {
            ensure!(
                allow_unsigned,
                "repository '{}' is unsigned; pass --allow-unsigned to trust it anyway",
                repo.name
            );
            false
        }
    };
    verify_versions(&index)?;
    Ok(VerifiedIndex { index, repository: repo.name.clone(), signed })
}

fn fetch_index_bytes(repo: &Repository) -> anyhow::Result<(Vec<u8>, Option<String>)> {
    if let Some(path) = repo.url.strip_prefix("file://") {
        let bytes = std::fs::read(Path::new(path).join("index.json"))?;
        let signature = std::fs::read_to_string(Path::new(path).join("index.sig"))
            .ok()
            .map(|text| text.trim().to_string());
        return Ok((bytes, signature));
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("xis/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let index = client
        .get(format!("{}/index.json", repo.url))
        .send()
        .context("cannot reach the repository index")?
        .error_for_status()
        .context("repository index request failed")?
        .bytes()?;
    let signature = client
        .get(format!("{}/index.sig", repo.url))
        .send()
        .ok()
        .filter(|response| response.status().is_success())
        .map(|response| response.text().unwrap_or_default())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    Ok((index.to_vec(), signature))
}

/// A version is immutable: the same version with different bytes anywhere in
/// the index is a verify failure, not a silent upgrade.
fn verify_versions(index: &RepositoryIndex) -> anyhow::Result<()> {
    let mut seen: std::collections::BTreeMap<(&str, &str), &str> = Default::default();
    for setup in &index.setups {
        for item in &setup.items {
            match seen.insert((&*setup.name, &*setup.version), &*item.sha256) {
                Some(previous) if previous != &*item.sha256 => bail!(
                    "setup '{}' version {} is not immutable: conflicting artifact hashes",
                    setup.name,
                    setup.version
                ),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Ed25519 verification against the operator's pinned key.
pub fn verify_signature(bytes: &[u8], key: &str, signature: &str) -> anyhow::Result<()> {
    verify_key_shape(key)?;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let key_hex = key.strip_prefix("ed25519:").unwrap();
    let key_bytes = hex::decode(key_hex)?;
    let verifying = VerifyingKey::from_bytes(
        key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("ed25519 keys are 32 bytes"))?,
    )?;
    let signature_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        signature,
    )
    .context("index signature is not valid base64")?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| anyhow::anyhow!("index signature is not a valid ed25519 signature"))?;
    verifying
        .verify(bytes, &signature)
        .map_err(|_| anyhow::anyhow!("index signature does not match the pinned key"))?;
    Ok(())
}

/// Fetch one artifact and verify its content address.
pub fn fetch_artifact(repo: &Repository, item: &Item) -> anyhow::Result<Bundle> {
    ensure!(
        item.sha256.len() == 64 && item.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "artifact addresses are lowercase sha256 hex"
    );
    let bytes = if let Some(path) = repo.url.strip_prefix("file://") {
        std::fs::read(Path::new(path).join("artifacts").join(&item.sha256))?
    } else {
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("xis/", env!("CARGO_PKG_VERSION")))
            .build()?;
        client
            .get(format!("{}/artifacts/{}", repo.url, item.sha256))
            .send()
            .context("cannot reach the repository artifact")?
            .error_for_status()
            .context("repository artifact request failed")?
            .bytes()?
            .to_vec()
    };
    ensure!(
        sha256_hex(&bytes) == item.sha256,
        "artifact {} does not match its content address",
        item.sha256
    );
    serde_json::from_slice(&bytes).context("artifact is not a valid xis bundle")
}

/// Resolve `repo/name@version` (or `repo/name` = newest) across pinned indexes.
pub fn resolve<'a>(
    indexes: &'a [VerifiedIndex],
    reference: &str,
) -> anyhow::Result<(&'a RepositoryIndex, &'a Setup)> {
    let (repo_name, rest) = reference
        .split_once('/')
        .context("references look like repo/name or repo/name@version")?;
    let (name, version) = match rest.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (rest, None),
    };
    let mut candidates: Vec<(&RepositoryIndex, &Setup)> = Vec::new();
    for index in indexes {
        if index.repository != repo_name {
            continue;
        }
        for setup in &index.index.setups {
            if setup.name == name && version.is_none_or(|version| setup.version == version) {
                candidates.push((&index.index, setup));
            }
        }
    }
    ensure!(!candidates.is_empty(), "no setup matches '{reference}'");
    // Newest version wins only when it is unique; a version conflict is a
    // failure rather than last-write-wins.
    candidates.sort_by(|a, b| a.1.version.cmp(&b.1.version));
    if let Some(version) = version {
        candidates.retain(|(_, setup)| setup.version == version);
    }
    let newest = &candidates.last().expect("checked above").1.version;
    let chosen: Vec<_> = candidates
        .into_iter()
        .filter(|(_, setup)| &setup.version == newest)
        .collect();
    ensure!(
        chosen.len() == 1,
        "'{reference}' resolves to more than one setup; pin an exact version"
    );
    Ok(chosen[0])
}
