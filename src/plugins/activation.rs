//! Selected-provider disk invalidation, not hot reload or atomic attestation.
//!
//! Reuse one `ActivationProbe` (and its clones) per selected route. A timed-out
//! or cancelled blocking job retains the probe's gate until it really exits.
//! Polling only detects states that are observed: it cannot promise instantaneous
//! revocation, detect changes restored between polls, or prove hashed bytes are
//! atomically the bytes executed. The caller must keep the epoch token latched,
//! gate accepted output on it, and let the bridge own cancellation/reaping cleanup.
//! Replacement identity uses Unix device/inode/mode where available; other
//! platforms retain declaration/path/byte checks but not identical-byte replacement
//! detection. The disk identity baseline starts at successful pinning, not loading.
use super::{contributions::ProcessContribution, PluginManifest, TrustRole};
use anyhow::{ensure, Context};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Hard byte budgets and an end-to-end deadline (including gate acquisition).
#[derive(Clone, Debug)]
pub struct ActivationLimits {
    pub max_manifest_bytes: usize,
    pub max_trust_bytes: usize,
    pub max_implementation_bytes: usize,
    pub probe_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for ActivationLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 1024 * 1024,
            max_trust_bytes: 1024 * 1024,
            max_implementation_bytes: 128 * 1024 * 1024,
            probe_timeout: Duration::from_secs(2),
            poll_interval: Duration::from_millis(100),
        }
    }
}

/// Exact loader provenance and caller-supplied registry selection/trust location.
/// No directory enumeration or reconstruction of package paths from owner IDs.
#[derive(Clone)]
pub struct ActivationProbe {
    owner: String,
    role: TrustRole,
    secrets: Vec<String>,
    declaration: ProcessContribution,
    data_dir: PathBuf,
    limits: ActivationLimits,
    gate: Arc<Semaphore>,
}

#[derive(Clone)]
pub struct ActivationPin {
    probe: ActivationProbe,
    fingerprint: Fingerprint,
    failed: Arc<AtomicBool>,
}

#[derive(Debug)]
struct ProbeStopped;

impl std::fmt::Display for ProbeStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Activation probe stopped")
    }
}

impl std::error::Error for ProbeStopped {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
}

fn identity(path: &Path, directory: bool) -> anyhow::Result<Identity> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "Activation path became a symlink"
    );
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "Activation path has wrong file type"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(Identity {})
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    package: Identity,
    manifest: Identity,
    executable: Identity,
    manifest_digest: [u8; 32],
    implementation_digest: [u8; 32],
    granted: TrustRole,
}

fn read_bounded(path: &Path, max: usize, deadline: Instant) -> anyhow::Result<Vec<u8>> {
    identity(path, false)?;
    let mut file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "Activation input is not a regular file"
    );
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        ensure!(
            Instant::now() < deadline,
            "Activation probe deadline exceeded"
        );
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        ensure!(
            count <= max.saturating_sub(bytes.len()),
            "Activation input exceeds byte limit"
        );
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

impl ActivationProbe {
    pub fn new(
        owner: String,
        role: TrustRole,
        secrets: Vec<String>,
        declaration: ProcessContribution,
        data_dir: PathBuf,
        limits: ActivationLimits,
    ) -> anyhow::Result<Self> {
        ensure!(
            declaration.source.is_some(),
            "Selected provider has no loader provenance"
        );
        ensure!(
            declaration.resolved.is_some(),
            "Selected provider is unresolved"
        );
        ensure!(
            limits.max_manifest_bytes > 0
                && limits.max_trust_bytes > 0
                && limits.max_implementation_bytes > 0
                && !limits.probe_timeout.is_zero()
                && !limits.poll_interval.is_zero(),
            "Activation limits must be positive"
        );
        declaration.validate("provider")?;
        ensure!(
            role >= TrustRole::Runtime
                && declaration.secrets.iter().all(|key| secrets.contains(key)),
            "Selected provider requires package runtime/secret grants"
        );
        Ok(Self {
            owner,
            role,
            secrets,
            declaration,
            data_dir,
            limits,
            gate: Arc::new(Semaphore::new(1)),
        })
    }

