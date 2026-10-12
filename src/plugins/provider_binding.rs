//! Explicit registry-selected chat invocation; existing host routing is unchanged.
use super::{trust, PluginRegistry, TrustRole};
use praxis_provider_api::{
    bridge, handshake::WorkerIdentity, process::WorkerLaunch, RequestEnvelope, StreamDelta,
    WireCapabilities, WireChatAttempt, WireLimits,
};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct PreparedChatWorker {
    pub owner: String,
    pub launch: WorkerLaunch,
    pub deadline: Duration,
}

/// Host-supplied source; workers and model output cannot select this store.
pub struct ChatWorkerSecrets<'a> {
    pub data_dir: &'a Path,
    pub secrets: &'a crate::db::secrets::Secrets,
}

impl PluginRegistry {
    /// Prepare an explicitly selected, enabled provider using current operator
    /// trust. Call for each invocation; never reuse across disable/trust changes.
    /// This is not an executable-content attestation or an atomic launch grant.
    /// This anonymous entry point rejects credential-requiring declarations;
    /// use the explicit secret-store entry point for scoped grants. Non-chat
    /// operations still fail closed rather than silently losing functionality.
    pub fn prepare_chat_worker(
        &self,
        data_dir: &Path,
        provider_id: &str,
        generation: u64,
        nonce: [u8; 16],
    ) -> anyhow::Result<PreparedChatWorker> {
        self.prepare_chat_worker_inner(data_dir, provider_id, generation, nonce, false)
    }

    /// Authorize only the selected owner's declared credential names. Never
    /// serialize the store or implicitly expose native gateway/admin keys.
    /// Missing or placeholder credentials fail before launching any executable.
    pub fn prepare_chat_worker_with_secrets(
        &self,
        data_dir: &Path,
        provider_id: &str,
        generation: u64,
        nonce: [u8; 16],
        secrets: &crate::db::secrets::Secrets,
    ) -> anyhow::Result<(
        PreparedChatWorker,
        Option<praxis_provider_api::credentials::CredentialGrant>,
    )> {
        let prepared =
            self.prepare_chat_worker_inner(data_dir, provider_id, generation, nonce, true)?;
        let declaration = self
            .plugins
            .get(&prepared.owner)
            .and_then(|plugin| {
                plugin
                    .provides
                    .providers
                    .iter()
                    .find(|value| value.id == provider_id)
            })
            .ok_or_else(|| anyhow::anyhow!("Provider contribution unavailable"))?;
        let grant = if declaration.secrets.is_empty() {
            None
        } else {
            let mut values = BTreeMap::new();
            for name in &declaration.secrets {
                let value = secrets
                    .plugin_secret(name)
                    .ok_or_else(|| anyhow::anyhow!("Provider credential unavailable"))?;
                values.insert(name.clone(), value.to_owned());
            }
            Some(praxis_provider_api::credentials::CredentialGrant::new(
                prepared.launch.identity.clone(),
                &declaration.secrets,
                values,
            )?)
        };
        Ok((prepared, grant))
    }

