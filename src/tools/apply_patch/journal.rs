//! Private write-ahead records. Filesystem sampling is not writer isolation.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::fs::{File, OpenOptions};

const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REVISION_BYTES: u64 = 16 * 1024;

/// A random identity prevents reuse after a missing record is reinitialized.
/// It is a freshness token, not a counter, receipt, or content hash.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    version: u32,
    root: PathBuf,
    id: String,
}

fn read_revision(root: &Path, store: &Path) -> anyhow::Result<String> {
    private(&fs::symlink_metadata(store)?, true)?;
    let path = store.join("revision.json");
    private(&fs::symlink_metadata(&path)?, false)?;
    let mut options = private_options();
    options.write(false);
    let file = options.open(path)?;
    let meta = file.metadata()?;
    private(&meta, false)?;
    anyhow::ensure!(
        meta.len() <= MAX_REVISION_BYTES,
        "Oversized workspace revision"
    );
    let mut bytes = Vec::new();
    file.take(MAX_REVISION_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_REVISION_BYTES as usize,
        "Oversized workspace revision"
    );
    let revision: Revision = serde_json::from_slice(&bytes).map_err(|_| {
        anyhow::anyhow!("Malformed workspace revision; operator inspection required")
    })?;
    anyhow::ensure!(
        revision.version == 1 && revision.root == root,
        "Invalid workspace revision version/root"
    );
    anyhow::ensure!(
        uuid::Uuid::parse_str(&revision.id).is_ok_and(|id| id.to_string() == revision.id),
        "Invalid workspace revision identity"
    );
    Ok(revision.id)
}

