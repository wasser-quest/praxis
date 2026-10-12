//! Explicit scoped grants on launcher-owned private pipes, after readiness.
//! This codec is not a secret-store policy or proof of authorization. The host
//! must supply the trusted owner's declaration, never worker-requested names.
//! Values must not be placed in arguments, environments, errors or audit logs.
use crate::{handshake::WorkerIdentity, transport, WireLimits, WIRE_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::{AsyncRead, AsyncWrite};

pub const MAX_GRANT_BYTES: usize = 64 * 1024;
pub const MAX_SECRET_BYTES: usize = 4096;
pub const MAX_SECRETS: usize = 32;

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("invalid or foreign provider credential grant")]
    Validation,
    #[error("provider credential grant missing")]
    Missing,
    #[error("provider credential pipe failed")]
    Transport,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretValue {
    name: String,
    value: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFrame {
    version: u16,
    #[serde(rename = "type")]
    kind: String,
    provider: String,
    generation: u64,
    nonce: [u8; 16],
    secrets: Vec<SecretValue>,
}

/// Deliberately not Serialize, Deserialize or Clone. Debug never reveals values.
/// No memory-zeroization guarantee: workers and HTTP clients also own copies.
pub struct CredentialGrant {
    frame: GrantFrame,
}

impl std::fmt::Debug for CredentialGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialGrant { redacted }")
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
fn declarations(names: &[String]) -> Result<BTreeSet<&str>, CredentialError> {
    let unique: BTreeSet<_> = names.iter().map(String::as_str).collect();
    if names.len() > MAX_SECRETS
        || unique.len() != names.len()
        || names.iter().any(|name| !valid_name(name))
    {
        return Err(CredentialError::Validation);
    }
    Ok(unique)
}

impl CredentialGrant {
    pub fn new(
        identity: WorkerIdentity,
        declared: &[String],
        values: BTreeMap<String, String>,
    ) -> Result<Self, CredentialError> {
        let grant = Self {
            frame: GrantFrame {
                version: WIRE_VERSION,
                kind: "credentials".into(),
                provider: identity.provider.clone(),
                generation: identity.generation,
                nonce: identity.nonce,
                secrets: values
                    .into_iter()
                    .map(|(name, value)| SecretValue { name, value })
                    .collect(),
            },
        };
        grant.validate(&identity, declared)?;
        if serde_json::to_vec(&grant.frame)
            .map_err(|_| CredentialError::Validation)?
            .len()
            > MAX_GRANT_BYTES
        {
            return Err(CredentialError::Validation);
        }
        Ok(grant)
    }
    fn validate(
        &self,
        expected: &WorkerIdentity,
        declared: &[String],
    ) -> Result<(), CredentialError> {
        expected
            .validate()
            .map_err(|_| CredentialError::Validation)?;
        let allowed = declarations(declared)?;
        let frame = &self.frame;
        if frame.version != WIRE_VERSION
            || frame.kind != "credentials"
            || frame.provider != expected.provider
            || frame.generation != expected.generation
            || frame.nonce != expected.nonce
            || frame.secrets.len() > MAX_SECRETS
        {
            return Err(CredentialError::Validation);
        }
        let mut seen = BTreeSet::new();
        for secret in &frame.secrets {
            if !allowed.contains(secret.name.as_str())
                || !seen.insert(secret.name.as_str())
                || secret.value.is_empty()
                || secret.value.len() > MAX_SECRET_BYTES
                || secret.value.contains('\0')
            {
                return Err(CredentialError::Validation);
            }
        }
        Ok(())
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        self.frame
            .secrets
            .iter()
            .find(|secret| secret.name == name)
            .map(|secret| secret.value.as_str())
    }
    /// Send only after readiness on the recipient's private pipe. On any error
    /// or cancellation, dispose of that pipe; a partial frame must not be retried.
    pub async fn send<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        expected: &WorkerIdentity,
        limits: WireLimits,
    ) -> Result<(), CredentialError> {
        let bytes = self.encode_for(expected, limits)?;
        transport::write_frame(writer, &bytes, limits)
            .await
            .map_err(|_| CredentialError::Transport)
    }

    pub(crate) fn encode_for(
        &self,
        expected: &WorkerIdentity,
        limits: WireLimits,
    ) -> Result<Vec<u8>, CredentialError> {
        limits.validate().map_err(|_| CredentialError::Validation)?;
        // Recheck the recipient before serializing any credential bytes.
        let names: Vec<_> = self
            .frame
            .secrets
            .iter()
            .map(|secret| secret.name.clone())
            .collect();
        self.validate(expected, &names)?;
        let bytes = serde_json::to_vec(&self.frame).map_err(|_| CredentialError::Validation)?;
        if bytes.len() > MAX_GRANT_BYTES || bytes.len() > limits.frame_bytes {
            return Err(CredentialError::Validation);
        }
        Ok(bytes)
    }
}

/// Read one grant after readiness. Cancellation/deadlines/pipe disposal are
/// session-owned. No grant is returned before all identity and scope checks.
pub async fn receive<R: AsyncRead + Unpin>(
    reader: &mut R,
    expected: &WorkerIdentity,
    declared: &[String],
    mut limits: WireLimits,
) -> Result<CredentialGrant, CredentialError> {
    limits.validate().map_err(|_| CredentialError::Validation)?;
    expected
        .validate()
        .map_err(|_| CredentialError::Validation)?;
    declarations(declared)?;
    limits.frame_bytes = limits.frame_bytes.min(MAX_GRANT_BYTES);
    let bytes = transport::read_frame(reader, limits)
        .await
        .map_err(|_| CredentialError::Transport)?
        .ok_or(CredentialError::Missing)?;
    let frame = serde_json::from_slice(&bytes).map_err(|_| CredentialError::Validation)?;
    let grant = CredentialGrant { frame };
    grant.validate(expected, declared)?;
    Ok(grant)
}
