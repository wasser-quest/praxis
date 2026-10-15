//! Terminal contributions: a package can show a status line in the TUI and add
//! `/` commands to it.
//!
//! Both are backed by one of the package's own declared tools, run by the kernel
//! exactly like a model-requested call of that tool (same sandbox, secrets,
//! enable switches and trust). The terminal only receives bounded, plain text:
//! control characters and escape sequences are removed before it leaves the
//! gateway, so a plugin cannot drive the operator's terminal.
//!
//! ```json
//! "provides": { "terminal": {
//!   "status":   {"tool": "bili_status", "every_seconds": 30},
//!   "commands": [{"name": "bili", "tool": "bili_status", "description": "Show bili cache usage"}]
//! }}
//! ```
//!
//! The status tool is called with `{}`; a command tool with `{"text": "<rest of the line>"}`.
use super::{Plugin, PluginTool};
use serde::{Deserialize, Serialize};

pub const MAX_COMMANDS: usize = 8;
pub const MIN_STATUS_SECONDS: u64 = 10;
pub const MAX_STATUS_SECONDS: u64 = 3600;
/// Longest status line shown in the TUI header.
pub const MAX_STATUS_CHARS: usize = 80;
/// Longest command output shown in the TUI.
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TerminalStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<TerminalCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalStatus {
    pub tool: String,
    #[serde(default = "default_every")]
    pub every_seconds: u64,
}

fn default_every() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalCommand {
    pub name: String,
    pub tool: String,
    #[serde(default)]
    pub description: String,
}

impl TerminalDeclaration {
    pub fn is_empty(&self) -> bool {
        self.status.is_none() && self.commands.is_empty()
    }

    /// Every referenced tool must be one of the package's own tools.
    pub fn validate(&self, tools: &[PluginTool]) -> anyhow::Result<()> {
        let own = |tool: &str| tools.iter().any(|declared| declared.name == tool);
        if let Some(status) = &self.status {
            anyhow::ensure!(own(&status.tool), "provides.terminal.status.tool '{}' is not a tool of this package", status.tool);
            anyhow::ensure!(
                (MIN_STATUS_SECONDS..=MAX_STATUS_SECONDS).contains(&status.every_seconds),
                "provides.terminal.status.every_seconds must be {MIN_STATUS_SECONDS}..={MAX_STATUS_SECONDS}"
            );
        }
        anyhow::ensure!(self.commands.len() <= MAX_COMMANDS, "provides.terminal declares more than {MAX_COMMANDS} commands");
        let mut seen = std::collections::HashSet::new();
        for command in &self.commands {
            anyhow::ensure!(valid_command_name(&command.name), "Invalid terminal command name '{}'", command.name);
            anyhow::ensure!(seen.insert(&command.name), "Duplicate terminal command '{}'", command.name);
            anyhow::ensure!(own(&command.tool), "Terminal command '{}' uses '{}', which is not a tool of this package", command.name, command.tool);
            anyhow::ensure!(command.description.chars().count() <= 120, "Terminal command '{}' description is longer than 120 characters", command.name);
        }
        Ok(())
    }
}

/// `[a-z][a-z0-9_-]{0,31}`: typed after `/`, so no spaces or upper case.
pub fn valid_command_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && name.len() <= 32
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// What the TUI may show for enabled packages. When two packages declare the
/// same command name, the first package by name keeps it.
pub fn listing(plugins: &[&Plugin]) -> serde_json::Value {
    let mut status = Vec::new();
    let mut commands: Vec<serde_json::Value> = Vec::new();
    for plugin in plugins.iter().filter(|plugin| plugin.enabled) {
        let Some(terminal) = &plugin.provides.terminal else { continue };
        if let Some(line) = &terminal.status {
            status.push(serde_json::json!({"plugin": plugin.name, "every_seconds": line.every_seconds}));
        }
        for command in &terminal.commands {
            if commands.iter().any(|known| known["name"] == command.name.as_str()) {
                continue;
            }
            commands.push(serde_json::json!({"name": command.name, "plugin": plugin.name, "description": command.description}));
        }
    }
    serde_json::json!({"status": status, "commands": commands})
}

