//! Host-side worker readiness negotiation on an already owned private pipe.
//! Correlation is not authentication: the launcher must establish pipe ownership.
use crate::{transport, WireCapabilities, WireLimits, WIRE_VERSION};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub provider: String,
    pub generation: u64,
    pub nonce: [u8; 16],
}

impl WorkerIdentity {
    pub(crate) fn validate(&self) -> Result<(), HandshakeError> {
        if self.provider.is_empty()
            || self.provider.len() > 64
            || !self
                .provider
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
            || self.nonce == [0; 16]
        {
            return Err(HandshakeError::InvalidIdentity);
        }
        Ok(())
    }

    pub(crate) fn matches(&self, context: &crate::RequestContext) -> bool {
        self.provider == context.binding.provider
            && self.generation == context.binding.worker_generation
            && self.nonce == context.nonce
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Handshake {
    Hello {
        version: u16,
        identity: WorkerIdentity,
        capabilities: WireCapabilities,
    },
    Ready {
        version: u16,
        identity: WorkerIdentity,
        capabilities: WireCapabilities,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("invalid worker identity")]
    InvalidIdentity,
    #[error("worker handshake timed out")]
    Timeout,
    #[error("worker handshake transport failed")]
    Transport,
    #[error("invalid worker handshake frame")]
    InvalidFrame,
    #[error("worker readiness mismatch")]
    Mismatch,
}

/// Verify exact identity and capabilities; never accept partial readiness.
/// On any error the caller must dispose of this connection, not reuse it.
pub async fn negotiate<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    identity: WorkerIdentity,
    capabilities: WireCapabilities,
    limits: WireLimits,
) -> Result<(), HandshakeError> {
    identity.validate()?;
    let operation = async {
        let hello = Handshake::Hello {
            version: WIRE_VERSION,
            identity: identity.clone(),
            capabilities,
        };
        let bytes = serde_json::to_vec(&hello).map_err(|_| HandshakeError::InvalidFrame)?;
        transport::write_frame(writer, &bytes, limits)
            .await
            .map_err(|_| HandshakeError::Transport)?;
        let bytes = transport::read_frame(reader, limits)
            .await
            .map_err(|_| HandshakeError::Transport)?
            .ok_or(HandshakeError::Transport)?;
        let response: Handshake =
            serde_json::from_slice(&bytes).map_err(|_| HandshakeError::InvalidFrame)?;
        match response {
            Handshake::Ready {
                version,
                identity: actual,
                capabilities: actual_caps,
            } if version == WIRE_VERSION
                && actual.provider == identity.provider
                && actual.generation == identity.generation
                && actual.nonce == identity.nonce
                && actual_caps.supports_output_limit == capabilities.supports_output_limit
                && actual_caps.supports_list_models == capabilities.supports_list_models
                && actual_caps.supports_embeddings == capabilities.supports_embeddings =>
            {
                Ok(())
            }
            _ => Err(HandshakeError::Mismatch),
        }
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, operation)
        .await
        .map_err(|_| HandshakeError::Timeout)?
}

/// Worker-side readiness on a private pipe established by the launcher.
/// Returns the host-assigned correlation identity, not an authority grant.
/// Dispose of the connection on any error; do not dispatch requests beforehand.
pub async fn accept<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    provider: &str,
    capabilities: WireCapabilities,
    limits: WireLimits,
) -> Result<WorkerIdentity, HandshakeError> {
    let operation = async {
        let bytes = transport::read_frame(reader, limits)
            .await
            .map_err(|_| HandshakeError::Transport)?
            .ok_or(HandshakeError::Transport)?;
        let hello: Handshake =
            serde_json::from_slice(&bytes).map_err(|_| HandshakeError::InvalidFrame)?;
        let identity = match hello {
            Handshake::Hello {
                version,
                identity,
                capabilities: declared,
            } if version == WIRE_VERSION
                && identity.provider == provider
                && declared.supports_output_limit == capabilities.supports_output_limit
                && declared.supports_list_models == capabilities.supports_list_models
                && declared.supports_embeddings == capabilities.supports_embeddings =>
            {
                identity
            }
            _ => return Err(HandshakeError::Mismatch),
        };
        identity.validate()?;
        let ready = Handshake::Ready {
            version: WIRE_VERSION,
            identity: identity.clone(),
            capabilities,
        };
        let bytes = serde_json::to_vec(&ready).map_err(|_| HandshakeError::InvalidFrame)?;
        transport::write_frame(writer, &bytes, limits)
            .await
            .map_err(|_| HandshakeError::Transport)?;
        Ok(identity)
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, operation)
        .await
        .map_err(|_| HandshakeError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> WorkerIdentity {
        WorkerIdentity {
            provider: "worker".into(),
            generation: 3,
            nonce: [7; 16],
        }
    }
    fn caps() -> WireCapabilities {
        WireCapabilities {
            supports_output_limit: true,
            supports_list_models: false,
            supports_embeddings: false,
        }
    }
    #[tokio::test]
    async fn exact_ready_accepted_and_stale_generation_rejected() {
        for generation in [3, 4] {
            let mut actual = identity();
            actual.generation = generation;
            let response = serde_json::to_vec(&Handshake::Ready {
                version: WIRE_VERSION,
                identity: actual,
                capabilities: caps(),
            })
            .unwrap();
            let mut frame = Vec::new();
            transport::write_frame(&mut frame, &response, WireLimits::default())
                .await
                .unwrap();
            let mut outgoing = Vec::new();
            let result = negotiate(
                &mut frame.as_slice(),
                &mut outgoing,
                identity(),
                caps(),
                WireLimits::default(),
            )
            .await;
            if generation == 3 {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(HandshakeError::Mismatch)));
            }
            assert!(!outgoing.is_empty());
        }
    }
    #[tokio::test]
    async fn host_and_worker_negotiate_over_private_pipe() {
        let (host, worker) = tokio::io::duplex(4096);
        let (mut host_read, mut host_write) = tokio::io::split(host);
        let (mut worker_read, mut worker_write) = tokio::io::split(worker);
        let (host_result, worker_result) = tokio::join!(
            negotiate(
                &mut host_read,
                &mut host_write,
                identity(),
                caps(),
                WireLimits::default()
            ),
            accept(
                &mut worker_read,
                &mut worker_write,
                "worker",
                caps(),
                WireLimits::default()
            ),
        );
        host_result.unwrap();
        let actual = worker_result.unwrap();
        assert_eq!(actual.provider, "worker");
        assert_eq!(actual.generation, 3);
        assert_eq!(actual.nonce, [7; 16]);
    }

    #[tokio::test]
    async fn worker_refuses_mismatched_hello_without_ready() {
        for kind in 0..4 {
            let mut actual = identity();
            let mut declared = caps();
            let mut version = WIRE_VERSION;
            match kind {
                0 => actual.provider = "foreign".into(),
                1 => declared.supports_embeddings = true,
                2 => version += 1,
                _ => actual.nonce = [0; 16],
            }
            let hello = serde_json::to_vec(&Handshake::Hello {
                version,
                identity: actual,
                capabilities: declared,
            })
            .unwrap();
            let mut frame = Vec::new();
            transport::write_frame(&mut frame, &hello, WireLimits::default())
                .await
                .unwrap();
            let mut outgoing = Vec::new();
            assert!(accept(
                &mut frame.as_slice(),
                &mut outgoing,
                "worker",
                caps(),
                WireLimits::default()
            )
            .await
            .is_err());
            assert!(outgoing.is_empty());
        }
    }

    #[test]
    fn worker_binding_rejects_foreign_provider_generation_and_nonce() {
        let mut context = crate::RequestContext {
            binding: crate::CallBinding {
                call_id: [1; 16],
                provider: "worker".into(),
                worker_generation: 3,
            },
            request_id: [2; 16],
            nonce: [7; 16],
        };
        assert!(identity().matches(&context));
        context.binding.provider = "foreign".into();
        assert!(!identity().matches(&context));
        context.binding.provider = "worker".into();
        context.binding.worker_generation = 4;
        assert!(!identity().matches(&context));
        context.binding.worker_generation = 3;
        context.nonce = [8; 16];
        assert!(!identity().matches(&context));
    }

    #[tokio::test]
    async fn unknown_fields_rejected_without_echoing_data() {
        let mut frame = Vec::new();
        transport::write_frame(
            &mut frame,
            br#"{"type":"ready","secret":"sensitive"}"#,
            WireLimits::default(),
        )
        .await
        .unwrap();
        let error = negotiate(
            &mut frame.as_slice(),
            &mut Vec::new(),
            identity(),
            caps(),
            WireLimits::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, HandshakeError::InvalidFrame));
        assert!(!error.to_string().contains("sensitive"));
    }
}
