//! Explicit, opt-in chat adapter. This does not change automatic routing.
//! The host supplies a logical provider generation, deadline and cancellation
//! scope; use a new adapter after implementation upgrades. No retries/fallback.
//! Loader-backed adapters pin at setup and poll every 100 ms (each bounded probe
//! may take up to 2 s); observed failure latches their epoch. Bridge cancellation
//! owns kill/wait cleanup (up to its 2 s cleanup budget), never a dropped future.
//! This is not instantaneous revocation or atomic launch attestation: changes
//! restored between observations may be missed, hashed bytes may race exec, and
//! already-delivered deltas, credentials and external effects cannot be retracted.
//! Source-less caller-built direct adapters explicitly retain legacy per-call
//! trust/byte checks without disk lifecycle monitoring; opt-in routes reject them.
use super::{
    activation::{ActivationLimits, ActivationPin, ActivationProbe},
    provider_binding::PreparedChatWorker,
    PluginRegistry,
};
use crate::db::secrets::Secrets;
use async_trait::async_trait;
use praxis_provider_api::{
    bridge::{self, InvocationError},
    credentials::CredentialGrant,
    exchange::ExchangeError,
    CallBinding, ChatAttempt, ChatProvider, ChatRequest, ChatResponse, ErrorKind,
    ProviderContinuation, ProviderError, RequestContext, RequestEnvelope, StreamDelta,
    WIRE_VERSION,
};
use std::{
    path::PathBuf,
    sync::{Arc, RwLock, TryLockError},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// Host-owned inputs, never derived from model output. Invocation locks are
/// held only to copy metadata/declared credentials, never for filesystem I/O.
pub struct RegistryChatConfig {
    pub registry: Arc<RwLock<PluginRegistry>>,
    pub secrets: Arc<RwLock<Secrets>>,
    pub data_dir: PathBuf,
    pub provider_id: String,
    pub generation: u64,
    pub deadline: Duration,
    pub cancel: CancellationToken,
}

pub struct RegistryChatProvider {
    config: RegistryChatConfig,
    owner: String,
    revision: String,
    preparation_slot: Arc<tokio::sync::Semaphore>,
    activation: Option<Arc<RegistryActivation>>,
}

/// Selected disk epoch shared across login rebuilds. Never repin on failure.
pub(crate) struct RegistryActivation {
    pin: ActivationPin,
    owner: String,
    role: super::TrustRole,
    secrets: Vec<String>,
    declaration: super::contributions::ProcessContribution,
    monitoring: std::sync::atomic::AtomicBool,
    invalidated: CancellationToken,
    stop: CancellationToken,
}
impl Drop for RegistryActivation {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl RegistryActivation {
    pub(crate) fn new(
        registry: &PluginRegistry,
        id: &str,
        data_dir: PathBuf,
    ) -> Result<Arc<Self>, ProviderError> {
        let mut selected = registry.plugins.values().flat_map(|owner| {
            owner
                .provides
                .providers
                .iter()
                .filter(move |p| p.id == id)
                .map(move |p| (owner, p))
        });
        let (owner, declaration) = selected.next().ok_or_else(unavailable)?;
        if selected.next().is_some() || !owner.enabled {
            return Err(unavailable());
        }
        let expected = (
            owner.name.clone(),
            owner.role,
            owner.secrets.clone(),
            declaration.clone(),
        );
        let probe = ActivationProbe::new(
            owner.name.clone(),
            owner.role,
            owner.secrets.clone(),
            declaration.clone(),
            data_dir,
            ActivationLimits::default(),
        )
        .map_err(|_| unavailable())?;
        // Synchronous setup stays API-compatible; never nest host block_on.
        let pin = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| unavailable())?;
            let result = runtime
                .block_on(probe.pin(&CancellationToken::new()))
                .map_err(|_| unavailable());
            // Timed-out jobs retain the probe gate until they really exit.
            runtime.shutdown_background();
            result
        })
        .join()
        .map_err(|_| unavailable())??;
        let epoch = Arc::new(Self {
            pin,
            owner: expected.0,
            role: expected.1,
            secrets: expected.2,
            declaration: expected.3,
            monitoring: std::sync::atomic::AtomicBool::new(false),
            invalidated: CancellationToken::new(),
            stop: CancellationToken::new(),
        });
        epoch.start_monitor();
        Ok(epoch)
    }
    pub(crate) fn available(&self) -> bool {
        !self.invalidated.is_cancelled()
    }
    pub(crate) fn validate_selection(
        &self,
        registry: &PluginRegistry,
    ) -> Result<(), ProviderError> {
        let mut matches = registry.plugins.values().flat_map(|owner| {
            owner
                .provides
                .providers
                .iter()
                .filter(|p| p.id == self.declaration.id)
                .map(move |p| (owner, p))
        });
        let valid = matches.next().is_some_and(|(owner, declaration)| {
            owner.name == self.owner
                && owner.enabled
                && owner.role == self.role
                && owner.secrets == self.secrets
                && declaration == &self.declaration
        }) && matches.next().is_none();
        if !valid {
            self.invalidated.cancel();
        }
        if !self.available() {
            return Err(unavailable());
        }
        Ok(())
    }
    fn start_monitor(&self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            if !self
                .monitoring
                .swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                // Own only pin/tokens, not the epoch: last owner drop stops polling.
                runtime.spawn(
                    self.pin
                        .clone()
                        .monitor(self.invalidated.clone(), self.stop.clone()),
                );
            }
        }
    }
    pub(crate) async fn check(&self) -> Result<(), ProviderError> {
        self.start_monitor();
        if !self.available() {
            return Err(unavailable());
        }
        if self.pin.check(&self.stop).await.is_err() {
            self.invalidated.cancel();
            return Err(unavailable());
        }
        if !self.available() {
            return Err(unavailable());
        }
        Ok(())
    }
}
struct CancelWatch(tokio::task::JoinHandle<()>);
impl Drop for CancelWatch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct PreparedRegistryChat {
    worker: PreparedChatWorker,
    grant: Option<CredentialGrant>,
    continuation: bool,
    revision: String,
}

