//! Native feature invocation seam. These are trusted Rust adapters, not an IPC
//! protocol or a sandbox. Only the host can issue an invocation's authority.
use crate::{db::Database, gateway::task_control, plugins::ServiceAdapter};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub const INVOCATION_API_VERSION: u32 = 1;
const RESULT_LIMIT: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) enum InvocationError {
    Cancelled,
    TimedOut,
    Unavailable,
    Failed,
    TooLarge,
}
impl std::fmt::Display for InvocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "Native service invocation cancelled",
            Self::TimedOut => "Native service invocation timed out",
            Self::Unavailable => "Native service invocation unavailable or scope stale",
            Self::Failed => "Native service invocation failed",
            Self::TooLarge => "Native service result too large",
        })
    }
}
impl std::error::Error for InvocationError {}
pub(crate) fn failure(error: &anyhow::Error) -> &'static str {
    match error.downcast_ref::<InvocationError>() {
        Some(InvocationError::Cancelled) => "cancelled",
        Some(InvocationError::TimedOut) => "timed_out",
        Some(InvocationError::Unavailable) => "service_unavailable",
        _ => "handler_failed",
    }
}

#[async_trait::async_trait]
pub trait NativeService: Send + Sync {
    /// Transitional, host-only access for native UI/CLI adapters. Model calls
    /// still go through invoke; independently hosted packages use IPC instead.
    fn as_any(&self) -> Option<&dyn std::any::Any> { None }
    /// Registration is effect-free. Only startup may initialize a worker.
    async fn initialize(&self) -> anyhow::Result<()> { Ok(()) }
    fn available(&self) -> bool { true }
    /// Synchronous signal used even when the shutdown budget is exhausted.
    /// Implementations must only stop resources owned by this binding.
    fn force_stop(&self) {}
    async fn invoke(
        &self,
        context: InvocationContext,
        operation: &str,
        args: Value,
    ) -> anyhow::Result<Value>;
    /// Adapter-owned cleanup must be safe after cancellation and on a forced
    /// stop. The host bounds this future; external-resource cleanup is the
    /// adapter's responsibility, including when its future is dropped.
    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Serialize)]
