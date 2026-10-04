use praxis_vm::runtime::{VmRuntime, VmSettings};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn read_only_attachment_never_starts_or_prepares_a_missing_guest() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let runtime = VmRuntime::new(VmSettings {
        data_dir: data.to_string_lossy().into(),
        arch: "x86_64".into(),
        socket_mode: "tcp".into(),
        cpu_cores: 2,
        ram_mb: 512,
        disk_size: "1G".into(),
    })
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut config = runtime.default_config("missing").unwrap();
    config.qmp_port = port;
    assert!(!runtime.manager().attach_existing(config).await.unwrap());
    assert!(runtime.manager().list_vms().await.is_empty());
    assert!(!data.exists());
}

#[tokio::test]
async fn read_only_attachment_checks_qmp_identity_and_sends_no_credential_or_power_commands() {
    for actual in ["existing", "other"] {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let runtime = VmRuntime::new(VmSettings {
            data_dir: data.to_string_lossy().into(),
            arch: "x86_64".into(),
            socket_mode: "tcp".into(),
            cpu_cores: 2,
            ram_mb: 512,
            disk_size: "1G".into(),
        })
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let qmp = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(b"{\"QMP\":{}}\n").await.unwrap();
            let mut reader = BufReader::new(socket);
            for (operation, result) in [
                ("qmp_capabilities", serde_json::json!({})),
                ("query-name", serde_json::json!({"name":actual})),
            ] {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&line).unwrap()["execute"],
                    operation
                );
                reader
                    .get_mut()
                    .write_all(format!("{}\n", serde_json::json!({"return":result})).as_bytes())
                    .await
                    .unwrap();
            }
        });
        let mut config = runtime.default_config("existing").unwrap();
        config.qmp_port = port;
        config.serial_port = 0;
        let attached = runtime.manager().attach_existing(config).await;
        if actual == "existing" {
            assert!(attached.unwrap());
            assert_eq!(runtime.manager().list_vms().await[0]["name"], "existing");
        } else {
            assert!(attached.is_err());
            assert!(runtime.manager().list_vms().await.is_empty());
        }
        qmp.await.unwrap();
        assert!(!data.exists());
    }
}