    /// Validate disk state against the selected registry declaration before use.
    /// Failure must leave the route unavailable; never silently repin a replacement.
    pub async fn pin(&self, stop: &CancellationToken) -> anyhow::Result<ActivationPin> {
        let fingerprint = self.snapshot(stop).await?;
        Ok(ActivationPin {
            probe: self.clone(),
            fingerprint,
            failed: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn snapshot(&self, stop: &CancellationToken) -> anyhow::Result<Fingerprint> {
        let probe = self.clone();
        self.run_blocking(stop, move |deadline| probe.snapshot_blocking(deadline))
            .await
    }

    async fn run_blocking<T, F>(&self, stop: &CancellationToken, work: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Instant) -> anyhow::Result<T> + Send + 'static,
    {
        let deadline = Instant::now()
            .checked_add(self.limits.probe_timeout)
            .context("Activation deadline overflow")?;
        let timeout = tokio::time::Instant::from_std(deadline);
        let permit = tokio::select! {
            biased;
            _ = stop.cancelled() => return Err(ProbeStopped.into()),
            _ = tokio::time::sleep_until(timeout) => anyhow::bail!("Activation probe deadline exceeded"),
            permit = self.gate.clone().acquire_owned() => permit?,
        };
        let mut job = tokio::task::spawn_blocking(move || {
            // Intentionally owned by the blocking closure, not the async waiter.
            let _permit = permit;
            work(deadline)
        });
        tokio::select! {
            biased;
            _ = stop.cancelled() => return Err(ProbeStopped.into()),
            _ = tokio::time::sleep_until(timeout) => anyhow::bail!("Activation probe deadline exceeded"),
            result = &mut job => result.context("Activation probe task failed")?,
        }
    }

    fn snapshot_blocking(&self, deadline: Instant) -> anyhow::Result<Fingerprint> {
        let source = self
            .declaration
            .source
            .as_ref()
            .context("Missing activation source")?;
        ensure!(
            source.package_dir.canonicalize()? == source.package_dir
                && source.manifest.canonicalize()? == source.manifest
                && source.manifest.starts_with(&source.package_dir),
            "Activation source path changed"
        );
        let package = identity(&source.package_dir, true)?;
        let manifest_identity = identity(&source.manifest, false)?;
        let bytes = read_bounded(&source.manifest, self.limits.max_manifest_bytes, deadline)?;
        let manifest: PluginManifest = serde_json::from_slice(&bytes)?;
        ensure!(
            manifest.name == self.owner && manifest.enabled,
            "Selected provider owner changed or disabled"
        );
        ensure!(
            manifest.role == self.role && manifest.secrets == self.secrets,
            "Selected provider package grants changed"
        );
        manifest.provides.validate(&manifest.name)?;
        let mut selected = manifest
            .provides
            .providers
            .iter()
            .filter(|value| value.id == self.declaration.id);
        let mut declaration = selected
            .next()
            .context("Selected provider declaration missing")?
            .clone();
        ensure!(
            selected.next().is_none(),
            "Duplicate selected provider declaration"
        );
        declaration.resolve(
            &source.package_dir,
            manifest.role,
            &manifest.secrets,
            "provider",
        )?;
        declaration.source = Some(source.clone());
        ensure!(
            declaration == self.declaration,
            "Selected provider declaration or executable path changed"
        );
        let trust_bytes = read_bounded(
            &self.data_dir.join("plugin_trust.json"),
            self.limits.max_trust_bytes,
            deadline,
        )?;
        let trust: BTreeMap<String, TrustRole> = serde_json::from_slice(&trust_bytes)?;
        let granted = trust.get(&self.owner).copied().unwrap_or(TrustRole::Tool);
        ensure!(
            granted >= self.role && granted >= TrustRole::Runtime,
            "Selected provider trust revoked"
        );
        let executable_path = declaration
            .resolved
            .as_ref()
            .context("Unresolved provider")?;
        let executable = identity(executable_path, false)?;
        let implementation = read_bounded(
            executable_path,
            self.limits.max_implementation_bytes,
            deadline,
        )?;
        ensure!(
            package == identity(&source.package_dir, true)?
                && manifest_identity == identity(&source.manifest, false)?
                && executable == identity(executable_path, false)?,
            "Activation replaced during probe"
        );
        let manifest_digest = Sha256::digest(&bytes).into();
        let implementation_digest = Sha256::digest(&implementation).into();
        ensure!(
            Instant::now() < deadline,
            "Activation probe deadline exceeded"
        );
        Ok(Fingerprint {
            package,
            manifest: manifest_identity,
            executable,
            manifest_digest,
            implementation_digest,
            granted,
        })
    }
}

impl ActivationPin {
    /// Check before launch/terminal acceptance as well as in the monitor. An
    /// observed failure latches this pin; stopping a waiter alone does not.
    pub async fn check(&self, stop: &CancellationToken) -> anyhow::Result<()> {
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "Selected provider activation already invalidated"
        );
        let result = self.probe.snapshot(stop).await.and_then(|current| {
            ensure!(
                current == self.fingerprint,
                "Selected provider activation changed"
            );
            Ok(())
        });
        self.latch_result(result)
    }

    fn latch_result(&self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        if let Err(error) = &result {
            // Only an actual stopped wait is normal shutdown. A concurrent stop
            // must not erase a validation/deadline failure already returned.
            if !error.is::<ProbeStopped>() {
                self.failed.store(true, Ordering::Release);
            }
        } else {
            ensure!(
                !self.failed.load(Ordering::Acquire),
                "Selected provider activation invalidated"
            );
        }
        result
    }

    /// Poll until stopped or invalidated. Checks immediately, then at the supplied
    /// interval. Never reloads, restores, or uncancels an epoch. Stop is normal
    /// shutdown, not invalidation. Bridge cleanup remains the caller's responsibility.
    pub async fn monitor(
        self,
        invalidated: CancellationToken,
        stop: CancellationToken,
    ) -> anyhow::Result<()> {
        loop {
            if stop.is_cancelled() || invalidated.is_cancelled() {
                return Ok(());
            }
            if let Err(error) = self.check(&stop).await {
                if error.is::<ProbeStopped>() {
                    return Ok(());
                }
                invalidated.cancel();
                return Err(error);
            }
            tokio::select! {
                biased;
                _ = stop.cancelled() => return Ok(()),
                _ = invalidated.cancelled() => return Ok(()),
                _ = tokio::time::sleep(self.probe.limits.poll_interval) => {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{load_installed_plugin, trust, TrustRole};
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, ActivationProbe) {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("actual-directory-not-owner");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("worker"), "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                package.join("worker"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let manifest = json!({"name":"selected", "description":"fixture", "version":"1", "role":"runtime", "enabled":true, "tools":[], "provides":{"providers":[{"id":"example", "executable":"worker", "protocol":"praxis.provider.v1", "capabilities":["chat"]}]}});
        std::fs::write(package.join("plugin.json"), manifest.to_string()).unwrap();
        trust::set(root.path(), "selected", Some(TrustRole::Runtime)).unwrap();
        let plugin = load_installed_plugin(&package).unwrap();
        let probe = ActivationProbe::new(
            plugin.name,
            plugin.role,
            plugin.secrets,
            plugin.provides.providers[0].clone(),
            root.path().to_path_buf(),
            ActivationLimits::default(),
        )
        .unwrap();
        (root, probe)
    }

    #[tokio::test]
    async fn stable_unrelated_changes_and_stop_do_not_invalidate() {
        let (root, probe) = fixture();
        let stop = CancellationToken::new();
        let pin = probe.pin(&stop).await.unwrap();
        trust::set(root.path(), "unrelated", Some(TrustRole::Channel)).unwrap();
        std::fs::create_dir(root.path().join(".backup-unrelated")).unwrap();
        std::fs::write(root.path().join(".backup-unrelated/plugin.json"), "invalid").unwrap();
        pin.check(&stop).await.unwrap();
        let invalidated = CancellationToken::new();
        stop.cancel();
        pin.monitor(invalidated.clone(), stop).await.unwrap();
        assert!(!invalidated.is_cancelled());
    }

    #[tokio::test]
    async fn selected_disk_mutations_fail_closed_and_restoration_never_unlatches() {
        for mutation in [
            "disable",
            "revoke",
            "missing",
            "partial",
            "declaration",
            "bytes",
            "manifest-replacement",
            "package-replacement",
            "executable-replacement",
            "trust-partial",
            "trust-missing",
            "executable-missing",
            "executable-path",
        ] {
            let (root, probe) = fixture();
            let stop = CancellationToken::new();
            let pin = probe.pin(&stop).await.unwrap();
            let package = root.path().join("actual-directory-not-owner");
            let manifest = package.join("plugin.json");
            let original = std::fs::read(&manifest).unwrap();
            match mutation {
                "disable" | "declaration" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
                    if mutation == "disable" {
                        value["enabled"] = json!(false);
                    } else {
                        value["provides"]["providers"][0]["args"] = json!(["changed"]);
                    }
                    std::fs::write(&manifest, value.to_string()).unwrap();
                }
                "revoke" => trust::set(root.path(), "selected", None).unwrap(),
                "missing" => std::fs::remove_file(&manifest).unwrap(),
                "partial" => std::fs::write(&manifest, "{\"name\":").unwrap(),
                "bytes" => std::fs::write(package.join("worker"), "#!/bin/sh\nexit 1\n").unwrap(),
                "manifest-replacement" => {
                    #[cfg(not(unix))]
                    {
                        continue;
                    }
                    std::fs::write(package.join("new.json"), &original).unwrap();
                    std::fs::rename(package.join("new.json"), &manifest).unwrap();
                }
                "package-replacement" => {
                    #[cfg(not(unix))]
                    {
                        continue;
                    }
                    std::fs::rename(&package, root.path().join(".backup-selected")).unwrap();
                    std::fs::create_dir(&package).unwrap();
                    std::fs::copy(
                        root.path().join(".backup-selected/worker"),
                        package.join("worker"),
                    )
                    .unwrap();
                    std::fs::write(&manifest, &original).unwrap();
                }
                "executable-replacement" => {
                    #[cfg(not(unix))]
                    {
                        continue;
                    }
                    std::fs::copy(package.join("worker"), package.join("new-worker")).unwrap();
                    std::fs::rename(package.join("new-worker"), package.join("worker")).unwrap();
                }
                "trust-partial" => {
                    std::fs::write(root.path().join("plugin_trust.json"), "{").unwrap()
                }
                "trust-missing" => {
                    std::fs::remove_file(root.path().join("plugin_trust.json")).unwrap()
                }
                "executable-missing" => std::fs::remove_file(package.join("worker")).unwrap(),
                "executable-path" => {
                    std::fs::copy(package.join("worker"), package.join("other-worker")).unwrap();
                    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
                    value["provides"]["providers"][0]["executable"] = json!("other-worker");
                    std::fs::write(&manifest, value.to_string()).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(pin.check(&stop).await.is_err(), "{mutation}");
            let invalidated = CancellationToken::new();
            pin.clone()
                .monitor(invalidated.clone(), stop.clone())
                .await
                .unwrap_err();
            assert!(invalidated.is_cancelled(), "{mutation}");
            std::fs::write(&manifest, &original).unwrap();
            trust::set(root.path(), "selected", Some(TrustRole::Runtime)).ok();
            stop.cancel();
            pin.monitor(invalidated.clone(), stop).await.unwrap();
            assert!(invalidated.is_cancelled(), "restoration: {mutation}");
        }
    }

    #[tokio::test]
    async fn monitor_observes_revocation_and_pin_remains_failed_after_restore() {
        let (root, probe) = fixture();
        let stop = CancellationToken::new();
        let pin = probe.pin(&stop).await.unwrap();
        let invalidated = CancellationToken::new();
        let monitor = tokio::spawn(pin.clone().monitor(invalidated.clone(), stop));
        trust::set(root.path(), "selected", None).unwrap();
        tokio::time::timeout(Duration::from_secs(1), invalidated.cancelled())
            .await
            .unwrap();
        assert!(monitor.await.unwrap().is_err());
        trust::set(root.path(), "selected", Some(TrustRole::Runtime)).unwrap();
        assert!(pin.check(&CancellationToken::new()).await.is_err());
    }

    #[tokio::test]
    async fn stop_during_polling_is_normal_shutdown_and_does_not_latch_pin() {
        let (_root, probe) = fixture();
        let stop = CancellationToken::new();
        let pin = probe.pin(&stop).await.unwrap();
        let invalidated = CancellationToken::new();
        let monitor = tokio::spawn(pin.clone().monitor(invalidated.clone(), stop.clone()));
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.cancel();
        monitor.await.unwrap().unwrap();
        assert!(!invalidated.is_cancelled());
        pin.check(&CancellationToken::new()).await.unwrap();
    }

    #[tokio::test]
    async fn only_stopped_results_are_unlatched_not_validation_failures_racing_stop() {
        let (_root, probe) = fixture();
        let stop = CancellationToken::new();
        let pin = probe.pin(&stop).await.unwrap();
        assert!(pin.latch_result(Err(ProbeStopped.into())).is_err());
        pin.check(&stop).await.unwrap();
        // Simulate receiving a validation failure just before stop is cancelled.
        let observed = Err(anyhow::anyhow!("Selected provider trust revoked"));
        stop.cancel();
        assert!(pin.latch_result(observed).is_err());
        assert!(pin.check(&CancellationToken::new()).await.is_err());
    }

    #[tokio::test]
    async fn timed_out_and_cancelled_jobs_retain_the_single_probe_gate() {
        for cancel in [false, true] {
            let (_root, mut probe) = fixture();
            probe.limits.probe_timeout = Duration::from_millis(50);
            let stop = CancellationToken::new();
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let first = probe.clone();
            let first_stop = stop.clone();
            let waiter = tokio::spawn(async move {
                first
                    .run_blocking(&first_stop, move |_| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(())
                    })
                    .await
            });
            entered_rx.await.unwrap();
            if cancel {
                stop.cancel();
            }
            assert!(waiter.await.unwrap().is_err());
            assert_eq!(probe.gate.available_permits(), 0);
            let started = Arc::new(AtomicBool::new(false));
            let flag = started.clone();
            assert!(probe
                .run_blocking(&CancellationToken::new(), move |_| {
                    flag.store(true, Ordering::Release);
                    Ok(())
                })
                .await
                .is_err());
            assert!(
                !started.load(Ordering::Acquire),
                "second blocking probe must not start"
            );
            release_tx.send(()).unwrap();
            let permit =
                tokio::time::timeout(Duration::from_secs(1), probe.gate.clone().acquire_owned())
                    .await
                    .unwrap()
                    .unwrap();
            drop(permit);
            probe.pin(&CancellationToken::new()).await.unwrap();
        }
    }

    #[tokio::test]
    async fn read_limits_and_registry_declaration_mismatch_fail_closed() {
        let (_root, mut probe) = fixture();
        probe.limits.max_implementation_bytes = 1;
        assert!(probe.pin(&CancellationToken::new()).await.is_err());
        let (_root, mut probe) = fixture();
        probe.limits.max_manifest_bytes = 1;
        assert!(probe.pin(&CancellationToken::new()).await.is_err());
        let (_root, mut probe) = fixture();
        probe.limits.max_trust_bytes = 1;
        assert!(probe.pin(&CancellationToken::new()).await.is_err());
        let (_root, mut probe) = fixture();
        probe.declaration.args.push("registry-only-change".into());
        assert!(probe.pin(&CancellationToken::new()).await.is_err());
    }
}