#[cfg(test)]
type PreparationHook = (PathBuf, Arc<dyn Fn() + Send + Sync>);
#[cfg(test)]
pub(super) static PREPARATION_HOOK: std::sync::Mutex<Option<PreparationHook>> =
    std::sync::Mutex::new(None);

fn unavailable() -> ProviderError {
    ProviderError::with_cause(ErrorKind::Configuration, "registry provider unavailable")
}

fn prepare(
    config: &RegistryChatConfig,
    nonce: [u8; 16],
) -> Result<PreparedRegistryChat, ProviderError> {
    let registry = {
        let guard = config.registry.read().map_err(|_| unavailable())?;
        registry_snapshot(&guard, &config.provider_id)
    };
    let secrets = {
        let guard = config.secrets.read().map_err(|_| unavailable())?;
        secrets_snapshot(&guard, &registry, &config.provider_id)
    };
    prepare_snapshot(config, &registry, &secrets, nonce)
}

// Preserve all matching owners so ambiguity continues to fail closed, without
// cloning unrelated registry services or exposing unrelated secret values.
fn registry_snapshot(registry: &PluginRegistry, provider_id: &str) -> PluginRegistry {
    let mut snapshot = PluginRegistry::new();
    snapshot.plugins = registry
        .plugins
        .iter()
        .filter(|(_, plugin)| {
            plugin
                .provides
                .providers
                .iter()
                .any(|p| p.id == provider_id)
        })
        .map(|(name, plugin)| {
            let mut provides = super::ProvidesDeclaration::default();
            provides.providers = plugin
                .provides
                .providers
                .iter()
                .filter(|provider| provider.id == provider_id)
                .cloned()
                .collect();
            (
                name.clone(),
                super::Plugin {
                    name: plugin.name.clone(),
                    enabled: plugin.enabled,
                    role: plugin.role,
                    secrets: plugin.secrets.clone(),
                    provides,
                    description: String::new(),
                    version: String::new(),
                    tools: Vec::new(),
                    context: Default::default(),
                    replaces: Vec::new(),
                    hooks: Default::default(),
                    requires: Default::default(),
                    frontend: None,
                    engine: None,
                    build: None,
                },
            )
        })
        .collect();
    snapshot
}

fn secrets_snapshot(secrets: &Secrets, registry: &PluginRegistry, provider_id: &str) -> Secrets {
    let mut snapshot = Secrets::default();
    for plugin in registry.plugins.values() {
        for declaration in plugin
            .provides
            .providers
            .iter()
            .filter(|p| p.id == provider_id)
        {
            for name in &declaration.secrets {
                if let Some(value) = secrets.plugin_secret(name) {
                    snapshot.custom.insert(name.clone(), value.to_owned());
                }
            }
        }
    }
    snapshot
}

