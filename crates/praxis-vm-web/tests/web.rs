use praxis_vm::runtime::{VmRuntime, VmSettings};
use praxis_vm_web::WebServer;
use serde_json::json;
use std::sync::Arc;

async fn fixture() -> (tempfile::TempDir, WebServer, reqwest::Client, String) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        VmRuntime::new(VmSettings {
            data_dir: dir.path().join("data").to_string_lossy().into(),
            arch: "x86_64".into(),
            socket_mode: "unix".into(),
            cpu_cores: 2,
            ram_mb: 512,
            disk_size: "1G".into(),
        })
        .unwrap(),
    );
    let server = WebServer::start(runtime, "private-host-nonce".into())
        .await
        .unwrap();
    let url = format!("http://127.0.0.1:{}", server.info().port);
    (dir, server, reqwest::Client::new(), url)
}

#[tokio::test]
async fn internal_api_and_assets_require_host_auth_and_do_not_create_guests() {
    let (dir, server, client, url) = fixture().await;
    for path in [
        "/api/plugins/vm",
        "/plugins/vm/ui/page.html",
        "/plugins/vm/novnc/core/rfb.js",
    ] {
        assert_eq!(
            client
                .get(format!("{url}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let response = client
            .get(format!("{url}{path}"))
            .header("x-praxis-plugin-key", "private-host-nonce")
            .header("x-praxis-principal", "operator")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert!(!response
            .text()
            .await
            .unwrap()
            .contains("private-host-nonce"));
    }
    let info = serde_json::to_string(&server.info()).unwrap();
    assert!(!info.contains("private-host-nonce"));
    assert!(!dir.path().join("data").exists());
}

#[tokio::test]
async fn invalid_admin_input_is_rejected_before_effects() {
    let (dir, _server, client, url) = fixture().await;
    for input in [
        json!({"name":"../escape"}),
        json!({"cpu_cores":0}),
        json!({"ram_mb":1}),
        json!({"disk_size":"$(touch secret)"}),
        json!({"user":"forged"}),
        json!({"arch":"wrong"}),
    ] {
        let response = client
            .post(format!("{url}/api/plugins/vm/start"))
            .header("x-praxis-plugin-key", "private-host-nonce")
            .header("x-praxis-principal", "operator")
            .json(&input)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "accepted {input}: {}",
            response.status()
        );
    }
    assert!(!dir.path().join("data").exists());
}

#[tokio::test]
async fn packaged_viewer_assets_and_licenses_are_served_without_query_interpolation() {
    let (_dir, _server, client, url) = fixture().await;
    for path in [
        "ui/vm.js",
        "ui/vm.css",
        "ui/vnc.html?vm=%3Cscript%3Eevil%3C/script%3E",
        "novnc/vendor/pako/lib/zlib/inflate.js",
        "novnc/docs/LICENSE.MPL-2.0",
    ] {
        let response = client
            .get(format!("{url}/plugins/vm/{path}"))
            .header("x-praxis-plugin-key", "private-host-nonce")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert!(!response
            .text()
            .await
            .unwrap()
            .contains("<script>evil</script>"));
    }
    assert_eq!(
        client
            .get(format!("{url}/plugins/vm/missing"))
            .header("x-praxis-plugin-key", "private-host-nonce")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn stopping_web_service_releases_listener_without_removing_existing_storage() {
    let (dir, server, client, url) = fixture().await;
    std::fs::create_dir_all(dir.path().join("data/vm/existing")).unwrap();
    std::fs::write(dir.path().join("data/vm/existing/disk.qcow2"), b"preserved").unwrap();
    server.stop();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while client
            .get(format!("{url}/api/plugins/vm"))
            .send()
            .await
            .is_ok()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("data/vm/existing/disk.qcow2")).unwrap(),
        b"preserved"
    );
}

#[tokio::test]
async fn vnc_relays_binary_data_and_disconnects_without_stopping_a_reattached_guest() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        VmRuntime::new(VmSettings {
            data_dir: dir.path().to_string_lossy().into(),
            arch: "x86_64".into(),
            socket_mode: "tcp".into(),
            cpu_cores: 2,
            ram_mb: 512,
            disk_size: "1G".into(),
        })
        .unwrap(),
    );
    let qmp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let qmp_port = qmp.local_addr().unwrap().port();
    let qmp_task = tokio::spawn(async move {
        let (mut socket, _) = qmp.accept().await.unwrap();
        socket.write_all(b"{\"QMP\":{}}\n").await.unwrap();
        let mut reader = BufReader::new(socket);
        for reply in [
            b"{\"return\":{}}\n".as_slice(),
            b"{\"return\":{\"name\":\"existing\"}}\n".as_slice(),
        ] {
            reader.read_line(&mut String::new()).await.unwrap();
            reader.get_mut().write_all(reply).await.unwrap();
        }
    });
    let vnc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = runtime.default_config("existing").unwrap();
    config.qmp_port = qmp_port;
    config.serial_port = 0;
    config.vnc_port = vnc.local_addr().unwrap().port();
    runtime
        .guests()
        .authorize(
            &praxis_vm::guests::Principal::user("alice"),
            "existing",
            "vm_start",
            || Ok(config.clone()),
        )
        .await
        .unwrap();
    assert!(runtime
        .manager()
        .start_vm(config)
        .await
        .unwrap()
        .contains("reattached"));
    qmp_task.await.unwrap();
    let tcp_task = tokio::spawn(async move {
        let (mut socket, _) = vnc.accept().await.unwrap();
        socket.write_all(b"RFB 003.008\n").await.unwrap();
        let mut reply = [0u8; 3];
        socket.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [1, 2, 3]);
        socket.write_all(b"ack").await.unwrap();
        assert_eq!(socket.read(&mut [0u8; 1]).await.unwrap(), 0);
    });
    let web = WebServer::start(runtime.clone(), "private-host-nonce".into())
        .await
        .unwrap();
    let url = format!(
        "ws://127.0.0.1:{}/api/plugins/vm/vnc/ws?vm=existing",
        web.info().port
    );
    assert!(tokio_tungstenite::connect_async(&url).await.is_err());
    let with = |principal: &str| {
        let mut request = url.clone().into_client_request().unwrap();
        request.headers_mut().insert(
            praxis_plugin_api::web::PRIVATE_HEADER,
            "private-host-nonce".parse().unwrap(),
        );
        request.headers_mut().insert(
            praxis_plugin_api::web::PRINCIPAL_HEADER,
            principal.parse().unwrap(),
        );
        request
    };
    // Another user is refused before any VNC TCP connection is made.
    assert!(tokio_tungstenite::connect_async(with("user:mallory"))
        .await
        .is_err());
    let request = with("user:alice");
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap().into_data(),
        b"RFB 003.008\n"
    );
    socket.send(Message::Binary(vec![1, 2, 3])).await.unwrap();
    assert_eq!(socket.next().await.unwrap().unwrap().into_data(), b"ack");
    web.stop();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
        }
        tcp_task.await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(runtime.manager().list_vms().await[0]["status"], "running");
}

