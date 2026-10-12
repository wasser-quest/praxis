//! A persistent, advisory OS lock shared by every lifecycle asset owner.
//! Never unlink or atomically replace this file: waiters must lock one inode.
//! As with asset_safety, parent directories must not be concurrently renamed.
use super::asset_safety;
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

/// Closing the file releases the lock on every return path (including errors).
/// Acquire only at the public apply/remove boundary; helpers must not reacquire
/// it, since independent opens are not a recursive lock.
pub(super) struct AssetLock {
    _file: File,
}

impl AssetLock {
    pub(super) fn acquire(data_dir: &Path) -> anyhow::Result<Self> {
        let path = data_dir.join("plugin_assets.lock");
        asset_safety::check(&path)?;
        fs::create_dir_all(data_dir)?;
        asset_safety::check(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // No symlink following; nonblocking open also avoids a swapped FIFO
            // hanging before we can reject its metadata. File::lock still waits.
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        // Do not truncate an existing file or replace it with a temporary inode.
        let file = match options.create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                options.create_new(false).open(&path)?
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_file(),
            "Invalid asset lock file: {}",
            path.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            anyhow::ensure!(
                metadata.nlink() == 1,
                "Refusing hardlinked asset lock: {}",
                path.display()
            );
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        asset_safety::check(&path)?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}
