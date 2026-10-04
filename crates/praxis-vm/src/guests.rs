//! Persisted guest ownership, connection endpoints and interrupted-operation
//! recovery. Records live next to each guest's disk in
//! `DATA_DIR/vm/<name>/guest.json` so disabling, upgrading or replacing the VM
//! package never loses them. Nothing in this module starts QEMU, kills a
//! process or deletes a disk; deletion stays an explicit operator action.
use crate::{validate_vm_name, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

pub const RECORD_SCHEMA: u32 = 1;
pub const RECORD_FILE: &str = "guest.json";
/// Share entry meaning "every authenticated user" (use access only).
pub const SHARE_ALL: &str = "*";
/// Principal used by host-authenticated operator surfaces (dashboard token,
/// autostart). It owns adopted legacy guests.
pub const OPERATOR: &str = "admin";

/// Host-authenticated identity. Never deserialized from model operands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    pub user: String,
    pub operator: bool,
}
impl Principal {
    pub fn user(user: &str) -> Self {
        Self {
            user: user.into(),
            operator: false,
        }
    }
    pub fn operator() -> Self {
        Self {
            user: OPERATOR.into(),
            operator: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Observe or interact: screenshots, VNC, input, shell, files, snapshots list/create.
    Use,
    /// Change lifecycle or durable state: stop, restore/delete snapshots,
    /// shared folders, reinstall, sharing and ownership.
    Manage,
}

/// Operations that may create a guest record when none exists.
pub fn creates_guest(operation: &str) -> bool {
    matches!(operation, "vm_start" | "vm_install")
}

pub fn required_access(operation: &str) -> Access {
    match operation {
        "vm_stop"
        | "vm_snapshot_restore"
        | "vm_snapshot_delete"
        | "vm_shared_folder"
        | "vm_install"
        | "reboot"
        | "cd" => Access::Manage,
        _ => Access::Use,
    }
}

/// Operation that was in progress when the worker last persisted the record.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PendingOperation {
    pub operation: String,
    pub principal: String,
    pub started_at: i64,
}

/// Outcome recorded when recovery finds an operation that never completed.
/// Guest effects (shell, network, package install) are not compensated.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InterruptedOperation {
    pub operation: String,
    pub principal: String,
    pub started_at: i64,
    pub detected_at: i64,
    pub outcome: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestRecord {
    pub schema: u32,
    pub name: String,
    pub owner: String,
    #[serde(default)]
    pub shared_with: BTreeSet<String>,
    /// Full configuration including QMP/serial/VNC endpoints and disk path.
    pub config: VmConfig,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub pending: Option<PendingOperation>,
    #[serde(default)]
    pub interrupted: Vec<InterruptedOperation>,
    /// True when the record was created for a pre-existing guest directory.
    #[serde(default)]
    pub adopted: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl GuestRecord {
    pub fn allows(&self, principal: &Principal, access: Access) -> bool {
        if principal.operator || self.owner == principal.user {
            return true;
        }
        access == Access::Use
            && (self.shared_with.contains(&principal.user) || self.shared_with.contains(SHARE_ALL))
    }
    pub fn public(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name, "owner": self.owner, "shared_with": self.shared_with,
            "adopted": self.adopted, "pending": self.pending, "interrupted": self.interrupted,
            "vnc_port": self.config.vnc_port, "updated_at": self.updated_at,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    NotFound,
    Forbidden,
}
impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Identical wording for "missing" and "not yours" would hide existence,
        // but callers need the distinction to map HTTP statuses. Neither reveals
        // the owner.
        match self {
            Denied::NotFound => write!(f, "VM not found"),
            Denied::Forbidden => write!(f, "VM access denied"),
        }
    }
}
impl std::error::Error for Denied {}

pub struct GuestStore {
    root: PathBuf,
    write: Mutex<()>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl GuestStore {
    pub fn new(data_dir: &str) -> Self {
        Self {
            root: Path::new(data_dir).join("vm"),
            write: Mutex::new(()),
        }
    }
    fn dir(&self, name: &str) -> anyhow::Result<PathBuf> {
        validate_vm_name(name)?;
        Ok(self.root.join(name))
    }
    fn path(&self, name: &str) -> anyhow::Result<PathBuf> {
        Ok(self.dir(name)?.join(RECORD_FILE))
    }

    pub fn load(&self, name: &str) -> anyhow::Result<Option<GuestRecord>> {
        let path = self.path(name)?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let record: GuestRecord = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            record.schema == RECORD_SCHEMA && record.name == name && record.config.name == name,
            "Guest record for '{name}' is invalid or from an unsupported schema"
        );
        Ok(Some(record))
    }

