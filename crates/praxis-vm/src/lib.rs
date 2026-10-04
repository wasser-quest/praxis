pub mod cli;
pub mod guests;
pub mod qmp;
pub mod runtime;
pub mod secrets_inject;
pub mod serial;
pub mod tools;

#[cfg(test)]
mod extraction_tests {
    #[test]
    fn vm_construction_is_read_only_until_explicit_start() {
        let root = tempfile::tempdir().unwrap();
        let _manager = super::VmManager::new(root.path().to_str().unwrap());
        assert!(!root.path().join("vm").exists());
        assert!(!root.path().join("shared").exists());
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VmStatus {
    Stopped,
    Running,
    Paused,
    Installing,
    Error,
}

impl std::fmt::Display for VmStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmStatus::Stopped => write!(f, "stopped"),
            VmStatus::Running => write!(f, "running"),
            VmStatus::Paused => write!(f, "paused"),
            VmStatus::Installing => write!(f, "installing"),
            VmStatus::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VmArch {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64")]
    Aarch64,
}

impl VmArch {
    pub fn qemu_binary(&self) -> &str {
        match self {
            VmArch::X86_64 => "qemu-system-x86_64",
            VmArch::Aarch64 => "qemu-system-aarch64",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "aarch64" => VmArch::Aarch64,
            _ => VmArch::X86_64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedFolder {
    pub host_path: String,
    pub mount_tag: String,
    pub mount_point: String,
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum KeyboardLayout {
    #[serde(rename = "us")]
    US,
    #[serde(rename = "de")]
    DE,
    #[serde(rename = "fr")]
    FR,
    #[serde(rename = "es")]
    ES,
    #[serde(rename = "it")]
    IT,
    #[serde(rename = "gb")]
    GB,
}

impl KeyboardLayout {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "de" | "german" | "qwertz" => KeyboardLayout::DE,
            "fr" | "french" | "azerty" => KeyboardLayout::FR,
            "es" | "spanish" => KeyboardLayout::ES,
            "it" | "italian" => KeyboardLayout::IT,
            "gb" | "uk" | "british" => KeyboardLayout::GB,
            _ => KeyboardLayout::US,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            KeyboardLayout::US => "us",
            KeyboardLayout::DE => "de",
            KeyboardLayout::FR => "fr",
            KeyboardLayout::ES => "es",
            KeyboardLayout::IT => "it",
            KeyboardLayout::GB => "gb",
        }
    }
}

impl std::fmt::Display for KeyboardLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Firmware {
    #[serde(rename = "bios")]
    Bios,
    #[serde(rename = "uefi")]
    Uefi,
}

impl Firmware {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "uefi" | "efi" | "ovmf" => Firmware::Uefi,
            _ => Firmware::Bios,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Firmware::Bios => "bios",
            Firmware::Uefi => "uefi",
        }
    }
}

impl std::fmt::Display for Firmware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub name: String,
    pub arch: VmArch,
    pub cpu_cores: u32,
    pub ram_mb: u32,
    pub disk_path: String,
    pub disk_size: String,
    pub iso_path: Option<String>,
    pub vnc_port: u16,
    pub qmp_port: u16,
    pub serial_port: u16,
    pub qmp_socket_path: Option<String>, // Unix socket path (only when socket_mode=unix)
    pub serial_socket_path: Option<String>, // Unix socket path (only when socket_mode=unix)
    pub socket_mode: String,             // "unix" or "tcp"
    pub shared_folders: Vec<SharedFolder>,
    pub network_mode: String,
    pub audio_enabled: bool,
    pub keyboard_layout: KeyboardLayout,
    pub firmware: Firmware,
}

/// Detect the best acceleration method for the current OS
fn detect_acceleration() -> &'static str {
    if cfg!(target_os = "linux") {
        // Check if KVM is available
        if std::path::Path::new("/dev/kvm").exists() {
            return "kvm";
        }
    } else if cfg!(target_os = "windows") {
        // WHPX (Windows Hypervisor Platform) or HAXM
        return "whpx";
    } else if cfg!(target_os = "macos") {
        return "hvf";
    }
    "tcg" // fallback: software emulation (slow but works everywhere)
}

/// Find OVMF firmware for UEFI boot
fn find_ovmf_firmware() -> Option<String> {
    let paths = if cfg!(target_os = "windows") {
        vec![
            r"C:\Program Files\qemu\share\edk2-x86_64-code.fd",
            r"C:\Program Files\qemu\share\OVMF_CODE.fd",
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            "/opt/homebrew/share/qemu/edk2-x86_64-code.fd",
            "/usr/local/share/qemu/edk2-x86_64-code.fd",
        ]
    } else {
        vec![
            "/usr/share/OVMF/OVMF_CODE.fd",
            "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
            "/usr/share/qemu/OVMF_CODE.fd",
        ]
    };
    for p in paths {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}

impl VmConfig {
    pub fn default_for_name(name: &str, data_dir: &str, vnc_offset: u16, arch: &str) -> Self {
        let vm_dir = format!("{}/vm/{}", data_dir, name);
        let base_port = 44400u16 + (vnc_offset * 10);
        let socket_mode = if cfg!(target_os = "linux") {
            "unix"
        } else {
            "tcp"
        }
        .to_string();

        let (qmp_socket_path, serial_socket_path) = if socket_mode == "unix" {
            (
                Some(format!("{}/qmp.sock", vm_dir)),
                Some(format!("{}/serial.sock", vm_dir)),
            )
        } else {
            (None, None)
        };

        Self {
            name: name.to_string(),
            arch: VmArch::from_str(arch),
            cpu_cores: 2,
            ram_mb: 4096,
            disk_path: format!("{}/disk.qcow2", vm_dir),
            disk_size: "40G".to_string(),
            iso_path: None,
            vnc_port: 5900 + vnc_offset,
            qmp_port: base_port,
            serial_port: base_port + 1,
            qmp_socket_path,
            serial_socket_path,
            socket_mode,
            shared_folders: Vec::new(),
            network_mode: "user".to_string(),
            audio_enabled: false,
            keyboard_layout: KeyboardLayout::US,
            firmware: Firmware::Bios,
        }
    }

    /// Host-provided selection; the backend never reads process environment.
    pub fn set_socket_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        anyhow::ensure!(matches!(mode, "unix" | "tcp"), "Invalid VM socket mode");
        if mode == "unix" {
            let directory = std::path::Path::new(&self.disk_path)
                .parent()
                .ok_or_else(|| anyhow::anyhow!("VM disk has no parent directory"))?;
            self.qmp_socket_path = Some(directory.join("qmp.sock").to_string_lossy().into_owned());
            self.serial_socket_path =
                Some(directory.join("serial.sock").to_string_lossy().into_owned());
        } else {
            self.qmp_socket_path = None;
            self.serial_socket_path = None;
        }
        self.socket_mode = mode.into();
        Ok(())
    }

    /// Keep a guest's recorded disk and connection endpoints across restarts
    /// and package upgrades. Socket paths are reused only for the same mode.
    pub fn reuse_endpoints(&mut self, saved: &VmConfig) {
        self.disk_path = saved.disk_path.clone();
        self.vnc_port = saved.vnc_port;
        self.qmp_port = saved.qmp_port;
        self.serial_port = saved.serial_port;
        if saved.socket_mode == self.socket_mode {
            self.qmp_socket_path = saved.qmp_socket_path.clone();
            self.serial_socket_path = saved.serial_socket_path.clone();
        }
        for folder in &saved.shared_folders {
            if !self
                .shared_folders
                .iter()
                .any(|f| f.mount_tag == folder.mount_tag)
            {
                self.shared_folders.push(folder.clone());
            }
        }
    }

    pub fn qmp_connect_addr(&self) -> String {
        if self.socket_mode == "unix" {
            self.qmp_socket_path
                .clone()
                .unwrap_or_else(|| format!("{}.sock", self.name))
        } else {
            format!("127.0.0.1:{}", self.qmp_port)
        }
    }

    pub fn serial_connect_addr(&self) -> String {
        if self.socket_mode == "unix" {
            self.serial_socket_path
                .clone()
                .unwrap_or_else(|| format!("{}.serial.sock", self.name))
        } else {
            format!("127.0.0.1:{}", self.serial_port)
        }
    }
}

/// Runtime state of a running VM
pub struct VmInstance {
    pub config: VmConfig,
    pub status: VmStatus,
    pub process: Option<tokio::process::Child>,
    pub qmp: Option<qmp::QmpClient>,
    pub serial: Option<serial::SerialShell>,
    pub pid: Option<u32>,
    pub current_iso: Option<String>,
    pub keyboard_layout: KeyboardLayout,
}

/// VM Manager — manages all VM instances
#[derive(Clone)]
pub struct VmManager {
    instances: Arc<RwLock<HashMap<String, VmInstance>>>,
    data_dir: String,
    programs: VmPrograms,
}

/// Executables are selected by the operator/host, never by tool operands.
#[derive(Clone)]
pub struct VmPrograms {
    pub x86_64: String,
    pub aarch64: String,
    pub image: String,
}
impl Default for VmPrograms {
    fn default() -> Self {
        Self {
            x86_64: "qemu-system-x86_64".into(),
            aarch64: "qemu-system-aarch64".into(),
            image: "qemu-img".into(),
        }
    }
}
impl VmPrograms {
    fn qemu(&self, arch: &VmArch) -> &str {
        match arch {
            VmArch::X86_64 => &self.x86_64,
            VmArch::Aarch64 => &self.aarch64,
        }
    }
}

impl VmManager {
    pub fn new(data_dir: &str) -> Self {
        Self::with_programs(data_dir, VmPrograms::default())
    }

