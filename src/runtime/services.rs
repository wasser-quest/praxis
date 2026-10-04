//! Small native background-service host used during feature extraction.
//! API versions describe this Rust adapter, not the proposed v2 IPC manifest.
use std::{collections::HashMap, future::Future, time::Duration};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const SERVICE_API_VERSION: u32 = 1;

struct Worker {
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

/// Owns workers independently of UI, providers and the tool/plugin registry.
/// Dropping the host aborts all its workers; normal shutdown drains first.
#[derive(Default)]
pub struct ServiceHost {
    workers: HashMap<String, Worker>,
}

impl ServiceHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.workers.contains_key(id)
    }

    /// Register a worker, then run its first tick immediately for compatibility.
    /// No callback is invoked until all declarations are validated. Ticks of one
    /// service never overlap; failures are logged and retried at the next tick.
    pub fn register_periodic<F, Fut>(
        &mut self,
        id: &str,
        api_version: u32,
        period: Duration,
        mut tick: F,
    ) -> anyhow::Result<()>
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        anyhow::ensure!(
            api_version == SERVICE_API_VERSION,
            "Unsupported native service API version {api_version}"
        );
        anyhow::ensure!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
            "Invalid service ID"
        );
        anyhow::ensure!(!period.is_zero(), "Service interval must be positive");
        anyhow::ensure!(
            !self.workers.contains_key(id),
            "Service owner conflict: {id}"
        );
        let runtime = tokio::runtime::Handle::try_current()?;
        let cancellation = CancellationToken::new();
        let stop = cancellation.clone();
        let service_id = id.to_string();
        let task = runtime.spawn(async move {
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = stop.cancelled() => break,
                    _ = interval.tick() => {
                        if stop.is_cancelled() { break; }
                        // Drain an in-flight tick rather than dropping its future
                        // on cancellation. The host imposes the shutdown deadline.
                        if let Err(error) = tick().await {
                            tracing::warn!(service = %service_id, %error, "Service tick failed");
                        }
                    }
                }
            }
        });
        self.workers
            .insert(id.to_string(), Worker { cancellation, task });
        Ok(())
    }

    /// Block new ticks, await the current tick, then abort if the grace expires.
    /// Returns false after a forced stop. This does not stop a feature's external
    /// processes: those belong to the adapter's separate resource lifecycle.
    pub async fn disable(&mut self, id: &str, grace: Duration) -> anyhow::Result<bool> {
        let worker = self
            .workers
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("Unknown service: {id}"))?;
        worker.cancellation.cancel();
        drain(worker, grace).await
    }

    /// Cancel all services together and apply one overall shutdown deadline.
    pub async fn shutdown(&mut self, grace: Duration) {
        let deadline = tokio::time::Instant::now() + grace;
        for worker in self.workers.values() {
            worker.cancellation.cancel();
        }
        for (id, worker) in self.workers.drain() {
            match drain(
                worker,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => tracing::warn!(service = %id, "Service shutdown deadline expired"),
                Err(error) => {
                    tracing::warn!(service = %id, %error, "Service worker exited unexpectedly")
                }
            }
        }
    }
}

async fn drain(mut worker: Worker, grace: Duration) -> anyhow::Result<bool> {
    match tokio::time::timeout(grace, &mut worker.task).await {
        Ok(result) => {
            result?;
            Ok(true)
        }
        Err(_) => {
            worker.task.abort();
            let _ = (&mut worker.task).await;
            Ok(false)
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

impl Drop for ServiceHost {
    fn drop(&mut self) {
        // Cancel every worker before any is dropped/aborted.
        for worker in self.workers.values() {
            worker.cancellation.cancel();
        }
    }
}