    /// Atomic replace: write temp file, fsync, rename, fsync directory.
    fn save_locked(&self, record: &GuestRecord) -> anyhow::Result<()> {
        use std::io::Write;
        let dir = self.dir(&record.name)?;
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".{RECORD_FILE}.{}.tmp", std::process::id()));
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(record)?)?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, dir.join(RECORD_FILE))?;
        if let Ok(handle) = std::fs::File::open(&dir) {
            let _ = handle.sync_all();
        }
        Ok(())
    }

    /// Names of guest directories, with or without a record.
    pub fn guest_names(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut names: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| validate_vm_name(name).is_ok())
            .collect();
        names.sort();
        names
    }

    pub fn records(&self) -> Vec<GuestRecord> {
        self.guest_names()
            .iter()
            .filter_map(|name| self.load(name).ok().flatten())
            .collect()
    }

    /// Authorize `principal` for `operation` on `name`. A missing record is
    /// claimed by the caller only for creating operations, and only when no
    /// legacy guest directory exists (legacy guests are adopted by recovery).
    pub async fn authorize(
        &self,
        principal: &Principal,
        name: &str,
        operation: &str,
        config: impl FnOnce() -> anyhow::Result<VmConfig>,
    ) -> anyhow::Result<GuestRecord> {
        let _guard = self.write.lock().await;
        if let Some(record) = self.load(name)? {
            let access = required_access(operation);
            if record.allows(principal, access) {
                return Ok(record);
            }
            // Re-running vm_start on a running/stopped guest is Use for sharers.
            return Err(Denied::Forbidden.into());
        }
        let legacy = self.dir(name)?.join("disk.qcow2").exists();
        if !creates_guest(operation) || (legacy && !principal.operator) {
            return Err(Denied::NotFound.into());
        }
        let record = GuestRecord {
            schema: RECORD_SCHEMA,
            name: name.into(),
            owner: principal.user.clone(),
            shared_with: BTreeSet::new(),
            config: config()?,
            pid: None,
            pending: None,
            interrupted: Vec::new(),
            adopted: legacy,
            created_at: now(),
            updated_at: now(),
        };
        self.save_locked(&record)?;
        Ok(record)
    }

    pub async fn update(
        &self,
        name: &str,
        change: impl FnOnce(&mut GuestRecord) -> anyhow::Result<()>,
    ) -> anyhow::Result<GuestRecord> {
        let _guard = self.write.lock().await;
        let mut record = self.load(name)?.ok_or(Denied::NotFound)?;
        change(&mut record)?;
        record.updated_at = now();
        self.save_locked(&record)?;
        Ok(record)
    }

    pub async fn begin(
        &self,
        name: &str,
        operation: &str,
        principal: &Principal,
    ) -> anyhow::Result<()> {
        self.update(name, |record| {
            record.pending = Some(PendingOperation {
                operation: operation.into(),
                principal: principal.user.clone(),
                started_at: now(),
            });
            Ok(())
        })
        .await
        .map(|_| ())
    }

    pub async fn finish(&self, name: &str, config: Option<&VmConfig>, pid: Option<Option<u32>>) {
        let _ = self
            .update(name, |record| {
                record.pending = None;
                if let Some(config) = config {
                    record.config = config.clone();
                }
                if let Some(pid) = pid {
                    record.pid = pid;
                }
                Ok(())
            })
            .await;
    }

    /// Owner/operator only. `user` may be [`SHARE_ALL`].
    pub async fn share(
        &self,
        principal: &Principal,
        name: &str,
        user: &str,
        grant: bool,
    ) -> anyhow::Result<GuestRecord> {
        anyhow::ensure!(
            user == SHARE_ALL
                || (!user.is_empty() && user.len() <= 128 && !user.chars().any(char::is_control)),
            "Invalid share principal"
        );
        let record = self.load(name)?.ok_or(Denied::NotFound)?;
        if !record.allows(principal, Access::Manage) {
            return Err(Denied::Forbidden.into());
        }
        self.update(name, |record| {
            if grant {
                record.shared_with.insert(user.into());
            } else {
                record.shared_with.remove(user);
            }
            Ok(())
        })
        .await
    }

    /// Operator-only ownership transfer (e.g. assigning an adopted guest).
    pub async fn transfer(
        &self,
        principal: &Principal,
        name: &str,
        owner: &str,
    ) -> anyhow::Result<GuestRecord> {
        if !principal.operator {
            return Err(Denied::Forbidden.into());
        }
        anyhow::ensure!(!owner.is_empty() && owner != SHARE_ALL, "Invalid owner");
        self.update(name, |record| {
            record.owner = owner.into();
            Ok(())
        })
        .await
    }

    /// Startup recovery. Adopts legacy guest directories (owner = operator,
    /// shared with everyone, preserving historical single-tenant behaviour
    /// until the operator narrows it), and converts any pending operation into
    /// an explicit interrupted outcome. Returns the records to reattach.
    pub async fn recover(
        &self,
        legacy_config: impl Fn(&str) -> anyhow::Result<VmConfig>,
    ) -> Vec<GuestRecord> {
        let _guard = self.write.lock().await;
        let mut out = Vec::new();
        for name in self.guest_names() {
            let record = match self.load(&name) {
                Ok(Some(record)) => Some(record),
                Ok(None) if self.root.join(&name).join("disk.qcow2").exists() => {
                    legacy_config(&name).ok().map(|config| GuestRecord {
                        schema: RECORD_SCHEMA,
                        name: name.clone(),
                        owner: OPERATOR.into(),
                        shared_with: [SHARE_ALL.to_string()].into(),
                        config,
                        pid: None,
                        pending: None,
                        interrupted: Vec::new(),
                        adopted: true,
                        created_at: now(),
                        updated_at: now(),
                    })
                }
                Ok(None) => None,
                Err(error) => {
                    // Never overwrite an unreadable record; leave it for the operator.
                    tracing::warn!("Skipping guest '{name}': {error}");
                    None
                }
            };
            let Some(mut record) = record else { continue };
            if let Some(pending) = record.pending.take() {
                record.interrupted.push(InterruptedOperation {
                    outcome: "interrupted; guest effects not compensated".into(),
                    operation: pending.operation,
                    principal: pending.principal,
                    started_at: pending.started_at,
                    detected_at: now(),
                });
                let excess = record.interrupted.len().saturating_sub(16);
                record.interrupted.drain(..excess);
            }
            record.updated_at = now();
            if self.save_locked(&record).is_ok() {
                out.push(record);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, GuestStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = GuestStore::new(dir.path().to_str().unwrap());
        (dir, store)
    }
    fn config(dir: &tempfile::TempDir, name: &str) -> VmConfig {
        VmConfig::default_for_name(name, dir.path().to_str().unwrap(), 1, "x86_64")
    }

    #[tokio::test]
    async fn creator_owns_and_others_are_denied() {
        let (dir, store) = store();
        let alice = Principal::user("alice");
        let bob = Principal::user("bob");
        assert!(store
            .authorize(&alice, "g", "vm_shell", || Ok(config(&dir, "g")))
            .await
            .is_err());
        assert!(
            !dir.path().join("vm/g").exists(),
            "non-creating op must not create storage"
        );
        let record = store
            .authorize(&alice, "g", "vm_start", || Ok(config(&dir, "g")))
            .await
            .unwrap();
        assert_eq!(record.owner, "alice");
        let err = store
            .authorize(&bob, "g", "vm_screenshot", || unreachable!())
            .await
            .unwrap_err();
        assert_eq!(err.downcast_ref::<Denied>(), Some(&Denied::Forbidden));
        // bob cannot claim by starting either
        assert!(store
            .authorize(&bob, "g", "vm_start", || unreachable!())
            .await
            .is_err());
        assert!(store
            .authorize(&Principal::operator(), "g", "vm_stop", || unreachable!())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn sharing_grants_use_but_not_manage() {
        let (dir, store) = store();
        let alice = Principal::user("alice");
        let bob = Principal::user("bob");
        store
            .authorize(&alice, "g", "vm_start", || Ok(config(&dir, "g")))
            .await
            .unwrap();
        assert!(store.share(&bob, "g", "bob", true).await.is_err());
        store.share(&alice, "g", "bob", true).await.unwrap();
        assert!(store
            .authorize(&bob, "g", "vm_keys", || unreachable!())
            .await
            .is_ok());
        assert!(store
            .authorize(&bob, "g", "vm_stop", || unreachable!())
            .await
            .is_err());
        assert!(store.share(&bob, "g", "carol", true).await.is_err());
        store.share(&alice, "g", "bob", false).await.unwrap();
        assert!(store
            .authorize(&bob, "g", "vm_keys", || unreachable!())
            .await
            .is_err());
        assert!(store.transfer(&alice, "g", "bob").await.is_err());
        store
            .transfer(&Principal::operator(), "g", "bob")
            .await
            .unwrap();
        assert!(store
            .authorize(&bob, "g", "vm_stop", || unreachable!())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn recovery_adopts_legacy_and_records_interruptions() {
        let (dir, store) = store();
        let legacy = dir.path().join("vm/old");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("disk.qcow2"), b"disk").unwrap();
        // A user cannot claim a legacy guest by starting it.
        assert!(store
            .authorize(&Principal::user("eve"), "old", "vm_start", || Ok(config(
                &dir, "old"
            )))
            .await
            .is_err());
        let alice = Principal::user("alice");
        store
            .authorize(&alice, "new", "vm_start", || Ok(config(&dir, "new")))
            .await
            .unwrap();
        store.begin("new", "vm_start", &alice).await.unwrap();

        let recovered = store.recover(|name| Ok(config(&dir, name))).await;
        assert_eq!(recovered.len(), 2);
        let old = store.load("old").unwrap().unwrap();
        assert!(old.adopted && old.owner == OPERATOR && old.shared_with.contains(SHARE_ALL));
        assert!(old.allows(&Principal::user("eve"), Access::Use));
        assert!(!old.allows(&Principal::user("eve"), Access::Manage));
        let new = store.load("new").unwrap().unwrap();
        assert!(new.pending.is_none());
        assert_eq!(new.interrupted[0].operation, "vm_start");
        assert_eq!(std::fs::read(legacy.join("disk.qcow2")).unwrap(), b"disk");
        // Recovery is idempotent.
        store.recover(|name| Ok(config(&dir, name))).await;
        assert_eq!(store.load("new").unwrap().unwrap().interrupted.len(), 1);
    }

    #[tokio::test]
    async fn corrupt_records_are_left_untouched() {
        let (dir, store) = store();
        let guest = dir.path().join("vm/bad");
        std::fs::create_dir_all(&guest).unwrap();
        std::fs::write(guest.join(RECORD_FILE), b"{not json").unwrap();
        assert!(store
            .recover(|name| Ok(config(&dir, name)))
            .await
            .is_empty());
        assert_eq!(
            std::fs::read(guest.join(RECORD_FILE)).unwrap(),
            b"{not json"
        );
        assert!(store
            .authorize(&Principal::operator(), "bad", "vm_start", || Ok(config(
                &dir, "bad"
            )))
            .await
            .is_err());
    }
}