    pub fn with_programs(data_dir: &str, programs: VmPrograms) -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            data_dir: data_dir.to_string(),
            programs,
        }
    }

    pub fn data_dir(&self) -> &str {
        &self.data_dir
    }

    /// Explicit storage preparation; constructing a backend is read-only.
    pub fn prepare_storage(&self) -> anyhow::Result<()> {
        for path in [
            self.vm_dir(),
            self.iso_dir(),
            format!("{}/vm/disks", self.data_dir),
            format!("{}/shared", self.data_dir),
        ] {
            std::fs::create_dir_all(path)?;
        }
        Ok(())
    }

    /// Get the base VM storage directory
    pub fn vm_dir(&self) -> String {
        format!("{}/vm", self.data_dir)
    }

    /// Get the ISO directory
    pub fn iso_dir(&self) -> String {
        format!("{}/vm/isos", self.data_dir)
    }

    /// List available ISOs in the iso directory
    pub fn list_isos(&self) -> Vec<serde_json::Value> {
        let iso_dir = self.iso_dir();
        let mut isos = Vec::new();

        // Scan the default iso dir
        Self::scan_iso_dir(&iso_dir, &mut isos);

        // Also scan context-configured paths
        let installation_disks = self.get_installation_disks();
        for disk in &installation_disks {
            if let Some(path) = disk.get("path").and_then(|v| v.as_str()) {
                let name = disk
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        std::path::Path::new(path)
                            .file_stem()
                            .and_then(|n| n.to_str())
                    })
                    .unwrap_or("unknown");
                let exists = std::path::Path::new(path).exists();
                isos.push(serde_json::json!({
                    "name": name,
                    "path": path,
                    "exists": exists,
                    "source": "context"
                }));
            }
        }

        isos
    }

    fn scan_iso_dir(dir: &str, isos: &mut Vec<serde_json::Value>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_stem().unwrap_or_default().to_string_lossy();
                let ext = path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                if ext == "iso" || ext == "img" || ext == "qcow2" {
                    let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                    isos.push(serde_json::json!({
                        "name": name,
                        "path": path.to_string_lossy(),
                        "size_bytes": size,
                        "exists": true,
                        "source": "iso_dir"
                    }));
                }
            }
        }
    }

    /// Get installation_disks from context (stored in a file for persistence)
    fn get_installation_disks(&self) -> Vec<serde_json::Value> {
        let config_path = format!("{}/vm/installation_disks.json", self.data_dir);
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(disks) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
                return disks;
            }
        }
        Vec::new()
    }

    /// Save installation_disks to context file
    pub fn set_installation_disks(&self, disks: &[serde_json::Value]) -> anyhow::Result<()> {
        let config_path = format!("{}/vm/installation_disks.json", self.data_dir);
        let content = serde_json::to_string_pretty(disks)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    /// Add an ISO path to the installation disks list
    pub fn add_installation_disk(&self, name: &str, path: &str) -> anyhow::Result<()> {
        let mut disks = self.get_installation_disks();
        // Check if already exists
        if disks
            .iter()
            .any(|d| d.get("path").and_then(|v| v.as_str()) == Some(path))
        {
            return Ok(());
        }
        disks.push(serde_json::json!({ "name": name, "path": path }));
        self.set_installation_disks(&disks)
    }

    /// Remove an ISO from the installation disks list
    pub fn remove_installation_disk(&self, name_or_path: &str) -> anyhow::Result<()> {
        let mut disks = self.get_installation_disks();
        disks.retain(|d| {
            d.get("name").and_then(|v| v.as_str()) != Some(name_or_path)
                && d.get("path").and_then(|v| v.as_str()) != Some(name_or_path)
        });
        self.set_installation_disks(&disks)
    }

    /// Connect only to the configured existing QMP endpoint. This never starts
    /// QEMU, creates storage, rewrites credentials or guesses process ownership.
    pub async fn attach_existing(&self, config: VmConfig) -> anyhow::Result<bool> {
        self.attach_with_grants(config, None).await
    }

    async fn attach_with_grants(
        &self,
        config: VmConfig,
        grants: Option<&std::collections::BTreeMap<String, String>>,
    ) -> anyhow::Result<bool> {
        validate_vm_name(&config.name)?;
        let mut instances = self.instances.write().await;
        if instances
            .get(&config.name)
            .is_some_and(|vm| vm.status == VmStatus::Running)
        {
            return Ok(true);
        }
        // Reattach an existing guest instead of matching/killing arbitrary processes.
        // Startup does not delete disks or terminate an existing VM.
        if let Ok(Ok(mut qmp)) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            qmp::QmpClient::connect(&config.qmp_connect_addr()),
        )
        .await
        {
            if matches!(
                tokio::time::timeout(std::time::Duration::from_secs(2), qmp.negotiate()).await,
                Ok(Ok(()))
            ) {
                let actual_name =
                    tokio::time::timeout(std::time::Duration::from_secs(2), qmp.query_name())
                        .await??;
                anyhow::ensure!(
                    actual_name == config.name,
                    "Existing QMP endpoint belongs to a different VM"
                );
                if let Some(grants) = grants {
                    secrets_inject::inject_secrets(
                        &config.name,
                        std::path::Path::new(&self.data_dir),
                        grants,
                    )?;
                }
                let serial = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    serial::SerialShell::connect(&config.serial_connect_addr()),
                )
                .await
                .ok()
                .and_then(Result::ok);
                instances.insert(
                    config.name.clone(),
                    VmInstance {
                        config: config.clone(),
                        status: VmStatus::Running,
                        process: None,
                        qmp: Some(qmp),
                        serial,
                        pid: None,
                        current_iso: config.iso_path.clone(),
                        keyboard_layout: config.keyboard_layout.clone(),
                    },
                );
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Start a VM with the given config
    pub async fn start_vm(&self, config: VmConfig) -> anyhow::Result<String> {
        self.start_vm_with_grants(config, &std::collections::BTreeMap::new())
            .await
    }

    pub async fn start_vm_with_grants(
        &self,
        config: VmConfig,
        grants: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<String> {
        validate_vm_name(&config.name)?;
        anyhow::ensure!(
            config.cpu_cores > 0 && config.cpu_cores <= 128 && config.ram_mb >= 128,
            "Invalid VM resources"
        );
        anyhow::ensure!(config.vnc_port >= 5900, "Invalid VNC port");
        secrets_inject::validate_grants(grants)?;
        let instances = self.instances.read().await;
        if let Some(inst) = instances.get(&config.name) {
            if inst.status == VmStatus::Running {
                return Ok(format!("VM '{}' is already running", config.name));
            }
        }

        drop(instances);
        if self
            .attach_with_grants(config.clone(), Some(grants))
            .await?
        {
            return Ok(format!("VM '{}' reattached", config.name));
        }
        let mut instances = self.instances.write().await;
        // Concurrent callers may have attached while this caller was connecting.
        if instances
            .get(&config.name)
            .is_some_and(|vm| vm.status == VmStatus::Running)
        {
            return Ok(format!("VM '{}' is already running", config.name));
        }
        let vm_dir = format!("{}/vm/{}", self.data_dir, config.name);
        let qmp_sock = format!("{}/qmp.sock", vm_dir);
        let serial_sock = format!("{}/serial.sock", vm_dir);
        secrets_inject::inject_secrets(&config.name, std::path::Path::new(&self.data_dir), grants)?;
        self.prepare_storage()?;
        // Clean up stale sockets
        let _ = std::fs::remove_file(&qmp_sock);
        let _ = std::fs::remove_file(&serial_sock);

        // Resolve all paths to absolute (QEMU needs absolute paths for sockets)
        let abs_data_dir = std::fs::canonicalize(&self.data_dir)
            .unwrap_or_else(|_| std::path::PathBuf::from(&self.data_dir));
        let abs_data_dir_str = abs_data_dir.to_string_lossy().to_string();
        let vm_dir = format!("{}/vm/{}", abs_data_dir_str, config.name);
        std::fs::create_dir_all(&vm_dir)?;

        // Fix relative paths in config
        let mut config = config.clone();
        if !std::path::Path::new(&config.disk_path).is_absolute() {
            config.disk_path = format!("{}/disk.qcow2", vm_dir);
        }
        if let Some(ref path) = config.qmp_socket_path {
            if !std::path::Path::new(path).is_absolute() {
                config.qmp_socket_path = Some(format!("{}/qmp.sock", vm_dir));
            }
        }
        if let Some(ref path) = config.serial_socket_path {
            if !std::path::Path::new(path).is_absolute() {
                config.serial_socket_path = Some(format!("{}/serial.sock", vm_dir));
            }
        }

        // Create disk if it doesn't exist
        if !std::path::Path::new(&config.disk_path).exists() {
            tracing::info!(disk = %config.disk_path, size = %config.disk_size, "Creating VM disk");
            let output = tokio::process::Command::new(&self.programs.image)
                .args([
                    "create",
                    "-f",
                    "qcow2",
                    &config.disk_path,
                    &config.disk_size,
                ])
                .output()
                .await?;
            if !output.status.success() {
                anyhow::bail!(
                    "Failed to create disk: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            tracing::info!("Disk created: {}", config.disk_path);
        }

        // Build QEMU command
        let args = self.build_qemu_args(&config);

        let vnc_port = config.vnc_port;
        let vnc_display_num = vnc_port - 5900;

        tracing::info!("=== VM START SEQUENCE ===");
        tracing::info!(vm = %config.name, arch = ?config.arch, binary = %config.arch.qemu_binary());
        tracing::info!("VNC: display :{} (port {})", vnc_display_num, vnc_port);
        tracing::info!("QMP socket: {:?}", config.qmp_socket_path);
        tracing::info!("Serial socket: {:?}", config.serial_socket_path);
        tracing::info!("Disk: {}", config.disk_path);
        tracing::info!("ISO: {:?}", config.iso_path);
        tracing::info!(
            "QEMU command: {} {}",
            config.arch.qemu_binary(),
            args.join(" ")
        );
        tracing::info!("========================");

        // Start QEMU process
        let mut cmd = tokio::process::Command::new(self.programs.qemu(&config.arch));
        cmd.args(&args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!(
                "Failed to start QEMU ({}): {}. Is qemu-system-x86_64 installed?",
                config.arch.qemu_binary(),
                e
            )
        })?;

        let mut child = StartupChild(Some(child));
        let pid = child.id();
        tracing::info!(vm = %config.name, pid = ?pid, "QEMU process started (PID: {:?})", pid);
        tracing::info!(
            "VNC available at: localhost:{} (display :{})",
            vnc_port,
            vnc_display_num
        );
        tracing::info!("VNC viewer: http://localhost:1337/vnc");

        // Check if QEMU died immediately (within 500ms)
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        match child.try_wait() {
            Ok(Some(status)) => {
                // QEMU exited already — capture stderr
                let stderr = child.stderr.take();
                let mut stderr_output = String::new();
                if let Some(mut stderr) = stderr {
                    use tokio::io::AsyncReadExt;
                    let _ = stderr.read_to_string(&mut stderr_output).await;
                }
                anyhow::bail!(
                    "QEMU exited immediately with status {}. Command: {} {}\nStderr: {}",
                    status,
                    config.arch.qemu_binary(),
                    args.join(" "),
                    stderr_output
                );
            }
            Ok(None) => {
                tracing::info!("QEMU process is running (PID: {:?})", pid);
            }
            Err(e) => {
                tracing::warn!("Could not check QEMU status: {}", e);
            }
        }

        // Wait for QEMU to start listening, with retries
        let qmp_addr = config.qmp_connect_addr();
        let serial_addr = config.serial_connect_addr();
        let mut qmp = None;
        let mut serial = None;

        for attempt in 1..=5 {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

            // Check if QEMU is still alive
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stderr = child.stderr.take();
                    let mut stderr_output = String::new();
                    if let Some(mut stderr) = stderr {
                        use tokio::io::AsyncReadExt;
                        let _ = stderr.read_to_string(&mut stderr_output).await;
                    }
                    anyhow::bail!(
                        "QEMU died during startup (exit {}). Stderr:\n{}",
                        status,
                        stderr_output
                    );
                }
                _ => {}
            }

            tracing::info!(attempt = attempt, qmp = %qmp_addr, "Trying QMP connection...");
            if qmp.is_none() {
                match qmp::QmpClient::connect(&qmp_addr).await {
                    Ok(mut client) => {
                        let _ = client.negotiate().await;
                        tracing::info!("QMP connected for VM '{}' at {}", config.name, qmp_addr);
                        qmp = Some(client);
                    }
                    Err(e) => {
                        tracing::debug!("QMP attempt {} failed: {}", attempt, e);
                    }
                }
            }

            if serial.is_none() {
                tracing::info!(attempt = attempt, serial = %serial_addr, "Trying Serial connection...");
                match serial::SerialShell::connect(&serial_addr).await {
                    Ok(shell) => {
                        tracing::info!(
                            "Serial connected for VM '{}' at {}",
                            config.name,
                            serial_addr
                        );
                        serial = Some(shell);
                    }
                    Err(e) => {
                        tracing::debug!("Serial attempt {} failed: {}", attempt, e);
                    }
                }
            }

            if qmp.is_some() && serial.is_some() {
                break;
            }
        }

        let instance = VmInstance {
            config: config.clone(),
            status: VmStatus::Running,
            process: Some(child.commit()),
            qmp,
            serial,
            pid,
            current_iso: config.iso_path.clone(),
            keyboard_layout: config.keyboard_layout.clone(),
        };

        instances.insert(config.name.clone(), instance);

        Ok(format!(
            "VM '{}' started (PID: {:?}, VNC: {}, QMP: {}, Serial: {})",
            config.name,
            pid,
            config.vnc_port,
            config.qmp_connect_addr(),
            config.serial_connect_addr()
        ))
    }

    pub async fn request_shutdown(&self, name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM not found"))?;
        instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP unavailable"))?
            .system_powerdown()
            .await?;
        Ok(format!(
            "Shutdown requested for VM '{}'; completion is not yet verified",
            name
        ))
    }

    pub async fn force_stop_vm(&self, name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM not found"))?;
        instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP unavailable"))?
            .quit()
            .await?;
        Ok(format!(
            "Quit requested for VM '{}'; completion is not yet verified",
            name
        ))
    }

    /// Stop a VM gracefully
    pub async fn stop_vm(&self, name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status == VmStatus::Stopped {
            return Ok(format!("VM '{}' is not running", name));
        }

        // Try QMP powerdown first (graceful)
        if let Some(ref mut qmp) = instance.qmp {
            let _ = qmp.system_powerdown().await;
            tracing::info!("Sent system_powerdown to VM '{}'", name);

            // Wait up to 10s for graceful shutdown
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Some(ref mut proc) = instance.process {
                    match proc.try_wait() {
                        Ok(Some(_)) => {
                            instance.status = VmStatus::Stopped;
                            instance.process = None;
                            instance.qmp = None;
                            instance.serial = None;
                            return Ok(format!("VM '{}' stopped gracefully", name));
                        }
                        _ => continue,
                    }
                }
            }
        }

        // Force kill if graceful failed
        if let Some(ref mut proc) = instance.process {
            proc.kill().await?;
            instance.status = VmStatus::Stopped;
            instance.process = None;
            instance.qmp = None;
            instance.serial = None;
            return Ok(format!("VM '{}' force-stopped", name));
        }

        Ok(format!("VM '{}' was not running", name))
    }

    /// Reboot a VM using QMP system_reset (preserves CD/media state)
    pub async fn reboot_vm(&self, name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running (status: {})", name, instance.status);
        }

        let qmp = instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP not connected for VM '{}'", name))?;

        qmp.system_reset().await?;
        tracing::info!("Sent system_reset to VM '{}'", name);

        Ok(format!("VM '{}' rebooted", name))
    }

    /// Write file content directly inside the VM using guest-agent (qemu-ga)
    pub async fn write_file(
        &self,
        name: &str,
        path: &str,
        content: &str,
    ) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        // Try guest-agent direct file write (fastest, most reliable)
        if let Some(ref mut qmp) = instance.qmp {
            match qmp.guest_file_write_all(path, content).await {
                Ok(_) => {
                    return Ok(format!(
                        "File written via guest-agent: {} ({} bytes)",
                        path,
                        content.len()
                    ))
                }
                Err(e) => tracing::debug!(
                    "guest-agent write_file failed for ({}): {} - falling back",
                    path,
                    e
                ),
            }
        }

        // Fallback: 9p shared folder + serial cp (existing method)
        let data_dir = &self.data_dir;
        let filename = std::path::Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let transfer_path = format!("{}/shared/{}", data_dir, filename);
        std::fs::write(&transfer_path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write shared folder copy: {}", e))?;

        let copy_result = self
            .shell_exec_with_instances(
                &mut *instances,
                name,
                &format!(
                    "cp -- {} {}",
                    shell_quote(&format!("/mnt/shared/{}", filename)),
                    shell_quote(path)
                ),
                10,
            )
            .await;
        match copy_result {
            Ok(_) => Ok(format!("File written via shared folder: {} ({} bytes)", path, content.len())),
            Err(e) => anyhow::bail!(
                "File written to shared folder but copy in VM failed: {}. File available at /mnt/shared/{}",
                e,
                filename
            ),
        }
    }

    /// Read file content from VM using guest-agent
    pub async fn read_file(&self, name: &str, path: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        if let Some(ref mut qmp) = instance.qmp {
            match qmp.guest_file_read_all(path).await {
                Ok(content) => return Ok(content),
                Err(e) => tracing::debug!("guest-agent read_file failed for ({}): {}", path, e),
            }
        }

        // Fallback: use serial cat
        self.shell_exec_with_instances(&mut *instances, name, &format!("cat {}", path), 10)
            .await
    }

    /// Execute a shell command in a VM and return the output
    pub async fn shell_exec(
        &self,
        name: &str,
        command: &str,
        timeout_secs: u64,
    ) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        self.shell_exec_with_instances(&mut *instances, name, command, timeout_secs)
            .await
    }

    async fn shell_exec_with_instances(
        &self,
        instances: &mut HashMap<String, VmInstance>,
        name: &str,
        command: &str,
        timeout_secs: u64,
    ) -> anyhow::Result<String> {
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        // Tier 1: Try guest-agent (QMP guest-exec) — most reliable
        if let Some(ref mut qmp) = instance.qmp {
            let shell_cmd = if cfg!(target_os = "linux") {
                "/bin/sh"
            } else {
                "cmd.exe"
            };
            let shell_args = if cfg!(target_os = "linux") {
                vec!["-c", command]
            } else {
                vec!["/c", command]
            };
            match qmp.guest_exec_full(shell_cmd, &shell_args).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    tracing::debug!("guest-agent exec failed: {} - falling back to serial", e)
                }
            }
        }

        // Tier 2: Use serial shell for command execution
        if let Some(ref mut serial) = instance.serial {
            match serial.execute(command, timeout_secs).await {
                Ok(output) => return Ok(output),
                Err(e) => tracing::debug!("Serial execute failed: {} - trying reconnect", e),
            }
        }

        // Tier 3: Reconnect serial and retry
        let serial_addr = instance.config.serial_connect_addr();
        match crate::serial::SerialShell::connect(&serial_addr).await {
            Ok(mut shell) => {
                let result = shell.execute(command, timeout_secs).await;
                instance.serial = Some(shell);
                result
            }
            Err(e) => {
                // Tier 4: Last resort — type via QMP keystrokes (no output)
                if let Some(ref mut qmp) = instance.qmp {
                    let layout = instance.keyboard_layout.clone();
                    for ch in command.chars() {
                        let events = Self::char_to_qmp_events(ch, &layout)?;
                        for (qcode, down) in events {
                            qmp.send_key_event(&qcode, down).await?;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    qmp.send_key_event("ret", true).await?;
                    qmp.send_key_event("ret", false).await?;
                    tokio::time::sleep(std::time::Duration::from_millis(
                        timeout_secs.min(5) * 1000,
                    ))
                    .await;
                    return Ok(format!("Command '{}' sent via QMP keystrokes (no return output available). Output visible in VNC.", command));
                }
                anyhow::bail!("Neither serial nor QMP connected: {}", e)
            }
        }
    }

    /// Send raw keystrokes to a VM (all input via QMP so it's visible in VNC)
    pub async fn send_keys(&self, name: &str, keys: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        let qmp = instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP not connected for VM '{}'", name))?;

        let layout = instance.keyboard_layout.clone();

        // Check if it's a single special key name
        let key_events = if Self::is_special_key(keys) {
            Self::keys_to_qmp(keys)?
        } else {
            // It's a string - send each character individually
            Self::string_to_qmp(keys, &layout)?
        };

        tracing::debug!(event_count = key_events.len(), "Sending QMP key events");
        for (qcode, down) in &key_events {
            if qcode == "delay" {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
            qmp.send_key_event(qcode, *down).await?;
            // Delay between key events for reliability
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        // Small pause after the full key sequence
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        Ok(format!("Sent keys: {}", keys))
    }

    /// Send keys with optional layout override
    pub async fn send_keys_with_layout(
        &self,
        name: &str,
        keys: &str,
        layout_override: Option<&str>,
    ) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        let qmp = instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP not connected for VM '{}'", name))?;

        // Use override layout, or instance layout
        let layout = layout_override
            .map(|s| KeyboardLayout::from_str(s))
            .unwrap_or(instance.keyboard_layout.clone());

        // Check if it's a single special key name
        let key_events = if Self::is_special_key(keys) {
            Self::keys_to_qmp(keys)?
        } else {
            // It's a string - send each character individually
            Self::string_to_qmp(keys, &layout)?
        };

        tracing::debug!(event_count = key_events.len(), layout = %layout, "Sending QMP key events");
        for (qcode, down) in &key_events {
            if qcode == "delay" {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
            qmp.send_key_event(qcode, *down).await?;
            // Delay between key events for reliability
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        // Small pause after the full key sequence
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        Ok(format!("Sent keys: {}", keys))
    }

    /// Check if the input is a known special key name
    fn is_special_key(key: &str) -> bool {
        // Check for modifier combinations like "ctrl+a", "alt+tab", etc.
        if key.contains('+') {
            return true;
        }
        matches!(
            key,
            "enter"
                | "return"
                | "esc"
                | "escape"
                | "tab"
                | "backspace"
                | "space"
                | "capslock"
                | "caps_lock"
                | "numlock"
                | "num_lock"
                | "scrolllock"
                | "scroll_lock"
                | "print"
                | "print_screen"
                | "prtsc"
                | "sysrq"
                | "arrow_up"
                | "up"
                | "arrow_down"
                | "down"
                | "arrow_left"
                | "left"
                | "arrow_right"
                | "right"
                | "pageup"
                | "page_up"
                | "pagedown"
                | "page_down"
                | "home"
                | "end"
                | "insert"
                | "delete"
                | "del"
                | "f1"
                | "f2"
                | "f3"
                | "f4"
                | "f5"
                | "f6"
                | "f7"
                | "f8"
                | "f9"
                | "f10"
                | "f11"
                | "f12"
                | "f13"
                | "f14"
                | "f15"
                | "f16"
                | "f17"
                | "f18"
                | "f19"
                | "f20"
                | "f21"
                | "f22"
                | "f23"
                | "f24"
                | "kp0"
                | "numpad0"
                | "kp1"
                | "numpad1"
                | "kp2"
                | "numpad2"
                | "kp3"
                | "numpad3"
                | "kp4"
                | "numpad4"
                | "kp5"
                | "numpad5"
                | "kp6"
                | "numpad6"
                | "kp7"
                | "numpad7"
                | "kp8"
                | "numpad8"
                | "kp9"
                | "numpad9"
                | "kp_enter"
                | "numpad_enter"
                | "kp_plus"
                | "numpad_plus"
                | "kp_minus"
                | "numpad_minus"
                | "kp_multiply"
                | "numpad_multiply"
                | "kp_divide"
                | "numpad_divide"
                | "kp_dot"
                | "numpad_dot"
                | "ctrl"
                | "left_ctrl"
                | "right_ctrl"
                | "alt"
                | "left_alt"
                | "right_alt"
                | "shift"
                | "left_shift"
                | "right_shift"
                | "super"
                | "meta"
                | "win"
                | "left_meta"
                | "right_meta"
        )
    }

    /// Convert a string to QMP key events, handling special characters, modifier combos, and macros
    fn string_to_qmp(s: &str, layout: &KeyboardLayout) -> anyhow::Result<Vec<(String, bool)>> {
        let mut events = Vec::new();
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            // Check for macro syntax: (key)count e.g. (arrowdown)5 or (enter)3
            if chars[i] == '(' {
                // Find closing paren
                if let Some(end_paren) = chars[i..].iter().position(|&c| c == ')') {
                    let key_name: String = chars[i + 1..i + end_paren].iter().collect();
                    let after_paren = i + end_paren + 1;

                    // Parse optional count after closing paren
                    let mut count_str = String::new();
                    let mut j = after_paren;
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        count_str.push(chars[j]);
                        j += 1;
                    }
                    let count: usize = if count_str.is_empty() {
                        1
                    } else {
                        count_str.parse().unwrap_or(1)
                    };

                    // Generate key events count times
                    let key_events = Self::keys_to_qmp(&key_name.to_lowercase())?;
                    for _ in 0..count {
                        events.extend(key_events.clone());
                        // Small delay between repeats
                        events.push(("delay".into(), true));
                    }

                    i = j;
                    continue;
                }
            }

            // Check for modifier combinations like "ctrl+a", "alt+tab"
            let remaining: String = chars[i..].iter().collect();
            if remaining.contains('+') && !remaining.starts_with('+') {
                let parts: Vec<&str> = remaining.split('+').collect();
                let mut modifiers = Vec::new();
                let mut main_key = None;
                let mut consumed = 0;

                for (idx, part) in parts.iter().enumerate() {
                    let part_lower = part.to_lowercase();
                    match part_lower.as_str() {
                        "ctrl" | "control" => {
                            modifiers.push("ctrl");
                            consumed += part.len() + 1; // +1 for the '+'
                        }
                        "alt" => {
                            modifiers.push("alt");
                            consumed += part.len() + 1;
                        }
                        "shift" => {
                            modifiers.push("shift");
                            consumed += part.len() + 1;
                        }
                        "super" | "meta" | "win" => {
                            modifiers.push("meta_l");
                            consumed += part.len() + 1;
                        }
                        _ => {
                            main_key = Some(part_lower);
                            consumed += part.len();
                            break;
                        }
                    }
                }

                if !modifiers.is_empty() && main_key.is_some() {
                    // Press all modifiers down
                    for mod_key in &modifiers {
                        events.push((mod_key.to_string(), true));
                    }

                    // Press the main key
                    if let Some(key) = main_key {
                        let key_events = Self::keys_to_qmp(&key)?;
                        events.extend(key_events);
                    }

                    // Release all modifiers (in reverse order)
                    for mod_key in modifiers.iter().rev() {
                        events.push((mod_key.to_string(), false));
                    }

                    i += consumed;
                    continue;
                }
            }

            // Regular character
            let c = chars[i];
            let char_events = match c {
                '\n' | '\r' => Self::keys_to_qmp("enter")?,
                '\t' => Self::keys_to_qmp("tab")?,
                '\x08' => Self::keys_to_qmp("backspace")?,
                '\x1b' => Self::keys_to_qmp("esc")?,
                _ => Self::char_to_qmp_events(c, layout)?,
            };
            events.extend(char_events);
            i += 1;
        }

        // Filter out delay markers
        let final_events: Vec<(String, bool)> = events
            .into_iter()
            .filter(|(qcode, _)| qcode != "delay")
            .collect();

        Ok(final_events)
    }

    /// Send mouse input to the VM
    pub async fn send_mouse(
        &self,
        name: &str,
        action: &str,
        x: Option<i32>,
        y: Option<i32>,
        dx: Option<i32>,
        dy: Option<i32>,
        button: Option<i32>,
        scroll_vertical: Option<i32>,
        scroll_horizontal: Option<i32>,
    ) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        let qmp = instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP not connected for VM '{}'", name))?;

        match action {
            "move_absolute" => {
                let mx = x.unwrap_or(0);
                let my = y.unwrap_or(0);
                qmp.mouse_move_absolute(mx, my).await?;
                Ok(format!("Mouse moved to ({}, {})", mx, my))
            }
            "move_relative" => {
                let mdx = dx.unwrap_or(0);
                let mdy = dy.unwrap_or(0);
                qmp.mouse_move_relative(mdx, mdy).await?;
                Ok(format!("Mouse moved by ({}, {})", mdx, mdy))
            }
            "click" => {
                let btn = button.unwrap_or(0);
                qmp.mouse_button(btn, true).await?;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                qmp.mouse_button(btn, false).await?;
                let btn_name = match btn { 0 => "left", 1 => "middle", 2 => "right", _ => "left" };
                Ok(format!("Mouse {} clicked at ({}, {})", btn_name, x.unwrap_or(-1), y.unwrap_or(-1)))
            }
            "double_click" => {
                let btn = button.unwrap_or(0);
                for _ in 0..2 {
                    qmp.mouse_button(btn, true).await?;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    qmp.mouse_button(btn, false).await?;
                    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                }
                Ok(format!("Mouse double-clicked"))
            }
            "drag" => {
                // Move to start, press, move to end, release
                let sx = x.unwrap_or(0);
                let sy = y.unwrap_or(0);
                let ex = dx.unwrap_or(0);
                let ey = dy.unwrap_or(0);
                let btn = button.unwrap_or(0);
                qmp.mouse_move_absolute(sx, sy).await?;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                qmp.mouse_button(btn, true).await?;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                qmp.mouse_move_absolute(ex, ey).await?;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                qmp.mouse_button(btn, false).await?;
                Ok(format!("Mouse dragged from ({},{}) to ({},{})", sx, sy, ex, ey))
            }
            "scroll" => {
                let sv = scroll_vertical.unwrap_or(0);
                let sh = scroll_horizontal.unwrap_or(0);
                qmp.mouse_scroll(sv, sh).await?;
                Ok(format!("Mouse scrolled (vertical: {}, horizontal: {})", sv, sh))
            }
            _ => anyhow::bail!(
                "Unknown mouse action: {}. Use: move_absolute, move_relative, click, double_click, drag, scroll",
                action
            ),
        }
    }

    /// Take a screenshot of the VM display
    pub async fn screenshot(&self, name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        let qmp = instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP not connected for VM '{}'", name))?;

        let vm_dir = format!("{}/vm/{}", self.data_dir, name);
        let screenshot_path = format!("{}/screenshot.ppm", vm_dir);

        tracing::debug!(vm = %name, path = %screenshot_path, "Taking screenshot via QMP screendump");

        // Small delay to let the display framebuffer update after input events
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        qmp.screendump(&screenshot_path).await?;

        let data = std::fs::read(&screenshot_path)?;
        tracing::debug!(vm = %name, size = data.len(), "Screenshot captured");

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);

        Ok(format!("data:image/ppm;base64,{}", b64))
    }

    /// Get VM status info
    pub async fn get_vm_info(&self, name: &str) -> anyhow::Result<serde_json::Value> {
        let instances = self.instances.read().await;
        let instance = instances
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        Ok(serde_json::json!({
            "name": instance.config.name,
            "status": instance.status.to_string(),
            "arch": instance.config.arch.qemu_binary(),
            "cpu_cores": instance.config.cpu_cores,
            "ram_mb": instance.config.ram_mb,
            "disk_path": instance.config.disk_path,
            "disk_size": instance.config.disk_size,
            "vnc_port": instance.config.vnc_port,
            "qmp_port": instance.config.qmp_port,
            "serial_port": instance.config.serial_port,
            "pid": instance.pid,
            "shared_folders": instance.config.shared_folders,
            "network_mode": instance.config.network_mode,
            "audio_enabled": instance.config.audio_enabled,
            "current_iso": instance.current_iso,
            "keyboard_layout": instance.keyboard_layout.to_string(),
        }))
    }

    pub async fn clipboard_set(&self, name: &str, text: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        if let Some(ref mut qmp) = instance.qmp {
            let res = qmp
                .guest_file_write_all("/tmp/praxis_clipboard.txt", text)
                .await;
            match res {
                Ok(_) => {
                    let _ = qmp.guest_exec_full("/bin/sh", &["-c", "cat /tmp/praxis_clipboard.txt | xclip -selection clipboard 2>/dev/null; cat /tmp/praxis_clipboard.txt | wl-copy 2>/dev/null; cat /tmp/praxis_clipboard.txt | xsel --clipboard --input 2>/dev/null; true"]).await;
                    return Ok(format!("Clipboard set ({} bytes)", text.len()));
                }
                Err(e) => tracing::debug!("guest-agent clipboard_set failed: {}", e),
            }
        }

        // Fallback: type via QMP keystrokes
        if let Some(ref mut qmp) = instance.qmp {
            let layout = instance.keyboard_layout.clone();
            for ch in text.chars() {
                let events = Self::char_to_qmp_events(ch, &layout)?;
                for (qcode, down) in events {
                    qmp.send_key_event(&qcode, down).await?;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            return Ok("Clipboard text typed via keystrokes".to_string());
        }

        anyhow::bail!("No QMP connection available")
    }

    pub async fn clipboard_get(&self, name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        if let Some(ref mut qmp) = instance.qmp {
            let _ = qmp.guest_exec_full("/bin/sh", &["-c", "(xclip -selection clipboard -o 2>/dev/null || wl-paste 2>/dev/null || xsel --clipboard --output 2>/dev/null) > /tmp/praxis_clipboard_out.txt; true"]).await;
            match qmp
                .guest_file_read_all("/tmp/praxis_clipboard_out.txt")
                .await
            {
                Ok(content) => return Ok(content),
                Err(e) => tracing::debug!("guest-agent clipboard_get failed: {}", e),
            }
        }

        anyhow::bail!("Could not read clipboard from VM")
    }

    /// Live configuration and process id of an attached guest, for persistence.
    pub async fn instance_state(&self, name: &str) -> Option<(VmConfig, Option<u32>, VmStatus)> {
        let instances = self.instances.read().await;
        instances
            .get(name)
            .map(|vm| (vm.config.clone(), vm.pid, vm.status.clone()))
    }

    /// List all VMs
    pub async fn list_vms(&self) -> Vec<serde_json::Value> {
        let instances = self.instances.read().await;
        instances
            .values()
            .map(|inst| {
                serde_json::json!({
                    "name": inst.config.name,
                    "status": inst.status.to_string(),
                    "pid": inst.pid,
                    "vnc_port": inst.config.vnc_port,
                    "current_iso": inst.current_iso,
                    "keyboard_layout": inst.keyboard_layout.to_string(),
                })
            })
            .collect()
    }

    /// Create a snapshot
    pub async fn create_snapshot(&self, name: &str, snapshot_name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        let qmp = instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP not connected"))?;

        qmp.blockdev_snapshotsync(snapshot_name).await?;
        Ok(format!(
            "Snapshot '{}' created for VM '{}'",
            snapshot_name, name
        ))
    }

    /// Insert or eject a CD/ISO in the VM
    pub async fn change_cd(&self, name: &str, iso_path: Option<&str>) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        let qmp = instance
            .qmp
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("QMP not connected for VM '{}'", name))?;

        match iso_path {
            Some(path) => {
                if !std::path::Path::new(path).exists() {
                    anyhow::bail!("ISO file not found: {}", path);
                }
                qmp.blockdev_change_medium("cd0", path).await?;
                instance.current_iso = Some(path.to_string());
                Ok(format!("CD '{}' inserted into VM '{}'", path, name))
            }
            None => {
                qmp.eject("cd0").await?;
                instance.current_iso = None;
                Ok(format!("CD ejected from VM '{}'", name))
            }
        }
    }

    /// Add a shared folder (requires VM restart)
    pub async fn add_shared_folder(
        &self,
        name: &str,
        folder: SharedFolder,
    ) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        instance.config.shared_folders.push(folder.clone());
        Ok(format!(
            "Shared folder added: {} -> {}. Restart VM to apply.",
            folder.host_path, folder.mount_point
        ))
    }

    /// Reconnect QMP/serial if VM is running but connections were lost
    pub async fn reconnect(&self, name: &str) -> anyhow::Result<String> {
        let mut instances = self.instances.write().await;
        let instance = instances
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", name))?;

        if instance.status != VmStatus::Running {
            anyhow::bail!("VM '{}' is not running", name);
        }

        if instance.qmp.is_none() {
            let qmp_addr = instance.config.qmp_connect_addr();
            match qmp::QmpClient::connect(&qmp_addr).await {
                Ok(mut client) => {
                    let _ = client.negotiate().await;
                    instance.qmp = Some(client);
                    tracing::info!("QMP reconnected for VM '{}'", name);
                }
                Err(e) => {
                    tracing::warn!("QMP reconnect failed: {}", e);
                }
            }
        }

        if instance.serial.is_none() {
            let serial_addr = instance.config.serial_connect_addr();
            match serial::SerialShell::connect(&serial_addr).await {
                Ok(shell) => {
                    instance.serial = Some(shell);
                    tracing::info!("Serial reconnected for VM '{}'", name);
                }
                Err(e) => {
                    tracing::warn!("Serial reconnect failed: {}", e);
                }
            }
        }

        Ok(format!("Reconnection attempted for VM '{}'", name))
    }

    fn build_qemu_args(&self, config: &VmConfig) -> Vec<String> {
        let accel = detect_acceleration();
        let cpu = if accel == "kvm" { "host" } else { "max" };

        let mut args = vec![
            "-name".to_string(),
            config.name.clone(),
            "-machine".to_string(),
            format!("q35,accel={}", accel),
            "-cpu".to_string(),
            cpu.to_string(),
            "-smp".to_string(),
            config.cpu_cores.to_string(),
            "-m".to_string(),
            config.ram_mb.to_string(),
        ];

        // Disk
        args.extend([
            "-drive".to_string(),
            format!("file={},format=qcow2,if=virtio", config.disk_path),
        ]);

        // Virtio-serial for guest-agent communication
        let abs_data = std::fs::canonicalize(&self.data_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| self.data_dir.clone());
        args.extend([
            "-device".to_string(),
            "virtio-serial-pci".to_string(),
            "-chardev".to_string(),
            format!(
                "socket,id=ga0,path={}/vm/{}/ga.sock,server=on,wait=off",
                abs_data, config.name
            ),
            "-device".to_string(),
            "virtserialport,chardev=ga0,name=org.qemu.guest_agent.0".to_string(),
        ]);

        // CD-ROM device (always present for hot-plug support)
        if let Some(ref iso) = config.iso_path {
            args.extend([
                "-drive".to_string(),
                format!("file={},readonly=on,media=cdrom,if=none,id=cd0", iso),
            ]);
        } else {
            args.extend([
                "-drive".to_string(),
                "if=none,id=cd0,media=cdrom".to_string(),
            ]);
        }
        args.extend(["-device".to_string(), "ide-cd,drive=cd0".to_string()]);
        if config.iso_path.is_some() {
            args.extend(["-boot".to_string(), "d".to_string()]);
        }

        // VNC with explicit VGA device
        let vnc_display = format!(":{}", config.vnc_port - 5900);
        args.extend(["-vnc".to_string(), vnc_display]);
        // Absolute pointer for vm_mouse move_absolute/click and noVNC; without
        // it QEMU rejects `input-send-event` abs events (found by real-QEMU test).
        args.extend([
            "-device".to_string(),
            "qemu-xhci,id=xhci".to_string(),
            "-device".to_string(),
            "usb-tablet,bus=xhci.0".to_string(),
        ]);

        // QMP: Unix socket or TCP based on config
        if config.socket_mode == "unix" {
            let qmp_path = config.qmp_socket_path.as_deref().unwrap_or("/tmp/qmp.sock");
            args.extend([
                "-qmp".to_string(),
                format!("unix:{},server,nowait", qmp_path),
            ]);
        } else {
            args.extend([
                "-qmp".to_string(),
                format!("tcp:127.0.0.1:{},server,nowait", config.qmp_port),
            ]);
        }

        // Serial: Unix socket or TCP based on config
        if config.socket_mode == "unix" {
            let serial_path = config
                .serial_socket_path
                .as_deref()
                .unwrap_or("/tmp/serial.sock");
            args.extend([
                "-chardev".to_string(),
                format!("socket,id=serial0,path={},server=on,wait=off", serial_path),
                "-serial".to_string(),
                "chardev:serial0".to_string(),
            ]);
        } else {
            args.extend([
                "-chardev".to_string(),
                format!(
                    "socket,id=serial0,host=127.0.0.1,port={},server=on,wait=off",
                    config.serial_port
                ),
                "-serial".to_string(),
                "chardev:serial0".to_string(),
            ]);
        }

        // Shared folders: 9p on Linux, SMB on Windows/macOS
        if cfg!(target_os = "linux") {
            for folder in &config.shared_folders {
                let security = if folder.readonly {
                    "passthrough,readonly"
                } else {
                    "passthrough"
                };
                let abs_host_path = std::fs::canonicalize(&folder.host_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| folder.host_path.clone());
                args.extend([
                    "-virtfs".to_string(),
                    format!(
                        "local,path={},mount_tag={},security_model={}",
                        abs_host_path, folder.mount_tag, security
                    ),
                ]);
            }
            // Secrets 9p mount — absolute path
            let abs_data = std::fs::canonicalize(&self.data_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| self.data_dir.clone());
            let secrets_dir = format!("{}/vm/{}/secrets", abs_data, config.name);
            args.extend([
                "-virtfs".to_string(),
                format!(
                    "local,path={},mount_tag=praxis-secrets,security_model=none,readonly",
                    secrets_dir
                ),
            ]);
        } else {
            // On Windows/macOS: use QEMU's built-in SMB server for shared folders
            // -smb <dir> exposes the directory as //10.0.2.4/qemu
            if !config.shared_folders.is_empty() {
                // Use the first shared folder as the SMB share
                args.extend([
                    "-smb".to_string(),
                    config.shared_folders[0].host_path.clone(),
                ]);
            }
        }

        // Network
        match config.network_mode.as_str() {
            "none" => {
                args.extend(["-net".to_string(), "none".to_string()]);
            }
            "bridge" => {
                args.extend([
                    "-device".to_string(),
                    "virtio-net,netdev=net0".to_string(),
                    "-netdev".to_string(),
                    "bridge,id=net0,br=br0".to_string(),
                ]);
            }
            _ => {
                args.extend([
                    "-device".to_string(),
                    "virtio-net,netdev=net0".to_string(),
                    "-netdev".to_string(),
                    "user,id=net0,hostfwd=tcp::2222-:22".to_string(),
                ]);
            }
        }

        // Audio: platform-aware
        if config.audio_enabled {
            if cfg!(target_os = "linux") {
                args.extend(["-audiodev".to_string(), "pa,id=audio0".to_string()]);
            } else if cfg!(target_os = "windows") {
                args.extend(["-audiodev".to_string(), "dsound,id=audio0".to_string()]);
            } else {
                args.extend(["-audiodev".to_string(), "sdl,id=audio0".to_string()]);
            }
            args.extend(["-device".to_string(), "AC97,audiodev=audio0".to_string()]);
        }

        // Firmware: UEFI (OVMF) or BIOS
        if config.firmware == Firmware::Uefi {
            if let Some(fw) = find_ovmf_firmware() {
                args.extend(["-bios".to_string(), fw]);
            } else {
                tracing::warn!("UEFI firmware requested but OVMF not found, falling back to BIOS");
            }
        }

        args
    }

    fn keys_to_qmp(key: &str) -> anyhow::Result<Vec<(String, bool)>> {
        // QEMU QCode strings from qemu-qmp-ref.html
        let press = |k: &str| vec![(k.to_string(), true), (k.to_string(), false)];

        match key {
            // Basic keys
            "enter" | "return" => Ok(press("ret")),
            "esc" | "escape" => Ok(press("esc")),
            "tab" => Ok(press("tab")),
            "backspace" => Ok(press("backspace")),
            "space" => Ok(press("spc")),
            "capslock" | "caps_lock" => Ok(press("caps_lock")),
            "numlock" | "num_lock" => Ok(press("num_lock")),
            "scrolllock" | "scroll_lock" => Ok(press("scroll_lock")),
            "print" | "print_screen" | "prtsc" | "sysrq" => Ok(press("print")),

            // Arrow keys
            "arrow_up" | "up" => Ok(press("up")),
            "arrow_down" | "down" => Ok(press("down")),
            "arrow_left" | "left" => Ok(press("left")),
            "arrow_right" | "right" => Ok(press("right")),

            // Navigation
            "pageup" | "page_up" => Ok(press("pgup")),
            "pagedown" | "page_down" => Ok(press("pgdn")),
            "home" => Ok(press("home")),
            "end" => Ok(press("end")),
            "insert" => Ok(press("insert")),
            "delete" | "del" => Ok(press("delete")),

            // Function keys
            "f1" => Ok(press("f1")),
            "f2" => Ok(press("f2")),
            "f3" => Ok(press("f3")),
            "f4" => Ok(press("f4")),
            "f5" => Ok(press("f5")),
            "f6" => Ok(press("f6")),
            "f7" => Ok(press("f7")),
            "f8" => Ok(press("f8")),
            "f9" => Ok(press("f9")),
            "f10" => Ok(press("f10")),
            "f11" => Ok(press("f11")),
            "f12" => Ok(press("f12")),
            "f13" => Ok(press("f13")),
            "f14" => Ok(press("f14")),
            "f15" => Ok(press("f15")),
            "f16" => Ok(press("f16")),
            "f17" => Ok(press("f17")),
            "f18" => Ok(press("f18")),
            "f19" => Ok(press("f19")),
            "f20" => Ok(press("f20")),
            "f21" => Ok(press("f21")),
            "f22" => Ok(press("f22")),
            "f23" => Ok(press("f23")),
            "f24" => Ok(press("f24")),

            // Numpad
            "kp0" | "numpad0" => Ok(press("kp_0")),
            "kp1" | "numpad1" => Ok(press("kp_1")),
            "kp2" | "numpad2" => Ok(press("kp_2")),
            "kp3" | "numpad3" => Ok(press("kp_3")),
            "kp4" | "numpad4" => Ok(press("kp_4")),
            "kp5" | "numpad5" => Ok(press("kp_5")),
            "kp6" | "numpad6" => Ok(press("kp_6")),
            "kp7" | "numpad7" => Ok(press("kp_7")),
            "kp8" | "numpad8" => Ok(press("kp_8")),
            "kp9" | "numpad9" => Ok(press("kp_9")),
            "kp_enter" | "numpad_enter" => Ok(press("kp_enter")),
            "kp_plus" | "numpad_plus" => Ok(press("kp_add")),
            "kp_minus" | "numpad_minus" => Ok(press("kp_subtract")),
            "kp_multiply" | "numpad_multiply" => Ok(press("kp_multiply")),
            "kp_divide" | "numpad_divide" => Ok(press("kp_divide")),
            "kp_dot" | "numpad_dot" => Ok(press("kp_decimal")),

            // Modifier keys (standalone press)
            "ctrl" | "left_ctrl" => Ok(press("ctrl")),
            "right_ctrl" => Ok(press("ctrl_r")),
            "alt" | "left_alt" => Ok(press("alt")),
            "right_alt" => Ok(press("alt_r")),
            "shift" | "left_shift" => Ok(press("shift")),
            "right_shift" => Ok(press("shift_r")),
            "super" | "meta" | "win" | "left_meta" => Ok(press("meta_l")),
            "right_meta" | "right_super" => Ok(press("meta_r")),

            // Single characters (a-z)
            "a" => Ok(press("a")),
            "b" => Ok(press("b")),
            "c" => Ok(press("c")),
            "d" => Ok(press("d")),
            "e" => Ok(press("e")),
            "f" => Ok(press("f")),
            "g" => Ok(press("g")),
            "h" => Ok(press("h")),
            "i" => Ok(press("i")),
            "j" => Ok(press("j")),
            "k" => Ok(press("k")),
            "l" => Ok(press("l")),
            "m" => Ok(press("m")),
            "n" => Ok(press("n")),
            "o" => Ok(press("o")),
            "p" => Ok(press("p")),
            "q" => Ok(press("q")),
            "r" => Ok(press("r")),
            "s" => Ok(press("s")),
            "t" => Ok(press("t")),
            "u" => Ok(press("u")),
            "v" => Ok(press("v")),
            "w" => Ok(press("w")),
            "x" => Ok(press("x")),
            "y" => Ok(press("y")),
            "z" => Ok(press("z")),

            // Digits
            "0" => Ok(press("0")),
            "1" => Ok(press("1")),
            "2" => Ok(press("2")),
            "3" => Ok(press("3")),
            "4" => Ok(press("4")),
            "5" => Ok(press("5")),
            "6" => Ok(press("6")),
            "7" => Ok(press("7")),
            "8" => Ok(press("8")),
            "9" => Ok(press("9")),

            // Symbol keys (unshifted)
            "-" => Ok(press("minus")),
            "=" => Ok(press("equal")),
            "[" => Ok(press("bracket_left")),
            "]" => Ok(press("bracket_right")),
            "\\" => Ok(press("backslash")),
            ";" => Ok(press("semicolon")),
            "'" => Ok(press("apostrophe")),
            "`" => Ok(press("grave_accent")),
            "," => Ok(press("comma")),
            "." => Ok(press("dot")),
            "/" => Ok(press("slash")),

            // Shifted symbol keys (shift + base key)
            "!" => Ok(vec![
                ("shift".into(), true),
                ("1".into(), true),
                ("1".into(), false),
                ("shift".into(), false),
            ]),
            "@" => Ok(vec![
                ("shift".into(), true),
                ("2".into(), true),
                ("2".into(), false),
                ("shift".into(), false),
            ]),
            "#" => Ok(vec![
                ("shift".into(), true),
                ("3".into(), true),
                ("3".into(), false),
                ("shift".into(), false),
            ]),
            "$" => Ok(vec![
                ("shift".into(), true),
                ("4".into(), true),
                ("4".into(), false),
                ("shift".into(), false),
            ]),
            "%" => Ok(vec![
                ("shift".into(), true),
                ("5".into(), true),
                ("5".into(), false),
                ("shift".into(), false),
            ]),
            "^" => Ok(vec![
                ("shift".into(), true),
                ("6".into(), true),
                ("6".into(), false),
                ("shift".into(), false),
            ]),
            "&" => Ok(vec![
                ("shift".into(), true),
                ("7".into(), true),
                ("7".into(), false),
                ("shift".into(), false),
            ]),
            "*" => Ok(vec![
                ("shift".into(), true),
                ("8".into(), true),
                ("8".into(), false),
                ("shift".into(), false),
            ]),
            "(" => Ok(vec![
                ("shift".into(), true),
                ("9".into(), true),
                ("9".into(), false),
                ("shift".into(), false),
            ]),
            ")" => Ok(vec![
                ("shift".into(), true),
                ("0".into(), true),
                ("0".into(), false),
                ("shift".into(), false),
            ]),
            "_" => Ok(vec![
                ("shift".into(), true),
                ("minus".into(), true),
                ("minus".into(), false),
                ("shift".into(), false),
            ]),
            "+" => Ok(vec![
                ("shift".into(), true),
                ("equal".into(), true),
                ("equal".into(), false),
                ("shift".into(), false),
            ]),
            "{" => Ok(vec![
                ("shift".into(), true),
                ("bracket_left".into(), true),
                ("bracket_left".into(), false),
                ("shift".into(), false),
            ]),
            "}" => Ok(vec![
                ("shift".into(), true),
                ("bracket_right".into(), true),
                ("bracket_right".into(), false),
                ("shift".into(), false),
            ]),
            "|" => Ok(vec![
                ("shift".into(), true),
                ("backslash".into(), true),
                ("backslash".into(), false),
                ("shift".into(), false),
            ]),
            ":" => Ok(vec![
                ("shift".into(), true),
                ("semicolon".into(), true),
                ("semicolon".into(), false),
                ("shift".into(), false),
            ]),
            "\"" => Ok(vec![
                ("shift".into(), true),
                ("apostrophe".into(), true),
                ("apostrophe".into(), false),
                ("shift".into(), false),
            ]),
            "~" => Ok(vec![
                ("shift".into(), true),
                ("grave_accent".into(), true),
                ("grave_accent".into(), false),
                ("shift".into(), false),
            ]),
            "<" => Ok(vec![
                ("shift".into(), true),
                ("comma".into(), true),
                ("comma".into(), false),
                ("shift".into(), false),
            ]),
            ">" => Ok(vec![
                ("shift".into(), true),
                ("dot".into(), true),
                ("dot".into(), false),
                ("shift".into(), false),
            ]),
            "?" => Ok(vec![
                ("shift".into(), true),
                ("slash".into(), true),
                ("slash".into(), false),
                ("shift".into(), false),
            ]),

            // Modifier combos
            "ctrl+a" => Ok(vec![
                ("ctrl".into(), true),
                ("a".into(), true),
                ("a".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+b" => Ok(vec![
                ("ctrl".into(), true),
                ("b".into(), true),
                ("b".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+c" => Ok(vec![
                ("ctrl".into(), true),
                ("c".into(), true),
                ("c".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+d" => Ok(vec![
                ("ctrl".into(), true),
                ("d".into(), true),
                ("d".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+e" => Ok(vec![
                ("ctrl".into(), true),
                ("e".into(), true),
                ("e".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+f" => Ok(vec![
                ("ctrl".into(), true),
                ("f".into(), true),
                ("f".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+g" => Ok(vec![
                ("ctrl".into(), true),
                ("g".into(), true),
                ("g".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+h" => Ok(vec![
                ("ctrl".into(), true),
                ("h".into(), true),
                ("h".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+i" => Ok(vec![
                ("ctrl".into(), true),
                ("i".into(), true),
                ("i".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+j" => Ok(vec![
                ("ctrl".into(), true),
                ("j".into(), true),
                ("j".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+k" => Ok(vec![
                ("ctrl".into(), true),
                ("k".into(), true),
                ("k".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+l" => Ok(vec![
                ("ctrl".into(), true),
                ("l".into(), true),
                ("l".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+m" => Ok(vec![
                ("ctrl".into(), true),
                ("m".into(), true),
                ("m".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+n" => Ok(vec![
                ("ctrl".into(), true),
                ("n".into(), true),
                ("n".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+o" => Ok(vec![
                ("ctrl".into(), true),
                ("o".into(), true),
                ("o".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+p" => Ok(vec![
                ("ctrl".into(), true),
                ("p".into(), true),
                ("p".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+q" => Ok(vec![
                ("ctrl".into(), true),
                ("q".into(), true),
                ("q".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+r" => Ok(vec![
                ("ctrl".into(), true),
                ("r".into(), true),
                ("r".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+s" => Ok(vec![
                ("ctrl".into(), true),
                ("s".into(), true),
                ("s".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+t" => Ok(vec![
                ("ctrl".into(), true),
                ("t".into(), true),
                ("t".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+u" => Ok(vec![
                ("ctrl".into(), true),
                ("u".into(), true),
                ("u".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+v" => Ok(vec![
                ("ctrl".into(), true),
                ("v".into(), true),
                ("v".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+w" => Ok(vec![
                ("ctrl".into(), true),
                ("w".into(), true),
                ("w".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+x" => Ok(vec![
                ("ctrl".into(), true),
                ("x".into(), true),
                ("x".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+y" => Ok(vec![
                ("ctrl".into(), true),
                ("y".into(), true),
                ("y".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+z" => Ok(vec![
                ("ctrl".into(), true),
                ("z".into(), true),
                ("z".into(), false),
                ("ctrl".into(), false),
            ]),
            "alt+f1" => Ok(vec![
                ("alt".into(), true),
                ("f1".into(), true),
                ("f1".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f2" => Ok(vec![
                ("alt".into(), true),
                ("f2".into(), true),
                ("f2".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f3" => Ok(vec![
                ("alt".into(), true),
                ("f3".into(), true),
                ("f3".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f4" => Ok(vec![
                ("alt".into(), true),
                ("f4".into(), true),
                ("f4".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f5" => Ok(vec![
                ("alt".into(), true),
                ("f5".into(), true),
                ("f5".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f6" => Ok(vec![
                ("alt".into(), true),
                ("f6".into(), true),
                ("f6".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f7" => Ok(vec![
                ("alt".into(), true),
                ("f7".into(), true),
                ("f7".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f8" => Ok(vec![
                ("alt".into(), true),
                ("f8".into(), true),
                ("f8".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f9" => Ok(vec![
                ("alt".into(), true),
                ("f9".into(), true),
                ("f9".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f10" => Ok(vec![
                ("alt".into(), true),
                ("f10".into(), true),
                ("f10".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f11" => Ok(vec![
                ("alt".into(), true),
                ("f11".into(), true),
                ("f11".into(), false),
                ("alt".into(), false),
            ]),
            "alt+f12" => Ok(vec![
                ("alt".into(), true),
                ("f12".into(), true),
                ("f12".into(), false),
                ("alt".into(), false),
            ]),
            "alt+tab" => Ok(vec![
                ("alt".into(), true),
                ("tab".into(), true),
                ("tab".into(), false),
                ("alt".into(), false),
            ]),
            "alt+enter" => Ok(vec![
                ("alt".into(), true),
                ("ret".into(), true),
                ("ret".into(), false),
                ("alt".into(), false),
            ]),
            "ctrl+alt+delete" => Ok(vec![
                ("ctrl".into(), true),
                ("alt".into(), true),
                ("delete".into(), true),
                ("delete".into(), false),
                ("alt".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+alt+f1" => Ok(vec![
                ("ctrl".into(), true),
                ("alt".into(), true),
                ("f1".into(), true),
                ("f1".into(), false),
                ("alt".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+alt+f2" => Ok(vec![
                ("ctrl".into(), true),
                ("alt".into(), true),
                ("f2".into(), true),
                ("f2".into(), false),
                ("alt".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+alt+f3" => Ok(vec![
                ("ctrl".into(), true),
                ("alt".into(), true),
                ("f3".into(), true),
                ("f3".into(), false),
                ("alt".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+alt+f4" => Ok(vec![
                ("ctrl".into(), true),
                ("alt".into(), true),
                ("f4".into(), true),
                ("f4".into(), false),
                ("alt".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+alt+f5" => Ok(vec![
                ("ctrl".into(), true),
                ("alt".into(), true),
                ("f5".into(), true),
                ("f5".into(), false),
                ("alt".into(), false),
                ("ctrl".into(), false),
            ]),
            "ctrl+alt+f6" => Ok(vec![
                ("ctrl".into(), true),
                ("alt".into(), true),
                ("f6".into(), true),
                ("f6".into(), false),
                ("alt".into(), false),
                ("ctrl".into(), false),
            ]),
            _ => anyhow::bail!("Unknown key: {}", key),
        }
    }

    /// Convert a single character to QMP key events based on keyboard layout
    fn char_to_qmp_events(
        ch: char,
        layout: &KeyboardLayout,
    ) -> anyhow::Result<Vec<(String, bool)>> {
        match layout {
            KeyboardLayout::DE => Self::char_to_qmp_de(ch),
            KeyboardLayout::FR => Self::char_to_qmp_fr(ch),
            KeyboardLayout::ES => Self::char_to_qmp_es(ch),
            KeyboardLayout::IT => Self::char_to_qmp_it(ch),
            KeyboardLayout::GB => Self::char_to_qmp_gb(ch),
            KeyboardLayout::US | _ => Self::char_to_qmp_us(ch),
        }
    }

    /// US keyboard layout (default)
    fn char_to_qmp_us(ch: char) -> anyhow::Result<Vec<(String, bool)>> {
        let press = |k: &str| vec![(k.to_string(), true), (k.to_string(), false)];
        let shift_press = |k: &str| {
            vec![
                ("shift".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("shift".into(), false),
            ]
        };

        match ch {
            'a'..='z' => Ok(press(&ch.to_string())),
            'A'..='Z' => Ok(shift_press(&ch.to_lowercase().to_string())),
            '0'..='9' => Ok(press(&ch.to_string())),
            ' ' => Ok(press("spc")),
            '\n' | '\r' => Ok(press("ret")),
            '\t' => Ok(press("tab")),
            '-' => Ok(press("minus")),
            '=' => Ok(press("equal")),
            '[' => Ok(press("bracket_left")),
            ']' => Ok(press("bracket_right")),
            '\\' => Ok(press("backslash")),
            ';' => Ok(press("semicolon")),
            '\'' => Ok(press("apostrophe")),
            '`' => Ok(press("grave_accent")),
            ',' => Ok(press("comma")),
            '.' => Ok(press("dot")),
            '/' => Ok(press("slash")),
            '!' => Ok(shift_press("1")),
            '@' => Ok(shift_press("2")),
            '#' => Ok(shift_press("3")),
            '$' => Ok(shift_press("4")),
            '%' => Ok(shift_press("5")),
            '^' => Ok(shift_press("6")),
            '&' => Ok(shift_press("7")),
            '*' => Ok(shift_press("8")),
            '(' => Ok(shift_press("9")),
            ')' => Ok(shift_press("0")),
            '_' => Ok(shift_press("minus")),
            '+' => Ok(shift_press("equal")),
            '{' => Ok(shift_press("bracket_left")),
            '}' => Ok(shift_press("bracket_right")),
            '|' => Ok(shift_press("backslash")),
            ':' => Ok(shift_press("semicolon")),
            '"' => Ok(shift_press("apostrophe")),
            '~' => Ok(shift_press("grave_accent")),
            '<' => Ok(shift_press("comma")),
            '>' => Ok(shift_press("dot")),
            '?' => Ok(shift_press("slash")),
            _ => {
                tracing::warn!(char = %ch, "Unsupported char for US layout");
                Ok(vec![])
            }
        }
    }

    /// German keyboard layout
    fn char_to_qmp_de(ch: char) -> anyhow::Result<Vec<(String, bool)>> {
        let press = |k: &str| vec![(k.to_string(), true), (k.to_string(), false)];
        let shift_press = |k: &str| {
            vec![
                ("shift".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("shift".into(), false),
            ]
        };
        let altgr_press = |k: &str| {
            vec![
                ("alt_r".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("alt_r".into(), false),
            ]
        };

        match ch {
            // German layout: Y and Z are swapped (QWERTZ vs QWERTY)
            'y' => Ok(press("z")), // German 'y' is on physical 'z' key
            'z' => Ok(press("y")), // German 'z' is on physical 'y' key
            'Y' => Ok(shift_press("z")),
            'Z' => Ok(shift_press("y")),
            // Other letters (a-x are the same)
            'a'..='x' => Ok(press(&ch.to_string())),
            'A'..='X' => Ok(shift_press(&ch.to_lowercase().to_string())),
            '0'..='9' => Ok(press(&ch.to_string())),
            ' ' => Ok(press("spc")),
            '\n' | '\r' => Ok(press("ret")),
            '\t' => Ok(press("tab")),
            'ß' => Ok(press("minus")),
            '?' => Ok(shift_press("minus")),
            '´' => Ok(press("equal")),
            '`' => Ok(shift_press("equal")),
            'ü' => Ok(press("bracket_left")),
            'Ü' => Ok(shift_press("bracket_left")),
            '+' => Ok(press("bracket_right")),
            '*' => Ok(shift_press("bracket_right")),
            '#' => Ok(press("backslash")),
            '\'' => Ok(shift_press("backslash")),
            'ö' => Ok(press("semicolon")),
            'Ö' => Ok(shift_press("semicolon")),
            'ä' => Ok(press("apostrophe")),
            'Ä' => Ok(shift_press("apostrophe")),
            '^' => Ok(press("grave_accent")),
            '°' => Ok(shift_press("grave_accent")),
            '-' => Ok(press("slash")),
            '_' => Ok(shift_press("slash")),
            ',' => Ok(press("comma")),
            ';' => Ok(shift_press("comma")),
            '.' => Ok(press("dot")),
            ':' => Ok(shift_press("dot")),
            '!' => Ok(shift_press("1")),
            '"' => Ok(shift_press("2")),
            '§' => Ok(shift_press("3")),
            '$' => Ok(shift_press("4")),
            '%' => Ok(shift_press("5")),
            '&' => Ok(shift_press("6")),
            '/' => Ok(shift_press("7")),
            '(' => Ok(shift_press("8")),
            ')' => Ok(shift_press("9")),
            '=' => Ok(shift_press("0")),
            '<' => Ok(press("less")),
            '>' => Ok(shift_press("less")),
            '{' => Ok(shift_press("bracket_left")),
            '}' => Ok(shift_press("bracket_right")),
            '|' => Ok(shift_press("backslash")),
            '~' => Ok(shift_press("grave_accent")),
            '@' => Ok(altgr_press("q")),
            '\\' => Ok(altgr_press("minus")),
            '€' => Ok(altgr_press("e")),
            _ => {
                tracing::warn!(char = %ch, "Unsupported char for DE layout");
                Ok(vec![])
            }
        }
    }

    /// French keyboard layout (AZERTY)
    fn char_to_qmp_fr(ch: char) -> anyhow::Result<Vec<(String, bool)>> {
        let press = |k: &str| vec![(k.to_string(), true), (k.to_string(), false)];
        let shift_press = |k: &str| {
            vec![
                ("shift".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("shift".into(), false),
            ]
        };
        let altgr_press = |k: &str| {
            vec![
                ("alt_r".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("alt_r".into(), false),
            ]
        };

        match ch {
            // AZERTY: a and q swapped, z and w swapped
            'a' => Ok(press("q")),
            'q' => Ok(press("a")),
            'z' => Ok(press("w")),
            'w' => Ok(press("z")),
            'A' => Ok(shift_press("q")),
            'Q' => Ok(shift_press("a")),
            'Z' => Ok(shift_press("w")),
            'W' => Ok(shift_press("z")),
            // Other letters same
            'b'..='y' | 'B'..='Y' => {
                if ch.is_lowercase() {
                    Ok(press(&ch.to_string()))
                } else {
                    Ok(shift_press(&ch.to_lowercase().to_string()))
                }
            }
            '0'..='9' => Ok(shift_press(&ch.to_string())), // Digits need shift on FR
            ' ' => Ok(press("spc")),
            '\n' | '\r' => Ok(press("ret")),
            '\t' => Ok(press("tab")),
            'é' => Ok(press("2")),
            '&' => Ok(press("1")),
            '"' => Ok(press("3")),
            '\'' => Ok(press("4")),
            '(' => Ok(press("5")),
            '-' => Ok(press("6")),
            'è' => Ok(press("7")),
            '_' => Ok(press("8")),
            'ç' => Ok(press("9")),
            'à' => Ok(press("0")),
            ')' => Ok(shift_press("minus")),
            '=' => Ok(shift_press("equal")),
            '^' => Ok(press("bracket_left")),
            '$' => Ok(press("bracket_right")),
            '*' => Ok(press("backslash")),
            'm' => Ok(press("semicolon")),
            'M' => Ok(shift_press("semicolon")),
            'ù' => Ok(press("apostrophe")),
            '!' => Ok(shift_press("backslash")),
            ',' => Ok(press("m")),
            '?' => Ok(shift_press("m")),
            ';' => Ok(press("comma")),
            '.' => Ok(press("e")),
            ':' => Ok(press("slash")),
            '/' => Ok(shift_press("slash")),
            '§' => Ok(shift_press("grave_accent")),
            '<' => Ok(press("less")),
            '>' => Ok(shift_press("less")),
            '@' => Ok(altgr_press("0")),
            '#' => Ok(altgr_press("3")),
            '{' => Ok(altgr_press("4")),
            '}' => Ok(altgr_press("equal")),
            '[' => Ok(altgr_press("5")),
            ']' => Ok(altgr_press("minus")),
            '|' => Ok(altgr_press("6")),
            '~' => Ok(altgr_press("2")),
            '\\' => Ok(altgr_press("grave_accent")),
            '€' => Ok(altgr_press("e")),
            _ => {
                tracing::warn!(char = %ch, "Unsupported char for FR layout");
                Ok(vec![])
            }
        }
    }

    /// Spanish keyboard layout
    fn char_to_qmp_es(ch: char) -> anyhow::Result<Vec<(String, bool)>> {
        let press = |k: &str| vec![(k.to_string(), true), (k.to_string(), false)];
        let shift_press = |k: &str| {
            vec![
                ("shift".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("shift".into(), false),
            ]
        };
        let altgr_press = |k: &str| {
            vec![
                ("alt_r".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("alt_r".into(), false),
            ]
        };

        match ch {
            'a'..='z' => Ok(press(&ch.to_string())),
            'A'..='Z' => Ok(shift_press(&ch.to_lowercase().to_string())),
            '0'..='9' => Ok(press(&ch.to_string())),
            ' ' => Ok(press("spc")),
            '\n' | '\r' => Ok(press("ret")),
            '\t' => Ok(press("tab")),
            '!' => Ok(shift_press("1")),
            '"' => Ok(shift_press("2")),
            '·' => Ok(shift_press("3")),
            '$' => Ok(shift_press("4")),
            '%' => Ok(shift_press("5")),
            '&' => Ok(shift_press("6")),
            '/' => Ok(shift_press("7")),
            '(' => Ok(shift_press("8")),
            ')' => Ok(shift_press("9")),
            '=' => Ok(shift_press("0")),
            '\'' => Ok(press("minus")),
            '¡' => Ok(shift_press("minus")),
            '¿' => Ok(press("equal")),
            '?' => Ok(shift_press("equal")),
            '`' => Ok(press("bracket_left")),
            '^' => Ok(shift_press("bracket_left")),
            '+' => Ok(press("bracket_right")),
            '*' => Ok(shift_press("bracket_right")),
            'ç' => Ok(press("backslash")),
            'Ç' => Ok(shift_press("backslash")),
            'ñ' => Ok(press("semicolon")),
            'Ñ' => Ok(shift_press("semicolon")),
            '´' => Ok(press("apostrophe")),
            '¨' => Ok(shift_press("apostrophe")),
            'º' => Ok(press("grave_accent")),
            'ª' => Ok(shift_press("grave_accent")),
            '-' => Ok(press("slash")),
            '_' => Ok(shift_press("slash")),
            ',' => Ok(press("comma")),
            ';' => Ok(shift_press("comma")),
            '.' => Ok(press("dot")),
            ':' => Ok(shift_press("dot")),
            '<' => Ok(press("less")),
            '>' => Ok(shift_press("less")),
            '{' => Ok(altgr_press("apostrophe")),
            '}' => Ok(altgr_press("grave_accent")),
            '[' => Ok(altgr_press("bracket_left")),
            ']' => Ok(altgr_press("bracket_right")),
            '\\' => Ok(altgr_press("minus")),
            '@' => Ok(altgr_press("2")),
            '#' => Ok(altgr_press("3")),
            '~' => Ok(altgr_press("4")),
            '|' => Ok(altgr_press("1")),
            '€' => Ok(altgr_press("e")),
            _ => {
                tracing::warn!(char = %ch, "Unsupported char for ES layout");
                Ok(vec![])
            }
        }
    }

    /// Italian keyboard layout
    fn char_to_qmp_it(ch: char) -> anyhow::Result<Vec<(String, bool)>> {
        let press = |k: &str| vec![(k.to_string(), true), (k.to_string(), false)];
        let shift_press = |k: &str| {
            vec![
                ("shift".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("shift".into(), false),
            ]
        };
        let altgr_press = |k: &str| {
            vec![
                ("alt_r".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("alt_r".into(), false),
            ]
        };

        match ch {
            'a'..='z' => Ok(press(&ch.to_string())),
            'A'..='Z' => Ok(shift_press(&ch.to_lowercase().to_string())),
            '0'..='9' => Ok(press(&ch.to_string())),
            ' ' => Ok(press("spc")),
            '\n' | '\r' => Ok(press("ret")),
            '\t' => Ok(press("tab")),
            '!' => Ok(shift_press("1")),
            '"' => Ok(shift_press("2")),
            '£' => Ok(shift_press("3")),
            '$' => Ok(shift_press("4")),
            '%' => Ok(shift_press("5")),
            '&' => Ok(shift_press("6")),
            '/' => Ok(shift_press("7")),
            '(' => Ok(shift_press("8")),
            ')' => Ok(shift_press("9")),
            '=' => Ok(shift_press("0")),
            '\'' => Ok(press("minus")),
            '?' => Ok(shift_press("minus")),
            'ì' => Ok(press("equal")),
            '^' => Ok(shift_press("equal")),
            'è' => Ok(press("bracket_left")),
            'é' => Ok(shift_press("bracket_left")),
            '+' => Ok(press("bracket_right")),
            '*' => Ok(shift_press("bracket_right")),
            'ù' => Ok(press("backslash")),
            '§' => Ok(shift_press("backslash")),
            'ò' => Ok(press("semicolon")),
            'ç' => Ok(shift_press("semicolon")),
            'à' => Ok(press("apostrophe")),
            '°' => Ok(shift_press("apostrophe")),
            '\\' => Ok(press("grave_accent")),
            '|' => Ok(shift_press("grave_accent")),
            '-' => Ok(press("slash")),
            '_' => Ok(shift_press("slash")),
            ',' => Ok(press("comma")),
            ';' => Ok(shift_press("comma")),
            '.' => Ok(press("dot")),
            ':' => Ok(shift_press("dot")),
            '<' => Ok(press("less")),
            '>' => Ok(shift_press("less")),
            '{' => Ok(altgr_press("bracket_left")),
            '}' => Ok(altgr_press("bracket_right")),
            '[' => Ok(altgr_press("equal")),
            ']' => Ok(altgr_press("apostrophe")),
            '@' => Ok(altgr_press("semicolon")),
            '#' => Ok(altgr_press("backslash")),
            '~' => Ok(altgr_press("grave_accent")),
            '€' => Ok(altgr_press("e")),
            _ => {
                tracing::warn!(char = %ch, "Unsupported char for IT layout");
                Ok(vec![])
            }
        }
    }

    /// British keyboard layout
    fn char_to_qmp_gb(ch: char) -> anyhow::Result<Vec<(String, bool)>> {
        let press = |k: &str| vec![(k.to_string(), true), (k.to_string(), false)];
        let shift_press = |k: &str| {
            vec![
                ("shift".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("shift".into(), false),
            ]
        };
        let altgr_press = |k: &str| {
            vec![
                ("alt_r".into(), true),
                (k.to_string(), true),
                (k.to_string(), false),
                ("alt_r".into(), false),
            ]
        };

        match ch {
            'a'..='z' => Ok(press(&ch.to_string())),
            'A'..='Z' => Ok(shift_press(&ch.to_lowercase().to_string())),
            '0'..='9' => Ok(press(&ch.to_string())),
            ' ' => Ok(press("spc")),
            '\n' | '\r' => Ok(press("ret")),
            '\t' => Ok(press("tab")),
            // GB layout differences from US
            '#' => Ok(press("backslash")),
            '~' => Ok(shift_press("backslash")),
            '£' => Ok(shift_press("3")),
            '¬' => Ok(shift_press("grave_accent")),
            '|' => Ok(altgr_press("less")),
            '@' => Ok(shift_press("apostrophe")),
            // Rest same as US
            '-' => Ok(press("minus")),
            '=' => Ok(press("equal")),
            '[' => Ok(press("bracket_left")),
            ']' => Ok(press("bracket_right")),
            ';' => Ok(press("semicolon")),
            '\'' => Ok(press("apostrophe")),
            '`' => Ok(press("grave_accent")),
            ',' => Ok(press("comma")),
            '.' => Ok(press("dot")),
            '/' => Ok(press("slash")),
            '\\' => Ok(press("less")),
            '!' => Ok(shift_press("1")),
            '"' => Ok(shift_press("2")),
            '$' => Ok(shift_press("4")),
            '%' => Ok(shift_press("5")),
            '^' => Ok(shift_press("6")),
            '&' => Ok(shift_press("7")),
            '*' => Ok(shift_press("8")),
            '(' => Ok(shift_press("9")),
            ')' => Ok(shift_press("0")),
            '_' => Ok(shift_press("minus")),
            '+' => Ok(shift_press("equal")),
            '{' => Ok(shift_press("bracket_left")),
            '}' => Ok(shift_press("bracket_right")),
            ':' => Ok(shift_press("semicolon")),
            '<' => Ok(shift_press("comma")),
            '>' => Ok(shift_press("dot")),
            '?' => Ok(shift_press("slash")),
            '€' => Ok(altgr_press("4")),
            _ => {
                tracing::warn!(char = %ch, "Unsupported char for GB layout");
                Ok(vec![])
            }
        }
    }
}

/// Names are storage components, not paths or regular expressions.
pub fn validate_vm_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name.len() <= 80
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            && !matches!(name, "." | ".." | "isos" | "disks" | "shared" | "secrets"),
        "Invalid VM name"
    );
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Dropping an unfinished startup kills only its newly spawned child. Once
/// inserted in the manager, guests survive host shutdown/feature uninstall.
struct StartupChild(Option<tokio::process::Child>);
impl StartupChild {
    fn commit(mut self) -> tokio::process::Child {
        self.0.take().expect("startup child owned")
    }
}
impl std::ops::Deref for StartupChild {
    type Target = tokio::process::Child;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("startup child owned")
    }
}
impl std::ops::DerefMut for StartupChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("startup child owned")
    }
}
impl Drop for StartupChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.start_kill();
        }
    }
}
