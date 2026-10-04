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
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        praxis_plugin_api::web::PRIVATE_HEADER,
        "private-host-nonce".parse().unwrap(),
    );
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