/// The tool behind a status line or command of an enabled package.
pub fn status_tool<'a>(plugins: &[&'a Plugin], plugin: &str) -> Option<&'a str> {
    let plugin = plugins.iter().find(|candidate| candidate.enabled && candidate.name == plugin)?;
    Some(plugin.provides.terminal.as_ref()?.status.as_ref()?.tool.as_str())
}

pub fn command_tool<'a>(plugins: &[&'a Plugin], name: &str) -> Option<(&'a str, &'a str)> {
    plugins.iter().filter(|plugin| plugin.enabled).find_map(|plugin| {
        let command = plugin.provides.terminal.as_ref()?.commands.iter().find(|command| command.name == name)?;
        Some((plugin.name.as_str(), command.tool.as_str()))
    })
}

/// The text a tool result carries: script output arrives as `{"result": …}`
/// unless it was JSON already; a `text` or `result` string or a bare JSON
/// string is unwrapped, anything else is shown as returned.
pub fn result_text(output: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(output.trim()) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(value) => ["text", "result"]
            .iter()
            .find_map(|key| value.get(key).and_then(serde_json::Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| output.to_owned()),
        Err(_) => output.to_owned(),
    }
}

/// Plain text only: escape sequences and control characters (except newline
/// and tab) are removed, then the text is bounded.
pub fn sanitize(text: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max_bytes));
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI `ESC [ … final` and OSC `ESC ] … BEL/ST`; otherwise drop ESC + next.
            match chars.next() {
                Some('[') => {
                    while let Some(c) = chars.next() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        if out.len() + c.len_utf8() > max_bytes {
            break;
        }
        out.push(c);
    }
    out
}

/// First non-empty line, at most `MAX_STATUS_CHARS` characters.
pub fn status_line(text: &str) -> String {
    let clean = sanitize(text, MAX_OUTPUT_BYTES);
    let line = clean.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    line.replace('\t', " ").chars().take(MAX_STATUS_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> PluginTool {
        serde_json::from_value(json!({"name": name, "description": "d", "parameters": {"type": "object"},
            "handler": {"type": "script", "path": "x.py", "interpreter": "python3"}}))
        .unwrap()
    }

    #[test]
    fn declarations_must_use_own_tools_and_valid_names() {
        let tools = [tool("status")];
        let ok: TerminalDeclaration = serde_json::from_value(json!({
            "status": {"tool": "status", "every_seconds": 30},
            "commands": [{"name": "bili", "tool": "status", "description": "show"}]})).unwrap();
        ok.validate(&tools).unwrap();
        for bad in [
            json!({"status": {"tool": "other"}}),
            json!({"status": {"tool": "status", "every_seconds": 1}}),
            json!({"commands": [{"name": "Bad Name", "tool": "status"}]}),
            json!({"commands": [{"name": "a", "tool": "status"}, {"name": "a", "tool": "status"}]}),
            json!({"commands": [{"name": "a", "tool": "missing"}]}),
        ] {
            let declaration: TerminalDeclaration = serde_json::from_value(bad.clone()).unwrap();
            assert!(declaration.validate(&tools).is_err(), "{bad}");
        }
        assert!(serde_json::from_value::<TerminalDeclaration>(json!({"panel": {}})).is_err(), "unknown keys are refused");
    }

    #[test]
    fn output_is_plain_bounded_text() {
        let hostile = "ok \u{1b}[2J\u{1b}]0;title\u{7}cache\u{1b}[31m 12k\u{0}\r\nsecond";
        assert_eq!(sanitize(hostile, 1024), "ok cache 12k\nsecond");
        assert_eq!(status_line("\n\n  first line  \nsecond"), "first line");
        assert_eq!(status_line(&"x".repeat(500)).chars().count(), MAX_STATUS_CHARS);
        assert!(sanitize(&"é".repeat(100), 11).len() <= 11);
        assert_eq!(result_text(r#"{"result": "line"}"#), "line");
        assert_eq!(result_text(r#"{"text": "t", "extra": 1}"#), "t");
        assert_eq!(result_text(r#"{"other": 1}"#), r#"{"other": 1}"#);
        assert_eq!(result_text("plain"), "plain");
    }
}
