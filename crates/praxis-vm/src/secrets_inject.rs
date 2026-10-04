//! An explicit, per-guest credential directory. No provider store or environment.
use std::{collections::BTreeMap, fs, path::Path};

pub fn validate_grants(grants: &BTreeMap<String, String>) -> anyhow::Result<()> {
    for name in grants.keys() {
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                && !name.as_bytes()[0].is_ascii_digit(),
            "Invalid VM credential name"
        );
    }
    Ok(())
}

fn plain_directory(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => anyhow::ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "VM credential path must be a plain directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Replace only this guest's credential files. Revoked grants (including files
/// from the old blanket injector) cannot remain available on the 9p mount.
/// Disks and shared folders are never touched.
pub fn inject_secrets(
    vm_name: &str,
    data_dir: &Path,
    grants: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    crate::validate_vm_name(vm_name)?;
    validate_grants(grants)?;
    let base = data_dir.join("vm");
    let guest = base.join(vm_name);
    let directory = guest.join("secrets");
    for path in [&base, &guest, &directory] {
        plain_directory(path)?;
    }
    // Preflight existing entries before removing/replacing any credential.
    let mut old = Vec::new();
    if directory.exists() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            anyhow::ensure!(
                ty.is_file() && !ty.is_symlink(),
                "Unexpected VM credential entry"
            );
            old.push(entry.path());
        }
    }
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    for path in old {
        fs::remove_file(path)?;
    }
    for (name, value) in grants {
        // create_new rejects an intervening link rather than following it.
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(directory.join(name))?;
        file.write_all(value.as_bytes())?;
    }
    // No environment loader: applications read only the granted file they need.
    let manifest = grants.keys().cloned().collect::<Vec<_>>().join("\n");
    fs::write(directory.join("manifest.txt"), manifest)?;
    Ok(())
}
