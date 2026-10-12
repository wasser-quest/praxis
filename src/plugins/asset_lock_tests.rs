use super::*;
use std::{
    fs,
    process::{Child, Command},
    time::{Duration, Instant},
};

const CHILD_TEST: &str = "plugins::lifecycle::asset_lock_tests::asset_lock_subprocess";

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_child(base: &Path, owner: &str, operation: &str) -> ChildGuard {
    ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env("PRAXIS_ASSET_LOCK_TEST_ROOT", base)
            .env("PRAXIS_ASSET_LOCK_TEST_OWNER", owner)
            .env("PRAXIS_ASSET_LOCK_TEST_OPERATION", operation)
            .spawn()
            .unwrap(),
    )
}

fn finish(child: &mut ChildGuard) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "asset subprocess failed: {status}");
            return;
        }
        assert!(Instant::now() < deadline, "asset subprocess deadlocked");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn asset_lock_subprocess() {
    let Some(base) = std::env::var_os("PRAXIS_ASSET_LOCK_TEST_ROOT") else {
        return;
    };
    let base = PathBuf::from(base);
    let owner = std::env::var("PRAXIS_ASSET_LOCK_TEST_OWNER").unwrap();
    let operation = std::env::var("PRAXIS_ASSET_LOCK_TEST_OPERATION").unwrap();
    let source = base.join("source");
    let data = base.join("data");
    let assets = base.join("assets");
    let paths: Vec<_> = (0..64).map(|i| format!("{owner}/{i}")).collect();
    let plugin: Plugin = serde_json::from_value(serde_json::json!({
        "name": owner, "description": "lock test", "version": "1",
        "provides": {"assets": paths}, "tools": []
    }))
    .unwrap();
    fs::write(base.join(format!("ready-{owner}")), "ready").unwrap();
    wait_for(&base.join("start"));
    if operation == "remove" {
        assert_eq!(
            remove_assets(&data, &assets, &owner).unwrap().removed.len(),
            64
        );
    } else {
        assert_eq!(
            apply_assets(&source, &plugin, &data, &assets)
                .unwrap()
                .written
                .len(),
            64
        );
    }
    fs::write(base.join(format!("done-{owner}")), "done").unwrap();
}

#[test]
fn asset_lock_concurrent_processes_preserve_distinct_owners() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path();
    let source = base.join("source");
    let data = base.join("data");
    let assets = base.join("assets");
    fs::create_dir_all(&data).unwrap();
    fs::write(asset_record_path(&data), "{}").unwrap();
    for owner in ["alpha", "beta"] {
        fs::create_dir_all(source.join(owner)).unwrap();
        for i in 0..64 {
            fs::write(source.join(format!("{owner}/{i}")), owner).unwrap();
        }
    }
    // The same persistent inode must serialize both apply and remove across
    // independent processes. Holding it also makes missing locking fail reliably.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("plugin_assets.lock"))
        .unwrap();
    for removing in [false, true] {
        lock.lock().unwrap();
        let mut alpha = spawn_child(base, "alpha", if removing { "remove" } else { "apply" });
        let mut beta = spawn_child(base, "beta", "apply");
        wait_for(&base.join("ready-alpha"));
        wait_for(&base.join("ready-beta"));
        fs::write(base.join("start"), "start").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !base.join("done-alpha").exists(),
            "alpha bypassed the shared lock"
        );
        assert!(
            !base.join("done-beta").exists(),
            "beta bypassed the shared lock"
        );
        lock.unlock().unwrap();
        finish(&mut alpha);
        finish(&mut beta);
        let records = load_asset_records(&data).unwrap();
        assert_eq!(records.contains_key("alpha"), !removing);
        assert_eq!(records["beta"].len(), 64);
        if !removing {
            assert_eq!(records["alpha"].len(), 64);
        }
        for i in 0..64 {
            assert_eq!(fs::read(assets.join(format!("beta/{i}"))).unwrap(), b"beta");
            assert_eq!(assets.join(format!("alpha/{i}")).exists(), !removing);
        }
        for marker in [
            "start",
            "ready-alpha",
            "ready-beta",
            "done-alpha",
            "done-beta",
        ] {
            fs::remove_file(base.join(marker)).unwrap();
        }
    }
    remove_assets(&data, &assets, "beta").unwrap();
    assert!(load_asset_records(&data).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn asset_lock_rejects_unsafe_paths_and_uses_private_persistent_inode() {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    for kind in ["parent", "leaf", "dangling", "hardlink", "directory"] {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let outside = temp.path().join("outside");
        fs::create_dir(&data).unwrap();
        fs::write(&outside, "untouched").unwrap();
        let path = data.join("plugin_assets.lock");
        match kind {
            "parent" => {
                fs::remove_dir(&data).unwrap();
                symlink(temp.path(), &data).unwrap();
            }
            "leaf" => symlink(&outside, &path).unwrap(),
            "dangling" => symlink(temp.path().join("missing"), &path).unwrap(),
            "hardlink" => fs::hard_link(&outside, &path).unwrap(),
            "directory" => fs::create_dir(&path).unwrap(),
            _ => unreachable!(),
        }
        assert!(asset_lock::AssetLock::acquire(&data).is_err(), "{kind}");
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
        assert!(!temp.path().join("missing").exists());
    }
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let path = data.join("plugin_assets.lock");
    let first = asset_lock::AssetLock::acquire(&data).unwrap();
    let metadata = fs::metadata(&path).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    drop(first);
    let second = asset_lock::AssetLock::acquire(&data).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), metadata.ino());
    drop(second);
    assert!(path.is_file(), "release must not unlink the shared inode");
}

#[test]
fn asset_lock_errors_release_for_another_process() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path();
    let data = base.join("data");
    let source = base.join("source");
    let assets = base.join("assets");
    fs::create_dir_all(source.join("beta")).unwrap();
    fs::create_dir(&data).unwrap();
    for i in 0..64 {
        fs::write(source.join(format!("beta/{i}")), "beta").unwrap();
    }
    let plugin: Plugin = serde_json::from_value(serde_json::json!({
        "name": "beta", "description": "test", "version": "1",
        "provides": {"assets": ["beta/0"]}, "tools": []
    }))
    .unwrap();
    fs::write(asset_record_path(&data), "not json").unwrap();
    assert!(apply_assets(&source, &plugin, &data, &assets).is_err());
    assert!(remove_assets(&data, &assets, "alpha").is_err());
    // Re-enter from a different process after both errors; a leaked lock or
    // recursive acquisition makes the bounded wait fail rather than hang CI.
    fs::write(asset_record_path(&data), "{}").unwrap();
    fs::write(base.join("start"), "start").unwrap();
    let mut child = spawn_child(base, "beta", "apply");
    finish(&mut child);
    assert_eq!(load_asset_records(&data).unwrap()["beta"].len(), 64);
}