/// Read without taking the OS lock: callers may already hold it, or the task
/// ledger. Only the lock owner initializes/advances revisions. Missing/corrupt
/// state fails closed here; migration happens in Workspace::open.
pub(crate) fn workspace_revision(root: &Path) -> anyhow::Result<Option<String>> {
    if !cfg!(unix) {
        return Ok(None);
    }
    let root = root.canonicalize()?;
    Ok(Some(read_revision(&root, &store_path(&root)?)?))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPermissions {
    mode: Option<u32>,
    readonly: bool,
}
impl SavedPermissions {
    fn capture(permissions: &Permissions) -> Self {
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            Some(permissions.mode() & 0o7777)
        };
        #[cfg(not(unix))]
        let mode = None;
        Self {
            mode,
            readonly: permissions.readonly(),
        }
    }
    fn restore(&self) -> anyhow::Result<Permissions> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = self
                .mode
                .ok_or_else(|| anyhow::anyhow!("Journal permissions have no mode"))?;
            anyhow::ensure!(mode <= 0o7777, "Invalid journal mode");
            Ok(Permissions::from_mode(mode))
        }
        #[cfg(not(unix))]
        anyhow::bail!("Durable patches currently require Unix")
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFile {
    path: String,
    before: Option<String>,
    before_permissions: Option<SavedPermissions>,
    after_sha256: Option<String>,
    after_permissions: Option<SavedPermissions>,
}
impl SavedFile {
    fn before(&self) -> anyhow::Result<Option<Snapshot>> {
        match (&self.before, &self.before_permissions) {
            (None, None) => Ok(None),
            (Some(bytes), Some(permissions)) => {
                anyhow::ensure!(
                    bytes.len() <= MAX_FILE_BYTES.div_ceil(3) * 4,
                    "Journal file too large"
                );
                let bytes = STANDARD.decode(bytes)?;
                anyhow::ensure!(bytes.len() <= MAX_FILE_BYTES, "Journal file too large");
                Ok(Some(Snapshot {
                    bytes,
                    permissions: permissions.restore()?,
                }))
            }
            _ => anyhow::bail!("Inconsistent original snapshot"),
        }
    }
    fn matches_after(&self, current: Option<&Snapshot>) -> anyhow::Result<bool> {
        Ok(
            match (&self.after_sha256, &self.after_permissions, current) {
                (None, None, None) => true,
                (Some(hash_value), Some(permissions), Some(now)) => {
                    hash(&now.bytes) == *hash_value
                        && same_permissions(&permissions.restore()?, &now.permissions)
                }
                _ => false,
            },
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    id: String,
    root: String,
    intended: usize,
    committed: bool,
    files: Vec<SavedFile>,
}
impl Record {
    fn validate(&self, root: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.version == 1 && self.root == root.to_string_lossy(),
            "Journal version/root mismatch"
        );
        let id = uuid::Uuid::parse_str(&self.id)?;
        anyhow::ensure!(id.to_string() == self.id, "Invalid journal identity");
        anyhow::ensure!(
            (1..=MAX_FILES).contains(&self.files.len()) && self.intended <= self.files.len(),
            "Invalid journal entry count"
        );
        anyhow::ensure!(
            !self.committed || self.intended == self.files.len(),
            "Incomplete committed journal"
        );
        let mut paths = HashSet::new();
        let mut total = 0;
        for file in &self.files {
            scoped_path(root, &file.path)?;
            anyhow::ensure!(paths.insert(&file.path), "Duplicate journal path");
            total += file.before()?.map_or(0, |s| s.bytes.len());
            anyhow::ensure!(
                total <= MAX_BATCH_BYTES,
                "Journal originals exceed batch limit"
            );
            match (&file.after_sha256, &file.after_permissions) {
                (None, None) => anyhow::ensure!(file.before.is_some(), "Invalid journal deletion"),
                (Some(value), Some(permissions)) => {
                    anyhow::ensure!(
                        value.len() == 64
                            && value
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                        "Invalid journal hash"
                    );
                    permissions.restore()?;
                }
                _ => anyhow::bail!("Inconsistent replacement snapshot"),
            }
        }
        Ok(())
    }
    fn stage_path(&self, root: &Path, index: usize, restoring: bool) -> anyhow::Result<PathBuf> {
        let target = scoped_path(root, &self.files[index].path)?;
        let parent = target
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing parent"))?;
        Ok(parent.join(format!(
            ".praxis-patch-{}-{index}-{}",
            self.id,
            if restoring { "restore" } else { "stage" }
        )))
    }
}

#[derive(Serialize)]
pub struct Report {
    pub outcome: &'static str,
    pub transaction_id: Option<String>,
    pub conflicts: Vec<String>,
}
fn store_path(root: &Path) -> anyhow::Result<PathBuf> {
    let parent = root.parent().ok_or_else(|| {
        anyhow::anyhow!("Workspace root requires a writable parent for durable journals")
    })?;
    let name = root
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Journal root must be UTF-8"))?;
    Ok(parent.join(format!(".praxis-patch-journal-{}", hash(name.as_bytes()))))
}
pub(crate) fn pending_exists(root: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return true;
    };
    let Ok(store) = store_path(&root) else {
        return cfg!(unix);
    };
    match fs::symlink_metadata(&store) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Ok(meta) if private(&meta, true).is_ok() => {}
        _ => return true,
    }
    !matches!(fs::symlink_metadata(store.join("pending.json")), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}
pub(super) fn sync_directory(path: &Path) -> anyhow::Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
fn private(meta: &fs::Metadata, directory: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        !meta.file_type().is_symlink()
            && if directory {
                meta.is_dir()
            } else {
                meta.is_file()
            },
        "Unsafe journal file type"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
            "Journal must be private to its owner"
        );
        anyhow::ensure!(
            directory || meta.nlink() == 1,
            "Hardlinked journal rejected"
        );
    }
    Ok(())
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options
}
fn remove_private(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(meta) => {
            private(&meta, false)?;
            anyhow::ensure!(meta.len() <= MAX_RECORD_BYTES, "Oversized journal file");
        }
    }
    fs::remove_file(path)?;
    sync_directory(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("Missing parent"))?,
    )
}

