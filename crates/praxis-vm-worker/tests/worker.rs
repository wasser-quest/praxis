use praxis_plugin_api::{CallContext, Client, LaunchSpec};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

fn operations() -> Vec<String> {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../plugins/vm/plugin.json")).unwrap();
    manifest["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().into())
        .collect()
}
fn launch(root: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from(env!("CARGO_BIN_EXE_praxis-vm-service")),
        args: vec!["--stdio".into()],
        cwd: root.into(),
        owner: "vm".into(),
        service: "vm".into(),
        operations: operations(),
        controls: vec!["autostart".into(), "web_info".into()],
        environment: BTreeMap::new(),
    }
}
fn initialization(root: &std::path::Path) -> serde_json::Value {
    json!({"data_dir":root.to_string_lossy(),"arch":"x86_64","socket_mode":"unix","cpu_cores":2,"ram_mb":512,"disk_size":"1G", "web_token":"test-host-private-nonce"})
}
fn caller() -> CallContext {
    CallContext {
        user: "operator-selected-user".into(),
        session: "session".into(),
        task_id: "task".into(),
        call_id: "call".into(),
        owner: "vm".into(),
        registry_revision: "revision".into(),
        workspace: "/pinned/host/project".into(),
        active_state: Some("working".into()),
        timeout_ms: 10000,
        attributes: json!({"preferences":{"keyboard_layout":"us","screenshot_enabled":false,"screenshot_limit":10},"grants":{}}),
        secrets: BTreeMap::new(),
    }
}

#[tokio::test]
async fn independently_launched_vm_worker_is_read_only_and_preserves_guest_scope() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let client = Client::launch(&launch(root.path()), initialization(&data))
        .await
        .unwrap();
    assert_eq!(client.health().await.unwrap()["healthy"], true);
    let result = client
        .invoke(
            caller(),
            "vm_shell",
            json!({"command":"true","user":"forged","workspace":"/wrong"}),
        )
        .await
        .unwrap();
    assert_eq!(result["scope"]["user"], "operator-selected-user");
    assert_eq!(result["scope"]["kind"], "guest");
    assert_eq!(result["verified"], false);
    assert_eq!(result["outcome"], "failed");
    assert!(!data.exists());
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn installed_worker_embeds_its_web_contribution_and_stops_it_on_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let client = Client::launch(&launch(root.path()), initialization(&data))
        .await
        .unwrap();
    let info: praxis_plugin_api::web::WebInfo =
        serde_json::from_value(client.control("web_info", json!({})).await.unwrap()).unwrap();
    info.validate("vm").unwrap();
    let http = reqwest::Client::new();
    for path in [
        info.descriptor.page.as_str(),
        info.descriptor.script.as_str(),
        "/plugins/vm/novnc/core/rfb.js",
        "/api/plugins/vm",
    ] {
        let url = format!("http://127.0.0.1:{}{path}", info.port);
        assert_eq!(http.get(&url).send().await.unwrap().status(), 401);
        let response = http
            .get(&url)
            .header(
                praxis_plugin_api::web::PRIVATE_HEADER,
                "test-host-private-nonce",
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(!response
            .text()
            .await
            .unwrap()
            .contains("test-host-private-nonce"));
    }
    assert!(!data.exists());
    client.shutdown().await.unwrap();
    assert!(http
        .get(format!("http://127.0.0.1:{}/api/plugins/vm", info.port))
        .send()
        .await
        .is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_worker_start_drops_the_owned_startup_child_and_preserves_disk() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let pid_path = root.path().join("guest.pid");
    let fake = bin.join("qemu-system-x86_64");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec /bin/sleep 60\n",
            pid_path.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let data = root.path().join("data");
    let disk = data.join("vm/temporary/disk.qcow2");
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"keep-existing-disk").unwrap();
    let mut spec = launch(root.path());
    spec.environment
        .insert("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()));
    let client = Arc::new(Client::launch(&spec, initialization(&data)).await.unwrap());
    let call = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .invoke(caller(), "vm_start", json!({"name":"temporary"}))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !pid_path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    call.abort();
    let _ = call.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.available() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.shutdown().await.unwrap();
    let pid = std::fs::read_to_string(pid_path).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while tokio::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .unwrap()
            .success()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(&disk).unwrap(), b"keep-existing-disk");
}