pub struct ServiceDescriptor {
    pub owner: String,
    pub id: String,
    pub api_version: u32,
    pub operations: Vec<String>,
    /// New bindings get a new generation even if the manifest is unchanged.
    pub generation: String,
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub(crate) fn validate_adapter(adapter: &ServiceAdapter) -> anyhow::Result<()> {
    anyhow::ensure!(
        adapter.api_version == INVOCATION_API_VERSION,
        "Unsupported native invocation API version"
    );
    anyhow::ensure!(
        identifier(&adapter.service) && identifier(&adapter.operation),
        "Invalid native service or operation ID"
    );
    anyhow::ensure!(
        (1..=300).contains(&adapter.timeout_secs),
        "Native service timeout_secs must be 1..300"
    );
    Ok(())
}

struct State {
    enabled: bool,
    in_flight: usize,
}
struct Binding {
    descriptor: ServiceDescriptor,
    service: Arc<dyn NativeService>,
    state: Mutex<State>,
    drained: Notify,
    forced: CancellationToken,
    shutdown: tokio::sync::Mutex<Option<bool>>,
    initialization: tokio::sync::Mutex<Option<bool>>,
}

#[derive(Clone)]
pub struct ServiceHandle(Arc<Binding>);

impl ServiceHandle {
    pub(crate) fn service_as<T: 'static>(&self) -> Option<&T> {
        if !self.enabled() { return None; }
        self.0.service.as_any()?.downcast_ref::<T>()
    }
    pub(crate) fn new(
        owner: &str,
        id: &str,
        version: u32,
        operations: &[&str],
        service: Arc<dyn NativeService>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            version == INVOCATION_API_VERSION,
            "Unsupported native invocation API version"
        );
        anyhow::ensure!(
            identifier(owner) && identifier(id),
            "Invalid native service owner or ID"
        );
        anyhow::ensure!(
            !operations.is_empty()
                && operations.len() <= 128
                && operations.iter().all(|op| identifier(op)),
            "Invalid native service operations"
        );
        let mut ops: Vec<_> = operations.iter().map(|op| op.to_string()).collect();
        ops.sort();
        ops.dedup();
        anyhow::ensure!(
            ops.len() == operations.len(),
            "Duplicate native service operation"
        );
        Ok(Self(Arc::new(Binding {
            descriptor: ServiceDescriptor {
                owner: owner.into(),
                id: id.into(),
                api_version: version,
                operations: ops,
                generation: uuid::Uuid::new_v4().to_string(),
            },
            service,
            state: Mutex::new(State {
                enabled: true,
                in_flight: 0,
            }),
            drained: Notify::new(),
            forced: CancellationToken::new(),
            shutdown: tokio::sync::Mutex::new(None),
            initialization: tokio::sync::Mutex::new(None),
        })))
    }

    pub fn descriptor(&self) -> &ServiceDescriptor {
        &self.0.descriptor
    }
    pub fn enabled(&self) -> bool {
        self.0.state.lock().is_ok_and(|state| state.enabled) && self.0.service.available()
    }
    pub async fn initialize(&self) -> anyhow::Result<()> {
        let mut initialized = self.0.initialization.lock().await;
        anyhow::ensure!(
            self.0.state.lock().is_ok_and(|state| state.enabled),
            "Service binding is disabled"
        );
        if let Some(success) = *initialized {
            anyhow::ensure!(
                success && self.0.service.available(),
                "Service initialization failed or instance stopped; bind a new instance"
            );
            return Ok(());
        }
        let success = matches!(
            tokio::time::timeout(Duration::from_secs(6), self.0.service.initialize()).await,
            Ok(Ok(()))
        ) && self.0.state.lock().is_ok_and(|state| state.enabled)
            && self.0.service.available();
        *initialized = Some(success);
        if !success {
            self.close();
            self.0.service.force_stop();
            anyhow::bail!("Service initialization failed; check installed executable, settings and protocol version");
        }
        Ok(())
    }
    pub(crate) fn stop_token(&self) -> CancellationToken {
        self.0.forced.clone()
    }
    pub(crate) fn require(&self, adapter: &ServiceAdapter) -> anyhow::Result<()> {
        self.validate_declaration(adapter)?;
        anyhow::ensure!(self.enabled(), "Native service is disabled");
        Ok(())
    }
    pub(crate) fn validate_declaration(&self, adapter: &ServiceAdapter) -> anyhow::Result<()> {
        validate_adapter(adapter)?;
        anyhow::ensure!(
            adapter.api_version == self.descriptor().api_version
                && self.descriptor().operations.contains(&adapter.operation),
            "Native service operation/version unavailable"
        );
        Ok(())
    }

    fn lease(&self, cancellation: CancellationToken) -> anyhow::Result<Lease> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Native service state unavailable"))?;
        if !state.enabled || !self.0.service.available() {
            return Err(InvocationError::Unavailable.into());
        }
        state.in_flight += 1;
        Ok(Lease {
            handle: self.clone(),
            cancellation,
        })
    }

    pub(crate) async fn invoke(
        &self,
        context: InvocationContext,
        operation: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        context.require_live()?;
        let _lease = self.lease(context.cancellation.clone())?;
        let result = tokio::select! {
            biased;
            _ = context.cancellation.cancelled() => return Err(InvocationError::Cancelled.into()),
            _ = self.0.forced.cancelled() => return Err(InvocationError::Cancelled.into()),
            _ = tokio::time::sleep_until(context.deadline) => return Err(InvocationError::TimedOut.into()),
            result = self.0.service.invoke(context.clone(), operation, args) => result.map_err(|_| InvocationError::Failed)?,
        };
        context.require_live()?;
        if serde_json::to_vec(&result)?.len() > RESULT_LIMIT {
            return Err(InvocationError::TooLarge.into());
        }
        Ok(result)
    }

    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.enabled = false;
        }
    }

    /// Reject new calls immediately, drain current calls, then cancel them when
    /// grace expires. False means forced stop or incomplete adapter cleanup;
    /// it is never evidence that external processes have been terminated.
    pub async fn disable(&self, grace: Duration) -> anyhow::Result<bool> {
        self.close();
        let deadline = tokio::time::Instant::now() + grace;
        let Ok(mut stopped) = tokio::time::timeout_at(deadline, self.0.shutdown.lock()).await
        else {
            self.0.forced.cancel();
            self.0.service.force_stop();
            return Ok(false);
        };
        if let Some(result) = *stopped {
            return Ok(result);
        }
        let drain = async {
            loop {
                let notified = self.0.drained.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self
                    .0
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Native service state unavailable"))?
                    .in_flight
                    == 0
                {
                    return Ok::<_, anyhow::Error>(());
                }
                notified.await;
            }
        };
        let clean = matches!(tokio::time::timeout_at(deadline, drain).await, Ok(Ok(())));
        if !clean {
            self.0.forced.cancel();
            self.0.service.force_stop();
        }
        let cleaned = matches!(
            tokio::time::timeout_at(deadline, self.0.service.shutdown()).await,
            Ok(Ok(()))
        );
        let result = clean && cleaned;
        if !cleaned { self.0.service.force_stop(); }
        *stopped = Some(result);
        Ok(result)
    }
}

struct Lease {
    handle: ServiceHandle,
    cancellation: CancellationToken,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Ok(mut state) = self.handle.0.state.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
        }
        self.handle.0.drained.notify_waiters();
    }
}