pub(crate) struct Workspace {
    root: PathBuf,
    store: PathBuf,
    _lock: File,
}
impl Drop for Workspace {
    fn drop(&mut self) {
        // Explicit unlock also releases the short-lived copy inherited by a
        // concurrent fork before exec closes its CLOEXEC file descriptors.
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self._lock.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
impl Workspace {
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(
            cfg!(unix),
            "Durable file patches currently require Unix directory synchronization"
        );
        let root = root.canonicalize()?;
        anyhow::ensure!(root.is_dir(), "Workspace must be a directory");
        let store = store_path(&root)?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&store) {
            Ok(()) => sync_directory(
                store
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("Missing parent"))?,
            )?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        private(&fs::symlink_metadata(&store)?, true)?;
        let lock_path = store.join("lock");
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        private(&lock.metadata()?, false)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            anyhow::ensure!(
                unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
                "Workspace transaction/recovery is busy in another process"
            );
        }
        // A death during an atomic record rewrite leaves only this private temp.
        remove_private(&store.join("pending.new"))?;
        remove_private(&store.join("revision.new"))?;
        let workspace = Self {
            root,
            store,
            _lock: lock,
        };
        match fs::symlink_metadata(workspace.store.join("revision.json")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Upgrade pre-revision stores; never reuse a default identity.
                workspace.save_revision()?;
            }
            Err(error) => return Err(error.into()),
            Ok(_) => {
                read_revision(&workspace.root, &workspace.store)?;
            }
        }
        Ok(workspace)
    }
    pub fn pending_path(&self) -> PathBuf {
        self.store.join("pending.json")
    }
    fn save_revision(&self) -> anyhow::Result<()> {
        let revision = Revision {
            version: 1,
            root: self.root.clone(),
            id: uuid::Uuid::new_v4().to_string(),
        };
        let bytes = serde_json::to_vec(&revision)?;
        anyhow::ensure!(
            bytes.len() <= MAX_REVISION_BYTES as usize,
            "Oversized workspace revision"
        );
        let path = self.store.join("revision.new");
        remove_private(&path)?;
        let file = private_options().create_new(true).open(&path)?;
        let mut temp =
            tempfile::NamedTempFile::from_parts(file, tempfile::TempPath::try_from_path(path)?);
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(self.store.join("revision.json"))
            .map_err(|error| error.error)?;
        sync_directory(&self.store)
    }
    fn advance_revision(&self) -> anyhow::Result<()> {
        read_revision(&self.root, &self.store)?;
        self.save_revision()
    }
    fn save(&self, record: &Record) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(record)?;
        anyhow::ensure!(
            bytes.len() <= MAX_RECORD_BYTES as usize,
            "Journal exceeds 8 MiB"
        );
        remove_private(&self.store.join("pending.new"))?;
        let path = self.store.join("pending.new");
        let file = private_options().create_new(true).open(&path)?;
        let mut temp =
            tempfile::NamedTempFile::from_parts(file, tempfile::TempPath::try_from_path(path)?);
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(self.pending_path())
            .map_err(|error| error.error)?;
        sync_directory(&self.store)
    }
    fn load(&self) -> anyhow::Result<Option<Record>> {
        let path = self.pending_path();
        let mut options = private_options();
        options.write(false);
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let meta = file.metadata()?;
        private(&meta, false)?;
        anyhow::ensure!(meta.len() <= MAX_RECORD_BYTES, "Journal exceeds 8 MiB");
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= MAX_RECORD_BYTES as usize,
            "Journal grew beyond limit"
        );
        let record: Record = serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!("Malformed patch journal; operator inspection required")
        })?;
        record.validate(&self.root)?;
        Ok(Some(record))
    }
    fn clean_stages(&self, record: &Record) -> Vec<String> {
        let mut conflicts = Vec::new();
        for index in 0..record.files.len() {
            for restoring in [false, true] {
                let result = (|| -> anyhow::Result<()> {
                    let path = record.stage_path(&self.root, index, restoring)?;
                    match fs::symlink_metadata(&path) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                        Err(error) => return Err(error.into()),
                        Ok(meta) => {
                            anyhow::ensure!(
                                meta.is_file()
                                    && !meta.file_type().is_symlink()
                                    && meta.len() <= MAX_FILE_BYTES as u64,
                                "Unsafe staging file"
                            );
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::MetadataExt;
                                anyhow::ensure!(
                                    meta.nlink() == 1 && meta.uid() == unsafe { libc::geteuid() },
                                    "Unsafe staging owner/link count"
                                );
                            }
                        }
                    }
                    fs::remove_file(&path)?;
                    sync_directory(
                        path.parent()
                            .ok_or_else(|| anyhow::anyhow!("Missing parent"))?,
                    )
                })();
                if result.is_err() {
                    conflicts.push(record.files[index].path.clone());
                }
            }
        }
        conflicts
    }
    pub fn recover(&mut self) -> anyhow::Result<Report> {
        let Some(record) = self.load()? else {
            return Ok(Report {
                outcome: "clean",
                transaction_id: None,
                conflicts: Vec::new(),
            });
        };
        // Persist freshness before any compensation/cleanup can become visible
        // to other Praxis processes. Repeated conflict recovery is conservative.
        self.advance_revision()?;
        crate::gateway::task_control::invalidate_workspace(&self.root)?;
        let mut conflicts = Vec::new();
        if !record.committed {
            for index in (0..record.intended).rev() {
                let file = &record.files[index];
                let result = (|| -> anyhow::Result<()> {
                    let path = scoped_path(&self.root, &file.path)?;
                    let current = snapshot(&path)?;
                    let before = file.before()?;
                    if match (&before, &current) {
                        (None, None) => true,
                        (Some(old), Some(now)) => {
                            old.bytes == now.bytes
                                && same_permissions(&old.permissions, &now.permissions)
                        }
                        _ => false,
                    } {
                        if before.is_some() {
                            let mut options = OpenOptions::new();
                            options.read(true);
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::OpenOptionsExt;
                                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                            }
                            options.open(&path)?.sync_all()?;
                        }
                        sync_directory(
                            path.parent()
                                .ok_or_else(|| anyhow::anyhow!("Missing parent"))?,
                        )?;
                        return Ok(());
                    }
                    anyhow::ensure!(file.matches_after(current.as_ref())?, "Recovery conflict");
                    match before {
                        Some(before) => {
                            let staging = record.stage_path(&self.root, index, true)?;
                            let temp = stage_named(&staging, &before.bytes, &before.permissions)?;
                            scoped_path(&self.root, &file.path)?;
                            anyhow::ensure!(
                                file.matches_after(snapshot(&path)?.as_ref())?,
                                "File changed before restoration"
                            );
                            if current.is_none() {
                                temp.persist_noclobber(&path).map_err(|error| error.error)?;
                            } else {
                                temp.persist(&path).map_err(|error| error.error)?;
                            }
                        }
                        None => {
                            scoped_path(&self.root, &file.path)?;
                            anyhow::ensure!(
                                file.matches_after(snapshot(&path)?.as_ref())?,
                                "File changed before restoration"
                            );
                            fs::remove_file(&path)?;
                        }
                    }
                    sync_directory(
                        path.parent()
                            .ok_or_else(|| anyhow::anyhow!("Missing parent"))?,
                    )?;
                    let restored = snapshot(&path)?;
                    anyhow::ensure!(
                        match (&file.before()?, &restored) {
                            (None, None) => true,
                            (Some(old), Some(now)) =>
                                old.bytes == now.bytes
                                    && same_permissions(&old.permissions, &now.permissions),
                            _ => false,
                        },
                        "File changed during recovery"
                    );
                    Ok(())
                })();
                if result.is_err() {
                    conflicts.push(file.path.clone());
                }
            }
        }
        conflicts.extend(self.clean_stages(&record));
        conflicts.sort();
        conflicts.dedup();
        if conflicts.is_empty() {
            remove_private(&self.pending_path())?;
        }
        Ok(Report {
            outcome: if !conflicts.is_empty() {
                "conflict"
            } else if record.committed {
                "committed_preserved"
            } else {
                "recovered"
            },
            transaction_id: Some(record.id),
            conflicts,
        })
    }
}