#[tokio::test]
async fn routes_enforce_guest_ownership_and_explicit_sharing() {
    let (dir, server, client, url) = fixture().await;
    let _ = server;
    let runtime_dir = dir.path().join("data");
    let get = |principal: &'static str| {
        client
            .get(format!("{url}/api/plugins/vm/guests"))
            .header("x-praxis-plugin-key", "private-host-nonce")
            .header("x-praxis-principal", principal)
    };
    // Missing principal fails closed.
    let response = client
        .get(format!("{url}/api/plugins/vm/guests"))
        .header("x-praxis-plugin-key", "private-host-nonce")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    // Seed a record owned by alice through the store (no QEMU needed).
    let store = praxis_vm::guests::GuestStore::new(runtime_dir.to_str().unwrap());
    let config =
        praxis_vm::VmConfig::default_for_name("g", runtime_dir.to_str().unwrap(), 1, "x86_64");
    store
        .authorize(
            &praxis_vm::guests::Principal::user("alice"),
            "g",
            "vm_start",
            || Ok(config),
        )
        .await
        .unwrap();
    let list = |r: reqwest::Response| async {
        r.json::<serde_json::Value>().await.unwrap()["guests"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(list(get("user:alice").send().await.unwrap()).await, 1);
    assert_eq!(list(get("user:bob").send().await.unwrap()).await, 0);
    assert_eq!(list(get("operator").send().await.unwrap()).await, 1);
    let post = |path: &str, principal: &str, body: serde_json::Value| {
        client
            .post(format!("{url}/api/plugins/vm/{path}"))
            .header("x-praxis-plugin-key", "private-host-nonce")
            .header("x-praxis-principal", principal)
            .json(&body)
    };
    for (path, body) in [
        ("stop", json!({"name":"g"})),
        ("clipboard/set", json!({"name":"g","content":"x"})),
        ("share", json!({"name":"g","user":"bob","grant":true})),
        ("owner", json!({"name":"g","owner":"bob"})),
    ] {
        assert_eq!(
            post(path, "user:bob", body).send().await.unwrap().status(),
            403,
            "{path}"
        );
    }
    assert_eq!(
        post("owner", "user:alice", json!({"name":"g","owner":"bob"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        post(
            "share",
            "user:alice",
            json!({"name":"g","user":"bob","grant":true})
        )
        .send()
        .await
        .unwrap()
        .status(),
        200
    );
    assert_eq!(list(get("user:bob").send().await.unwrap()).await, 1);
    // Shared users may use but not manage.
    assert_eq!(
        post("stop", "user:bob", json!({"name":"g"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        post("stop", "user:bob", json!({"name":"missing"}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let forged = client
        .get(format!("{url}/api/plugins/vm/guests"))
        .header("x-praxis-plugin-key", "private-host-nonce")
        .header("x-praxis-principal", "admin")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 401);
}

#[tokio::test]
async fn restart_recovery_reattaches_through_persisted_endpoints_and_keeps_disks() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let dir = tempfile::tempdir().unwrap();
    let settings = || VmSettings {
        data_dir: dir.path().to_string_lossy().into(),
        arch: "x86_64".into(),
        socket_mode: "tcp".into(),
        cpu_cores: 2,
        ram_mb: 512,
        disk_size: "1G".into(),
    };
    // Simulate a guest started by a previous worker generation on a
    // non-default QMP port, interrupted mid-operation.
    let qmp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let qmp_port = qmp.local_addr().unwrap().port();
    {
        let before = VmRuntime::new(settings()).unwrap();
        let mut config = before.default_config("kept").unwrap();
        config.qmp_port = qmp_port;
        config.serial_port = 0;
        let alice = praxis_vm::guests::Principal::user("alice");
        before
            .guests()
            .authorize(&alice, "kept", "vm_start", || Ok(config.clone()))
            .await
            .unwrap();
        before
            .guests()
            .begin("kept", "vm_snapshot_restore", &alice)
            .await
            .unwrap();
        std::fs::write(dir.path().join("vm/kept/disk.qcow2"), b"committed").unwrap();
    }
    let fixture = tokio::spawn(async move {
        let (mut socket, _) = qmp.accept().await.unwrap();
        socket.write_all(b"{\"QMP\":{}}\n").await.unwrap();
        let mut reader = BufReader::new(socket);
        for reply in [
            &b"{\"return\":{}}\n"[..],
            &b"{\"return\":{\"name\":\"kept\"}}\n"[..],
        ] {
            reader.read_line(&mut String::new()).await.unwrap();
            reader.get_mut().write_all(reply).await.unwrap();
        }
        reader
    });
    let after = VmRuntime::new(settings()).unwrap();
    let report = after.recover().await;
    let _keep_open = fixture.await.unwrap();
    assert_eq!(report["guests"][0]["attached"], true, "{report}");
    assert_eq!(
        report["guests"][0]["interrupted"]["operation"],
        "vm_snapshot_restore"
    );
    let record = after.guests().load("kept").unwrap().unwrap();
    assert_eq!(record.owner, "alice");
    assert!(record.pending.is_none());
    assert_eq!(after.manager().list_vms().await[0]["status"], "running");
    assert_eq!(
        std::fs::read(dir.path().join("vm/kept/disk.qcow2")).unwrap(),
        b"committed"
    );
    // Ownership survives the restart.
    let bob = praxis_vm::guests::Principal::user("bob");
    assert!(after
        .authorize(&bob, "kept", praxis_vm::guests::Access::Use)
        .await
        .is_err());
}

/// Real-QEMU smoke test (opt-in): set PRAXIS_REAL_QEMU_ISO to a bootable ISO.
/// Covers boot, keyboard/mouse, screenshots, VNC, ownership denial and
/// restart recovery against an actual QEMU process (TCG is fine).
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn real_qemu_guest_boot_input_vnc_screenshots_and_restart_recovery() {
    use futures_util::StreamExt;
    use praxis_vm::guests::Principal;
    use praxis_vm::runtime::VmPreferences;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let iso = std::env::var("PRAXIS_REAL_QEMU_ISO").expect("set PRAXIS_REAL_QEMU_ISO");
    let dir = tempfile::tempdir().unwrap();
    let settings = || VmSettings {
        data_dir: dir.path().to_string_lossy().into(),
        arch: "x86_64".into(),
        socket_mode: "unix".into(),
        cpu_cores: 1,
        ram_mb: 512,
        disk_size: "1G".into(),
    };
    let prefs = VmPreferences::default();
    let grants: std::collections::BTreeMap<String, String> = Default::default();
    let alice = Principal::user("alice");
    let bob = Principal::user("bob");
    let first = Arc::new(VmRuntime::new(settings()).unwrap());
    let run = |rt: Arc<VmRuntime>, who: Principal, op: &'static str, args: serde_json::Value| {
        let prefs = prefs.clone();
        let grants: std::collections::BTreeMap<String, String> = grants.clone();
        async move {
            rt.execute_as(&who, "task", op, &args, &prefs, &grants)
                .await
                .unwrap()
        }
    };
    let started = run(
        first.clone(),
        alice.clone(),
        "vm_start",
        json!({"name":"real","iso_path":iso}),
    )
    .await;
    assert_eq!(started["outcome"], "succeeded", "{started}");
    let pid = first
        .guests()
        .load("real")
        .unwrap()
        .unwrap()
        .pid
        .expect("pid persisted");
    tokio::time::sleep(std::time::Duration::from_secs(20)).await; // let firmware/bootloader draw
    let shot = run(
        first.clone(),
        alice.clone(),
        "vm_screenshot",
        json!({"name":"real"}),
    )
    .await;
    let path = shot["screenshot_path"]
        .as_str()
        .expect("screenshot path")
        .to_owned();
    assert!(std::fs::read(&path).unwrap().starts_with(b"\x89PNG"));
    for (op, args) in [
        ("vm_keys", json!({"name":"real","keys":"a"})),
        (
            "vm_mouse",
            json!({"name":"real","action":"move_absolute","x":100,"y":100}),
        ),
        (
            "vm_mouse",
            json!({"name":"real","action":"click","x":100,"y":100}),
        ),
    ] {
        let out = run(first.clone(), alice.clone(), op, args).await;
        assert_eq!(out["outcome"], "succeeded", "{op}: {out}");
    }
    let denied = run(
        first.clone(),
        bob.clone(),
        "vm_screenshot",
        json!({"name":"real"}),
    )
    .await;
    assert_eq!(denied["outcome"], "denied");
    // VNC through the package web listener, as owner and as stranger.
    let web = WebServer::start(first.clone(), "private-host-nonce".into())
        .await
        .unwrap();
    let url = format!(
        "ws://127.0.0.1:{}/api/plugins/vm/vnc/ws?vm=real",
        web.info().port
    );
    let with = |who: &str| {
        let mut r = url.clone().into_client_request().unwrap();
        r.headers_mut().insert(
            praxis_plugin_api::web::PRIVATE_HEADER,
            "private-host-nonce".parse().unwrap(),
        );
        r.headers_mut().insert(
            praxis_plugin_api::web::PRINCIPAL_HEADER,
            who.parse().unwrap(),
        );
        r
    };
    assert!(tokio_tungstenite::connect_async(with("user:bob"))
        .await
        .is_err());
    let (mut socket, _) = tokio_tungstenite::connect_async(with("user:alice"))
        .await
        .unwrap();
    let banner = socket.next().await.unwrap().unwrap().into_data();
    assert!(banner.starts_with(b"RFB 003."), "{banner:?}");
    drop(socket);
    web.stop();
    // Simulate worker restart/upgrade: drop the runtime without stopping QEMU.
    drop(first);
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "guest must survive"
    );
    let second = Arc::new(VmRuntime::new(settings()).unwrap());
    let report = second.recover().await;
    assert_eq!(report["guests"][0]["attached"], true, "{report}");
    let shot = run(
        second.clone(),
        alice.clone(),
        "vm_screenshot",
        json!({"name":"real"}),
    )
    .await;
    assert_eq!(shot["outcome"], "succeeded", "{shot}");
    let stop = run(
        second.clone(),
        bob.clone(),
        "vm_stop",
        json!({"name":"real"}),
    )
    .await;
    assert_eq!(stop["outcome"], "denied");
    let _ = second.manager().force_stop_vm("real").await;
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
    assert!(
        dir.path().join("vm/real/disk.qcow2").exists(),
        "disk preserved"
    );
}
