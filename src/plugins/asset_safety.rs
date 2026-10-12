//! Filesystem checks and per-file publication for lifecycle assets.
//! Like the bundled installer, these checks assume directories are not being
//! concurrently replaced by another process. They are not a descriptor-relative
//! sandbox against hostile rename races.
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

pub(super) fn relative(asset: &str) -> anyhow::Result<&Path> {
    let path = Path::new(asset);
    anyhow::ensure!(
        !asset.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_))),
        "Asset must be a path inside its package"
    );
    let canonical: PathBuf = path.components().collect();
    anyhow::ensure!(
        canonical.as_os_str() == path.as_os_str(),
        "Asset path must use canonical components"
    );
    Ok(path)
}

/// Check every component, including the supplied root and dangling symlinks,
/// without first canonicalizing away evidence of a link. Missing paths are OK.
pub(super) fn check(path: &Path) -> anyhow::Result<bool> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let components: Vec<_> = absolute.components().collect();
    let mut current = PathBuf::new();
    let mut exists = false;
    for (index, component) in components.iter().enumerate() {
        anyhow::ensure!(
            !matches!(component, Component::ParentDir),
            "Invalid asset path: {}",
            path.display()
        );
        if matches!(component, Component::CurDir) {
            continue;
        }
        current.push(component.as_os_str());
        let leaf = index + 1 == components.len();
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                anyhow::ensure!(
                    !meta.file_type().is_symlink(),
                    "Refusing symlink asset path: {}",
                    current.display()
                );
                anyhow::ensure!(
                    if leaf { meta.is_file() } else { meta.is_dir() },
                    "Invalid asset path: {}",
                    current.display()
                );
                #[cfg(unix)]
                if leaf {
                    use std::os::unix::fs::MetadataExt;
                    anyhow::ensure!(
                        meta.nlink() == 1,
                        "Refusing hardlinked asset path: {}",
                        current.display()
                    );
                }
                exists = leaf;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(exists)
}

pub(super) fn check_sources(root: &Path, assets: &[String]) -> anyhow::Result<()> {
    for asset in assets {
        let path = root.join(relative(asset)?);
        anyhow::ensure!(check(&path)?, "Asset '{asset}' is missing from the package");
    }
    Ok(())
}

/// Stage next to the destination, sync the complete bytes, then publish by
/// rename. New files must not clobber a file that appeared after preflight.
pub(super) fn publish(path: &Path, bytes: &[u8], replace: bool) -> anyhow::Result<()> {
    check(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Asset has no parent"))?;
    fs::create_dir_all(parent)?;
    check(path)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    // Keep private tempfile defaults for new files; never broaden permissions
    // beyond umask. Existing assets retain their mode, including execute bits.
    if replace {
        temporary.as_file().set_permissions(fs::metadata(path)?.permissions())?;
    }
    temporary.as_file().sync_all()?;
    check(path)?;
    if replace {
        temporary.persist(path)?;
    } else {
        temporary.persist_noclobber(path)?;
    }
    Ok(())
}