fn prepare_snapshot(
    config: &RegistryChatConfig,
    registry: &PluginRegistry,
    secrets: &Secrets,
    nonce: [u8; 16],
) -> Result<PreparedRegistryChat, ProviderError> {
    let (prepared, grant) = registry
        .prepare_chat_worker_with_secrets(
            &config.data_dir,
            &config.provider_id,
            config.generation,
            nonce,
            secrets,
        )
        .map_err(|_| unavailable())?;
    let declaration = registry
        .plugins
        .get(&prepared.owner)
        .and_then(|plugin| {
            plugin
                .provides
                .providers
                .iter()
                .find(|provider| provider.id == config.provider_id)
        })
        .ok_or_else(unavailable)?;
    let continuation = declaration
        .capabilities
        .iter()
        .any(|value| value == "continuation");
    // Pin both declared launch settings and bounded implementation bytes. This
    // detects upgrades between invocations, not a race-free exec attestation.
    #[cfg(test)]
    {
        let hook = PREPARATION_HOOK
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(path, _)| path == &config.data_dir)
            .map(|(_, hook)| hook.clone());
        if let Some(hook) = hook {
            hook();
        }
    }
    let bytes_digest = declaration
        .implementation_digest()
        .map_err(|_| unavailable())?;
    let declaration_bytes = serde_json::to_vec(declaration).map_err(|_| unavailable())?;
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(bytes_digest.as_bytes());
    digest.update(declaration_bytes);
    let revision = format!("{:x}", digest.finalize());
    // Hashing can be slow: check operator trust again after filesystem work.
    registry
        .prepare_chat_worker_with_secrets(
            &config.data_dir,
            &config.provider_id,
            config.generation,
            nonce,
            secrets,
        )
        .map_err(|_| unavailable())?;
    Ok(PreparedRegistryChat {
        worker: prepared,
        grant,
        continuation,
        revision,
    })
}

fn invocation_error(error: InvocationError) -> ProviderError {
    match error {
        InvocationError::Exchange(ExchangeError::Provider(error)) => error
            .to_provider()
            .unwrap_or_else(|_| ProviderError::new(ErrorKind::InvalidResponse)),
        InvocationError::Cancelled | InvocationError::Exchange(ExchangeError::Cancelled) => {
            ProviderError::new(ErrorKind::Cancelled)
        }
        InvocationError::Timeout | InvocationError::Exchange(ExchangeError::Timeout) => {
            ProviderError::new(ErrorKind::Timeout)
        }
        InvocationError::Validation | InvocationError::Exchange(ExchangeError::InvalidDeadline) => {
            ProviderError::new(ErrorKind::InvalidRequest)
        }
        InvocationError::Exchange(ExchangeError::Validation | ExchangeError::MissingTerminal) => {
            ProviderError::new(ErrorKind::InvalidResponse)
        }
        _ => ProviderError::with_cause(ErrorKind::Transport, "registry worker exchange failed"),
    }
}

async fn bounded<T>(
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
    future: impl std::future::Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProviderError::new(ErrorKind::Cancelled)),
        _ = tokio::time::sleep_until(deadline) => Err(ProviderError::new(ErrorKind::Timeout)),
        result = future => result,
    }
}