/// A read-only host identity plus narrowly scoped storage/credential access.
/// No Deserialize, public constructor, raw Database or global secret store.
/// Clones expire together when the invocation finishes, times out or is stopped.
#[derive(Clone)]
pub struct InvocationContext {
    db: Database,
    user: String,
    session: String,
    task_id: String,
    call_id: String,
    owner: String,
    tool: String,
    registry_revision: String,
    workspace: PathBuf,
    active_state: Option<String>,
    deadline: tokio::time::Instant,
    cancellation: CancellationToken,
    service_stop: CancellationToken,
    secrets: HashMap<String, String>,
    storage_write: bool,
}

impl InvocationContext {
    pub(crate) fn issue(
        db: &Database,
        user: &str,
        call: &str,
        owner: &str,
        tool: &str,
        secrets: HashMap<String, String>,
        timeout: Duration,
        storage_write: bool,
        service_stop: CancellationToken,
    ) -> anyhow::Result<Self> {
        let task_id = task_control::task_id(user)
            .ok_or_else(|| anyhow::anyhow!("Native service requires an active task"))?;
        let registry_revision = task_control::registry_revision(user)
            .ok_or_else(|| anyhow::anyhow!("Native service requires a pinned registry"))?;
        let workspace = task_control::workspace(user)
            .ok_or_else(|| anyhow::anyhow!("Native service requires a host-pinned workspace"))?;
        let ctx = db.load_context(user)?;
        anyhow::ensure!(ctx.user_id == user, "Native service identity mismatch");
        let cancellation = task_control::cancellation(user)
            .ok_or_else(|| anyhow::anyhow!("Native service requires an active task"))?
            .child_token();
        let result = Self {
            db: db.clone(),
            user: user.into(),
            session: ctx.session_id,
            task_id,
            call_id: call.into(),
            owner: owner.into(),
            tool: tool.into(),
            registry_revision,
            workspace,
            active_state: ctx.active_state,
            deadline: tokio::time::Instant::now() + timeout,
            cancellation,
            secrets,
            storage_write,
            service_stop,
        };
        result.require_live()?;
        Ok(result)
    }

    pub fn api_version(&self) -> u32 {
        INVOCATION_API_VERSION
    }
    pub fn user(&self) -> &str {
        &self.user
    }
    pub fn session(&self) -> &str {
        &self.session
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn call_id(&self) -> &str {
        &self.call_id
    }
    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn registry_revision(&self) -> &str {
        &self.registry_revision
    }
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
    pub fn active_state(&self) -> Option<&str> {
        self.active_state.as_deref()
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub fn secret(&self, name: &str) -> Option<&str> {
        self.secrets.get(name).map(String::as_str)
    }

    pub(crate) fn require_live(&self) -> anyhow::Result<()> {
        if self.cancellation.is_cancelled() || self.service_stop.is_cancelled() {
            return Err(InvocationError::Cancelled.into());
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(InvocationError::TimedOut.into());
        }
        if task_control::task_id(&self.user).as_deref() != Some(&self.task_id)
            || task_control::registry_revision(&self.user).as_deref()
                != Some(&self.registry_revision)
            || self.db.load_context(&self.user)?.session_id != self.session
            || !crate::db::tools::get_plugin_tool_enabled(&self.db, &self.tool)
        {
            return Err(InvocationError::Unavailable.into());
        }
        Ok(())
    }

    pub fn storage_get(&self, key: &str) -> anyhow::Result<Option<Value>> {
        self.require_live()?;
        crate::db::service_storage::get(&self.db, &self.owner, &self.user, key)
    }
    pub fn storage_put(&self, key: &str, value: &Value) -> anyhow::Result<()> {
        self.require_live()?;
        anyhow::ensure!(
            self.storage_write,
            "Read-only native invocation cannot write storage"
        );
        crate::db::service_storage::put(&self.db, &self.owner, &self.user, key, value)
    }
    pub fn storage_compare_exchange(
        &self,
        key: &str,
        expected: Option<&Value>,
        value: &Value,
    ) -> anyhow::Result<bool> {
        self.require_live()?;
        anyhow::ensure!(
            self.storage_write,
            "Read-only native invocation cannot write storage"
        );
        crate::db::service_storage::compare_exchange(
            &self.db,
            &self.owner,
            &self.user,
            key,
            expected,
            value,
        )
    }
}

/// Prepared by the host dispatcher, never reconstructed from model operands.
pub(crate) struct ServiceInvocation {
    pub handle: ServiceHandle,
    pub context: InvocationContext,
    pub operation: String,
}
impl ServiceInvocation {
    pub(crate) fn ready(&self) -> anyhow::Result<()> {
        self.context.require_live()?;
        if !self.handle.enabled() {
            return Err(InvocationError::Unavailable.into());
        }
        Ok(())
    }
    pub(crate) async fn invoke(&self, args: &Value) -> anyhow::Result<Value> {
        self.handle
            .invoke(self.context.clone(), &self.operation, args.clone())
            .await
    }
}
impl Drop for ServiceInvocation {
    fn drop(&mut self) {
        self.context.cancellation.cancel();
    }
}
