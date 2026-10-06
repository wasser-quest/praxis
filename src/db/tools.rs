use super::Database;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    pub parameters: serde_json::Value,
    pub is_enabled: bool,
}

fn tools_file(db: &Database) -> std::path::PathBuf {
    db.data_dir().join("tools.json")
}

fn plugin_tools_file(db: &Database) -> std::path::PathBuf {
    db.data_dir().join("plugin_tools.json")
}

fn load_plugin_tools(db: &Database) -> anyhow::Result<HashMap<String, bool>> {
    let path = plugin_tools_file(db);
    if path.exists() {
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    } else {
        Ok(HashMap::new())
    }
}

fn save_plugin_tools(db: &Database, tools: &HashMap<String, bool>) -> anyhow::Result<()> {
    let path = plugin_tools_file(db);
    let json = serde_json::to_string_pretty(tools)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// One-time move of a bundled package's legacy per-tool flags. Only absent
/// entries are written: an explicit plugin flag is the operator's later choice.
fn migrate_bundled_flags(db: &Database, rows: &[Tool]) -> anyhow::Result<()> {
    let mut flags = load_plugin_tools(db)?;
    let mut changed = false;
    for row in rows {
        if crate::tools::packages::bundled_tool(&row.name).is_some()
            && !flags.contains_key(&row.name)
        {
            tracing::info!(tool = %row.name, "Moving bundled tool flag to the plugin flag store");
            flags.insert(row.name.clone(), row.is_enabled);
            changed = true;
        }
    }
    if changed {
        save_plugin_tools(db, &flags)?;
    }
    Ok(())
}

pub fn get_plugin_tool_enabled(db: &Database, name: &str) -> bool {
    plugin_tool_enabled(db, name).unwrap_or(false)
}

/// Native VM ownership moved to a plugin. Keep old operator choices until an
/// explicit plugin flag overrides them, without enabling tools on startup.
pub fn plugin_tool_enabled(db: &Database, name: &str) -> anyhow::Result<bool> {
    // A kernel-bundled package's switch is the only way to make its tools
    // absent; per-tool flags are preserved underneath it.
    if !crate::tools::packages::bundled_tool_enabled(&db.data_dir(), name)? {
        return Ok(false);
    }
    if let Some(enabled) = load_plugin_tools(db)?.get(name) { return Ok(*enabled); }
    let row = load_tools(db)?.into_iter().find(|t| t.name == name);
    if crate::runtime::vm::is_vm_tool(name) {
        // VM adoption: a stale row keeps the choice; no row means not adopted.
        return Ok(row.is_some_and(|t| t.is_enabled));
    }
    // Legacy rows keep the operator's earlier choice until an explicit plugin
    // flag overrides them (migrated bundled tools rely on this).
    Ok(row.is_none_or(|t| t.is_enabled))
}

/// Effective switch for a tool name: the persisted builtin row when one exists,
/// otherwise the plugin flag store that bundled packages and plugins use.
pub fn tool_enabled(db: &Database, name: &str) -> anyhow::Result<bool> {
    if let Some(tool) = load_tools(db)?.into_iter().find(|t| t.name == name) {
        return Ok(tool.is_enabled);
    }
    plugin_tool_enabled(db, name)
}

pub fn set_plugin_tool_enabled(db: &Database, name: &str, enabled: bool) -> anyhow::Result<()> {
    let mut tools = load_plugin_tools(db)?;
    tools.insert(name.to_string(), enabled);
    save_plugin_tools(db, &tools)
}

pub fn list_plugin_tools(db: &Database) -> anyhow::Result<HashMap<String, bool>> {
    load_plugin_tools(db)
}

/// An operator-invoked compatibility install, never a startup side effect.
pub fn enable_vm_compatibility(db: &Database) -> anyhow::Result<()> {
    let mut flags = load_plugin_tools(db)?;
    for name in crate::runtime::vm::TOOL_NAMES { flags.insert((*name).into(), true); }
    save_plugin_tools(db, &flags)
}

fn load_tools(db: &Database) -> anyhow::Result<Vec<Tool>> {
    let path = tools_file(db);
    if path.exists() {
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    } else {
        Ok(Vec::new())
    }
}

fn save_tools(db: &Database, tools: &[Tool]) -> anyhow::Result<()> {
    let path = tools_file(db);
    let json = serde_json::to_string_pretty(tools)?;
    std::fs::write(&path, json)?;
    Ok(())
}

pub fn list_enabled(db: &Database) -> anyhow::Result<Vec<Tool>> {
    let tools = load_tools(db)?;
    Ok(tools.into_iter().filter(|t| t.is_enabled).collect())
}

pub fn list(db: &Database) -> anyhow::Result<Vec<Tool>> {
    load_tools(db)
}

pub fn get(db: &Database, name: &str) -> anyhow::Result<Tool> {
    let tools = load_tools(db)?;
    tools
        .into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))
}