async fn read_snapshot<T, S>(
    lock: &RwLock<T>,
    snapshot: impl Fn(&T) -> S,
) -> Result<S, ProviderError> {
    loop {
        match lock.try_read() {
            Ok(guard) => return Ok(snapshot(&guard)),
            Err(TryLockError::Poisoned(_)) => return Err(unavailable()),
            Err(TryLockError::WouldBlock) => {}
        }
        // The caller bounds this retry loop by one absolute deadline/cancel.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

impl RegistryChatProvider {
    /// Synchronous metadata-only setup: authorize and pin implementation bytes;
    /// never launch a worker. Call off-executor if setup filesystem I/O may block.
    /// All per-invocation preparation is bounded and offloads filesystem I/O.
    pub fn new(config: RegistryChatConfig) -> Result<Self, ProviderError> {
        let mut adapter =
            Self::new_with_preparation(config, Arc::new(tokio::sync::Semaphore::new(1)))?;
        // Explicit source-less direct adapters retain legacy trust/byte checks.
        let registry = adapter.config.registry.read().map_err(|_| unavailable())?;
        let has_source = registry
            .plugins
            .get(&adapter.owner)
            .and_then(|p| {
                p.provides
                    .providers
                    .iter()
                    .find(|p| p.id == adapter.config.provider_id)
            })
            .is_some_and(|p| p.source.is_some());
        if has_source {
            adapter.activation = Some(RegistryActivation::new(
                &registry,
                &adapter.config.provider_id,
                adapter.config.data_dir.clone(),
            )?);
        }
        drop(registry);
        Ok(adapter)
    }

    /// Route-owned admission survives adapter recreation and login rebuilds.
    /// The constructor's caller must already hold admission while preparing.
    pub(crate) fn new_with_preparation(
        config: RegistryChatConfig,
        preparation_slot: Arc<tokio::sync::Semaphore>,
    ) -> Result<Self, ProviderError> {
        if config.deadline.is_zero() || config.deadline > Duration::from_secs(300) {
            return Err(ProviderError::new(ErrorKind::InvalidRequest));
        }
        let prepared = prepare(&config, *uuid::Uuid::new_v4().as_bytes())?;
        Ok(Self {
            config,
            owner: prepared.worker.owner,
            revision: prepared.revision,
            preparation_slot,
            activation: None,
        })
    }
    pub(crate) fn with_activation(mut self, activation: Arc<RegistryActivation>) -> Self {
        self.activation = Some(activation);
        self
    }
}

#[async_trait]
impl ChatProvider for RegistryChatProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        Ok(self.chat_attempt(request, None, None).await?.response)
    }

    async fn chat_stream_events(
        &self,
        request: ChatRequest,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        Ok(self
            .chat_attempt(request, None, Some(on_delta))
            .await?
            .response)
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
        on_token: &(dyn Fn(String) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        self.chat_stream_events(request, &|delta| {
            if let StreamDelta::Text { text } = delta {
                on_token(text);
            }
        })
        .await
    }

    async fn chat_attempt(
        &self,
        request: ChatRequest,
        continuation: Option<&ProviderContinuation>,
        on_delta: Option<&(dyn Fn(StreamDelta) + Send + Sync)>,
    ) -> Result<ChatAttempt, ProviderError> {
        if self.config.cancel.is_cancelled() {
            return Err(ProviderError::new(ErrorKind::Cancelled));
        }
        // These fields are host routing directives, not worker wire fields.
        // Reject unresolved directives rather than silently dropping them.
        if request.vision_provider.is_some() || request.vision_model.is_some() {
            return Err(ProviderError::new(ErrorKind::Unsupported));
        }
        let started = tokio::time::Instant::now();
        if let Some(epoch) = &self.activation {
            bounded(&self.config.cancel, started + self.config.deadline, async {
                read_snapshot(&self.config.registry, |registry| {
                    epoch.validate_selection(registry)
                })
                .await??;
                epoch.check().await
            })
            .await?;
        }
        let (binding, state) = match continuation {
            None => (
                CallBinding {
                    provider: self.config.provider_id.clone(),
                    worker_generation: self.config.generation,
                    call_id: *uuid::Uuid::new_v4().as_bytes(),
                },
                None,
            ),
            Some(ProviderContinuation::Worker { state })
                if state.binding().provider == self.config.provider_id
                    && state.binding().worker_generation == self.config.generation =>
            {
                (state.binding().clone(), Some(state.clone()))
            }
            Some(_) => return Err(ProviderError::new(ErrorKind::InvalidRequest)),
        };
        let context = RequestContext {
            binding,
            request_id: *uuid::Uuid::new_v4().as_bytes(),
            nonce: *uuid::Uuid::new_v4().as_bytes(),
        };
        let deadline = started + self.config.deadline;
        let selection = bounded(&self.config.cancel, deadline, async {
            let permit = self
                .preparation_slot
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| unavailable())?;
            let registry = read_snapshot(&self.config.registry, |registry| {
                registry_snapshot(registry, &self.config.provider_id)
            })
            .await?;
            let provider_deadline = registry
                .plugins
                .values()
                .flat_map(|plugin| &plugin.provides.providers)
                .filter(|provider| provider.id == self.config.provider_id)
                .map(|provider| started + Duration::from_secs(provider.timeout_secs))
                .min()
                .unwrap_or(deadline)
                .min(deadline);
            // The registry guard has already been dropped, even if this waits.
            let secrets = bounded(
                &self.config.cancel,
                provider_deadline,
                read_snapshot(&self.config.secrets, |secrets| {
                    secrets_snapshot(secrets, &registry, &self.config.provider_id)
                }),
            )
            .await?;
            let expected = registry.clone();
            let config = RegistryChatConfig {
                registry: self.config.registry.clone(),
                secrets: self.config.secrets.clone(),
                data_dir: self.config.data_dir.clone(),
                provider_id: self.config.provider_id.clone(),
                generation: self.config.generation,
                deadline: self.config.deadline,
                cancel: self.config.cancel.clone(),
            };
            // Abort prevents a queued job from starting; an already-running
            // filesystem syscall cannot be cancelled. Its owned permit stays
            // live, retaining shared route admission even after cancellation.
            let task = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                if config.cancel.is_cancelled() {
                    return Err(ProviderError::new(ErrorKind::Cancelled));
                }
                if tokio::time::Instant::now() >= provider_deadline {
                    return Err(ProviderError::new(ErrorKind::Timeout));
                }
                prepare_snapshot(&config, &registry, &secrets, context.nonce)
            });
            struct AbortOnDrop(tokio::task::AbortHandle);
            impl Drop for AbortOnDrop {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _abort = AbortOnDrop(task.abort_handle());
            let selection = bounded(&self.config.cancel, provider_deadline, async {
                task.await.map_err(|_| unavailable())?
            })
            .await?;
            // Reject changes during a secrets wait/hash before authorizing
            // launch. This is still not an atomic exec/revocation grant.
            let current = bounded(
                &self.config.cancel,
                provider_deadline,
                read_snapshot(&self.config.registry, |registry| {
                    let current = registry_snapshot(registry, &self.config.provider_id);
                    current.plugins.len() == expected.plugins.len()
                        && expected.plugins.iter().all(|(name, old)| {
                            current.plugins.get(name).is_some_and(|new| {
                                new.name == old.name
                                    && new.enabled == old.enabled
                                    && new.role == old.role
                                    && new.secrets == old.secrets
                                    && new.provides.providers == old.provides.providers
                            })
                        })
                }),
            )
            .await?;
            if !current {
                if let Some(epoch) = &self.activation {
                    epoch.invalidated.cancel();
                }
                return Err(unavailable());
            }
            Ok(selection)
        })
        .await?;
        if selection.worker.owner != self.owner || selection.revision != self.revision {
            return Err(unavailable());
        }
        let prepared = selection.worker;
        let grant = selection.grant;
        let allows_continuation = selection.continuation;
        if state.is_some() && !allows_continuation {
            return Err(ProviderError::new(ErrorKind::Unsupported));
        }
        let envelope = RequestEnvelope {
            version: WIRE_VERSION,
            context,
            continuation: state,
            request: request.into(),
            conversation: praxis_provider_api::conversation::current(),
        };
        let launch_deadline = started + self.config.deadline.min(prepared.deadline);
        if self.config.cancel.is_cancelled() {
            return Err(ProviderError::new(ErrorKind::Cancelled));
        }
        // Never select away from the bridge: let cancellation kill and reap.
        if let Some(epoch) = &self.activation {
            bounded(&self.config.cancel, launch_deadline, epoch.check()).await?;
        }
        let budget = launch_deadline.saturating_duration_since(tokio::time::Instant::now());
        if self.config.cancel.is_cancelled() {
            return Err(ProviderError::new(ErrorKind::Cancelled));
        }
        if budget.is_zero() {
            return Err(ProviderError::new(ErrorKind::Timeout));
        }
        let call_cancel = self.config.cancel.child_token();
        let _watch = self.activation.as_ref().map(|epoch| {
            let invalidated = epoch.invalidated.clone();
            let token = call_cancel.clone();
            CancelWatch(tokio::spawn(async move {
                invalidated.cancelled().await;
                token.cancel();
            }))
        });
        let callback = |delta: StreamDelta| {
            if !call_cancel.is_cancelled()
                && self
                    .activation
                    .as_ref()
                    .is_none_or(|epoch| epoch.available())
            {
                if let Some(callback) = on_delta {
                    callback(delta);
                }
            }
        };
        let attempt = match grant {
            Some(grant) => {
                bridge::invoke_with_credentials(
                    prepared.launch,
                    &envelope,
                    &grant,
                    budget,
                    &call_cancel,
                    &callback,
                )
                .await
            }
            None => {
                bridge::invoke(prepared.launch, &envelope, budget, &call_cancel, &callback).await
            }
        }
        .map_err(invocation_error)?;
        if let Some(epoch) = &self.activation {
            bounded(&self.config.cancel, launch_deadline, async {
                read_snapshot(&self.config.registry, |registry| {
                    epoch.validate_selection(registry)
                })
                .await??;
                epoch.check().await
            })
            .await?;
        }
        if self.config.cancel.is_cancelled() {
            return Err(ProviderError::new(ErrorKind::Cancelled));
        }
        if attempt.continuation.is_some() && !allows_continuation {
            return Err(ProviderError::new(ErrorKind::InvalidResponse));
        }
        Ok(ChatAttempt {
            response: attempt.response,
            continuation: attempt
                .continuation
                .map(|state| ProviderContinuation::Worker { state }),
        })
    }

    fn supports_output_limit(&self) -> bool {
        self.config
            .registry
            .try_read()
            .ok()
            .and_then(|registry| {
                registry
                    .plugins
                    .get(&self.owner)
                    .filter(|plugin| plugin.enabled)
                    .and_then(|plugin| {
                        plugin
                            .provides
                            .providers
                            .iter()
                            .find(|provider| provider.id == self.config.provider_id)
                    })
                    .map(|provider| {
                        provider
                            .capabilities
                            .iter()
                            .any(|value| value == "output_limit")
                    })
            })
            .unwrap_or(false)
    }
    fn name(&self) -> &str {
        &self.config.provider_id
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    // No protocol health operation yet: do not claim readiness from registration.
    async fn health_check(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn poisoned_snapshot_lock_fails_closed_with_safe_configuration_error() {
        let lock = Arc::new(RwLock::new("PRIVATE credential".to_owned()));
        let poisoned = lock.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.write().unwrap();
            panic!("poison authorization lock");
        })
        .join();
        let error = read_snapshot(&lock, |value| value.clone())
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Configuration);
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }

    #[test]
    fn safe_provider_metadata_survives_adapter_without_raw_causes() {
        let mut error = ProviderError::with_cause(ErrorKind::RateLimited, "PRIVATE cause");
        error.status = Some(429);
        error.retry_after = Some(Duration::from_secs(2));
        error.request_id = Some("request-1".into());
        let wire = praxis_provider_api::WireProviderError::from_provider(&error).unwrap();
        let restored = invocation_error(InvocationError::Exchange(ExchangeError::Provider(wire)));
        assert_eq!(restored.kind, ErrorKind::RateLimited);
        assert_eq!(restored.status, Some(429));
        assert_eq!(restored.retry_after, Some(Duration::from_secs(2)));
        // The existing safe wire schema deliberately omits raw request IDs.
        assert_eq!(restored.request_id, None);
        assert!(!format!("{restored:?}").contains("PRIVATE"));
    }

    #[test]
    fn invocation_failures_remain_typed_and_fail_closed() {
        for (error, expected) in [
            (InvocationError::Cancelled, ErrorKind::Cancelled),
            (InvocationError::Timeout, ErrorKind::Timeout),
            (
                InvocationError::Exchange(ExchangeError::Timeout),
                ErrorKind::Timeout,
            ),
            (
                InvocationError::Exchange(ExchangeError::Cancelled),
                ErrorKind::Cancelled,
            ),
            (
                InvocationError::Exchange(ExchangeError::InvalidDeadline),
                ErrorKind::InvalidRequest,
            ),
            (InvocationError::Validation, ErrorKind::InvalidRequest),
            (
                InvocationError::Exchange(ExchangeError::Validation),
                ErrorKind::InvalidResponse,
            ),
            (
                InvocationError::Exchange(ExchangeError::MissingTerminal),
                ErrorKind::InvalidResponse,
            ),
            (
                InvocationError::Exchange(ExchangeError::Cleanup),
                ErrorKind::Transport,
            ),
        ] {
            assert_eq!(invocation_error(error).kind, expected);
        }
    }
}
