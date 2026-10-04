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
        controls: vec!["autostart".into(), "web_info".into(), "capture".into()],
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

async fn cli(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    cli_with_mode(root, args, "unix").await
}

async fn cli_with_mode(root: &std::path::Path, args: &[&str], mode: &str) -> std::process::Output {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_praxis-vm-service"))
        .arg("--cli")
        .args(args)
        .current_dir(root)
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = initialization(&root.join("data"));
    input.as_object_mut().unwrap().remove("web_token");
    input["socket_mode"] = mode.into();
    let mut stdin = child.stdin.take().unwrap();
    // Help/parse errors can exit before consuming configuration.
    let _ = stdin.write_all(&serde_json::to_vec(&input).unwrap()).await;
    drop(stdin);
    child.wait_with_output().await.unwrap()
}

#[tokio::test]
async fn standalone_vm_cli_owns_help_and_read_only_commands_without_host_services() {
    let root = tempfile::tempdir().unwrap();
    let help = cli(root.path(), &["--help"]).await;
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help = String::from_utf8(help.stdout).unwrap();
    for command in ["start", "stop", "status", "screenshot", "disk", "shell"] {
        assert!(help.contains(command), "{help}");
    }
    for command in [vec!["status"], vec!["list-isos"], vec!["disk", "list"]] {
        let output = cli(root.path(), &command).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!root.path().join("data").exists());
}

#[tokio::test]
async fn standalone_vm_cli_reports_failures_and_rejects_guest_paths_before_effects() {
    let root = tempfile::tempdir().unwrap();
    for command in [
        vec!["cd", "--name", "missing", "/explicit/install.iso"],
        vec!["start", "--name", "../outside"],
        vec!["start", "--cpu", "0"],
        vec!["snapshots", "--name", "../outside"],
        vec!["disk", "info", "/missing.qcow2"],
        vec!["shell", "--timeout", "0", "true"],
    ] {
        let output = cli(root.path(), &command).await;
        assert!(!output.status.success(), "accepted {command:?}");
        assert!(!output.stderr.is_empty(), "no diagnostic for {command:?}");
    }
    assert!(!root.path().join("data").exists());
}

#[tokio::test]
async fn screenshot_control_uses_typed_host_preferences_without_creating_a_guest() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let spec = launch(root.path());
    let client = Client::launch(&spec, initialization(&data)).await.unwrap();
    let input = json!({"name":"missing", "preferences":{"keyboard_layout":"de", "screenshot_enabled":false, "screenshot_limit":3}});
    assert!(client
        .control("capture", input.clone())
        .await
        .unwrap()
        .is_null());
    for extra in [
        json!({"name":"../outside","preferences":input["preferences"]}),
        json!({"name":"missing","preferences":input["preferences"],"data_dir":"/wrong"}),
    ] {
        assert!(client.control("capture", extra).await.is_err());
    }
    assert!(client.available());
    assert!(!data.exists());
    client.shutdown().await.unwrap();
}

#[cfg(unix)]
static QMP_FIXTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(unix)]
async fn qmp_fixture(data: &std::path::Path, name: &str) -> tokio::task::JoinHandle<Vec<String>> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let directory = data.join("vm").join(name);
    std::fs::create_dir_all(&directory).unwrap();
    // The engine's current default TCP endpoint. Serialize these independent
    // executable tests until guest metadata exposes per-guest TCP endpoints.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:44410")
        .await
        .unwrap();
    let name = name.to_owned();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.write_all(b"{\"QMP\":{}}\n").await.unwrap();
        let mut reader = BufReader::new(socket);
        let mut calls = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let command: serde_json::Value = serde_json::from_str(&line).unwrap();
            let operation = command["execute"].as_str().unwrap();
            calls.push(operation.to_owned());
            let result = match operation {
                "qmp_capabilities" | "quit" | "system_powerdown" => json!({}),
                "query-name" => json!({"name":name}),
                "screendump" => {
                    std::fs::write(
                        command["arguments"]["filename"].as_str().unwrap(),
                        b"P6\n1 1\n255\n\xff\x00\x00",
                    )
                    .unwrap();
                    json!({})
                }
                "blockdev-change-medium" => {
                    assert_eq!(command["arguments"]["device"], "cd0");
                    assert!(std::path::Path::new(
                        command["arguments"]["filename"].as_str().unwrap()
                    )
                    .is_file());
                    json!({})
                }
                other => panic!("unexpected QMP operation {other}"),
            };
            reader
                .get_mut()
                .write_all(format!("{}\n", json!({"return":result})).as_bytes())
                .await
                .unwrap();
        }
        calls
    })
}

