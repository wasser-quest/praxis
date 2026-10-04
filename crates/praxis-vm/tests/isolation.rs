use praxis_vm::{secrets_inject, VmConfig, VmManager};
use std::collections::BTreeMap;

#[tokio::test]
async fn invalid_vm_name_fails_before_storage_or_process_creation() {
    let root = tempfile::tempdir().unwrap();
    let manager = VmManager::new(root.path().to_str().unwrap());
    let config =
        VmConfig::default_for_name("../escape", root.path().to_str().unwrap(), 1, "x86_64");
    assert!(manager
        .start_vm(config)
        .await
        .unwrap_err()
        .to_string()
        .contains("name"));
    assert!(!root.path().join("vm").exists());
}

#[test]
fn credentials_are_explicit_and_revocation_removes_old_files_without_touching_disks() {
    let root = tempfile::tempdir().unwrap();
    let vm_dir = root.path().join("vm/test");
    std::fs::create_dir_all(vm_dir.join("secrets")).unwrap();
    std::fs::write(vm_dir.join("disk.qcow2"), b"disk").unwrap();
    std::fs::write(vm_dir.join("secrets/DASHBOARD_ADMIN_PASSWORD"), b"old").unwrap();
    let grants = BTreeMap::from([("VM_GIT_TOKEN".into(), "explicit".into())]);
    secrets_inject::inject_secrets("test", root.path(), &grants).unwrap();
    assert_eq!(
        std::fs::read_to_string(vm_dir.join("secrets/VM_GIT_TOKEN")).unwrap(),
        "explicit"
    );
    assert!(!vm_dir.join("secrets/DASHBOARD_ADMIN_PASSWORD").exists());
    secrets_inject::inject_secrets("test", root.path(), &BTreeMap::new()).unwrap();
    assert!(!vm_dir.join("secrets/VM_GIT_TOKEN").exists());
    assert_eq!(std::fs::read(vm_dir.join("disk.qcow2")).unwrap(), b"disk");
}

#[test]
fn invalid_credential_key_is_rejected_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let grants = BTreeMap::from([("../outside".into(), "value".into())]);
    assert!(secrets_inject::inject_secrets("test", root.path(), &grants).is_err());
    assert!(!root.path().join("vm").exists());
}

#[cfg(unix)]
#[test]
fn credential_directory_cannot_follow_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("vm/test")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("vm/test/secrets")).unwrap();
    let grants = BTreeMap::from([("VM_TOKEN".into(), "value".into())]);
    assert!(secrets_inject::inject_secrets("test", root.path(), &grants).is_err());
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[test]
fn socket_selection_is_explicit_and_resets_stale_socket_paths() {
    let mut config = VmConfig::default_for_name("test", "/configured/data", 1, "x86_64");
    config.set_socket_mode("tcp").unwrap();
    assert!(config.qmp_socket_path.is_none());
    assert_eq!(config.qmp_connect_addr(), "127.0.0.1:44410");
    config.set_socket_mode("unix").unwrap();
    assert_eq!(
        config.qmp_socket_path.as_deref(),
        Some("/configured/data/vm/test/qmp.sock")
    );
    assert!(config.set_socket_mode("invalid").is_err());
}

#[tokio::test]
async fn existing_guest_is_reattached_through_qmp_without_starting_qemu() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.write_all(b"{\"QMP\":{}}\n").await.unwrap();
        let mut reader = BufReader::new(socket);
        let mut request = String::new();
        reader.read_line(&mut request).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request).unwrap()["execute"],
            "qmp_capabilities"
        );
        reader
            .get_mut()
            .write_all(b"{\"return\":{}}\n")
            .await
            .unwrap();
        request.clear();
        reader.read_line(&mut request).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request).unwrap()["execute"],
            "query-name"
        );
        reader
            .get_mut()
            .write_all(b"{\"return\":{\"name\":\"existing\"}}\n")
            .await
            .unwrap();
    });
    let manager = VmManager::new(root.path().to_str().unwrap());
    let mut config =
        VmConfig::default_for_name("existing", root.path().to_str().unwrap(), 1, "x86_64");
    config.set_socket_mode("tcp").unwrap();
    config.qmp_port = port;
    config.serial_port = 0;
    let result = manager.start_vm(config).await.unwrap();
    assert!(result.contains("reattached"), "{result}");
    assert!(!root.path().join("vm/existing/disk.qcow2").exists());
    assert_eq!(manager.list_vms().await.len(), 1);
    server.await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_startup_kills_only_its_new_qemu_process() {
    use praxis_vm::VmPrograms;
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("fake-qemu");
    let pid_file = root.path().join("pid");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 60\n",
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let guest = root.path().join("vm/cancelled");
    std::fs::create_dir_all(&guest).unwrap();
    std::fs::write(guest.join("disk.qcow2"), "fixture disk").unwrap();
    let manager = VmManager::with_programs(
        root.path().to_str().unwrap(),
        VmPrograms {
            x86_64: script.to_str().unwrap().into(),
            aarch64: script.to_str().unwrap().into(),
            image: "must-not-run".into(),
        },
    );
    let mut config =
        VmConfig::default_for_name("cancelled", root.path().to_str().unwrap(), 1, "x86_64");
    config.set_socket_mode("unix").unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), manager.start_vm(config))
            .await
            .is_err()
    );
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let gone = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let status = tokio::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .unwrap();
            if !status.success() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "new QEMU process was left alive");
    assert!(manager.list_vms().await.is_empty());
    assert_eq!(
        std::fs::read_to_string(guest.join("disk.qcow2")).unwrap(),
        "fixture disk"
    );
}