pub fn save(db: &Database, tool: &Tool) -> anyhow::Result<()> {
    let mut tools = load_tools(db)?;
    if let Some(existing) = tools.iter_mut().find(|t| t.name == tool.name) {
        *existing = tool.clone();
    } else {
        tools.push(tool.clone());
    }
    save_tools(db, &tools)?;
    Ok(())
}

pub fn enable(db: &Database, name: &str) -> anyhow::Result<()> {
    let mut tools = load_tools(db)?;
    if let Some(tool) = tools.iter_mut().find(|t| t.name == name) {
        tool.is_enabled = true;
        save_tools(db, &tools)?;
    }
    Ok(())
}

pub fn disable(db: &Database, name: &str) -> anyhow::Result<()> {
    let mut tools = load_tools(db)?;
    if let Some(tool) = tools.iter_mut().find(|t| t.name == name) {
        tool.is_enabled = false;
        save_tools(db, &tools)?;
    }
    Ok(())
}

pub fn set_enabled(db: &Database, name: &str, enabled: bool) -> anyhow::Result<()> {
    let mut tools = load_tools(db)?;
    if let Some(tool) = tools.iter_mut().find(|t| t.name == name) {
        tool.is_enabled = enabled;
        save_tools(db, &tools)?;
    }
    Ok(())
}

pub fn init_default_tools(db: &Database) -> anyhow::Result<()> {
    let existing = load_tools(db)?;
    if existing.is_empty() {
        // Fresh install: write all defaults
        let defaults = get_default_tools();
        save_tools(db, &defaults)?;
        return Ok(());
    }

    // Existing install: add missing tools, update parameters for known tools,
    // and remove tools that have been deprecated/dropped from the registry.
    let mut tools = existing;
    let defaults = get_default_tools();
    let default_names: std::collections::HashSet<String> =
        defaults.iter().map(|t| t.name.clone()).collect();

    // Tools that moved to a kernel-bundled package keep the operator's choice:
    // copy their flags into the plugin store before the legacy rows go away.
    migrate_bundled_flags(db, &tools)?;

    let mut changed = false;

    // Drop any persisted tool that is no longer in the defaults. This keeps
    // the on-disk registry in sync after we remove tools (e.g. duplicate or
    // deprecated VM input/cron variants).
    let before = tools.len();
    tools.retain(|t| {
        let keep = default_names.contains(&t.name);
        if !keep {
            tracing::info!("Removing stale/deprecated tool from registry: {}", t.name);
        }
        keep
    });
    if tools.len() != before {
        changed = true;
    }

    for default in &defaults {
        if let Some(existing) = tools.iter_mut().find(|t| t.name == default.name) {
            // Update parameters and description from defaults (preserves is_enabled)
            if existing.parameters != default.parameters
                || existing.description != default.description
            {
                existing.parameters = default.parameters.clone();
                existing.description = default.description.clone();
                changed = true;
                tracing::info!("Updated default tool parameters: {}", default.name);
            }
        } else {
            tracing::info!("Adding missing default tool: {}", default.name);
            tools.push(default.clone());
            changed = true;
        }
    }
    if changed {
        save_tools(db, &tools)?;
    }
    Ok(())
}

