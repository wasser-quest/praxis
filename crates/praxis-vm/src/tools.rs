use crate::VmManager;
use std::collections::BTreeMap;

/// Convert raw PPM (P6 binary) bytes to PNG bytes
fn ppm_to_png(ppm: &[u8]) -> anyhow::Result<Vec<u8>> {
    // Parse PPM header: P6\n<width> <height>\n<maxval>\n<pixels>
    if ppm.len() < 3 || ppm[0] != b'P' || ppm[1] != b'6' {
        anyhow::bail!("Not a P6 PPM file");
    }
    let mut pos = 2; // skip "P6"
                     // Skip whitespace and comments
    loop {
        while pos < ppm.len()
            && (ppm[pos] == b'\n' || ppm[pos] == b'\r' || ppm[pos] == b' ' || ppm[pos] == b'\t')
        {
            pos += 1;
        }
        if pos < ppm.len() && ppm[pos] == b'#' {
            while pos < ppm.len() && ppm[pos] != b'\n' {
                pos += 1;
            }
        } else {
            break;
        }
    }
    // Parse width
    let width_start = pos;
    while pos < ppm.len() && ppm[pos].is_ascii_digit() {
        pos += 1;
    }
    let width: u32 = std::str::from_utf8(&ppm[width_start..pos])?.parse()?;
    // Skip whitespace
    while pos < ppm.len() && (ppm[pos] == b' ' || ppm[pos] == b'\t') {
        pos += 1;
    }
    // Parse height
    let height_start = pos;
    while pos < ppm.len() && ppm[pos].is_ascii_digit() {
        pos += 1;
    }
    let height: u32 = std::str::from_utf8(&ppm[height_start..pos])?.parse()?;
    // Skip whitespace
    while pos < ppm.len()
        && (ppm[pos] == b' ' || ppm[pos] == b'\t' || ppm[pos] == b'\n' || ppm[pos] == b'\r')
    {
        pos += 1;
    }
    // Parse maxval
    let maxval_start = pos;
    while pos < ppm.len() && ppm[pos].is_ascii_digit() {
        pos += 1;
    }
    let _maxval: u32 = std::str::from_utf8(&ppm[maxval_start..pos])?.parse()?;
    // Skip single whitespace after maxval
    if pos < ppm.len() && (ppm[pos] == b'\n' || ppm[pos] == b'\r' || ppm[pos] == b' ') {
        pos += 1;
    }
    let pixel_data = &ppm[pos..];
    let expected = (width as usize) * (height as usize) * 3;
    if pixel_data.len() < expected {
        anyhow::bail!(
            "PPM pixel data too short: got {}, expected {}",
            pixel_data.len(),
            expected
        );
    }
    // Encode as PNG
    let mut buf = Vec::new();
    {
        let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut buf), width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&pixel_data[..expected])?;
    }
    Ok(buf)
}

pub async fn dispatch_vm_tool(
    manager: &VmManager,
    tool_name: &str,
    args: &serde_json::Value,
    grants: &BTreeMap<String, String>,
) -> Option<String> {
    dispatch_vm_tool_with(manager, tool_name, args, grants, None).await
}

pub const OPERATIONS: &[&str] = &[
    "vm_start",
    "vm_stop",
    "vm_shell",
    "vm_keys",
    "vm_input",
    "vm_screenshot",
    "vm_file_transfer",
    "vm_snapshot",
    "vm_shared_folder",
    "vm_mouse",
    "vm_look_screenshot",
    "vm_process_list",
    "vm_file_read",
    "vm_network_test",
    "vm_service_list",
    "vm_package_install",
    "vm_snapshot_list",
    "vm_snapshot_restore",
    "vm_snapshot_delete",
    "vm_wait_for_text",
    "vm_window_list",
    "vm_window_focus",
    "vm_clipboard_set",
    "vm_clipboard_get",
    "vm_install",
];
pub fn is_known_operation(operation: &str) -> bool {
    OPERATIONS.contains(&operation)
}

/// `persisted` carries the guest's recorded endpoints and disk so restarts
/// reuse the same QMP/serial/VNC addresses instead of re-deriving them.
pub async fn dispatch_vm_tool_with(
    manager: &VmManager,
    tool_name: &str,
    args: &serde_json::Value,
    grants: &BTreeMap<String, String>,
    persisted: Option<&crate::VmConfig>,
) -> Option<String> {
    let result = match tool_name {
        "vm_start" => handle_vm_start(manager, args, grants, persisted).await,
        "vm_stop" => handle_vm_stop(manager, args).await,
        "vm_shell" => handle_vm_shell(manager, args).await,
        "vm_keys" => handle_vm_keys(manager, args).await,
        "vm_input" => handle_vm_input(manager, args).await,
        "vm_screenshot" => handle_vm_screenshot(manager, args).await,
        "vm_file_transfer" => handle_vm_file_transfer(manager, args).await,
        "vm_snapshot" => handle_vm_snapshot(manager, args).await,
        "vm_shared_folder" => handle_vm_shared_folder(manager, args).await,
        "vm_mouse" => handle_vm_mouse(manager, args).await,
        "vm_look_screenshot" => handle_vm_look_screenshot(manager, args).await,
        "vm_process_list" => handle_vm_process_list(manager, args).await,
        "vm_file_read" => handle_vm_file_read(manager, args).await,
        "vm_network_test" => handle_vm_network_test(manager, args).await,
        "vm_service_list" => handle_vm_service_list(manager, args).await,
        "vm_package_install" => handle_vm_package_install(manager, args).await,
        "vm_snapshot_list" => handle_vm_snapshot_list(manager, args).await,
        "vm_snapshot_restore" => handle_vm_snapshot_restore(manager, args).await,
        "vm_snapshot_delete" => handle_vm_snapshot_delete(manager, args).await,
        "vm_wait_for_text" => handle_vm_wait_for_text(manager, args).await,
        "vm_window_list" => handle_vm_window_list(manager, args).await,
        "vm_window_focus" => handle_vm_window_focus(manager, args).await,
        "vm_clipboard_set" => handle_vm_clipboard_set(manager, args).await,
        "vm_clipboard_get" => handle_vm_clipboard_get(manager, args).await,
        "vm_install" => handle_vm_install(manager, args, grants).await,
        _ => return None,
    };

    Some(result)
}

