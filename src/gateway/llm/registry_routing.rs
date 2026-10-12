//! Operator-selected chat transport in the existing host policy pipeline.
//! Declarations are the gateway startup snapshot; trust and credentials are
//! reauthorized for every exchange. No installation-driven activation.
use super::{
    provider::{ChatRequest, ChatResponse, LLMProvider},
    LLMRouter,
};
use crate::plugins::{
    registry_chat::{RegistryActivation, RegistryChatConfig, RegistryChatProvider},
    PluginRegistry,
};
use async_trait::async_trait;
use praxis_provider_api::{ErrorKind, ProviderError};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) struct RegistryRoute {
    pub(super) registry: Arc<RwLock<PluginRegistry>>,
    data_dir: PathBuf,
    id: String,
    pub(super) preparation: Arc<tokio::sync::Semaphore>,
    activation: Arc<RegistryActivation>,
}
pub(super) struct BoundRegistryCall {
    route: RegistryRoute,
    generation: u64,
    revision: Arc<Mutex<Option<String>>>,
    pub cancel: CancellationToken,
}
fn unavailable() -> ProviderError {
    ProviderError::new(ErrorKind::Configuration)
}

impl RegistryRoute {
    // Pin only the selected owner/declaration/implementation, not unrelated tools.
    fn revision(&self) -> Result<String, ProviderError> {
        let registry = self.registry.read().map_err(|_| unavailable())?;
        self.activation.validate_selection(&registry)?;
        let owners = registry.list();
        let mut selected = owners.iter().filter_map(|owner| {
            owner
                .provides
                .providers
                .iter()
                .find(|p| p.id == self.id)
                .map(|p| (*owner, p))
        });
        let (owner, declaration) = selected.next().ok_or_else(unavailable)?;
        if selected.next().is_some() {
            return Err(unavailable());
        }
        Ok(format!(
            "{}:{}:{}",
            owner.name,
            serde_json::to_string(declaration).map_err(|_| unavailable())?,
            declaration
                .implementation_digest()
                .map_err(|_| unavailable())?
        ))
    }
    fn adapter(
        &self,
        generation: u64,
        remaining: Duration,
        cancel: CancellationToken,
    ) -> Result<RegistryChatProvider, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::new(ErrorKind::Cancelled));
        }
        let names = self
            .registry
            .read()
            .map_err(|_| unavailable())?
            .list()
            .iter()
            .flat_map(|owner| &owner.provides.providers)
            .find(|p| p.id == self.id)
            .map(|p| p.secrets.clone())
            .ok_or_else(unavailable)?;
        RegistryChatProvider::new_with_preparation(
            RegistryChatConfig {
                registry: self.registry.clone(),
                secrets: Arc::new(RwLock::new(crate::db::secrets::registry_chat_snapshot(
                    &names,
                )?)),
                data_dir: self.data_dir.clone(),
                provider_id: self.id.clone(),
                generation,
                deadline: remaining.min(Duration::from_secs(300)),
                cancel,
            },
            self.preparation.clone(),
        )
        .map(|adapter| adapter.with_activation(self.activation.clone()))
    }
    pub fn bind(&self, cancel: &CancellationToken) -> BoundRegistryCall {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        BoundRegistryCall {
            route: self.clone(),
            generation: NEXT.fetch_add(1, Ordering::Relaxed),
            revision: Arc::new(Mutex::new(None)),
            cancel: cancel.child_token(),
        }
    }
    /// Nonblocking inventory/credential readiness only. Filesystem trust must
    /// be checked with validate_async (and is rechecked before every launch).
    pub fn validate(&self) -> Result<(), ProviderError> {
        if !self.activation.available() {
            return Err(unavailable());
        }
        let registry = self.registry.try_read().map_err(|_| unavailable())?;
        self.activation.validate_selection(&registry)?;
        let owners = registry.list();
        let mut matches = owners.iter().flat_map(|owner| {
            owner
                .provides
                .providers
                .iter()
                .filter(|p| p.id == self.id)
                .map(move |p| (*owner, p))
        });
        let (owner, declaration) = matches.next().ok_or_else(unavailable)?;
        if matches.next().is_some() || !owner.enabled {
            return Err(unavailable());
        }
        let secrets = crate::db::secrets::registry_chat_snapshot(&declaration.secrets)?;
        if declaration
            .secrets
            .iter()
            .any(|name| secrets.plugin_secret(name).is_none())
        {
            return Err(unavailable());
        }
        Ok(())
    }

    fn validate_blocking(&self) -> Result<(), ProviderError> {
        let registry = self.registry.read().map_err(|_| unavailable())?;
        self.activation.validate_selection(&registry)?;
        let names = registry
            .list()
            .iter()
            .flat_map(|owner| &owner.provides.providers)
            .find(|p| p.id == self.id)
            .map(|p| p.secrets.clone())
            .ok_or_else(unavailable)?;
        let secrets = crate::db::secrets::registry_chat_snapshot(&names)?;
        registry
            .prepare_chat_worker_with_secrets(
                &self.data_dir,
                &self.id,
                0,
                *uuid::Uuid::new_v4().as_bytes(),
                &secrets,
            )
            .map(|_| ())
            .map_err(|_| unavailable())
    }

    pub async fn validate_async(
        &self,
        deadline: tokio::time::Instant,
        cancel: &CancellationToken,
    ) -> Result<(), ProviderError> {
        let permit = super::resilience::interruptible(
            deadline,
            cancel,
            self.preparation.clone().acquire_owned(),
        )
        .await?
        .map_err(|_| unavailable())?;
        super::resilience::interruptible(deadline, cancel, self.activation.check()).await??;
        let route = self.clone();
        let token = cancel.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if token.is_cancelled() {
                return Err(ProviderError::new(ErrorKind::Cancelled));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ProviderError::new(ErrorKind::Timeout));
            }
            route.validate_blocking()
        });
        let _queued = AbortQueued(task.abort_handle());
        super::resilience::interruptible(deadline, cancel, task)
            .await?
            .map_err(|_| unavailable())??;
        super::resilience::interruptible(deadline, cancel, self.activation.check()).await?
    }
}
struct AbortQueued(tokio::task::AbortHandle);
impl Drop for AbortQueued {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl BoundRegistryCall {
    pub async fn attempt(
        &self,
        deadline: tokio::time::Instant,
        cancel: &CancellationToken,
    ) -> Result<RegistryChatProvider, ProviderError> {
        let permit = super::resilience::interruptible(
            deadline,
            cancel,
            self.route.preparation.clone().acquire_owned(),
        )
        .await?
        .map_err(|_| unavailable())?;
        super::resilience::interruptible(deadline, cancel, self.route.activation.check()).await??;
        let route = self.route.clone();
        let revision = self.revision.clone();
        let generation = self.generation;
        let token = cancel.clone();
        let task = tokio::task::spawn_blocking(move || {
            // Keep the slot until even an uninterruptible filesystem syscall
            // returns. Never let timed-out preparation multiply blocking jobs.
            let _permit = permit;
            if token.is_cancelled() {
                return Err(ProviderError::new(ErrorKind::Cancelled));
            }
            let current = route.revision()?;
            let mut pinned = revision.lock().map_err(|_| unavailable())?;
            if pinned.as_ref().is_some_and(|old| old != &current) {
                return Err(unavailable());
            }
            *pinned = Some(current);
            drop(pinned);
            route.adapter(
                generation,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                token,
            )
        });
        let _queued = AbortQueued(task.abort_handle());
        let adapter = super::resilience::interruptible(deadline, cancel, task)
            .await?
            .map_err(|_| unavailable())??;
        super::resilience::interruptible(deadline, cancel, self.route.activation.check()).await??;
        Ok(adapter)
    }
}
impl Drop for BoundRegistryCall {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[async_trait]
impl LLMProvider for RegistryRoute {
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, ProviderError> {
        Err(unavailable())
    }
    fn name(&self) -> &str {
        &self.id
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn supports_output_limit(&self) -> bool {
        self.registry.try_read().ok().is_some_and(|registry| {
            registry.list().iter().any(|owner| {
                owner
                    .provides
                    .providers
                    .iter()
                    .any(|p| p.id == self.id && p.capabilities.iter().any(|c| c == "output_limit"))
            })
        })
    }
    async fn health_check(&self) -> bool {
        false
    }
}
impl LLMRouter {
    pub(crate) fn enable_registry(
        &mut self,
        registry: Arc<PluginRegistry>,
        data_dir: PathBuf,
        id: String,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !id.is_empty()
                && !crate::gateway::providers::PROVIDERS
                    .iter()
                    .any(|p| p.name == id)
                && id != "free_router"
                && !self.provider_names().contains(&id),
            "registry provider name unavailable"
        );
        let activation = RegistryActivation::new(&registry, &id, data_dir.clone())?;
        let binding = RegistryRoute {
            activation,
            registry: Arc::new(RwLock::new((*registry).clone())),
            data_dir,
            id,
            preparation: Arc::new(tokio::sync::Semaphore::new(
                self.policy.max_concurrent.clamp(1, 64) as usize,
            )),
        };
        // Startup setup remains synchronous; request preflight is off-executor.
        binding.validate_blocking()?;
        self.gates.insert(
            binding.id.clone(),
            Arc::new(super::resilience::ProviderGate::new(&self.policy)),
        );
        self.providers.push(Box::new(binding));
        Ok(())
    }
    pub(crate) fn retain_registry_from(&mut self, previous: &LLMRouter) {
        for provider in &previous.providers {
            if let Some(binding) = provider.as_any().downcast_ref::<RegistryRoute>() {
                if !self.provider_names().contains(&binding.id) {
                    // Preserve quota, concurrency, cooldown and preparation slots
                    // across native login while old calls finish on that same gate.
                    self.gates
                        .insert(binding.id.clone(), previous.gates[&binding.id].clone());
                    self.providers.push(Box::new(binding.clone()));
                }
            }
        }
    }
}
