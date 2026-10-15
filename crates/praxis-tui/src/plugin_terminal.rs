//! What installed packages show in the terminal: status lines in the header and
//! extra `/` commands. The gateway runs the package's tool and returns plain,
//! bounded text; this side strips control characters again before drawing.
use serde_json::Value;
use std::time::Duration;

/// How often the list of contributions is re-read (packages enabled/disabled).
pub const LISTING_EVERY: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq)]
pub struct PluginCommand {
    pub name: String,
    pub plugin: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusSource {
    pub plugin: String,
    pub every: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct PluginTerminal {
    pub commands: Vec<PluginCommand>,
    /// Latest status text per package, in listing order.
    pub status: Vec<(String, String)>,
}

/// Parse `GET /v1/terminal`.
pub fn parse_listing(value: &Value) -> (Vec<StatusSource>, Vec<PluginCommand>) {
    let text = |item: &Value, key: &str| item[key].as_str().map(clean);
    let status = value["status"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(StatusSource {
                plugin: text(item, "plugin")?,
                every: Duration::from_secs(item["every_seconds"].as_u64().unwrap_or(60).clamp(10, 3600)),
            })
        })
        .collect();
    let commands = value["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(PluginCommand {
                name: text(item, "name")?,
                plugin: text(item, "plugin")?,
                description: text(item, "description").unwrap_or_default(),
            })
        })
        .collect();
    (status, commands)
}

impl PluginTerminal {
    pub fn set_listing(&mut self, sources: &[StatusSource], commands: Vec<PluginCommand>) {
        self.commands = commands;
        self.status.retain(|(plugin, _)| sources.iter().any(|source| &source.plugin == plugin));
    }

    pub fn set_status(&mut self, plugin: &str, text: &str) {
        let text = clean(text);
        match self.status.iter_mut().find(|(known, _)| known == plugin) {
            Some(entry) => entry.1 = text,
            None => self.status.push((plugin.to_owned(), text)),
        }
    }

    /// A package command typed as `/name`; built-in commands are matched first.
    pub fn command(&self, slash: &str) -> Option<&PluginCommand> {
        let name = slash.strip_prefix('/')?;
        self.commands.iter().find(|command| command.name == name)
    }

    /// ` bili: cache 12k │ other: ok ` for the header, or empty.
    pub fn header_label(&self) -> String {
        let parts: Vec<String> = self
            .status
            .iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(plugin, text)| format!("{plugin}: {text}"))
            .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!(" {} ", parts.join(" │ "))
        }
    }

    pub fn help_lines(&self) -> String {
        self.commands
            .iter()
            .map(|command| format!("  /{} — {} (package {})", command.name, command.description, command.plugin))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Background refresh for one session: reads the listing, then each status
/// line on its own interval, and hands results to the UI loop as stream events.
/// Failures leave the last text in place; the listing is re-read periodically.
pub async fn poll(remote: crate::remote::Remote, user: String, tx: tokio::sync::mpsc::Sender<(String, String, String)>) {
    use tokio::time::Instant;
    let mut listed_at: Option<Instant> = None;
    let mut sources: Vec<(StatusSource, Instant)> = Vec::new();
    loop {
        if listed_at.is_none_or(|at| at.elapsed() >= LISTING_EVERY) {
            if let Ok(listing) = remote.request(reqwest::Method::GET, "/v1/terminal", None).await {
                let (status, _) = parse_listing(&listing);
                let now = Instant::now();
                sources = status.into_iter().map(|source| (source, now)).collect();
                if tx.send((user.clone(), "plugin_terminal".into(), listing.to_string())).await.is_err() {
                    return;
                }
            }
            listed_at = Some(Instant::now());
        }
        for (source, due) in sources.iter_mut() {
            if Instant::now() < *due {
                continue;
            }
            *due = Instant::now() + source.every;
            let path = format!("/v1/terminal/{}/status/{}", urlencoding::encode(&user), urlencoding::encode(&source.plugin));
            if let Ok(value) = remote.request(reqwest::Method::POST, &path, None).await {
                let event = serde_json::json!({"plugin": source.plugin, "text": value["text"]}).to_string();
                if tx.send((user.clone(), "plugin_status".into(), event)).await.is_err() {
                    return;
                }
            }
        }
        let next = sources.iter().map(|(_, due)| *due).min().unwrap_or_else(|| Instant::now() + LISTING_EVERY);
        tokio::time::sleep_until(next.min(listed_at.unwrap_or_else(Instant::now) + LISTING_EVERY)).await;
    }
}

/// One line, no control characters or escape sequences, at most 80 chars.
fn clean(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    line.chars().filter(|c| !c.is_control()).take(80).collect()
}

/// Command output for the transcript: keep newlines/tabs, drop other control
/// characters (including ESC), so package text cannot drive the terminal.
pub fn clean_output(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn listing_status_and_commands_render_as_plain_text() {
        let (sources, commands) = parse_listing(&json!({
            "status": [{"plugin": "bili", "every_seconds": 1}],
            "commands": [{"name": "bili", "plugin": "bili", "description": "Show cache"}, {"plugin": "broken"}]}));
        assert_eq!(sources, vec![StatusSource { plugin: "bili".into(), every: Duration::from_secs(10) }]);
        let mut terminal = PluginTerminal::default();
        terminal.set_listing(&sources, commands);
        assert_eq!(terminal.commands.len(), 1, "entries without a name are skipped");
        assert!(terminal.command("/bili").is_some());
        assert!(terminal.command("bili").is_none());
        assert_eq!(terminal.header_label(), "");
        terminal.set_status("bili", "\u{1b}[2Jcache 12k\nsecond line");
        assert_eq!(terminal.header_label(), " bili: [2Jcache 12k ");
        terminal.set_listing(&[], vec![]);
        assert_eq!(terminal.header_label(), "", "a removed package loses its status line");
        assert_eq!(clean_output("a\u{1b}[31mb\nc\u{7}"), "a[31mb\nc");
    }
}