/// Canonical built-in contracts shared by discovery and state-based routing.
/// Tools of a kernel-bundled package (`runtime_control`) are deliberately not
/// seeded here: their names, schemas and ownership come from the bundled
/// manifest under `packages/`, while the implementations stay host-owned.
pub(crate) fn get_default_tools() -> Vec<Tool> {
    vec![
        crate::tools::apply_patch::definition(),
        crate::tools::apply_patch::inspect_definition(),
        Tool {
            name: "execute_terminal".into(),
            description: Some("Run shell command. Long-running commands: use run_background instead, then poll with background_status (finished jobs also announce themselves).".into()),
            parameters: serde_json::json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
            is_enabled: true,
        },
        Tool {
            name: "run_background".into(),
            description: Some("Start a long-running shell command DETACHED (compile, download, server). Returns a job id immediately so you can keep working. The job announces its completion to the user's dashboard automatically; poll background_status only if you need the result mid-task.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string"}},"required":["command"]}),
            is_enabled: true,
        },
        Tool {
            name: "background_status".into(),
            description: Some("Check status/output of a detached background command. Call without job_id to list all jobs.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"job_id":{"type":"string"}}}),
            is_enabled: true,
        },
        Tool {
            name: "write_file".into(),
            description: Some("Create or overwrite file. For a checked transactional write, pass expected_absent=true (create only) or expected_sha256 from inspect_file (replace only); without a precondition this is a raw write.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"expected_absent":{"type":"boolean","description":"Require that the file does not exist (versioned checked write)"},"expected_sha256":{"type":"string","description":"Require this lowercase SHA-256 before replacing (from inspect_file)"}},"required":["path","content"]}),
            is_enabled: true,
        },
        Tool {
            name: "edit_file".into(),
            description: Some("Edit file with find/replace".into()),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"}},"required":["path","old_string","new_string"]}),
            is_enabled: true,
        },
        Tool {
            name: "read_file".into(),
            description: Some("Read file contents".into()),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
            is_enabled: true,
        },
        Tool {
            name: "discord_upload_file".into(),
            description: Some("Upload file to Discord channel. If channel_id is omitted, sends to the channel where the request originated.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"channel_id":{"type":"string","description":"Discord channel ID. If omitted, uses the originating channel."},"filename":{"type":"string","description":"Display filename for the attachment"},"base64_content":{"type":"string","description":"Base64-encoded file content"},"message":{"type":"string","description":"Optional message text"}},"required":["filename","base64_content"]}),
            is_enabled: true,
        },
        Tool {
            name: "discord_send_message".into(),
            description: Some("Send message to Discord channel. If channel_id is omitted, sends to the originating channel.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"channel_id":{"type":"string","description":"Discord channel ID. If omitted, uses the originating channel."},"message":{"type":"string"}},"required":["message"]}),
            is_enabled: true,
        },
        Tool {
            name: "discord_send_embed".into(),
            description: Some("Send rich embed to Discord channel. If channel_id is omitted, sends to the originating channel.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"channel_id":{"type":"string","description":"Discord channel ID. If omitted, uses the originating channel."},"title":{"type":"string"},"description":{"type":"string"},"url":{"type":"string"},"color":{"type":["string","number"],"description":"Hex color (e.g. '6C5CE7' or '#FF0000') or integer"},"footer":{"type":"string"},"author":{"type":"string"},"thumbnail":{"type":"string","description":"URL to thumbnail image"},"image":{"type":"string","description":"URL to full image"},"fields":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"value":{"type":"string"},"inline":{"type":"boolean"}},"required":["name","value"]}}},"required":[]}),
            is_enabled: true,
        },
        // RAG Tools
        // Cron Tools
        Tool {
            name: "understand_image".into(),
            description: Some("Load an image for visual analysis. The image is sent to the vision API - you WILL see and understand the image content in your next response. After calling this tool, describe what you see in the image. Use for screenshots, photos, or any visual content.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the image file (supports PPM, PNG, JPEG)"},"prompt":{"type":"string","description":"What to look for in the image. E.g. 'What menu options are shown?' or 'Describe the installation step shown'."}},"required":["path","prompt"]}),
            is_enabled: true,
        },
        // VM Tools (only enabled when VM=true)
        Tool {
            name: "vm_start".into(),
            description: Some("Start a QEMU VM. Creates a new VM if none exists. The VM runs a full Linux environment you control.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","default":"praxis-vm","description":"VM name"},"cpu_cores":{"type":"integer","default":2},"ram_mb":{"type":"integer","default":4096},"disk_size":{"type":"string","default":"40G"},"iso_path":{"type":"string","description":"Path to ISO for OS installation"},"arch":{"type":"string","enum":["x86_64","aarch64"],"default":"x86_64"},"keyboard_layout":{"type":"string","enum":["us","de","fr","es","it","gb"],"default":"us","description":"Keyboard layout for the VM (us, de, fr, es, it, gb)"},"firmware":{"type":"string","enum":["bios","uefi"],"default":"bios","description":"Boot firmware: 'bios' (legacy) or 'uefi' (OVMF). Use 'uefi' for modern OSes that require EFI boot."}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_stop".into(),
            description: Some("Stop a running VM".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","default":"praxis-vm"},"force":{"type":"boolean","default":false,"description":"Force kill instead of graceful shutdown"}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_shell".into(),
            description: Some(
                "Run a command inside the VM via the QEMU serial console.\n\
                Returns combined stdout/stderr and the exit code as a single string.\n\
                The shell runs as root in `/root` by default. For long-running tasks, raise `timeout_secs`.\n\n\
                Tips for the LLM:\n\
                - Prefer this over `vm_keys`/`vm_input` for anything you can do non-interactively\n\
                  (it's much faster and gives you the output directly).\n\
                - Use `cwd` to keep your reasoning self-contained (no `cd …; …` chains).\n\
                - Use `stdin` to feed input to commands like `tee`, `sort`, `python -`.\n\
                - Use `env` to inject environment variables for one invocation.\n\
                - Combine commands with `&&`, `;`, or pipelines as you would in bash.".into(),
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute. Bash syntax. Examples: 'ls -la /etc', 'cat /etc/os-release', 'systemctl status sshd', 'python3 -c \"print(1+1)\"'."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory. Defaults to /root. Example: '/var/log'."
                    },
                    "stdin": {
                        "type": "string",
                        "description": "Optional text piped to the command's stdin. Useful for `tee`, `sort`, `python -`, etc."
                    },
                    "env": {
                        "type": "object",
                        "description": "Extra environment variables for this command, e.g. {\"DEBIAN_FRONTEND\":\"noninteractive\"}.",
                        "additionalProperties": {"type": "string"}
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "default": 30,
                        "minimum": 1,
                        "maximum": 1800,
                        "description": "Hard timeout in seconds. Raise for slow commands like `xbps-install` or `make`."
                    },
                    "name": {"type": "string", "default": "praxis-vm", "description": "VM name."}
                },
                "required": ["command"]
            }),
            is_enabled: false,
        },
        Tool {
            name: "vm_keys".into(),
            description: Some("Send keyboard input to the VM. Supports:\n\
                - Text: 'root', 'ls -la', 'hello world'\n\
                - Special keys: 'enter', 'esc', 'tab', 'backspace', 'space', 'delete', 'insert', 'home', 'end', 'pageup', 'pagedown'\n\
                - Arrow keys: 'arrow_up', 'arrow_down', 'arrow_left', 'arrow_right'\n\
                - Function keys: 'f1'-'f24'\n\
                - Modifiers: 'ctrl', 'alt', 'shift', 'super'/'meta'/'win'\n\
                - Numpad: 'kp0'-'kp9', 'kp_enter', 'kp_plus', 'kp_minus', 'kp_multiply', 'kp_divide', 'kp_dot'\n\
                - Symbols: - = [ ] \\ ; ' ` , . / and shifted: ! @ # $ % ^ & * ( ) _ + { } | : \" ~ < > ?\n\
                - Modifier combos: 'ctrl+a', 'alt+tab', 'ctrl+alt+delete'\n\
                - Macros (repeat): '(key)count' - e.g. '(arrowdown)5' presses down 5 times, '(enter)3' presses enter 3 times, '(tab)2' presses tab twice\n\
                - Newlines '\\n' are converted to enter key\n\
                - Use keyboard_layout parameter to set VM keyboard layout (us, de, fr, es, it, gb)".into()),
            parameters: serde_json::json!({"type":"object","properties":{"keys":{"type":"string","description":"Text, special key, or macro. Examples: 'root\\n', '(arrowdown)5', 'ctrl+c', 'enter'"},"name":{"type":"string","default":"praxis-vm"},"keyboard_layout":{"type":"string","enum":["us","de","fr","es","it","gb"],"default":"us","description":"Keyboard layout for this input"}},"required":["keys"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_input".into(),
            description: Some("High-level VM input tool for easy navigation. Vim-like shortcuts:\n\
                - Navigation: up/k, down/j, left/h, right/l (with count)\n\
                - Pages: pageup/pgup, pagedown/pgdn, home, end\n\
                - Tab: tab, shifttab/backtab\n\
                - Actions: enter/confirm/select, esc/cancel/back, space/toggle, backspace/delete\n\
                - Type: type (text), typeenter (text+enter), num (number+enter)\n\
                - Function keys: f1-f12\n\
                - Ctrl: ctrl_c (interrupt), ctrl_z, ctrl_a, ctrl_l (clear)\n\
                - Wait: wait (ms parameter)\n\
                - Bash: bash (command) - Execute command directly via serial, returns output\n\
                - File: write_file (path, content) - Write file directly to VM\n\
                - Status: system_status - Check keymap, locale, network, disk, memory, services\n\
                - Service: restart_service (service) - Restart a Void Linux service\n\
                - Use keyboard_layout parameter to set VM keyboard layout (us, de, fr, es, it, gb)\n\
                Examples: {action:'bash', command:'ls -la'}, {action:'write_file', path:'/tmp/test.txt', content:'hello'}, {action:'system_status'}".into()),
            parameters: serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"Action: up/down/left/right, tab/shifttab, enter/confirm, esc/cancel, space/toggle, type, typeenter, num, f1-f12, ctrl_c, wait"},"text":{"type":"string","description":"Text for type/typeenter/num actions"},"count":{"type":"integer","default":1,"description":"Repeat count for navigation"},"ms":{"type":"integer","default":1000,"description":"Wait time in ms for wait action"},"enter":{"type":"boolean","default":false,"description":"Press enter after type"},"name":{"type":"string","default":"praxis-vm"},"keyboard_layout":{"type":"string","enum":["us","de","fr","es","it","gb"],"default":"us","description":"Keyboard layout for this input"}},"required":["action"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_screenshot".into(),
            description: Some("Take a screenshot of the VM display. Returns base64-encoded image. Use this to see what's on the VM screen (GUI, TUI apps, etc).".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","default":"praxis-vm"}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_file_transfer".into(),
            description: Some("Transfer a file to/from the VM. Use direction='to_vm' to upload or 'from_vm' to download. Content is base64 encoded.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"File path in VM"},"content":{"type":"string","description":"File content (base64 for binary, plain text for text)"},"direction":{"type":"string","enum":["to_vm","from_vm"],"default":"to_vm"},"name":{"type":"string","default":"praxis-vm"}},"required":["path"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_snapshot".into(),
            description: Some("Create a VM snapshot for later restore. Great for saving state before risky operations.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"snapshot_name":{"type":"string","description":"Name for the snapshot"},"name":{"type":"string","default":"praxis-vm"}},"required":["snapshot_name"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_shared_folder".into(),
            description: Some("Add a shared folder between host and VM. Files in host_path will be accessible at mount_point inside the VM. Requires VM restart.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"host_path":{"type":"string","description":"Path on the host machine"},"mount_point":{"type":"string","description":"Mount path inside VM","default":"/mnt/shared"},"readonly":{"type":"boolean","default":false},"name":{"type":"string","default":"praxis-vm"}},"required":["host_path"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_mouse".into(),
            description: Some("Control the mouse in the VM. Actions: move_absolute (x,y), move_relative (dx,dy), click (x,y,button), double_click, drag (x,y to dx,dy), scroll (vertical/horizontal). Button: 0=left, 1=middle, 2=right.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"action":{"type":"string","enum":["move_absolute","move_relative","click","double_click","drag","scroll"]},"x":{"type":"integer"},"y":{"type":"integer"},"dx":{"type":"integer"},"dy":{"type":"integer"},"button":{"type":"integer","description":"0=left,1=middle,2=right","default":0},"scroll_vertical":{"type":"integer","description":"Positive=up, negative=down"},"scroll_horizontal":{"type":"integer","description":"Positive=left, negative=right"},"name":{"type":"string","default":"praxis-vm"}},"required":["action"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_look_screenshot".into(),
            description: Some("Look at a saved VM screenshot. index: -1=latest, 0=oldest, N=specific screenshot number. Returns base64 image. Max 500 screenshots are kept (oldest auto-deleted).".into()),
            parameters: serde_json::json!({"type":"object","properties":{"index":{"type":"integer","description":"-1=latest, 0=oldest, N=specific","default":-1},"name":{"type":"string","default":"praxis-vm"}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_process_list".into(),
            description: Some(
                "List running processes in the VM as a `ps aux` table.\n\
                Use to check what's running, find PIDs to kill, or audit resource hogs.".into(),
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "string",
                        "description": "Optional substring to grep for (matches against the full command line). Example: 'nginx', 'python'."
                    },
                    "sort_by": {
                        "type": "string",
                        "enum": ["cpu", "mem", "pid"],
                        "default": "cpu",
                        "description": "Sort key. 'cpu' (default) shows hot processes first, 'mem' shows memory hogs, 'pid' is creation order."
                    },
                    "limit": {
                        "type": "integer",
                        "default": 30,
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum number of rows to return."
                    },
                    "name": {"type": "string", "default": "praxis-vm"}
                }
            }),
            is_enabled: false,
        },
        Tool {
            name: "vm_file_read".into(),
            description: Some(
                "Read a file from the VM. Returns its content as a string.\n\
                Supports line-range reads for big files (logs, configs).\n\
                Use this instead of `vm_shell` with `cat` so you don't truncate by accident.".into(),
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute file path inside the VM, e.g. '/var/log/messages'."
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional: 1-based first line to return. If omitted, starts at 1."
                    },
                    "line_count": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional: max number of lines to return from start_line. Combine with start_line to page through large files."
                    },
                    "max_bytes": {
                        "type": "integer",
                        "default": 65536,
                        "minimum": 256,
                        "maximum": 1048576,
                        "description": "Hard cap on returned size in bytes. Defaults to 64 KiB; the file is truncated and a `[truncated]` marker is appended."
                    },
                    "name": {"type": "string", "default": "praxis-vm"}
                },
                "required": ["path"]
            }),
            is_enabled: false,
        },
        Tool {
            name: "vm_network_test".into(),
            description: Some(
                "Diagnose network from inside the VM.\n\n\
                Actions:\n\
                - `ping`: ICMP echo (provide host in `target`).\n\
                - `tcp`: open a TCP connection (provide host in `target` and `port`).\n\
                - `curl`: HTTP(S) GET (provide URL in `target`).\n\
                - `dns`: resolve a hostname (provide name in `target`).\n\
                - `interfaces`: list NICs and their IPs.\n\
                - `routes`: show the routing table.".into(),
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["ping", "tcp", "curl", "dns", "interfaces", "routes"],
                        "default": "interfaces"
                    },
                    "target": {
                        "type": "string",
                        "description": "Host (for ping/tcp/dns) or URL (for curl). Required for ping/tcp/curl/dns."
                    },
                    "port": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 65535,
                        "description": "TCP port for action='tcp'. E.g. 22, 80, 443."
                    },
                    "count": {
                        "type": "integer",
                        "default": 4,
                        "minimum": 1,
                        "maximum": 30,
                        "description": "Probe count for action='ping'."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "default": 10,
                        "minimum": 1,
                        "maximum": 120,
                        "description": "Per-action timeout."
                    },
                    "name": {"type": "string", "default": "praxis-vm"}
                }
            }),
            is_enabled: false,
        },
        Tool {
            name: "vm_service_list".into(),
            description: Some("List all services in the VM and their status (Void Linux runit). Shows running/stopped services.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","default":"praxis-vm"}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_package_install".into(),
            description: Some(
                "Install one or more packages in the VM via xbps-install (Void Linux).\n\
                The repo index is synced first by default so you don't get a stale-cache failure.".into(),
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "packages": {
                        "type": "string",
                        "description": "Space-separated package names. Example: 'git curl htop'."
                    },
                    "update_first": {
                        "type": "boolean",
                        "default": true,
                        "description": "Run `xbps-install -Suy` before the install. Set false if the index was just synced."
                    },
                    "assume_yes": {
                        "type": "boolean",
                        "default": true,
                        "description": "Pass -y so xbps doesn't prompt for confirmation."
                    },
                    "name": {"type": "string", "default": "praxis-vm"}
                },
                "required": ["packages"]
            }),
            is_enabled: false,
        },
        Tool {
            name: "vm_snapshot_list".into(),
            description: Some("List all snapshots for the VM. Shows snapshot names and creation info.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","default":"praxis-vm"}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_snapshot_restore".into(),
            description: Some("Restore a VM snapshot. The VM will be reset to the state when the snapshot was created.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"snapshot_name":{"type":"string","description":"Name of the snapshot to restore"},"name":{"type":"string","default":"praxis-vm"}},"required":["snapshot_name"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_snapshot_delete".into(),
            description: Some("Delete a VM snapshot.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"snapshot_name":{"type":"string","description":"Name of the snapshot to delete"},"name":{"type":"string","default":"praxis-vm"}},"required":["snapshot_name"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_wait_for_text".into(),
            description: Some(
                "Poll the VM screen until the given `text` appears (or timeout).\n\
                Useful for waiting on installer prompts, login banners, GUI dialogs.\n\
                Returns the matching screenshot path on success, or an error after timeout.".into(),
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text or regex to look for on screen."
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "default": false,
                        "description": "If false (default), case is ignored."
                    },
                    "regex": {
                        "type": "boolean",
                        "default": false,
                        "description": "If true, treat `text` as a regular expression instead of a literal substring."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "default": 60,
                        "minimum": 1,
                        "maximum": 1800,
                        "description": "Total wait time before giving up."
                    },
                    "interval_secs": {
                        "type": "integer",
                        "default": 5,
                        "minimum": 1,
                        "maximum": 120,
                        "description": "How often to take a fresh screenshot and check."
                    },
                    "name": {"type": "string", "default": "praxis-vm"}
                },
                "required": ["text"]
            }),
            is_enabled: false,
        },
        Tool {
            name: "vm_window_list".into(),
            description: Some("List open windows in the VM (if running a desktop environment). Uses wmctrl or xdotool.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","default":"praxis-vm"}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_window_focus".into(),
            description: Some("Focus/activate a window in the VM by title or ID.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"window":{"type":"string","description":"Window title or ID to focus"},"name":{"type":"string","default":"praxis-vm"}},"required":["window"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_clipboard_set".into(),
            description: Some("Set clipboard content in the VM. Requires xclip or xsel.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"content":{"type":"string","description":"Content to set in clipboard"},"name":{"type":"string","default":"praxis-vm"}},"required":["content"]}),
            is_enabled: false,
        },
        Tool {
            name: "vm_clipboard_get".into(),
            description: Some("Get clipboard content from the VM. Requires xclip or xsel.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","default":"praxis-vm"}}}),
            is_enabled: false,
        },
        Tool {
            name: "vm_install".into(),
            description: Some("Start a VM with an installation ISO to install an OS. Provide either iso_name (searches in installation_disks context) or iso_path (direct path). The VM boots from the ISO. Use vm_keys and vm_screenshot to complete the installation.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"iso_name":{"type":"string","description":"Name to search for in installation_disks (e.g. 'alpine', 'ubuntu', 'arch')"},"iso_path":{"type":"string","description":"Direct path to ISO file (alternative to iso_name)"},"vm_name":{"type":"string","default":"praxis-vm"},"cpu_cores":{"type":"integer","default":2},"ram_mb":{"type":"integer","default":4096},"disk_size":{"type":"string","default":"40G"},"firmware":{"type":"string","enum":["bios","uefi"],"default":"bios","description":"Boot firmware: 'bios' (legacy) or 'uefi' (OVMF). Use 'uefi' for modern OSes that require EFI boot."}}}),
            is_enabled: false,
        },
        Tool {
            name: "send_screenshot".into(),
            description: Some("Take a VM screenshot and send it to the chat UI. If channel_id is 'web' or omitted, sends to the web dashboard chat. Otherwise sends to the specified Discord channel. Caption is optional.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"channel_id":{"type":"string","description":"Channel ID. Use 'web' or omit for web chat, otherwise a Discord channel ID."},"caption":{"type":"string","description":"Optional caption for the screenshot"},"vm_name":{"type":"string","default":"praxis-vm"}},"required":[]}),
            is_enabled: true,
        },
        Tool {
            name: "ask_questions".into(),
            description: Some("Ask the user questions via Discord. Each question needs 'label' (short key for the answer), 'question' (the text to ask), and optionally 'suggestions' (array of plain strings like [\"yes\", \"no\", \"maybe\"] shown as emoji buttons). Do NOT use 'options' or objects - use 'suggestions' with simple strings only. If 'channel_id' is omitted, questions are sent to the channel where the request originated.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"channel_id":{"type":"string","description":"Discord channel ID. If omitted, sends to the channel where the request originated."},"questions":{"type":"array","items":{"type":"object","properties":{"label":{"type":"string","description":"Short key for answer, e.g. 'color', 'choice'"},"question":{"type":"string","description":"The question text to ask"},"suggestions":{"type":"array","items":{"type":"string"},"description":"Quick reply options as plain strings, e.g. [\"red\", \"blue\", \"green\"]"}},"required":["label","question"]},"description":"Array of questions to ask"},"timeout_secs":{"type":"integer","default":120,"description":"Timeout per question in seconds"}},"required":["questions"]}),
            is_enabled: true,
        },
        Tool {
            name: "search_skills".into(),
            description: Some("Search the persistent skill metadata index using a few keywords. Returns at most 20 names, short descriptions, required_parameters and activation flags, never instructions. Hidden skills are excluded. A user_only result needs human selection via /skill or authenticated context controls. Refine the query rather than enumerating the catalog. Follow the POML discovery policy.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"query":{"type":"string","maxLength":512,"description":"Literal word-prefix search over names/descriptions. Empty lists a bounded first page."},"limit":{"type":"integer","minimum":1,"maximum":20,"default":5}},"required":["query"],"additionalProperties":false}),
            is_enabled: true,
        },
        Tool {
            name: "use_skill".into(),
            description: Some("Load one registered skill's instructions on demand. Discover matching names and required_parameters with search_skills, following the POML discovery policy. Hidden dependencies may be loaded by exact name. User-only skills cannot be activated by this tool. Follow returned instructions with normal tools; loading does not execute scripts or complete the task.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","maxLength":64,"description":"Exact registered skill name"},"parameters":{"type":"object","description":"Required non-empty string inputs advertised by discovery; additional inputs are skill-specific.","additionalProperties":true}},"required":["name","parameters"],"additionalProperties":false}),
            is_enabled: true,
        },
        Tool {
            name: "update_template".into(),
            description: Some("Update or create a POML template. Strictly renders a temporary file before replacing the destination; validation failure leaves existing content unchanged. Requires Node and POML_CLI.".into()),
            parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"Template name without .poml, e.g. tasks/custom. Use letters, numbers, underscores, hyphens and / separators; no absolute paths or traversal."},"content":{"type":"string","description":"Full POML template content"},"context":{"type":"object","description":"Optional complete JSON context for validation; omitted uses synthetic system variables, never saved user data."}},"required":["name","content"]}),
            is_enabled: true,
        },
    ]
}

