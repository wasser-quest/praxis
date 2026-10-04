use crate::{WebState, ASSETS};
use axum::http::HeaderMap;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use praxis_plugin_api::web::PRINCIPAL_HEADER;
use praxis_vm::guests::{Access, Denied, GuestRecord, Principal};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};

pub(crate) fn router() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/plugins/vm", get(list_vm_status))
        .route("/api/plugins/vm/start", post(vm_start))
        .route("/api/plugins/vm/stop", post(vm_stop))
        .route("/api/plugins/vm/reboot", post(vm_reboot))
        .route("/api/plugins/vm/snapshot", post(vm_snapshot))
        .route("/api/plugins/vm/shared-folder", post(vm_shared_folder))
        .route("/api/plugins/vm/cd", post(vm_cd))
        .route("/api/plugins/vm/clipboard/set", post(vm_clipboard_set))
        .route("/api/plugins/vm/clipboard/get", get(vm_clipboard_get))
        .route("/api/plugins/vm/guests", get(list_guests))
        .route("/api/plugins/vm/share", post(vm_share))
        .route("/api/plugins/vm/owner", post(vm_owner))
        .route("/api/plugins/vm/vnc", get(viewer))
        .route("/api/plugins/vm/vnc/ws", get(vnc_ws))
        .route("/plugins/vm/*path", get(asset))
}
async fn viewer() -> axum::response::Response {
    serve_asset("ui/vnc.html")
}
async fn asset(Path(path): Path<String>) -> axum::response::Response {
    serve_asset(&path)
}
fn serve_asset(path: &str) -> axum::response::Response {
    let filename = format!("plugins/vm/{path}");
    let Some(asset) = ASSETS.iter().find(|asset| asset.path == filename) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    };
    (
        [(axum::http::header::CONTENT_TYPE, content_type)],
        asset.bytes,
    )
        .into_response()
}
fn guest_name(name: Option<String>) -> Result<String, StatusCode> {
    let name = name.unwrap_or_else(|| "praxis-vm".into());
    praxis_vm::validate_vm_name(&name).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(name)
}
/// Principal asserted by the host proxy. Requests without one are rejected,
/// so a misconfigured host fails closed rather than as operator.
pub(crate) fn principal(headers: &HeaderMap) -> Result<Principal, StatusCode> {
    let value = headers
        .get(PRINCIPAL_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if value == "operator" {
        return Ok(Principal::operator());
    }
    match value.strip_prefix("user:") {
        Some(user) if !user.is_empty() && user.len() <= 128 => Ok(Principal::user(user)),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
fn denied(error: anyhow::Error) -> StatusCode {
    match error.downcast_ref::<Denied>() {
        Some(Denied::NotFound) => StatusCode::NOT_FOUND,
        Some(Denied::Forbidden) => StatusCode::FORBIDDEN,
        None => StatusCode::BAD_REQUEST,
    }
}
async fn guard(
    state: &WebState,
    headers: &HeaderMap,
    name: &str,
    access: Access,
) -> Result<(Principal, GuestRecord), StatusCode> {
    let principal = principal(headers)?;
    let record = state
        .runtime
        .authorize(&principal, name, access)
        .await
        .map_err(denied)?;
    Ok((principal, record))
}
fn valid_disk_size(size: &str) -> bool {
    let Some(digits) = size.strip_suffix(['M', 'G', 'T']) else {
        return false;
    };
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && digits.parse::<u64>().is_ok_and(|n| n > 0)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmStartRequest {
    pub name: Option<String>,
    pub cpu_cores: Option<u32>,
    pub ram_mb: Option<u32>,
    pub disk_size: Option<String>,
    pub iso_path: Option<String>,
    pub arch: Option<String>,
    pub keyboard_layout: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmStopRequest {
    pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmSnapshotRequest {
    pub snapshot_name: String,
    pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmSharedFolderRequest {
    pub host_path: String,
    pub mount_point: Option<String>,
    pub readonly: Option<bool>,
    pub name: Option<String>,
}

async fn list_vm_status(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = principal(&headers)?;
    let settings = state.runtime.settings();
    Ok(Json(
        serde_json::json!({"vms":state.runtime.list_for(&principal).await,"config":{
            "vm_enabled":true,"vm_compiled":true,"vm_cpu_cores":settings.cpu_cores,
            "vm_ram_mb":settings.ram_mb,"vm_disk_size":settings.disk_size,"vm_arch":settings.arch,
        }}),
    ))
}

async fn vm_start(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<VmStartRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runtime = &state.runtime;
    let config = runtime.settings();
    let manager = runtime.manager();

    if req.arch.as_ref().is_some_and(|arch| arch != &config.arch) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let name = guest_name(req.name)?;
    let principal = principal(&headers)?;
    let data_dir = config.data_dir.clone();
    // Unique per guest; recorded endpoints are reused below.
    let vnc_offset = state.runtime.guests().guest_names().len().max(1) as u16;

    let mut vm_config =
        praxis_vm::VmConfig::default_for_name(&name, &data_dir, vnc_offset, &config.arch);
    vm_config.cpu_cores = config.cpu_cores;
    vm_config.ram_mb = config.ram_mb;
    vm_config.disk_size = config.disk_size.clone();
    if let Some(cpu) = req.cpu_cores {
        vm_config.cpu_cores = cpu;
    }
    if let Some(ram) = req.ram_mb {
        vm_config.ram_mb = ram;
    }
    if let Some(size) = req.disk_size {
        vm_config.disk_size = size;
    }
    vm_config.iso_path = req.iso_path;

    // Manual administrator starts use explicit request settings, never a
    // default-user DB lookup or implicit credential grant.
    let keyboard_layout = req.keyboard_layout.unwrap_or_else(|| "us".into());
    if vm_config.cpu_cores == 0
        || vm_config.cpu_cores > 128
        || vm_config.ram_mb < 128
        || !valid_disk_size(&vm_config.disk_size)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    vm_config
        .set_socket_mode(&config.socket_mode)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    vm_config.keyboard_layout = praxis_vm::KeyboardLayout::from_str(&keyboard_layout);

    // Authorize (and claim a new name) only after input validation.
    let record = state
        .runtime
        .guests()
        .authorize(&principal, &name, "vm_start", || {
            runtime.default_config(&name)
        })
        .await
        .map_err(denied)?;
    vm_config.reuse_endpoints(&record.config);
    let guests = state.runtime.guests();
    guests
        .begin(&name, "vm_start", &principal)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let result = manager.start_vm(vm_config).await;
    let live = manager.instance_state(&name).await;
    guests
        .finish(
            &name,
            live.as_ref().map(|(c, _, _)| c),
            live.as_ref().map(|(_, p, _)| *p),
        )
        .await;
    match result {
        Ok(msg) => Ok(Json(serde_json::json!({"message": msg}))),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn vm_stop(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<VmStopRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runtime = &state.runtime;
    let manager = runtime.manager();

    let name = guest_name(req.name)?;
    guard(&state, &headers, &name, Access::Manage).await?;
    match manager.stop_vm(&name).await {
        Ok(msg) => Ok(Json(serde_json::json!({"message": msg}))),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn vm_reboot(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<VmStopRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runtime = &state.runtime;
    let manager = runtime.manager();

    let name = guest_name(req.name)?;
    guard(&state, &headers, &name, Access::Manage).await?;
    match manager.reboot_vm(&name).await {
        Ok(msg) => Ok(Json(serde_json::json!({"message": msg}))),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn vm_snapshot(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<VmSnapshotRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runtime = &state.runtime;
    let manager = runtime.manager();

    let name = guest_name(req.name)?;
    guard(&state, &headers, &name, Access::Use).await?;
    match manager.create_snapshot(&name, &req.snapshot_name).await {
        Ok(msg) => Ok(Json(serde_json::json!({"message": msg}))),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

async fn vm_shared_folder(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<VmSharedFolderRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runtime = &state.runtime;
    let manager = runtime.manager();

    let name = guest_name(req.name)?;
    guard(&state, &headers, &name, Access::Manage).await?;
    let tag = format!(
        "shared-{}",
        std::path::Path::new(&req.host_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );

    let folder = praxis_vm::SharedFolder {
        host_path: req.host_path,
        mount_tag: tag,
        mount_point: req.mount_point.unwrap_or_else(|| "/mnt/shared".to_string()),
        readonly: req.readonly.unwrap_or(false),
    };

    match manager.add_shared_folder(&name, folder).await {
        Ok(msg) => Ok(Json(serde_json::json!({"message": msg}))),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmCdRequest {
    pub name: Option<String>,
    pub iso_path: Option<String>,
}

async fn vm_cd(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<VmCdRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let runtime = &state.runtime;
    let manager = runtime.manager();

    let name = guest_name(req.name)?;
    guard(&state, &headers, &name, Access::Manage).await?;

    match manager.change_cd(&name, req.iso_path.as_deref()).await {
        Ok(msg) => Ok(Json(serde_json::json!({"message": msg}))),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipboardRequest {
    name: Option<String>,
    content: String,
}
async fn vm_clipboard_set(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<ClipboardRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let name = guest_name(req.name)?;
    guard(&state, &headers, &name, Access::Use).await?;
    state
        .runtime
        .manager()
        .clipboard_set(&name, &req.content)
        .await
        .map(|message| Json(serde_json::json!({"success":true,"message":message})))
        .map_err(|_| StatusCode::BAD_GATEWAY)
}
async fn vm_clipboard_get(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let name = guest_name(params.get("name").cloned())?;
    guard(&state, &headers, &name, Access::Use).await?;
    state
        .runtime
        .manager()
        .clipboard_get(&name)
        .await
        .map(|content| Json(serde_json::json!({"success":true,"content":content})))
        .map_err(|_| StatusCode::BAD_GATEWAY)
}
async fn list_guests(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = principal(&headers)?;
    Ok(Json(
        serde_json::json!({"guests": state.runtime.list_for(&principal).await}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShareRequest {
    name: String,
    user: String,
    grant: bool,
}
async fn vm_share(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<ShareRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = principal(&headers)?;
    let name = guest_name(Some(req.name))?;
    state
        .runtime
        .guests()
        .share(&principal, &name, &req.user, req.grant)
        .await
        .map(|record| Json(record.public()))
        .map_err(denied)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRequest {
    name: String,
    owner: String,
}
async fn vm_owner(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(req): Json<OwnerRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let principal = principal(&headers)?;
    let name = guest_name(Some(req.name))?;
    state
        .runtime
        .guests()
        .transfer(&principal, &name, &req.owner)
        .await
        .map(|record| Json(record.public()))
        .map_err(denied)
}

async fn vnc_ws(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Result<axum::response::Response, StatusCode> {
    let name = guest_name(params.get("vm").cloned())?;
    guard(&state, &headers, &name, Access::Use).await?;
    let info = state
        .runtime
        .manager()
        .get_vm_info(&name)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let port = info["vnc_port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port >= 5900)
        .ok_or(StatusCode::BAD_GATEWAY)?;
    Ok(ws
        .max_message_size(1024 * 1024)
        .max_frame_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _ = handle_vnc(socket, port, state.stop.clone()).await;
        })
        .into_response())
}
async fn handle_vnc(
    socket: axum::extract::ws::WebSocket,
    port: u16,
    stop: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    // Connect to QEMU VNC TCP port
    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await??;
    let (tcp_read, tcp_write) = tcp.into_split();

    let (ws_sink, ws_source) = socket.split();

    // TCP -> WebSocket (binary frames)
    let tcp_to_ws = async move {
        let mut reader = tokio::io::BufReader::new(tcp_read);
        let mut ws_sink = ws_sink;
        let mut buf = vec![0u8; 65536];
        loop {
            use tokio::io::AsyncReadExt;
            let n = match tokio::time::timeout(
                std::time::Duration::from_secs(55),
                reader.read(&mut buf),
            )
            .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => n,
                Ok(Err(_)) => break,
                Err(_) => {
                    if ws_sink
                        .send(axum::extract::ws::Message::Ping(vec![]))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
            };
            if ws_sink
                .send(axum::extract::ws::Message::Binary(buf[..n].to_vec()))
                .await
                .is_err()
            {
                break;
            }
        }
    };

    // WebSocket -> TCP (binary frames)
    let ws_to_tcp = async move {
        let mut writer = tokio::io::BufWriter::new(tcp_write);
        let mut ws_source = ws_source;
        while let Some(msg) = ws_source.next().await {
            match msg {
                Ok(axum::extract::ws::Message::Binary(data)) => {
                    use tokio::io::AsyncWriteExt;
                    if writer.write_all(&data).await.is_err() {
                        break;
                    }
                    let _ = writer.flush().await;
                }
                Ok(axum::extract::ws::Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = stop.cancelled() => {},
        _ = tcp_to_ws => {},
        _ = ws_to_tcp => {},
    }

    Ok(())
}
