//! Content-derived evidence for declared resources. This detects changes at
//! sampling boundaries; it does not freeze a filesystem or sandbox writers.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, Metadata},
    io::Read,
    path::{Component, Path, PathBuf},
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub sha256: String,
    pub paths: Vec<String>,
    pub entries: usize,
    pub bytes: u64,
}

pub(crate) fn validate(paths: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(paths.len() <= 16, "At most 16 resource scopes per check");
    let mut unique = HashSet::new();
    for path in paths {
        anyhow::ensure!(
            !path.is_empty() && path.len() <= 4096 && !path.contains(['\\', '\0']),
            "Invalid resource path"
        );
        anyhow::ensure!(
            path == "."
                || (path
                    .split('/')
                    .all(|s| !s.is_empty() && s != "." && s != "..")
                    && Path::new(path)
                        .components()
                        .all(|c| matches!(c, Component::Normal(_)))),
            "Resources must be normalized paths relative to the pinned root"
        );
        anyhow::ensure!(unique.insert(path), "Duplicate resource scope");
    }
    Ok(())
}

struct Inventory {
    digest: Sha256,
    seen: HashSet<String>,
    bytes: u64,
}
impl Inventory {
    fn field(&mut self, bytes: &[u8]) {
        self.digest.update((bytes.len() as u64).to_le_bytes());
        self.digest.update(bytes);
    }
    fn visit(&mut self, root: &Path, relative: &str, depth: usize) -> anyhow::Result<()> {
        anyhow::ensure!(depth <= MAX_DEPTH, "Resource tree exceeds depth limit");
        if !self.seen.insert(relative.into()) {
            return Ok(());
        }
        anyhow::ensure!(
            self.seen.len() <= MAX_ENTRIES,
            "Resource tree exceeds 4096 entries"
        );
        let path = root.join(relative);
        self.field(relative.as_bytes());
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.field(b"missing");
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            !meta.file_type().is_symlink(),
            "Symlink resources are not supported"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.field(&meta.permissions().mode().to_le_bytes());
        }
        #[cfg(not(unix))]
        self.field(&[meta.permissions().readonly() as u8]);
        if meta.is_dir() {
            self.field(b"directory");
            let mut children = Vec::new();
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                anyhow::ensure!(
                    children.len() + self.seen.len() < MAX_ENTRIES,
                    "Resource tree exceeds 4096 entries"
                );
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("Resource names must be UTF-8"))?;
                children.push(if relative == "." {
                    name
                } else {
                    format!("{relative}/{name}")
                });
            }
            children.sort();
            for child in children {
                self.visit(root, &child, depth + 1)?;
            }
            anyhow::ensure!(
                stable(&meta, &fs::symlink_metadata(&path)?),
                "Resource directory changed while sampling"
            );
        } else {
            anyhow::ensure!(
                meta.is_file(),
                "Resources must be regular files or directories"
            );
            anyhow::ensure!(
                meta.len() <= MAX_BYTES - self.bytes,
                "Resource bytes exceed 16 MiB"
            );
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let mut file = options.open(&path)?;
            let opened = file.metadata()?;
            anyhow::ensure!(
                opened.is_file() && stable(&meta, &opened),
                "Resource file changed before sampling"
            );
            self.field(b"file");
            let mut contents = Sha256::new();
            let mut size = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                size += read as u64;
                anyhow::ensure!(
                    size <= MAX_BYTES - self.bytes,
                    "Resource bytes exceed 16 MiB"
                );
                contents.update(&buffer[..read]);
            }
            anyhow::ensure!(
                size == opened.len()
                    && stable(&opened, &file.metadata()?)
                    && stable(&opened, &fs::symlink_metadata(&path)?),
                "Resource file changed while sampling"
            );
            self.bytes += size;
            self.field(&size.to_le_bytes());
            self.field(&contents.finalize());
        }
        Ok(())
    }
}
fn stable(before: &Metadata, after: &Metadata) -> bool {
    let common = before.len() == after.len()
        && before.is_file() == after.is_file()
        && before.is_dir() == after.is_dir()
        && before.modified().ok() == after.modified().ok()
        && before.permissions().readonly() == after.permissions().readonly();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        common
            && before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.mode() == after.mode()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        common
    }
}
fn check_ancestors(root: &Path, relative: &str) -> anyhow::Result<()> {
    let mut path = PathBuf::from(root);
    if relative == "." {
        return Ok(());
    }
    for part in Path::new(relative).components() {
        path.push(part.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(meta) => anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "Symlink resource paths are not supported"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
pub(crate) fn capture(root: &Path, paths: &[String]) -> anyhow::Result<ResourceSnapshot> {
    validate(paths)?;
    anyhow::ensure!(!paths.is_empty(), "Resource snapshot requires a scope");
    let root = root.canonicalize()?;
    anyhow::ensure!(root.is_dir(), "Resource root must be a directory");
    let mut paths = paths.to_vec();
    paths.sort();
    let mut inventory = Inventory {
        digest: Sha256::new(),
        seen: HashSet::new(),
        bytes: 0,
    };
    inventory.field(b"praxis-resource-snapshot-v1");
    inventory.field(
        root.to_str()
            .ok_or_else(|| anyhow::anyhow!("Resource root must be UTF-8"))?
            .as_bytes(),
    );
    for path in &paths {
        check_ancestors(&root, path)?;
        inventory.visit(&root, path, 0)?;
    }
    Ok(ResourceSnapshot {
        sha256: format!("{:x}", inventory.digest.finalize()),
        paths,
        entries: inventory.seen.len(),
        bytes: inventory.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_snapshot_is_stable_across_scope_order_and_detects_inventory() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/a"), "a").unwrap();
        fs::write(root.path().join("manifest"), "m").unwrap();
        let first = capture(root.path(), &["src".into(), "manifest".into()]).unwrap();
        assert_eq!(
            first,
            capture(root.path(), &["manifest".into(), "src".into()]).unwrap()
        );
        fs::create_dir(root.path().join("src/empty")).unwrap();
        assert_ne!(
            first.sha256,
            capture(root.path(), &["src".into(), "manifest".into()])
                .unwrap()
                .sha256
        );
    }
    #[test]
    fn resource_snapshot_rejects_excessive_entry_counts() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..MAX_ENTRIES {
            fs::write(root.path().join(index.to_string()), []).unwrap();
        }
        assert!(capture(root.path(), &[".".into()]).is_err());
    }
}