async fn handle_vm_start(
    manager: &VmManager,
    args: &serde_json::Value,
    grants: &BTreeMap<String, String>,
    persisted: Option<&crate::VmConfig>,
) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let cpu_cores = args["cpu_cores"].as_u64().unwrap_or(2) as u32;
    let ram_mb = args["ram_mb"].as_u64().unwrap_or(4096) as u32;
    let disk_size = args["disk_size"].as_str().unwrap_or("40G");
    let iso_path = args["iso_path"].as_str().map(|s| s.to_string());
    let arch = args["arch"].as_str().unwrap_or("x86_64");
    let firmware_str = args["firmware"].as_str().unwrap_or("bios");

    let keyboard_layout = args["keyboard_layout"].as_str().unwrap_or("us").to_string();

    tracing::info!(layout = %keyboard_layout, "VM starting with keyboard layout");

    let data_dir = manager.data_dir();

    // Use fixed VNC port to avoid port drift on restarts
    let vnc_offset: u16 = 1;

    let mut config = crate::VmConfig::default_for_name(name, &data_dir, vnc_offset, arch);
    if let Err(e) = config.set_socket_mode(args["socket_mode"].as_str().unwrap_or(
        if cfg!(target_os = "linux") {
            "unix"
        } else {
            "tcp"
        },
    )) {
        return format!("Error: {e}");
    }
    config.cpu_cores = cpu_cores;
    config.ram_mb = ram_mb;
    config.disk_size = disk_size.to_string();
    config.iso_path = iso_path;
    config.keyboard_layout = crate::KeyboardLayout::from_str(&keyboard_layout);
    config.firmware = crate::Firmware::from_str(firmware_str);

    // Auto-add shared folders from data directory
    config.shared_folders.push(crate::SharedFolder {
        host_path: format!("{}/shared", data_dir),
        mount_tag: "praxis-shared".to_string(),
        mount_point: "/mnt/shared".to_string(),
        readonly: false,
    });
    if let Some(saved) = persisted.filter(|saved| saved.name == name) {
        config.reuse_endpoints(saved);
    }

    match manager.start_vm_with_grants(config, grants).await {
        Ok(msg) => {
            // Clear old screenshots from previous sessions
            let ss_dir = format!("{}/vm/{}/screenshots", data_dir, name);
            if let Ok(entries) = std::fs::read_dir(&ss_dir) {
                for entry in entries.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            let _ = std::fs::remove_file(format!("{}/vm/{}/screenshot.ppm", data_dir, name));
            msg
        }
        Err(e) => format!("Error starting VM: {}", e),
    }
}

async fn handle_vm_stop(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    match manager.stop_vm(name).await {
        Ok(msg) => msg,
        Err(e) => format!("Error stopping VM: {}", e),
    }
}

async fn handle_vm_shell(manager: &VmManager, args: &serde_json::Value) -> String {
    let command = args["command"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let timeout = args["timeout_secs"].as_u64().unwrap_or(30).clamp(1, 1800);
    let cwd = args["cwd"].as_str().unwrap_or("");
    let stdin = args["stdin"].as_str().unwrap_or("");
    let env = args["env"].as_object();

    if command.is_empty() {
        return "Error: command is required".to_string();
    }

    // Compose a robust one-liner: optional env exports, optional cd, optional
    // here-doc on stdin. We always wrap in `bash -c '...'` so quoting is
    // predictable (callers don't need to escape their `command`).
    let mut prefix = String::new();
    if let Some(env_map) = env {
        for (k, v) in env_map {
            // Skip non-string env values; reject suspicious keys.
            let Some(val) = v.as_str() else { continue };
            if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            prefix.push_str(&format!("export {k}={}; ", shell_quote(val)));
        }
    }
    if !cwd.is_empty() {
        prefix.push_str(&format!("cd {} && ", shell_quote(cwd)));
    }

    let composed = if stdin.is_empty() {
        format!("{prefix}{command}")
    } else {
        // Use a here-doc with a unique sentinel based on the command hash so
        // it can't accidentally collide with literal user content.
        let sentinel = format!("PRAXIS_EOF_{:08X}", crc32(stdin));
        format!("{prefix}{command} <<'{sentinel}'\n{stdin}\n{sentinel}\n")
    };

    match manager.shell_exec(name, &composed, timeout).await {
        Ok(output) => output,
        Err(e) => format!("Error: {}", e),
    }
}

/// Quote a string for safe inclusion in a single-quoted shell context.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | '@' | ':' | '+' | ',')
    }) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Tiny CRC32 (IEEE polynomial) — used only to derive a unique here-doc