pub(crate) fn ready(root: &Path) -> anyhow::Result<Option<Workspace>> {
    // Existing named checks and inspections remain usable on other platforms
    // when there is no outstanding journal; durable writes are Unix-only.
    #[cfg(not(unix))]
    {
        anyhow::ensure!(
            !pending_exists(root),
            "Journal recovery requires a Unix host"
        );
        return Ok(None);
    }
    #[cfg(unix)]
    {
        let mut workspace = Workspace::open(root)?;
        let report = workspace.recover()?;
        anyhow::ensure!(
            report.conflicts.is_empty(),
            "Interrupted patch requires recovery; conflicting paths: {:?}",
            report.conflicts
        );
        Ok(Some(workspace))
    }
}

pub(super) struct Active {
    workspace: Workspace,
    record: Record,
}
impl Active {
    pub fn begin(workspace: Workspace, files: &[Prepared]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !workspace.pending_path().try_exists()?,
            "Recover the existing journal first"
        );
        let record = Record {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            root: workspace
                .root
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Root must be UTF-8"))?
                .into(),
            intended: 0,
            committed: false,
            files: files
                .iter()
                .map(|file| SavedFile {
                    path: file.path.clone(),
                    before: file.before.as_ref().map(|s| STANDARD.encode(&s.bytes)),
                    before_permissions: file
                        .before
                        .as_ref()
                        .map(|s| SavedPermissions::capture(&s.permissions)),
                    after_sha256: file.after.as_ref().map(|b| hash(b)),
                    after_permissions: file
                        .after_permissions
                        .as_ref()
                        .map(SavedPermissions::capture),
                })
                .collect(),
        };
        record.validate(&workspace.root)?;
        workspace.save(&record)?;
        // The journal exists first, so a death during this revision rewrite is
        // recoverable. No target or adjacent stage has been touched yet.
        workspace.advance_revision()?;
        Ok(Self { workspace, record })
    }
    pub fn id(&self) -> &str {
        &self.record.id
    }
    pub fn intent(&mut self, index: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            index == self.record.intended && index < self.record.files.len(),
            "Out-of-order journal intent"
        );
        self.record.intended = index + 1;
        self.workspace.save(&self.record)
    }
    pub fn stage(
        &self,
        index: usize,
        bytes: &[u8],
        permissions: &Permissions,
    ) -> anyhow::Result<tempfile::NamedTempFile> {
        stage_named(
            &self.record.stage_path(&self.workspace.root, index, false)?,
            bytes,
            permissions,
        )
    }
    pub fn commit(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.record.intended == self.record.files.len(),
            "Incomplete transaction"
        );
        self.record.committed = true;
        self.workspace.save(&self.record)
    }
    pub fn rollback(&mut self) -> anyhow::Result<Report> {
        // A failed commit sync may have left a visible commit record. Durably
        // choose compensation before restoring even the first target.
        self.record.committed = false;
        self.workspace.save(&self.record)?;
        self.workspace.recover()
    }
    pub fn cleanup(&mut self) -> anyhow::Result<Report> {
        anyhow::ensure!(self.record.committed, "Cleanup requires a durable commit");
        let conflicts = self.workspace.clean_stages(&self.record);
        if conflicts.is_empty() {
            remove_private(&self.workspace.pending_path())?;
        }
        Ok(Report {
            outcome: if conflicts.is_empty() {
                "committed_preserved"
            } else {
                "conflict"
            },
            transaction_id: Some(self.record.id.clone()),
            conflicts,
        })
    }
}
fn stage_named(
    path: &Path,
    bytes: &[u8],
    permissions: &Permissions,
) -> anyhow::Result<tempfile::NamedTempFile> {
    // The name is derived from the owned, validated journal identity. An orphan
    // partial stage is runtime data; symlinks/special files are never removed.
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(meta) => {
            anyhow::ensure!(
                meta.is_file()
                    && !meta.file_type().is_symlink()
                    && meta.len() <= MAX_FILE_BYTES as u64,
                "Unsafe restoration stage"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                anyhow::ensure!(
                    meta.uid() == unsafe { libc::geteuid() } && meta.nlink() == 1,
                    "Unsafe restoration stage owner"
                );
            }
            fs::remove_file(path)?;
        }
    }
    let file = private_options().create_new(true).open(path)?;
    let mut temp = tempfile::NamedTempFile::from_parts(
        file,
        tempfile::TempPath::try_from_path(path.to_path_buf())?,
    );
    temp.write_all(bytes)?;
    temp.as_file().set_permissions(permissions.clone())?;
    temp.as_file().sync_all()?;
    sync_directory(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("Missing parent"))?,
    )?;
    Ok(temp)
}