#[cfg(unix)]
#[tokio::test]
async fn worker_captures_real_qmp_frames_as_png_and_obeys_the_host_retention_limit() {
    let _guard = QMP_FIXTURE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let qmp = qmp_fixture(&data, "existing").await;
    let disk = data.join("vm/existing/disk.qcow2");
    std::fs::write(&disk, b"committed-disk").unwrap();
    let mut settings = initialization(&data);
    settings["socket_mode"] = "tcp".into();
    let client = Client::launch(&launch(root.path()), settings)
        .await
        .unwrap();
    let result = client
        .invoke(caller(), "vm_start", json!({"name":"existing"}))
        .await
        .unwrap();
    assert_eq!(result["scope"]["kind"], "guest");
    assert_eq!(result["verified"], false);
    assert!(result["output"].as_str().unwrap().contains("reattached"));
    let screenshots = data.join("vm/existing/screenshots");
    std::fs::create_dir_all(&screenshots).unwrap();
    for name in ["legacy_a.ppm", "legacy_b.ppm"] {
        std::fs::write(screenshots.join(name), b"old-image").unwrap();
    }
    for _ in 0..3 {
        let path = client.control("capture", json!({"name":"existing", "preferences":{"keyboard_layout":"de", "screenshot_enabled":false, "screenshot_limit":2}})).await.unwrap();
        assert!(std::fs::read(path.as_str().unwrap())
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n"));
    }
    assert_eq!(
        std::fs::read_dir(data.join("vm/existing/screenshots"))
            .unwrap()
            .count(),
        2
    );
    let latest = client.control("capture", json!({"name":"existing", "preferences":{"keyboard_layout":"us", "screenshot_enabled":false, "screenshot_limit":0}})).await.unwrap();
    assert!(std::path::Path::new(latest.as_str().unwrap()).is_file());
    assert_eq!(std::fs::read_dir(&screenshots).unwrap().count(), 1);
    client.shutdown().await.unwrap();
    let calls = qmp.await.unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "screendump")
            .count(),
        4
    );
    assert!(!calls
        .iter()
        .any(|call| matches!(call.as_str(), "quit" | "system_powerdown")));
    assert_eq!(std::fs::read(disk).unwrap(), b"committed-disk");
}

#[cfg(unix)]
#[tokio::test]
async fn standalone_cli_delivers_a_png_from_an_existing_guest_without_revoking_credentials() {
    let _guard = QMP_FIXTURE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let qmp = qmp_fixture(&data, "existing").await;
    let credential = data.join("vm/existing/secrets/VM_TOKEN");
    std::fs::create_dir_all(credential.parent().unwrap()).unwrap();
    std::fs::write(&credential, b"existing-guest-token").unwrap();
    let output_path = root.path().join("operator screenshot.png");
    let output = cli_with_mode(
        root.path(),
        &[
            "screenshot",
            "--name",
            "existing",
            "--output",
            output_path.to_str().unwrap(),
        ],
        "tcp",
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(std::fs::read(output_path)
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    let calls = qmp.await.unwrap();
    assert_eq!(calls, ["qmp_capabilities", "query-name", "screendump"]);
    assert_eq!(std::fs::read(credential).unwrap(), b"existing-guest-token");
}

#[cfg(unix)]
#[tokio::test]
async fn standalone_cli_force_stop_requests_quit_and_reports_only_the_acknowledged_request() {
    let _guard = QMP_FIXTURE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let qmp = qmp_fixture(&root.path().join("data"), "existing").await;
    let output = cli_with_mode(
        root.path(),
        &["stop", "--name", "existing", "--force"],
        "tcp",
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("completion is not yet verified"));
    assert_eq!(
        qmp.await.unwrap(),
        ["qmp_capabilities", "query-name", "quit"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn standalone_cli_cd_sends_the_actual_qmp_media_operation() {
    let _guard = QMP_FIXTURE.lock().await;
    let root = tempfile::tempdir().unwrap();
    let qmp = qmp_fixture(&root.path().join("data"), "existing").await;
    let iso = root.path().join("explicit install.iso");
    std::fs::write(&iso, b"fixture-media").unwrap();
    let output = cli_with_mode(
        root.path(),
        &["cd", "--name", "existing", iso.to_str().unwrap()],
        "tcp",
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        qmp.await.unwrap(),
        ["qmp_capabilities", "query-name", "blockdev-change-medium"]
    );
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