/// sentinel; not security-relevant.
fn crc32(s: &str) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in s.as_bytes() {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

async fn handle_vm_keys(manager: &VmManager, args: &serde_json::Value) -> String {
    let keys = args["keys"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    let layout_override = args["keyboard_layout"].as_str().map(str::to_owned);

    tracing::debug!(layout = ?layout_override, "vm_keys called");

    if keys.is_empty() {
        return "Error: keys is required".to_string();
    }

    match manager
        .send_keys_with_layout(name, keys, layout_override.as_deref())
        .await
    {
        Ok(msg) => msg,
        Err(e) => format!("Error: {}", e),
    }
}

/// High-level input tool for VM navigation
/// Supports vim-like commands and intelligent menu navigation
async fn handle_vm_input(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let action = args["action"].as_str().unwrap_or("");
    let text = args["text"].as_str().unwrap_or("");
    let count = args["count"].as_u64().unwrap_or(1) as usize;

    let layout_override = args["keyboard_layout"].as_str().map(str::to_owned);

    tracing::debug!(action = %action, layout = ?layout_override, "vm_input called");

    match action {
        // Navigation
        "up" | "k" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(arrowup){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Moved up {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "down" | "j" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(arrowdown){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Moved down {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "left" | "h" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(arrowleft){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Moved left {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "right" | "l" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(arrowright){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Moved right {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Page navigation
        "pageup" | "pgup" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(pageup){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Page up {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "pagedown" | "pgdn" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(pagedown){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Page down {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "home" => {
            match manager
                .send_keys_with_layout(name, "home", layout_override.as_deref())
                .await
            {
                Ok(_) => "Pressed Home".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "end" => {
            match manager
                .send_keys_with_layout(name, "end", layout_override.as_deref())
                .await
            {
                Ok(_) => "Pressed End".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Tab navigation (common in installers)
        "tab" => {
            match manager
                .send_keys_with_layout(name, &format!("(tab){}", count), layout_override.as_deref())
                .await
            {
                Ok(_) => format!("Tab {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "shifttab" | "backtab" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!(
                        "shift+tab{}",
                        if count > 1 {
                            format!("(shift+tab){}", count - 1)
                        } else {
                            String::new()
                        }
                    ),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Shift+Tab {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Common actions
        "enter" | "confirm" | "select" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(enter){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Enter {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "esc" | "escape" | "cancel" | "back" => {
            match manager
                .send_keys_with_layout(name, &format!("(esc){}", count), layout_override.as_deref())
                .await
            {
                Ok(_) => format!("Escape {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "space" | "toggle" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(space){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Space {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }
        "backspace" | "delete" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("(backspace){}", count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Backspace {} time(s)", count),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Type text (with optional enter)
        "type" => {
            if text.is_empty() {
                return "Error: text is required for type action".to_string();
            }
            let enter_after = args["enter"].as_bool().unwrap_or(false);
            let keys = if enter_after {
                format!("{}\n", text)
            } else {
                text.to_string()
            };
            match manager
                .send_keys_with_layout(name, &keys, layout_override.as_deref())
                .await
            {
                Ok(_) => {
                    if enter_after {
                        format!("Typed: {} (+ enter)", text)
                    } else {
                        format!("Typed: {}", text)
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }

        // Type and press enter (shorthand)
        "typeenter" => {
            if text.is_empty() {
                return "Error: text is required for typeenter action".to_string();
            }
            match manager
                .send_keys_with_layout(name, &format!("{}\n", text), layout_override.as_deref())
                .await
            {
                Ok(_) => format!("Typed: {} + enter", text),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Number shortcuts (for menu selection)
        "num" | "number" => {
            if text.is_empty() {
                return "Error: text (the number) is required for num action".to_string();
            }
            match manager
                .send_keys_with_layout(name, &format!("{}\n", text), layout_override.as_deref())
                .await
            {
                Ok(_) => format!("Selected option: {}", text),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Function keys
        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            match manager
                .send_keys_with_layout(
                    name,
                    &format!("({}){}", action, count),
                    layout_override.as_deref(),
                )
                .await
            {
                Ok(_) => format!("Pressed {} {} time(s)", action, count),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Ctrl combinations
        "ctrl_c" | "interrupt" => {
            match manager
                .send_keys_with_layout(name, "ctrl+c", layout_override.as_deref())
                .await
            {
                Ok(_) => "Sent Ctrl+C".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "ctrl_z" => {
            match manager
                .send_keys_with_layout(name, "ctrl+z", layout_override.as_deref())
                .await
            {
                Ok(_) => "Sent Ctrl+Z".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "ctrl_a" => {
            match manager
                .send_keys_with_layout(name, "ctrl+a", layout_override.as_deref())
                .await
            {
                Ok(_) => "Sent Ctrl+A".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "ctrl_l" | "clear" => {
            match manager
                .send_keys_with_layout(name, "ctrl+l", layout_override.as_deref())
                .await
            {
                Ok(_) => "Sent Ctrl+L (clear)".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Wait (useful between actions)
        "wait" => {
            let ms = args["ms"].as_u64().unwrap_or(1000);
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            format!("Waited {}ms", ms)
        }

        // Bash/Shell mode - execute command directly via serial (no keyboard simulation)
        "bash" | "shell" | "exec" => {
            let command = args["command"].as_str().unwrap_or(text);
            if command.is_empty() {
                return "Error: command is required for bash action".to_string();
            }
            let timeout = args["timeout_secs"].as_u64().unwrap_or(30);
            match manager.shell_exec(name, command, timeout).await {
                Ok(output) => format!("Output:\n{}", output),
                Err(e) => format!("Error: {}", e),
            }
        }

        // Write file directly to VM
        "write_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");
            if path.is_empty() {
                return "Error: path is required for write_file action".to_string();
            }
            match manager.write_file(name, path, content).await {
                Ok(msg) => msg,
                Err(e) => format!("Error writing file in VM: {}", e),
            }
        }

        // System status checker
        "system_status" | "status" => {
            let checks = vec![
                (
                    "keymap",
                    "cat /etc/vconsole.conf 2>/dev/null || echo 'not set'",
                ),
                (
                    "locale",
                    "locale 2>/dev/null | head -5 || echo 'not available'",
                ),
                ("hostname", "hostname"),
                ("kernel", "uname -r"),
                ("uptime", "uptime"),
                ("disk", "df -h / | tail -1"),
                ("memory", "free -h | grep Mem"),
                (
                    "network",
                    "ip addr show 2>/dev/null | grep -E 'inet.*scope global' || echo 'no network'",
                ),
                ("users", "who 2>/dev/null || echo 'no users logged in'"),
                (
                    "services",
                    "ls /var/service/ 2>/dev/null || echo 'no services'",
                ),
            ];
            let mut status = String::new();
            for (name, cmd) in checks {
                match manager.shell_exec(name, cmd, 5).await {
                    Ok(output) => {
                        status.push_str(&format!("{}: {}\n", name, output.trim()));
                    }
                    Err(_) => {
                        status.push_str(&format!("{}: unavailable\n", name));
                    }
                }
            }
            status
        }

        // Restart a service (Void Linux runit)
        "restart_service" => {
            let service = args["service"].as_str().unwrap_or("");
            if service.is_empty() {
                return "Error: service name is required for restart_service action".to_string();
            }
            match manager
                .shell_exec(name, &format!("sv restart {}", service), 10)
                .await
            {
                Ok(output) => format!("Service '{}' restarted: {}", service, output.trim()),
                Err(e) => format!("Error restarting service '{}': {}", service, e),
            }
        }

        _ => {
            format!("Unknown action: {}. Valid actions: up/down/left/right, tab/shifttab, enter/confirm, esc/cancel, space/toggle, type, typeenter, num, f1-f12, ctrl_c/ctrl_z/ctrl_a/ctrl_l, wait, bash, write_file, system_status, restart_service", action)
        }
    }
}

async fn handle_vm_screenshot(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    match save_screenshot_to_disk(
        manager,
        name,
        args["screenshot_limit"].as_u64().unwrap_or(5000) as usize,
    )
    .await
    {
        Some(path) => format!("Screenshot saved to: {}", path),
        None => "Error: Failed to capture screenshot. Is the VM running?".to_string(),
    }
}

async fn handle_vm_file_transfer(manager: &VmManager, args: &serde_json::Value) -> String {
    let path = args["path"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let direction = args["direction"].as_str().unwrap_or("to_vm");

    if path.is_empty() {
        return "Error: path is required".to_string();
    }

    match direction {
        "to_vm" => {
            let content = args["content"].as_str().unwrap_or("");
            match manager.write_file(name, path, content).await {
                Ok(msg) => msg,
                Err(e) => format!("Error: File transfer to VM failed: {}", e),
            }
        }
        "from_vm" => match manager.read_file(name, path).await {
            Ok(content) => format!("File content from VM:\n{}", content),
            Err(e) => format!("Error reading file from VM: {}", e),
        },
        _ => "Error: direction must be 'to_vm' or 'from_vm'".to_string(),
    }
}

async fn handle_vm_snapshot(manager: &VmManager, args: &serde_json::Value) -> String {
    let snapshot_name = args["snapshot_name"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    if snapshot_name.is_empty() {
        return "Error: snapshot_name is required".to_string();
    }

    match manager.create_snapshot(name, snapshot_name).await {
        Ok(msg) => msg,
        Err(e) => format!("Error creating snapshot: {}", e),
    }
}

async fn handle_vm_shared_folder(manager: &VmManager, args: &serde_json::Value) -> String {
    let host_path = args["host_path"].as_str().unwrap_or("");
    let mount_point = args["mount_point"].as_str().unwrap_or("/mnt/shared");
    let readonly = args["readonly"].as_bool().unwrap_or(false);
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    if host_path.is_empty() {
        return "Error: host_path is required".to_string();
    }

    let tag = format!(
        "shared-{}",
        std::path::Path::new(host_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );

    let folder = crate::SharedFolder {
        host_path: host_path.to_string(),
        mount_tag: tag,
        mount_point: mount_point.to_string(),
        readonly,
    };

    match manager.add_shared_folder(name, folder).await {
        Ok(msg) => msg,
        Err(e) => format!("Error adding shared folder: {}", e),
    }
}

async fn handle_vm_mouse(manager: &VmManager, args: &serde_json::Value) -> String {
    let action = args["action"].as_str().unwrap_or("click");
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let x = args["x"].as_i64().map(|v| v as i32);
    let y = args["y"].as_i64().map(|v| v as i32);
    let dx = args["dx"].as_i64().map(|v| v as i32);
    let dy = args["dy"].as_i64().map(|v| v as i32);
    let button = args["button"].as_i64().map(|v| v as i32);
    let scroll_vertical = args["scroll_vertical"].as_i64().map(|v| v as i32);
    let scroll_horizontal = args["scroll_horizontal"].as_i64().map(|v| v as i32);

    match manager
        .send_mouse(
            name,
            action,
            x,
            y,
            dx,
            dy,
            button,
            scroll_vertical,
            scroll_horizontal,
        )
        .await
    {
        Ok(msg) => msg,
        Err(e) => format!("Error: {}", e),
    }
}

/// Take a screenshot and return just the base64 data URL (for agent loop injection)
pub async fn take_screenshot_for_context(
    manager: &VmManager,
    vm_name: &str,
    enabled: bool,
    limit: usize,
) -> Option<String> {
    if !enabled {
        return None;
    }
    match manager.screenshot(vm_name).await {
        Ok(data_url) => {
            // Enforce screenshot limit
            cleanup_screenshot_limit(manager.data_dir(), limit);
            Some(data_url)
        }
        Err(_) => None,
    }
}

/// Save screenshot to disk and return path (for vm_look_screenshot)
pub async fn save_screenshot_to_disk(
    manager: &VmManager,
    vm_name: &str,
    limit: usize,
) -> Option<String> {
    let data_dir = manager.data_dir();
    let screenshot_data = manager.screenshot(vm_name).await.ok()?;
    tracing::info!(vm = %vm_name, data_len = screenshot_data.len(), "Screenshot data received from QMP");

    let ss_dir = format!("{}/vm/{}/screenshots", data_dir, vm_name);
    std::fs::create_dir_all(&ss_dir).ok()?;

    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S_%f");
    let filename = format!("{}/screenshot_{}.png", ss_dir, ts);

    if let Some(b64) = screenshot_data.strip_prefix("data:image/ppm;base64,") {
        use base64::Engine;
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
            match ppm_to_png(&bytes) {
                Ok(png_bytes) => {
                    std::fs::write(&filename, &png_bytes).ok()?;
                    cleanup_screenshot_limit_dir(&ss_dir, limit.max(1));
                    // Clean up the temporary PPM file
                    let _ =
                        std::fs::remove_file(format!("{}/vm/{}/screenshot.ppm", data_dir, vm_name));
                    return Some(filename);
                }
                Err(e) => {
                    tracing::warn!("PPM to PNG conversion failed: {}, falling back to PPM", e);
                }
            }
            let ppm_filename = format!("{}/screenshot_{}.ppm", ss_dir, ts);
            std::fs::write(&ppm_filename, bytes).ok()?;
            cleanup_screenshot_limit_dir(&ss_dir, limit.max(1));
            let _ = std::fs::remove_file(format!("{}/vm/{}/screenshot.ppm", data_dir, vm_name));
            return Some(ppm_filename);
        }
    }
    None
}

/// Read a screenshot file and return a base64 data URL
pub fn screenshot_to_data_url(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mime = if path.ends_with(".png") {
        "image/png"
    } else {
        "image/ppm"
    };
    Some(format!("data:{};base64,{}", mime, b64))
}

async fn handle_vm_look_screenshot(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let index = args["index"].as_i64().map(|v| v as i32); // -1 = latest, 0 = oldest, N = specific
    let data_dir = manager.data_dir();

    let ss_dir = format!("{}/vm/{}/screenshots", data_dir, name);

    if !std::path::Path::new(&ss_dir).exists() {
        return "No screenshots found. Screenshots are saved when vm_auto_screenshot is enabled."
            .to_string();
    }

    // List all screenshots sorted by name (which includes timestamp)
    let mut files: Vec<String> = std::fs::read_dir(&ss_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            matches!(
                e.path().extension().and_then(|s| s.to_str()),
                Some("png" | "ppm")
            )
        })
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();

    files.sort();

    if files.is_empty() {
        return "No screenshots found.".to_string();
    }

    let file_path = match index {
        Some(-1) | None => files.last().cloned(), // latest
        Some(0) => files.first().cloned(),        // oldest
        Some(n) => {
            let idx = n as usize;
            if idx < files.len() {
                files.get(idx).cloned()
            } else {
                return format!(
                    "Screenshot index {} out of range (have {} screenshots)",
                    n,
                    files.len()
                );
            }
        }
    };

    match file_path {
        Some(path) => {
            format!("Screenshot: {}\nTotal screenshots: {}", path, files.len())
        }
        None => "No screenshot found at that index.".to_string(),
    }
}

/// Enforce screenshot limit globally
fn cleanup_screenshot_limit(data_dir: &str, limit: usize) {
    let vm_dir = format!("{}/vm", data_dir);
    if let Ok(entries) = std::fs::read_dir(&vm_dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let ss_dir = entry.path().join("screenshots");
                cleanup_screenshot_limit_dir(&ss_dir.to_string_lossy(), limit);
            }
        }
    }
}

/// Enforce screenshot limit in a specific directory
fn cleanup_screenshot_limit_dir(ss_dir: &str, max_screenshots: usize) {
    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = std::fs::read_dir(ss_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            matches!(
                e.path().extension().and_then(|s| s.to_str()),
                Some("png" | "ppm")
            )
        })
        .filter_map(|e| {
            let time = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((e.path(), time))
        })
        .collect();

    if files.len() <= max_screenshots {
        return;
    }

    // Sort oldest first
    files.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    // Delete oldest files to get back to limit
    let to_delete = files.len() - max_screenshots;
    for (path, _) in files.iter().take(to_delete) {
        let _ = std::fs::remove_file(path);
    }
    tracing::info!("Cleaned {} old screenshots from {}", to_delete, ss_dir);
}

async fn handle_vm_install(
    manager: &VmManager,
    args: &serde_json::Value,
    grants: &BTreeMap<String, String>,
) -> String {
    let iso_name = args["iso_name"].as_str().unwrap_or("");
    let iso_path_arg = args["iso_path"].as_str();
    let vm_name = args["vm_name"].as_str().unwrap_or("praxis-vm");
    let cpu_cores = args["cpu_cores"].as_u64().unwrap_or(2) as u32;
    let ram_mb = args["ram_mb"].as_u64().unwrap_or(4096) as u32;
    let disk_size = args["disk_size"].as_str().unwrap_or("40G");
    let firmware_str = args["firmware"].as_str().unwrap_or("bios");

    // Resolve ISO path: check iso_name against installation_disks, or use iso_path directly
    let iso_path = if let Some(path) = iso_path_arg {
        path.to_string()
    } else if !iso_name.is_empty() {
        // Search in installation_disks
        let isos = manager.list_isos();
        let found = isos.iter().find(|iso| {
            iso.get("name")
                .and_then(|v| v.as_str())
                .map(|n| n.to_lowercase().contains(&iso_name.to_lowercase()))
                .unwrap_or(false)
                || iso
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|p| p.to_lowercase().contains(&iso_name.to_lowercase()))
                    .unwrap_or(false)
        });
        match found {
            Some(iso) => iso
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            None => {
                // Also check the iso_dir for a file matching the name
                let iso_dir = manager.iso_dir();
                let candidates: Vec<String> = std::fs::read_dir(&iso_dir)
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        let fname = e.file_name().to_string_lossy().to_lowercase();
                        fname.contains(&iso_name.to_lowercase())
                            && (fname.ends_with(".iso") || fname.ends_with(".img"))
                    })
                    .map(|e| e.path().to_string_lossy().to_string())
                    .collect();
                if let Some(path) = candidates.first() {
                    path.clone()
                } else {
                    return format!(
                        "ISO '{}' not found in installation_disks or {}. Available ISOs: {:?}",
                        iso_name,
                        iso_dir,
                        isos.iter()
                            .filter_map(|i| i.get("name").and_then(|v| v.as_str()))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    } else {
        return "Error: either iso_name or iso_path is required".to_string();
    };

    // Verify the ISO exists
    if !std::path::Path::new(&iso_path).exists() {
        return format!("ISO not found at: {}", iso_path);
    }

    let data_dir = manager.data_dir();
    let arch = args["arch"].as_str().unwrap_or("x86_64");
    let vnc_offset = manager.list_vms().await.len() as u16 + 1;

    let mut config = crate::VmConfig::default_for_name(vm_name, &data_dir, vnc_offset, &arch);
    if let Err(e) = config.set_socket_mode(args["socket_mode"].as_str().unwrap_or(
        if cfg!(target_os = "linux") {
            "unix"
        } else {
            "tcp"
        },
    )) {
        return format!("Error: {e}");
    }
    config.cpu_cores = cpu_cores;
    config.ram_mb = ram_mb;
    config.disk_size = disk_size.to_string();
    config.iso_path = Some(iso_path.clone());
    config.firmware = crate::Firmware::from_str(firmware_str);

    // Add shared folder
    config.shared_folders.push(crate::SharedFolder {
        host_path: format!("{}/shared", data_dir),
        mount_tag: "praxis-shared".to_string(),
        mount_point: "/mnt/shared".to_string(),
        readonly: false,
    });

    match manager.start_vm_with_grants(config, grants).await {
        Ok(msg) => format!("VM '{}' started with ISO '{}'. The installer should boot.\n{}\nUse vm_keys and vm_screenshot to interact with the installer.", vm_name, iso_path, msg),
        Err(e) => format!("Error starting VM with ISO: {}", e),
    }
}

// ── New VM Tools ────────────────────────────────────────────────────────────

async fn handle_vm_process_list(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let filter = args["filter"].as_str().unwrap_or("").trim();
    let sort_by = args["sort_by"].as_str().unwrap_or("cpu");
    let limit = args["limit"].as_u64().unwrap_or(30).clamp(1, 200);

    let sort_flag = match sort_by {
        "mem" => "--sort=-%mem",
        "pid" => "--sort=pid",
        _ => "--sort=-%cpu",
    };
    // Always include the header (first line) when grepping.
    let mut cmd = format!("ps aux {sort_flag}");
    if !filter.is_empty() {
        // shell_quote ensures the filter substring can't break out.
        cmd.push_str(&format!(
            " | awk 'NR==1 || /{}/'",
            filter.replace('/', "\\/").replace('\'', "")
        ));
    }
    cmd.push_str(&format!(" | head -{}", limit + 1)); // +1 for header
    match manager.shell_exec(name, &cmd, 10).await {
        Ok(output) => format!("Running processes (sort={sort_by}, filter={filter:?}):\n{output}"),
        Err(e) => format!("Error: {}", e),
    }
}

async fn handle_vm_file_read(manager: &VmManager, args: &serde_json::Value) -> String {
    let path = args["path"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let start_line = args["start_line"].as_u64();
    let line_count = args["line_count"].as_u64();
    let max_bytes = args["max_bytes"]
        .as_u64()
        .unwrap_or(65536)
        .clamp(256, 1_048_576) as usize;

    if path.is_empty() {
        return "Error: path is required".to_string();
    }

    // If a line range is requested, use sed to read only the requested slice
    // — much friendlier on huge files than read_file.
    let raw = if start_line.is_some() || line_count.is_some() {
        let start = start_line.unwrap_or(1).max(1);
        let count = line_count.unwrap_or(500).max(1);
        let end = start.saturating_add(count.saturating_sub(1));
        let cmd = format!("sed -n '{start},{end}p' {}", shell_quote(path));
        match manager.shell_exec(name, &cmd, 30).await {
            Ok(s) => s,
            Err(e) => return format!("Error reading file '{}': {}", path, e),
        }
    } else {
        match manager.read_file(name, path).await {
            Ok(s) => s,
            Err(e) => return format!("Error reading file '{}': {}", path, e),
        }
    };

    let truncated = if raw.len() > max_bytes {
        let mut t = raw.chars().take(max_bytes).collect::<String>();
        t.push_str("\n[truncated]");
        t
    } else {
        raw
    };
    format!("Content of {}:\n{}", path, truncated)
}

async fn handle_vm_network_test(manager: &VmManager, args: &serde_json::Value) -> String {
    let action = args["action"].as_str().unwrap_or("interfaces");
    let target = args["target"].as_str().unwrap_or("").trim();
    let port = args["port"].as_u64();
    let count = args["count"].as_u64().unwrap_or(4).clamp(1, 30);
    let timeout = args["timeout_secs"].as_u64().unwrap_or(10).clamp(1, 120);
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    let cmd = match action {
        "ping" => {
            if target.is_empty() {
                return "Error: target host is required for ping".to_string();
            }
            format!("ping -c {count} -W {timeout} {}", shell_quote(target))
        }
        "tcp" => {
            let Some(p) = port else {
                return "Error: port is required for tcp test".to_string();
            };
            if target.is_empty() {
                return "Error: target host is required for tcp test".to_string();
            }
            // Use bash's /dev/tcp pseudo-device — works on Void without extra tooling.
            format!(
                "timeout {timeout} bash -c 'cat < /dev/tcp/{}/{p}' >/dev/null 2>&1 && echo 'OPEN: {} {p}' || echo 'CLOSED or unreachable: {} {p}'",
                shell_quote(target),
                shell_quote(target),
                shell_quote(target),
            )
        }
        "curl" => {
            if target.is_empty() {
                return "Error: URL is required for curl".to_string();
            }
            format!(
                "curl -sIL --max-time {timeout} {} | head -20",
                shell_quote(target)
            )
        }
        "dns" => {
            if target.is_empty() {
                return "Error: domain is required for dns".to_string();
            }
            let q = shell_quote(target);
            format!("nslookup {q} 2>&1 || host {q} 2>&1 || dig {q} 2>&1")
        }
        "interfaces" => "ip addr show 2>/dev/null || ifconfig".to_string(),
        "routes" => "ip route show 2>/dev/null || route -n".to_string(),
        _ => {
            return format!(
                "Unknown action: {action}. Use: ping, tcp, curl, dns, interfaces, routes"
            )
        }
    };

    match manager.shell_exec(name, &cmd, timeout + 5).await {
        Ok(output) => format!("Network {action}:\n{output}"),
        Err(e) => format!("Error: {e}"),
    }
}

async fn handle_vm_service_list(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    // List services and their status (Void Linux runit)
    let cmd = "for svc in /var/service/*; do name=$(basename $svc); if [ -f $svc/supervise/ok ]; then status='running'; else status='stopped'; fi; echo \"$name: $status\"; done 2>/dev/null || ls -la /var/service/ 2>/dev/null";

    match manager.shell_exec(name, cmd, 10).await {
        Ok(output) => format!("Services:\n{}", output),
        Err(e) => format!("Error: {}", e),
    }
}

async fn handle_vm_package_install(manager: &VmManager, args: &serde_json::Value) -> String {
    let packages = args["packages"].as_str().unwrap_or("").trim();
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    let update_first = args["update_first"].as_bool().unwrap_or(true);
    let assume_yes = args["assume_yes"].as_bool().unwrap_or(true);

    if packages.is_empty() {
        return "Error: packages is required".to_string();
    }

    // Whitelist package names — alphanum, dash, dot, plus, underscore.
    // This blocks shell-meta-character injection through the `packages` arg.
    for pkg in packages.split_whitespace() {
        if !pkg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.'))
        {
            return format!("Error: invalid package name `{pkg}`");
        }
    }

    let mut flags = String::from("-S");
    if update_first {
        flags.push('u');
    }
    if assume_yes {
        flags.push('y');
    }
    let cmd = format!("xbps-install {flags} {}", packages);
    match manager.shell_exec(name, &cmd, 300).await {
        Ok(output) => format!("Package install result:\n{output}"),
        Err(e) => format!("Error installing packages: {e}"),
    }
}

// ── Snapshot Tools ──────────────────────────────────────────────────────────

async fn handle_vm_snapshot_list(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");
    match manager
        .shell_exec(
            name,
            "qemu-img snapshot -l /data/vm/*/disk.qcow2 2>/dev/null || echo 'No snapshots found'",
            10,
        )
        .await
    {
        Ok(output) => format!("Snapshots:\n{}", output),
        Err(e) => format!("Error listing snapshots: {}", e),
    }
}

async fn handle_vm_snapshot_restore(manager: &VmManager, args: &serde_json::Value) -> String {
    let snapshot_name = args["snapshot_name"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    if snapshot_name.is_empty() {
        return "Error: snapshot_name is required".to_string();
    }

    // This requires VM to be stopped first, then restore, then start
    match manager.stop_vm(name).await {
        Ok(_) => {
            let data_dir = manager.data_dir();
            let disk_path = format!("{}/vm/{}/disk.qcow2", data_dir, name);
            match manager
                .shell_exec(
                    name,
                    &format!("qemu-img snapshot -a {} {}", snapshot_name, disk_path),
                    30,
                )
                .await
            {
                Ok(_) => {
                    match manager
                        .start_vm(crate::VmConfig::default_for_name(
                            name, &data_dir, 1, "x86_64",
                        ))
                        .await
                    {
                        Ok(msg) => format!(
                            "Snapshot '{}' restored. VM restarted.\n{}",
                            snapshot_name, msg
                        ),
                        Err(e) => format!("Error: Snapshot restored but VM restart failed: {}", e),
                    }
                }
                Err(e) => format!("Error restoring snapshot: {}", e),
            }
        }
        Err(e) => format!("Error stopping VM for snapshot restore: {}", e),
    }
}

async fn handle_vm_snapshot_delete(manager: &VmManager, args: &serde_json::Value) -> String {
    let snapshot_name = args["snapshot_name"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    if snapshot_name.is_empty() {
        return "Error: snapshot_name is required".to_string();
    }

    let data_dir = manager.data_dir();
    let disk_path = format!("{}/vm/{}/disk.qcow2", data_dir, name);
    match manager
        .shell_exec(
            name,
            &format!("qemu-img snapshot -d {} {}", snapshot_name, disk_path),
            10,
        )
        .await
    {
        Ok(output) => format!("Snapshot '{}' deleted: {}", snapshot_name, output),
        Err(e) => format!("Error deleting snapshot: {}", e),
    }
}

// ── Cron Job Tools ──────────────────────────────────────────────────────────

async fn handle_vm_wait_for_text(manager: &VmManager, args: &serde_json::Value) -> String {
    let text = args["text"].as_str().unwrap_or("");
    let case_sensitive = args["case_sensitive"].as_bool().unwrap_or(false);
    let regex_mode = args["regex"].as_bool().unwrap_or(false);
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(60).clamp(1, 1800);
    let interval_secs = args["interval_secs"].as_u64().unwrap_or(5).clamp(1, 120);
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    if text.is_empty() {
        return "Error: text is required".to_string();
    }

    let pattern = if regex_mode {
        let raw = match regex::Regex::new(text) {
            Ok(_) => text.to_string(),
            Err(e) => return format!("Error: invalid regex: {e}"),
        };
        if case_sensitive {
            raw
        } else {
            format!("(?i){raw}")
        }
    } else if case_sensitive {
        regex::escape(text)
    } else {
        format!("(?i){}", regex::escape(text))
    };
    let re = match regex::Regex::new(&pattern) {
        Ok(r) => r,
        Err(e) => return format!("Error: failed to compile pattern: {e}"),
    };

    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    loop {
        if start.elapsed() > timeout {
            return format!("Timeout: '{text}' not seen after {timeout_secs}s");
        }
        // Snapshot screen + screen buffer; we match on the buffer only
        // (true OCR would require a heavier dependency).
        let _ = manager.screenshot(name).await;
        match manager
            .shell_exec(name, "cat /tmp/screen_buffer 2>/dev/null || true", 5)
            .await
        {
            Ok(buf) if re.is_match(&buf) => {
                return format!(
                    "Found '{text}' on screen after {:.1}s",
                    start.elapsed().as_secs_f32()
                );
            }
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
    }
}

// ── Window Management Tools ─────────────────────────────────────────────────

async fn handle_vm_window_list(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    // Try wmctrl first, then xdotool
    let cmd = "wmctrl -l 2>/dev/null || xdotool search --name '' getwindowname 2>/dev/null || echo 'No window manager detected'";

    match manager.shell_exec(name, cmd, 10).await {
        Ok(output) => format!("Windows:\n{}", output),
        Err(e) => format!("Error listing windows: {}", e),
    }
}

async fn handle_vm_window_focus(manager: &VmManager, args: &serde_json::Value) -> String {
    let window = args["window"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    if window.is_empty() {
        return "Error: window is required".to_string();
    }

    // Try wmctrl first, then xdotool
    let cmd = format!("wmctrl -a '{}' 2>/dev/null || xdotool search --name '{}' windowactivate 2>/dev/null || echo 'Could not focus window'", window, window);

    match manager.shell_exec(name, &cmd, 10).await {
        Ok(output) => format!("Window focus result: {}", output),
        Err(e) => format!("Error focusing window: {}", e),
    }
}

// ── Clipboard Tools ─────────────────────────────────────────────────────────

async fn handle_vm_clipboard_set(manager: &VmManager, args: &serde_json::Value) -> String {
    let content = args["content"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    if content.is_empty() {
        return "Error: content is required".to_string();
    }

    // Write content to temp file, then use xclip
    let cmd = format!("echo '{}' > /tmp/clipboard_content && xclip -selection clipboard < /tmp/clipboard_content 2>/dev/null || xsel --clipboard < /tmp/clipboard_content 2>/dev/null || echo 'Clipboard tool not available'", content);

    match manager.shell_exec(name, &cmd, 10).await {
        Ok(output) => {
            if output.contains("not available") {
                "Error: xclip or xsel not installed. Install with: xbps-install xclip".to_string()
            } else {
                format!("Clipboard set to: {}", content)
            }
        }
        Err(e) => format!("Error setting clipboard: {}", e),
    }
}

async fn handle_vm_clipboard_get(manager: &VmManager, args: &serde_json::Value) -> String {
    let name = args["name"].as_str().unwrap_or("praxis-vm");

    let cmd = "xclip -selection clipboard -o 2>/dev/null || xsel --clipboard 2>/dev/null || echo 'Clipboard tool not available'";

    match manager.shell_exec(name, cmd, 10).await {
        Ok(output) => {
            if output.contains("not available") {
                "Error: xclip or xsel not installed. Install with: xbps-install xclip".to_string()
            } else {
                format!("Clipboard content:\n{}", output)
            }
        }
        Err(e) => format!("Error getting clipboard: {}", e),
    }
}

// ── Mouse Tools ─────────────────────────────────────────────────────────────
// (low-level per-action mouse handlers were removed; use `vm_mouse` which
// dispatches `move_absolute`/`click`/`double_click`/`drag`/`scroll` via a
// single `action` parameter.)