/// Convert enabled tools to LLM tool definitions
pub fn to_tool_definition(
    tool: &Tool,
) -> crate::gateway::llm::provider::ToolDefinition {
    crate::gateway::llm::provider::ToolDefinition {
        tool_type: "function".to_string(),
        function: crate::gateway::llm::provider::FunctionDefinition {
            name: tool.name.clone(),
            description: tool.description.clone().unwrap_or_default(),
            parameters: tool.parameters.clone(),
        },
    }
}

pub fn to_tool_definitions(
    db: &Database,
) -> anyhow::Result<Vec<crate::gateway::llm::provider::ToolDefinition>> {
    let tools = list_enabled(db)?;
    Ok(tools.iter().map(to_tool_definition).collect())
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::new(dir.path()).unwrap();
        (db, dir)
    }

    #[test]
    fn test_use_skill_registration_and_upgrade() {
        let (db, _dir) = test_db();
        save(&db, &get_default_tools()[0]).unwrap();
        init_default_tools(&db).unwrap();
        let tool = get(&db, "use_skill").unwrap();
        assert!(tool.is_enabled);
        assert_eq!(tool.parameters["properties"]["parameters"]["type"], "object");
        disable(&db, "use_skill").unwrap();
        init_default_tools(&db).unwrap();
        assert!(!get(&db, "use_skill").unwrap().is_enabled);
        assert!(!to_tool_definitions(&db).unwrap().iter().any(|t| t.function.name == "use_skill"));
    }

    #[test]
    fn test_init_default_tools() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();
        let tools = list(&db).unwrap();
        assert_eq!(tools.len(), get_default_tools().len());
    }

    #[test]
    fn test_init_default_tools_idempotent() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();
        init_default_tools(&db).unwrap();
        let tools = list(&db).unwrap();
        assert_eq!(tools.len(), get_default_tools().len());
    }

    #[test]
    fn test_list_enabled() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();
        let expected = get_default_tools().iter().filter(|tool| tool.is_enabled).count();
        let enabled = list_enabled(&db).unwrap();
        assert_eq!(enabled.len(), expected);

        disable(&db, "execute_terminal").unwrap();
        let enabled = list_enabled(&db).unwrap();
        assert_eq!(enabled.len(), expected - 1);
        assert!(enabled.iter().all(|tool| tool.name != "execute_terminal"));
    }

    #[test]
    fn test_get_tool() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();
        let tool = get(&db, "execute_terminal").unwrap();
        assert_eq!(tool.name, "execute_terminal");
        assert!(tool.is_enabled);
    }

    #[test]
    fn test_get_tool_not_found() {
        let (db, _dir) = test_db();
        assert!(get(&db, "nonexistent").is_err());
    }

    #[test]
    fn test_enable_disable() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();

        disable(&db, "write_file").unwrap();
        let tool = get(&db, "write_file").unwrap();
        assert!(!tool.is_enabled);

        enable(&db, "write_file").unwrap();
        let tool = get(&db, "write_file").unwrap();
        assert!(tool.is_enabled);
    }

    #[test]
    fn test_set_enabled() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();

        set_enabled(&db, "edit_file", false).unwrap();
        let tool = get(&db, "edit_file").unwrap();
        assert!(!tool.is_enabled);

        set_enabled(&db, "edit_file", true).unwrap();
        let tool = get(&db, "edit_file").unwrap();
        assert!(tool.is_enabled);
    }

    #[test]
    fn test_save_custom_tool() {
        let (db, _dir) = test_db();
        let tool = Tool {
            name: "custom_tool".to_string(),
            description: Some("My custom tool".to_string()),
            parameters: serde_json::json!({"type": "object"}),
            is_enabled: true,
        };
        save(&db, &tool).unwrap();

        let loaded = get(&db, "custom_tool").unwrap();
        assert_eq!(loaded.description, Some("My custom tool".to_string()));
    }

    #[test]
    fn test_save_update_existing() {
        let (db, _dir) = test_db();
        let tool = Tool {
            name: "my_tool".to_string(),
            description: Some("v1".to_string()),
            parameters: serde_json::json!({}),
            is_enabled: true,
        };
        save(&db, &tool).unwrap();

        let tool2 = Tool {
            name: "my_tool".to_string(),
            description: Some("v2".to_string()),
            parameters: serde_json::json!({}),
            is_enabled: false,
        };
        save(&db, &tool2).unwrap();

        let loaded = get(&db, "my_tool").unwrap();
        assert_eq!(loaded.description, Some("v2".to_string()));
        assert!(!loaded.is_enabled);
    }

    #[test]
    fn test_to_tool_definitions() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();
        let defs = to_tool_definitions(&db).unwrap();
        let expected = get_default_tools().iter().filter(|tool| tool.is_enabled).count();
        assert_eq!(defs.len(), expected);
        // Preserve the full enabled-default order, not a stale hard-coded
        // assumption that execute_terminal precedes progressive discovery.
        let expected_names: Vec<_> = get_default_tools().into_iter()
            .filter(|tool| tool.is_enabled).map(|tool| tool.name).collect();
        let actual_names: Vec<_> = defs.iter().map(|tool| tool.function.name.clone()).collect();
        assert_eq!(actual_names, expected_names);
        for name in ["execute_terminal", "write_file", "inspect_file"] {
            assert!(actual_names.iter().any(|actual| actual == name));
        }
    }

    #[test]
    fn test_to_tool_definitions_filtered() {
        let (db, _dir) = test_db();
        init_default_tools(&db).unwrap();
        disable(&db, "execute_terminal").unwrap();
        disable(&db, "write_file").unwrap();

        let defs = to_tool_definitions(&db).unwrap();
        let expected = get_default_tools().iter().filter(|tool| tool.is_enabled).count();
        assert_eq!(defs.len(), expected - 2);
        assert!(defs.iter().all(|d| d.function.name != "execute_terminal"));
    }

    /// Tools removed from `get_default_tools()` between releases must be
    /// purged from the persisted on-disk registry on next startup so they
    /// don't continue to be advertised to the LLM.
    #[test]
    fn test_init_default_tools_removes_stale_tools() {
        let (db, _dir) = test_db();
        // Pretend a previous version installed a now-deprecated tool.
        let stale = Tool {
            name: "vm_cron_add".to_string(),
            description: Some("legacy".to_string()),
            parameters: serde_json::json!({}),
            is_enabled: true,
        };
        save(&db, &stale).unwrap();

        // First init: registry has only the stale tool, so it's treated as a
        // fresh install. Save the defaults explicitly to simulate an upgrade
        // from a state where stale tools coexist with defaults.
        let mut defaults = get_default_tools();
        defaults.push(stale.clone());
        save_tools(&db, &defaults).unwrap();
        let pre = list(&db).unwrap();
        assert!(pre.iter().any(|t| t.name == "vm_cron_add"));

        // Run init again — should drop vm_cron_add (no longer in defaults).
        init_default_tools(&db).unwrap();
        let post = list(&db).unwrap();
        assert!(!post.iter().any(|t| t.name == "vm_cron_add"));
    }
}