    fn prepare_chat_worker_inner(
        &self,
        data_dir: &Path,
        provider_id: &str,
        generation: u64,
        nonce: [u8; 16],
        allow_credentials: bool,
    ) -> anyhow::Result<PreparedChatWorker> {
        let mut owners = self.plugins.values().filter_map(|plugin| {
            plugin
                .provides
                .providers
                .iter()
                .find(|value| value.id == provider_id)
                .map(|value| (plugin, value))
        });
        let (plugin, declaration) = owners
            .next()
            .ok_or_else(|| anyhow::anyhow!("Provider contribution unavailable"))?;
        anyhow::ensure!(
            owners.next().is_none(),
            "Provider contribution ownership is ambiguous"
        );
        anyhow::ensure!(plugin.enabled, "Provider contribution is disabled");
        let granted = trust::load(data_dir)?
            .get(&plugin.name)
            .copied()
            .unwrap_or(TrustRole::Tool);
        anyhow::ensure!(
            granted >= plugin.role && granted >= TrustRole::Runtime,
            "Provider contribution requires current operator runtime trust"
        );
        declaration.validate_registration(plugin.role, &plugin.secrets, "provider")?;
        anyhow::ensure!(
            allow_credentials || declaration.secrets.is_empty(),
            "Provider credentials require an explicitly authorized secret store"
        );
        anyhow::ensure!(
            declaration.capabilities.iter().any(|value| value == "chat")
                && declaration.capabilities.iter().all(|value| matches!(
                    value.as_str(),
                    "chat" | "stream" | "continuation" | "output_limit"
                )),
            "Provider contribution requires unsupported operations"
        );
        anyhow::ensure!(
            declaration.timeout_secs <= 300,
            "Provider bridge deadline exceeds 300 seconds"
        );
        let launch = WorkerLaunch {
            executable: declaration
                .resolved
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Provider contribution unresolved"))?,
            arguments: declaration.args.clone(),
            environment: BTreeMap::new(),
            identity: WorkerIdentity {
                provider: provider_id.into(),
                generation,
                nonce,
            },
            capabilities: WireCapabilities {
                supports_output_limit: declaration
                    .capabilities
                    .iter()
                    .any(|value| value == "output_limit"),
                supports_list_models: false,
                supports_embeddings: false,
            },
            limits: WireLimits::default(),
        };
        launch.validate()?;
        Ok(PreparedChatWorker {
            owner: plugin.name.clone(),
            launch,
            deadline: Duration::from_secs(declaration.timeout_secs),
        })
    }

    /// Explicit credential-bearing invocation. Rechecks registry/trust before
    /// selecting names from the host's store; no native-key remapping or retry.
    /// Active trust/disable subscriptions are not provided by this method.
    pub async fn invoke_chat_worker_with_secrets(
        &self,
        source: ChatWorkerSecrets<'_>,
        request: &RequestEnvelope,
        remaining: Duration,
        cancel: &CancellationToken,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> anyhow::Result<WireChatAttempt> {
        if remaining.is_zero() {
            return Err(bridge::InvocationError::Validation.into());
        }
        if cancel.is_cancelled() {
            return Err(bridge::InvocationError::Cancelled.into());
        }
        let started = tokio::time::Instant::now();
        let (prepared, grant) = self.prepare_chat_worker_with_secrets(
            source.data_dir,
            &request.context.binding.provider,
            request.context.binding.worker_generation,
            request.context.nonce,
            source.secrets,
        )?;
        let budget = remaining
            .min(prepared.deadline)
            .saturating_sub(started.elapsed());
        if budget.is_zero() {
            return Err(bridge::InvocationError::Timeout.into());
        }
        Ok(match grant {
            Some(grant) => {
                bridge::invoke_with_credentials(
                    prepared.launch,
                    request,
                    &grant,
                    budget,
                    cancel,
                    on_delta,
                )
                .await?
            }
            None => bridge::invoke(prepared.launch, request, budget, cancel, on_delta).await?,
        })
    }

    /// Invoke one explicitly host-selected chat request. Context is host-owned,
    /// not worker/model-supplied authority. Rechecks current trust and registration
    /// for every call and caps the bridge budget to the contribution declaration.
    /// Opaque continuation is never decoded here. No retries or native fallback.
    /// This does not yet subscribe active calls to disable/trust-change events.
    pub async fn invoke_chat_worker(
        &self,
        data_dir: &Path,
        request: &RequestEnvelope,
        remaining: Duration,
        cancel: &CancellationToken,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> anyhow::Result<WireChatAttempt> {
        if remaining.is_zero() {
            return Err(bridge::InvocationError::Validation.into());
        }
        let started = tokio::time::Instant::now();
        let prepared = self.prepare_chat_worker(
            data_dir,
            &request.context.binding.provider,
            request.context.binding.worker_generation,
            request.context.nonce,
        )?;
        // Preparation consumes the same host-owned budget as readiness and I/O.
        let budget = remaining
            .min(prepared.deadline)
            .saturating_sub(started.elapsed());
        if budget.is_zero() {
            return Err(bridge::InvocationError::Timeout.into());
        }
        Ok(bridge::invoke(prepared.launch, request, budget, cancel, on_delta).await?)
    }
}
