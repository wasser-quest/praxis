//! TUI chat application — state, event loop, and dispatch.

use crate::model::Message;
use crate::sessions::{Session, SessionStore};
use crate::ui;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
        EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;
use tokio::time::Instant;

/// One slash command the TUI knows about.
pub struct SlashCmd {
    pub name: &'static str,
    pub desc: &'static str,
}

pub const SLASH_COMMANDS: &[SlashCmd] = &[
    SlashCmd { name: "/clear", desc: "Clear messages in this session" },
    SlashCmd { name: "/new", desc: "Create a new chat session (forks current)" },
    SlashCmd { name: "/sessions", desc: "Toggle the sessions sidebar" },
    SlashCmd { name: "/stop", desc: "Stop the running agent loop" },
    SlashCmd { name: "/status", desc: "Print agent status" },
    SlashCmd { name: "/delegations", desc: "Show delegated subtasks (status + result)" },
    SlashCmd { name: "/context", desc: "Get/set context variables (e.g. set foo=bar)" },
    SlashCmd { name: "/thinking", desc: "Reasoning level: off|low|medium|high|xhigh|auto" },
    SlashCmd { name: "/show_thinking", desc: "Show reasoning: on|off" },
    SlashCmd { name: "/rename", desc: "Rename the current session" },
    SlashCmd { name: "/delete", desc: "Delete the current session" },
    SlashCmd { name: "/plugins", desc: "List plugin tools (use: /plugins [enable|disable] <tool_name>)" },
    SlashCmd { name: "/login", desc: "Provider login on the gateway: /login | /login codex | /login <provider> <api_key> [model] [api_base]" },
    SlashCmd { name: "/logout", desc: "Remove a provider credential: /logout <provider>" },
    SlashCmd { name: "/mouse", desc: "Toggle mouse capture (F2); OFF enables native selection" },
    SlashCmd { name: "/copy", desc: "Select a saved message to copy (F3); arrows select, Ctrl+C/y copies" },
    SlashCmd { name: "/usage", desc: "Show token usage for current response and session" },
    SlashCmd { name: "/fold", desc: "Toggle output folding for long tool results" },
    SlashCmd { name: "/help", desc: "Show available commands" },
    SlashCmd { name: "/quit", desc: "Exit the TUI" },
];

/// What the input-line popup is currently showing.
#[derive(Debug, Clone, PartialEq)]
pub enum Popup {
    None,
    /// Slash command picker. `selected` is an index into `matches`.
    Slash { matches: Vec<usize>, selected: usize },
    /// File picker triggered by an unfinished `@partial` token at the cursor.
    File {
        /// The portion of the path the user has typed after `@`.
        prefix: String,
        /// Resolved candidate paths (absolute).
        matches: Vec<String>,
        selected: usize,
    },
}

/// One chat row in the visible transcript. Built from DB messages plus
/// transient banners (system, agent loop start/stop).
#[derive(Debug, Clone)]
pub enum Bubble {
    User { content: String },
    Assistant { content: String },
    Thinking { content: String },
    Tool { name: String, content: String },
    // Original receipt retained for raw view/copy; formatting cached once per saved row.
    ToolResult { name: String, content: String, display: String },
    System { content: String },
    Banner { kind: BannerKind, content: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    AgentStart,
    AgentStop,
    Info,
    Error,
}

/// Whole-message selection: snapshot the text so live/history updates cannot
/// silently replace what an explicit copy action writes to the clipboard.
pub struct CopySelection {
    pub index: usize,
    pub content: String,
    pub reveal: bool,
}

const MAX_CLIPBOARD_BYTES: usize = 100_000;

/// Top-level TUI state.
pub struct App {
    pub gateway_url: String,
    pub gateway_api_key: String,
    pub data_dir: String,
    pub sessions: SessionStore,

    /// Currently displayed transcript (rebuilt from the API on session switch
    /// and on poll ticks). The Vec is in chronological order.
    pub transcript: Vec<Bubble>,
    pub live: super::streaming::LiveOutput,
    pub remote: crate::remote::Remote,
    /// Durable message IDs already rendered; text prefixes are not identities.
    seen_keys: std::collections::HashSet<String>,
    /// Transcript rows shown before their persisted IDs arrive (user echoes and
    /// validated assistant completions). Bind history to these rows in place.
    pending_echoes: Vec<usize>,
    /// Assistant rows received since stream_start, including history that beat
    /// the final SSE event. This prevents either handoff order duplicating them.
    stream_history: Vec<usize>,

    /// Multi-line input buffer.
    pub input: String,
    /// Caret position in chars (NOT bytes).
    pub cursor: usize,
    /// Pending file attachments for the next message (absolute paths).
    pub attachments: Vec<String>,

    pub popup: Popup,

    pub show_sidebar: bool,
    /// F2 releases mouse reporting so the terminal owns native selection.
    pub mouse_capture: bool,
    pub copy_selection: Option<CopySelection>,
    pending_clipboard: Option<String>,
    /// Vertical scroll offset (0 = pinned to bottom).
    pub scroll_offset: usize,
    /// Track if user was at bottom before new messages (for auto-scroll).
    pub at_bottom: bool,
    viewport: (usize, u16, u16),
    event_tx: Option<tokio::sync::mpsc::Sender<(String, String, String)>>,
    /// Track whether the agent loop is currently running for the active session.
    pub agent_active: bool,

    pub status_msg: Option<(String, Instant)>,

    pub should_quit: bool,

    // Session usage tracking.
    pub session_prompt_tokens: u32,
    pub session_completion_tokens: u32,
    pub session_total_tokens: u32,
    pub session_generation_ms: u64,
    pub last_response_usage: Option<super::streaming::UsageInfo>,

    // Output folding state: indices of folded bubbles.
    pub folded_bubbles: std::collections::HashSet<usize>,
}

impl App {
    pub fn new(
        remote: crate::remote::Remote,
        gateway_url: String,
        gateway_api_key: String,
        data_dir: String,
    ) -> Self {
        let sessions = SessionStore::load(&data_dir);
        Self {
            gateway_url,
            gateway_api_key,
            data_dir,
            sessions,
            transcript: Vec::new(),
            live: Default::default(),
            remote,
            seen_keys: std::collections::HashSet::new(),
            pending_echoes: Vec::new(),
            stream_history: Vec::new(),
            input: String::new(),
            cursor: 0,
            attachments: Vec::new(),
            popup: Popup::None,
            show_sidebar: true,
            mouse_capture: true,
            copy_selection: None,
            pending_clipboard: None,
            scroll_offset: 0,
            at_bottom: true,
            viewport: (0, 0, 0),
            event_tx: None,
            agent_active: false,
            status_msg: None,
            should_quit: false,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            session_total_tokens: 0,
            session_generation_ms: 0,
            last_response_usage: None,
            folded_bubbles: std::collections::HashSet::new(),
        }
    }

    pub fn active_user_id(&self) -> &str {
        &self.sessions.active
    }

    pub fn username(&self) -> &str {
        &self.sessions.username
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status_msg = Some((msg.into(), Instant::now()));
    }

    fn toggle_mouse_capture(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        self.flash(if self.mouse_capture { "Mouse: ON — wheel scrolls" }
            else { "Mouse: OFF — native selection; PgUp/PgDn scroll; F2 restores" });
    }

    fn select_copy_message(&mut self, index: usize) {
        if let Some(bubble) = self.transcript.get(index) {
            let content = match bubble {
                Bubble::User {content} | Bubble::Assistant {content} | Bubble::Thinking {content}
                | Bubble::Tool {content, ..} | Bubble::ToolResult {content, ..} | Bubble::System {content} | Bubble::Banner {content, ..} => content,
            };
            self.copy_selection = Some(CopySelection {index, content: content.clone(), reveal: true});
            self.popup = Popup::None;
            self.status_msg = None;
        }
    }

    fn toggle_copy_mode(&mut self) {
        if self.copy_selection.take().is_none() {
            if self.transcript.is_empty() { self.flash("Nothing saved to copy yet"); }
            else { self.select_copy_message(self.transcript.len() - 1); }
        }
    }

    fn request_copy(&mut self) {
        let Some(selection) = &self.copy_selection else { return; };
        if selection.content.len() > MAX_CLIPBOARD_BYTES {
            self.flash("Copy refused: message exceeds 100,000 bytes; use F2 native selection");
        } else {
            // No shell, provider, gateway or clipboard read. Only an explicit
            // copy key queues this request; receipt by the terminal is unknown.
            self.pending_clipboard = Some(selection.content.clone());
        }
    }

    fn flush_clipboard(&mut self, writer: &mut impl io::Write) {
        if let Some(text) = self.pending_clipboard.take() {
            match write_clipboard(writer, &text) {
                Ok(()) => self.flash("Copy requested (OSC 52); if blocked, use F2 native selection"),
                Err(_) => self.flash("Clipboard write failed; use F2 native selection"),
            }
        }
    }

    fn sync_mouse_capture(&self, writer: &mut impl io::Write, captured: &mut bool) -> io::Result<()> {
        if *captured != self.mouse_capture {
            if self.mouse_capture { execute!(writer, EnableMouseCapture)?; }
            else { execute!(writer, DisableMouseCapture)?; }
            *captured = self.mouse_capture;
        }
        Ok(())
    }

    pub fn update_viewport(&mut self, rows: usize, width: u16, height: u16) {
        let (old_rows, old_width, old_height) = self.viewport;
        if (width, height) != (old_width, old_height) {
            if let Some(selection) = &mut self.copy_selection { selection.reveal = true; }
        }
        if !self.at_bottom && width == old_width {
            // Hold the reader's position while streamed content grows/shrinks.
            self.scroll_offset = if rows >= old_rows {
                self.scroll_offset.saturating_add(rows - old_rows)
            } else {
                self.scroll_offset.saturating_sub(old_rows - rows)
            };
        }
        self.viewport = (rows, width, height);
        self.scroll_offset = self.scroll_offset.min(rows.saturating_sub(height as usize));
        self.at_bottom = self.scroll_offset == 0;
    }

    fn scroll_up(&mut self, rows: usize) {
        let max = self.viewport.0.saturating_sub(self.viewport.2 as usize);
        self.scroll_offset = self.scroll_offset.saturating_add(rows).min(max);
        self.at_bottom = self.scroll_offset == 0;
    }

    fn scroll_down(&mut self, rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(rows);
        self.at_bottom = self.scroll_offset == 0;
    }

    pub(super) fn stream_event(&mut self, event: &str, data: &str) {
        match event {
            "stream_start" => {
                self.stream_history.clear();
                self.live.apply(event, data);
            }
            "assistant" => {
                if !data.is_empty() && !self.stream_history.iter().any(|&index|
                    matches!(self.transcript.get(index), Some(Bubble::Assistant {content}) if content == data))
                {
                    let index = self.transcript.len();
                    self.transcript.push(Bubble::Assistant {content: data.into()});
                    self.pending_echoes.push(index);
                    self.stream_history.push(index);
                }
                // Final text is validated by the gateway. Keep its bubble while
                // waiting for history, even across tool continuations/reconnects.
                self.live.text.clear();
                self.live.disconnected = false;
            }
            "assistant_saved" => {
                match serde_json::from_str::<Message>(data) {
                    Ok(message) if message.role == "assistant" && message.id.is_some_and(|id| id > 0) => {
                        let replaces_live = message.content == self.live.text
                            || (self.live.disconnected && message.content.starts_with(&self.live.text));
                        // Install the persisted bubble BEFORE removing live text.
                        // Remote history polling can be a second behind this event.
                        self.append_messages(vec![message]);
                        if replaces_live { self.live.text.clear(); self.live.disconnected = false; }
                    }
                    _ => tracing::warn!("Ignoring malformed assistant_saved event; keeping visible reply"),
                }
            }
            "history" => {
                if let Ok(messages) = serde_json::from_str(data) { self.append_messages(messages); }
            }
            "reasoning" => {
                if !data.is_empty() { self.transcript.push(Bubble::Thinking { content: data.into() }); }
                self.live.reasoning.clear();
            }
            "feedback" | "request_failed" => {
                let failed = event == "request_failed" || data.starts_with("LLM request failed") || data.starts_with("LLM error:");
                if event == "request_failed" {
                    self.agent_active = false;
                    self.live.apply("stream_abort", "");
                }
                // The POST result may repeat the latest SSE error. Do not
                // dedupe across separate user requests with the same failure.
                if event != "request_failed" || !matches!(self.transcript.last(), Some(Bubble::Banner {content, ..}) if content == data) {
                    self.transcript.push(Bubble::Banner {
                        kind: if failed { BannerKind::Error } else { BannerKind::Info }, content: data.into(),
                    });
                }
            }
            "connection_error" => self.flash(data),
            "agent_start" => self.agent_active = true,
            "agent_stop" => { self.agent_active = false; self.live.tools.clear(); }
            "usage" => {
                self.live.apply(event, data);
                if let Some(ref usage) = self.live.last_usage {
                    self.session_prompt_tokens = self.session_prompt_tokens.saturating_add(usage.prompt_tokens);
                    self.session_completion_tokens = self.session_completion_tokens.saturating_add(usage.completion_tokens);
                    self.session_total_tokens = self.session_total_tokens.saturating_add(usage.total_tokens);
                    self.session_generation_ms = self.session_generation_ms.saturating_add(usage.generation_ms);
                    self.last_response_usage = Some(usage.clone());
                }
            }
            _ => self.live.apply(event, data),
        }
    }

    /// Drop transient state when switching session.
    pub fn reset_view(&mut self) {
        self.transcript.clear();
        self.copy_selection = None;
        self.pending_clipboard = None;
        self.live = Default::default();
        self.seen_keys.clear();
        self.pending_echoes.clear();
        self.stream_history.clear();
        self.scroll_offset = 0;
        self.at_bottom = true;
        self.viewport = (0, 0, 0);
        self.input.clear();
        self.cursor = 0;
        self.attachments.clear();
        self.popup = Popup::None;
        self.session_prompt_tokens = 0;
        self.session_completion_tokens = 0;
        self.session_total_tokens = 0;
        self.session_generation_ms = 0;
        self.last_response_usage = None;
        self.folded_bubbles.clear();
    }

    /// Refresh the transcript from the gateway. Idempotent — uses `seen_keys`
    /// to avoid duplicates so it is safe to call on every poll tick.
    pub async fn refresh_transcript(&mut self) {
        let user_id = self.active_user_id().to_string();
        let Ok(msgs) = self.remote.messages(&user_id).await else {
            return;
        };
        self.append_messages(msgs);
    }

    /// Commands come only from complete, authoritative history arguments, not
    /// the 200-character tool_call preview. The parent message key deduplicates
    /// the whole batch, so reused provider call IDs remain distinct.
    fn append_terminal_commands(&mut self, message: &Message) {
        if message.role != "assistant" { return; }
        for call in message.tool_calls.iter().flatten() {
            if !matches!(call.function.name.as_str(), "execute_terminal" | "run_background" | "vm_shell") {
                continue;
            }
            let arguments = serde_json::from_str::<serde_json::Value>(&call.function.arguments).ok();
            let command = arguments.as_ref().and_then(|args| args.get("command")).and_then(|v| v.as_str());
            self.transcript.push(Bubble::Tool {
                name: format!("{} — command", call.function.name),
                content: command.unwrap_or("(command unavailable: invalid or incomplete arguments)").into(),
            });
        }
    }

    fn append_messages(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            let key = bubble_key(&m);
            if self.seen_keys.contains(&key) {
                continue;
            }
            // Polling also recovers a reply if its final SSE notification was
            // lost. Replace the disconnected preview only with a fresh row.
            if self.live.disconnected && m.role == "assistant" && !m.content.is_empty()
                && m.content.starts_with(&self.live.text)
            {
                self.live = Default::default();
            }
            // A tool-only assistant still owns visible command rows; omit only
            // its empty prose bubble.
            if m.role == "assistant"
                && m.tool_calls.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
                && m.content.is_empty()
            {
                self.append_terminal_commands(&m);
                self.seen_keys.insert(key);
                continue;
            }
            // Both SSE-before-history and history-before-SSE must leave one
            // stable row. Do not confuse equal text from different message IDs.
            if let Some(pending) = self.pending_echoes.iter().position(|&index| {
                match (self.transcript.get(index), m.role.as_str()) {
                    (Some(Bubble::User {content}), "user") |
                    (Some(Bubble::Assistant {content}), "assistant") => content == &m.content,
                    _ => false,
                }
            }) {
                let index = self.pending_echoes.remove(pending);
                if m.role == "assistant" { self.stream_history.push(index); }
                self.append_terminal_commands(&m);
                self.seen_keys.insert(key);
                continue;
            }
            let bubble = match m.role.as_str() {
                "user" => Bubble::User {
                    content: m.content.clone(),
                },
                "assistant" => Bubble::Assistant {
                    content: m.content.clone(),
                },
                "tool" => Bubble::ToolResult {
                    display: super::tool_result::format(&m.content).unwrap_or_else(|| m.content.clone()),
                    name: m.tool_name.clone().unwrap_or_else(|| "tool".to_string()),
                    content: m.content.clone(),
                },
                "system" => Bubble::System {
                    content: m.content.clone(),
                },
                // Discord-mirror rows (persisted for the dashboard chat):
                // shown with an origin tag so TUI and dashboard stay in sync.
                "discord_user" | "discord_bot" => {
                    let author = m
                        .discord_meta
                        .as_ref()
                        .and_then(|v| v.get("author"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(if m.role == "discord_user" { "discord" } else { "bot" });
                    Bubble::System {
                        content: format!("🎮 Discord {}: {}", author, m.content),
                    }
                }
                _ => continue,
            };
            if m.role == "assistant" { self.stream_history.push(self.transcript.len()); }
            self.transcript.push(bubble);
            self.append_terminal_commands(&m);
            self.seen_keys.insert(key);
        }
    }

    /// Recompute the slash-command popup from the current input + cursor.
    fn update_popup(&mut self) {
        // Pull the token immediately before the cursor.
        let before: String = self.input.chars().take(self.cursor).collect();
        // Slash command: must be at the very start of the input line.
        if before.starts_with('/') && !before.contains(char::is_whitespace) {
            let q = before[1..].to_ascii_lowercase();
            let matches: Vec<usize> = SLASH_COMMANDS
                .iter()
                .enumerate()
                .filter(|(_, c)| c.name[1..].to_ascii_lowercase().starts_with(&q))
                .map(|(i, _)| i)
                .collect();
            if !matches.is_empty() {
                self.popup = Popup::Slash {
                    matches,
                    selected: 0,
                };
                return;
            }
        }
        // File @ token (anywhere in the input).
        if let Some(at_idx) = before.rfind('@') {
            let after_at = &before[at_idx + 1..];
            if !after_at.contains(char::is_whitespace) {
                let prefix = after_at.to_string();
                let matches = file_matches(&prefix);
                if !matches.is_empty() {
                    self.popup = Popup::File {
                        prefix,
                        matches,
                        selected: 0,
                    };
                    return;
                }
            }
        }
        self.popup = Popup::None;
    }

    /// Apply a slash command. The full command line (including any args
    /// after the verb) is passed in as `line`.
    async fn run_slash(&mut self, line: &str) {
        // The first whitespace-separated token is the command name; the rest
        // is forwarded as-is so commands like `/context set foo=bar` work.
        let mut parts = line.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_string();
        let _rest = parts.next().unwrap_or("").to_string();
        match name.as_str() {
            "/clear" => {
                let result = self.remote
                    .request(reqwest::Method::DELETE, &format!("/v1/messages/{}",urlencoding::encode(self.active_user_id())),None).await.map(|_|());
                if let Err(e) = result {
                    self.flash(format!("clear failed: {e}"));
                } else {
                    self.transcript.clear();
                    self.copy_selection = None;
                    self.pending_clipboard = None;
                    self.seen_keys.clear();
                    self.pending_echoes.clear();
                    self.stream_history.clear();
                    self.live = Default::default();
                    self.scroll_offset = 0;
                    self.at_bottom = true;
                    self.flash("cleared");
                }
            }
            "/new" => {
                self.new_session().await;
            }
            "/sessions" => {
                self.show_sidebar = !self.show_sidebar;
            }
            "/stop" => {
                self.stop_agent().await;
            }
            "/status" => {
                let active = if self.agent_active { "active" } else { "idle" };
                self.transcript.push(Bubble::Banner {
                    kind: BannerKind::Info,
                    content: format!("Agent is {active} for session {}", self.active_user_id()),
                });
            }
            "/delegations" => {
                self.flash("Use the backend dashboard for delegated subtasks");
            }
            "/thinking" | "/show_thinking" => {
                let value = line.split_whitespace().nth(1).unwrap_or("").to_ascii_lowercase();
                let setting = if name == "/show_thinking" {
                    match value.as_str() {
                        "on" => Some(("show_thinking", serde_json::json!(true))),
                        "off" => Some(("show_thinking", serde_json::json!(false))),
                        _ => None,
                    }
                } else if ["off", "on", "low", "medium", "high", "xhigh", "auto"].contains(&value.as_str()) {
                    Some(("thinking_mode", serde_json::json!(value)))
                } else { None };
                if let Some((key, value)) = setting {
                    let result = self.remote
                        .command(self.active_user_id(), &format!("/context set settings.{key}={value}")).await.map(|_|());
                    match result {
                        Ok(_) => self.flash("Thinking setting saved"),
                        Err(e) => self.flash(format!("{e}")),
                    }
                } else { self.flash("Invalid thinking setting; see /help"); }
            }
            "/context" | "/ctx" => {
                let response = self.remote.command(self.active_user_id(),line).await;
                self.transcript.push(Bubble::System {content:response.unwrap_or_else(|e|format!("{e}"))});
            }
            "/rename" => {
                let title = _rest.trim();
                if title.is_empty() || title.chars().count() > 128 {
                    self.flash("Usage: /rename <name> (1–128 characters)"); return;
                }
                let id = self.active_user_id().to_string();
                let result = self.remote
                    .command(&id,&format!("/context set custom_data.session_title={}",serde_json::json!(title))).await.map(|_|());
                match result {
                    Ok(_) => {
                        if let Some(session) = self.sessions.sessions.iter_mut().find(|s|s.id==id) { session.name=title.into(); }
                        self.sessions.save(&self.data_dir);
                        self.flash(format!("Renamed to {title}; context: {id}"));
                    }
                    Err(e) => self.flash(format!("Rename failed: {e}")),
                }
            }
            "/delete" => {
                self.delete_active_session().await;
            }
            "/mouse" => self.toggle_mouse_capture(),
            "/copy" => self.toggle_copy_mode(),
            "/usage" => {
                let mut lines = Vec::new();
                if let Some(ref usage) = self.last_response_usage {
                    let tps = if usage.tokens_per_sec > 0.0 {
                        format!("{:.1} tok/s", usage.tokens_per_sec)
                    } else {
                        "-".to_string()
                    };
                    lines.push(format!("Last response: {} in / {} out / {} total tokens ({})",
                        usage.prompt_tokens, usage.completion_tokens, usage.total_tokens, tps));
                } else {
                    lines.push("Last response: no usage data".to_string());
                }
                if self.session_total_tokens > 0 {
                    let avg_tps = if self.session_generation_ms > 0 {
                        (self.session_completion_tokens as f64 / (self.session_generation_ms as f64 / 1000.0)).round()
                    } else { 0.0 };
                    lines.push(format!("Session total: {} in / {} out / {} tokens (avg {:.1} tok/s)",
                        self.session_prompt_tokens, self.session_completion_tokens, self.session_total_tokens, avg_tps));
                }
                self.transcript.push(Bubble::Banner {
                    kind: BannerKind::Info,
                    content: lines.join("\n"),
                });
            }
            "/fold" => {
                // Toggle folding for all tool/result bubbles, or specific index.
                let arg = _rest.trim();
                if arg.is_empty() {
                    // Toggle global: if any folded, unfold all; else fold all tool results.
                    if self.folded_bubbles.is_empty() {
                        for (i, bubble) in self.transcript.iter().enumerate() {
                            if matches!(bubble, Bubble::Tool { .. } | Bubble::ToolResult { .. }) {
                                self.folded_bubbles.insert(i);
                            }
                        }
                        self.flash("Folded all tool outputs");
                    } else {
                        self.folded_bubbles.clear();
                        self.flash("Unfolded all tool outputs");
                    }
                } else if let Ok(idx) = arg.parse::<usize>() {
                    if self.folded_bubbles.contains(&idx) {
                        self.folded_bubbles.remove(&idx);
                    } else {
                        self.folded_bubbles.insert(idx);
                    }
                }
            }
            "/help" => {
                let body = SLASH_COMMANDS
                    .iter()
                    .map(|c| format!("  {} — {}", c.name, c.desc))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.transcript.push(Bubble::Banner {
                    kind: BannerKind::Info,
                    content: format!(
                        "Keys: F2 mouse ON/OFF (native selection when OFF); F3 message copy mode.\nCopy: tool results show/copy their full raw receipt (not the formatted view).\nUp/Down/Home/End select; Ctrl+C or y requests terminal clipboard (OSC 52).\nEsc/F3 leaves copy mode; Ctrl+Q always quits; Ctrl+C quits outside copy mode.\nPgUp/PgDn scroll with or without mouse capture.\nPaste: use your terminal paste shortcut; multiline/Unicode stays in the draft.\nIf OSC 52 is unsupported/blocked, use F2 then native selection/copy.\n\nCommands:\n{body}\n\nAttach files with @path. Use Tab/Enter to autocomplete.\n\
                         /context examples:\n  \
                            /context set custom_data.device=main settings.max_llm_turns=20\n  \
                            /context set settings.voice_tts_enabled=true\n  \
                            /context get settings.max_llm_turns\n  \
                            /context show settings"
                    ),
                });
            }
            "/login" | "/logout" => {
                self.run_login(&name, &_rest).await;
            }
            "/quit" | "/exit" => {
                self.should_quit = true;
            }
            _ => {
                self.flash(format!("unknown command: {name}"));
            }
        }
    }

    /// Provider login. Always executed by the gateway (local or remote), because
    /// that is the process that talks to the model; a remote TUI therefore logs
    /// the remote machine in. Credentials travel once, over the authenticated
    /// gateway connection, and are not stored by the TUI.
    async fn run_login(&mut self, verb: &str, rest: &str) {
        let args = shell_words(rest);
        let request = match parse_login_args(verb, &args) {
            Ok(None) => {
                match self.gateway_get("/v1/providers").await {
                    Ok(v) => {
                        let text = v["text"].as_str().unwrap_or("No provider information").to_string();
                        self.transcript.push(Bubble::Banner { kind: BannerKind::Info, content: text });
                    }
                    Err(e) => self.transcript.push(Bubble::Banner { kind: BannerKind::Error, content: format!("/login: {e}") }),
                }
                return;
            }
            Ok(Some(request)) => request,
            Err(usage) => { self.flash(usage); return; }
        };
        self.flash(format!("Contacting gateway for {} login…", request["provider"].as_str().unwrap_or("provider")));
        match self.gateway_post("/v1/providers/login", request).await {
            Ok(v) => {
                let mut content = v["message"].as_str().unwrap_or("Done").to_string();
                let setup: Vec<&str> = v["setup"].as_array().map(|a| a.iter().filter_map(|s| s.as_str()).collect()).unwrap_or_default();
                if !setup.is_empty() {
                    content.push_str("\n\nNext steps:\n  ");
                    content.push_str(&setup.join("\n  "));
                }
                let kind = if v["status"] == "pending" { BannerKind::Info } else { BannerKind::AgentStop };
                self.transcript.push(Bubble::Banner { kind, content });
            }
            Err(e) => self.transcript.push(Bubble::Banner { kind: BannerKind::Error, content: format!("{verb}: {e}") }),
        }
    }

    async fn gateway_get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let response = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?
            .get(format!("{}{path}", self.gateway_url.trim_end_matches('/'))).bearer_auth(&self.gateway_api_key).send().await
            .map_err(|_| anyhow::anyhow!("Gateway not reachable at {} (is `praxis run` running?)", self.gateway_url))?;
        anyhow::ensure!(response.status().is_success(), "Gateway returned HTTP {}", response.status().as_u16());
        Ok(response.json().await?)
    }

    async fn gateway_post(&self, path: &str, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        // Login can take a while (endpoint probes, spawning the Codex CLI).
        let client = reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?;
        let response = client.post(format!("{}{path}", self.gateway_url.trim_end_matches('/'))).bearer_auth(&self.gateway_api_key).json(&body).send().await
            .map_err(|_| anyhow::anyhow!("Gateway not reachable at {} (is `praxis run` running?)", self.gateway_url))?;
        let status = response.status();
        let value: serde_json::Value = response.json().await.unwrap_or_default();
        if !status.is_success() {
            let detail = value["error"].as_str().unwrap_or("request failed");
            anyhow::bail!("{detail} (HTTP {})", status.as_u16());
        }
        Ok(value)
    }

    async fn send_message(&mut self) {
        let text = self.input.clone();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        // Slash commands run locally — they should never leave the TUI.
        // We accept both single-token (`/help`) and multi-token (`/context set foo=bar`).
        if text.trim_start().starts_with('/') {
            self.input.clear();
            self.cursor = 0;
            self.popup = Popup::None;
            self.run_slash(text.trim()).await;
            return;
        }

        // Strip @file tokens out of the visible text and add their resolved
        // paths to the attachment list. The gateway sees a clean message
        // and a separate `attachments` array.
        let (clean_text, mut extra_attachments) = split_at_attachments(&text);
        let mut atts = std::mem::take(&mut self.attachments);
        atts.append(&mut extra_attachments);

        // Optimistic local echo.
        let display = if clean_text.is_empty() {
            "(attachments)".to_string()
        } else {
            clean_text.clone()
        };
        self.pending_echoes.push(self.transcript.len());
        self.transcript.push(Bubble::User {
            content: display.clone(),
        });

        self.input.clear();
        self.cursor = 0;
        self.popup = Popup::None;

        // Fire-and-forget HTTP POST to the local gateway. The gateway runs
        // the agent loop and persists results to the DB; our poll task picks
        // them up.
        let url = format!("{}/v1/chat", self.gateway_url.trim_end_matches('/'));
        let api_key = self.gateway_api_key.clone();
        let user_id = self.active_user_id().to_string();
        let body = serde_json::json!({
            "user_id": user_id,
            "message": clean_text,
            "attachments": atts,
        });
        self.scroll_offset = 0;
        self.at_bottom = true;
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(60 * 30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());
            let result: anyhow::Result<()> = async {
                let response = client.post(&url).bearer_auth(&api_key).json(&body).send().await
                    ?;
                anyhow::ensure!(response.status().is_success(), "Chat gateway returned HTTP {} (check connection and gateway key)", response.status().as_u16());
                let value: serde_json::Value = response.json().await
                    .map_err(|_| anyhow::anyhow!("Chat gateway returned invalid JSON"))?;
                anyhow::ensure!(value["success"] == true, "{}", value["error"].as_str().unwrap_or("Chat gateway returned no success result"));
                Ok(())
            }.await;
            if let Err(error) = result {
                tracing::warn!(error = %error, "TUI chat request failed");
                if let Some(tx) = event_tx {
                    let _ = tx.send((user_id, "request_failed".into(), error.to_string())).await;
                }
            }
        });
        self.agent_active = true;
        self.flash("sent");
    }

    async fn stop_agent(&mut self) {
        let user_id = self.active_user_id().to_string();
        let url = format!("{}/v1/stop/{}", self.gateway_url.trim_end_matches('/'), urlencoding::encode(&user_id));
        if reqwest::Client::new().post(url).bearer_auth(&self.gateway_api_key)
            .timeout(Duration::from_secs(5)).send().await.and_then(|r| r.error_for_status()).is_err() {
            self.flash("Failed to send stop signal to gateway");
            return;
        }
        self.agent_active = false;
        self.transcript.push(Bubble::Banner {
            kind: BannerKind::AgentStop,
            content: "Stop signal sent".to_string(),
        });
    }

    async fn new_session(&mut self) {
        let parent = self.active_user_id().to_string();
        let new_id = SessionStore::generate_id();
        let username = self.sessions.username.clone();
        let result = self.remote
            .request(reqwest::Method::POST,&format!("/v1/context/{}/fork",urlencoding::encode(&parent)),
                Some(serde_json::json!({"new_user_id":new_id,"username":username}))).await.map(|_|());
        match result {
            Ok(_) => {
                let name = format!("Session {}", self.sessions.sessions.len() + 1);
                self.sessions.sessions.push(Session {
                    id: new_id.clone(),
                    name,
                    username,
                });
                self.sessions.set_active(&new_id);
                self.sessions.save(&self.data_dir);
                self.reset_view();
                self.refresh_transcript().await;
                self.flash("new session");
            }
            Err(e) => self.flash(format!("fork failed: {e}")),
        }
    }

    async fn delete_active_session(&mut self) {
        if self.sessions.sessions.len() <= 1 {
            self.flash("cannot delete the last session");
            return;
        }
        let id = self.active_user_id().to_string();
        let result = self.remote
            .request(reqwest::Method::DELETE,&format!("/v1/context/{}",urlencoding::encode(&id)),None).await.map(|_|());
        if let Err(e) = result { self.flash(format!("{e}")); return; }
        self.sessions.sessions.retain(|s| s.id != id);
        let new_active = self.sessions.sessions[0].id.clone();
        self.sessions.set_active(&new_active);
        self.sessions.save(&self.data_dir);
        self.reset_view();
        self.refresh_transcript().await;
        self.flash("session deleted");
    }

    pub async fn switch_session(&mut self, idx: usize) {
        if let Some(s) = self.sessions.sessions.get(idx) {
            let id = s.id.clone();
            self.sessions.set_active(&id);
            self.sessions.save(&self.data_dir);
            self.reset_view();
            self.refresh_transcript().await;
        }
    }
}

/// Compute a stable dedup key for a gateway message.
fn bubble_key(m: &Message) -> String {
    if let Some(id) = m.id { return format!("id:{id}"); }
    // Compatibility with older history endpoints lacking IDs: never truncate
    // content, or two long replies with the same opening collapse into one.
    // Tool-only assistant rows have empty content. Include their structured
    // calls, or every such legacy row would collapse into the first command.
    format!("legacy:{}", serde_json::json!([
        m.role, m.tool_call_id, m.content, m.tool_calls
    ]))
}

/// Find file completion candidates for `@<prefix>`.
///
/// Rules:
/// - If `prefix` is empty → list cwd entries.
/// - If `prefix` ends with `/` → list entries inside that directory.
/// - Otherwise → list entries inside the parent directory whose basename
///   starts with `prefix`'s basename.
///
/// Hidden files (leading `.`) are included only when the prefix's basename
/// itself starts with `.`. Up to 50 results are returned.
pub fn file_matches(prefix: &str) -> Vec<String> {
    use std::path::PathBuf;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let raw = if prefix.starts_with('~') {
        if let Ok(home) = std::env::var("HOME") {
            prefix.replacen('~', &home, 1)
        } else {
            prefix.to_string()
        }
    } else {
        prefix.to_string()
    };
    let pb = PathBuf::from(&raw);
    let (dir, partial) = if raw.is_empty() {
        (cwd.clone(), String::new())
    } else if raw.ends_with('/') {
        (
            if pb.is_absolute() {
                pb.clone()
            } else {
                cwd.join(&pb)
            },
            String::new(),
        )
    } else {
        let parent = pb.parent().map(PathBuf::from).unwrap_or_default();
        let base = pb
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let abs_parent = if parent.as_os_str().is_empty() {
            cwd.clone()
        } else if parent.is_absolute() {
            parent
        } else {
            cwd.join(&parent)
        };
        (abs_parent, base)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let want_hidden = partial.starts_with('.');
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !want_hidden && name.starts_with('.') {
                return None;
            }
            if !name.to_lowercase().starts_with(&partial.to_lowercase()) {
                return None;
            }
            let mut path = e.path();
            if path.is_dir() {
                path.push("");
            }
            Some(path.to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out.truncate(50);
    out
}

/// Walk through the input and split out `@/path/to/file` tokens. Returns
/// `(clean_text_without_attachments, resolved_paths)`. Tokens that don't
/// resolve to an existing file are passed through into `clean_text`.
fn split_at_attachments(input: &str) -> (String, Vec<String>) {
    let mut clean = String::new();
    let mut paths = Vec::new();
    for word in input.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end();
        if let Some(rest) = trimmed.strip_prefix('@') {
            // Try to resolve to an existing path. Tilde expansion + cwd join.
            let p = if let Some(stripped) = rest.strip_prefix('~') {
                if let Ok(home) = std::env::var("HOME") {
                    std::path::PathBuf::from(format!("{home}{stripped}"))
                } else {
                    std::path::PathBuf::from(rest)
                }
            } else {
                std::path::PathBuf::from(rest)
            };
            let abs = if p.is_absolute() {
                p
            } else {
                std::env::current_dir().unwrap_or_default().join(&p)
            };
            if abs.exists() {
                paths.push(abs.to_string_lossy().into_owned());
                // Remove an inline separator along with the attachment, but
                // never swallow a newline (or indentation on the next line).
                let separator = &word[trimmed.len()..];
                if separator != " " { clean.push_str(separator); }
                continue;
            }
        }
        clean.push_str(word);
    }
    // Chat text is not a shell command: preserve newlines, blank lines and
    // code indentation exactly, both in the local echo and the gateway input.
    (clean, paths)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Event loop
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run(gateway_url: Option<String>, gateway_key: Option<String>) -> anyhow::Result<()> {
    // The terminal UI never opens the backend database or its secrets: it
    // needs a gateway URL and key and does everything else over the API.
    let port = std::env::var("GATEWAY_PORT").unwrap_or_else(|_| "3537".to_string());
    let gateway_url = gateway_url.unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
    let api_key = gateway_key
        .or_else(|| std::env::var("PRAXIS_GATEWAY_KEY").ok())
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("Chat requires --gateway-key or PRAXIS_GATEWAY_KEY")
        })?;
    let data_dir = match std::env::var("DATA_DIR") {
        Ok(dir) => dir,
        Err(_) => {
            let dir = tempfile::tempdir()?;
            let path = dir.path().to_string_lossy().into_owned();
            std::mem::forget(dir);
            path
        }
    };
    let remote = crate::remote::Remote::new(gateway_url.clone(), api_key.clone())?;
    let mut app = App::new(remote.clone(), gateway_url.clone(), api_key.clone(), data_dir);
    let data = remote.request(reqwest::Method::GET,"/v1/sessions",None).await?;
    let sessions: Vec<Session> = serde_json::from_value(data["sessions"].clone())?;
    if !sessions.is_empty() {
        app.sessions.active = sessions[0].id.clone();
        app.sessions.sessions = sessions;
    } else {
        let id = SessionStore::generate_id();
        app.sessions.sessions[0].id = id.clone(); app.sessions.active = id;
    }
    app.append_messages(remote.messages(app.active_user_id()).await?);

    // Terminal setup. Bracketed paste so multi-line paste arrives as one event.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app).await;

    // Tear down terminal.
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    app.sessions.save(&app.data_dir);
    result
}

async fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> anyhow::Result<()> {
    let mut events = EventStream::new();
    let mut mouse_captured = true; // enabled during terminal setup
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut status_clear = tokio::time::interval(Duration::from_millis(500));
    let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(1024);
    app.event_tx = Some(stream_tx.clone());
    let mut stream_user = String::new();
    let mut stream_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut history_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        if stream_user != app.active_user_id() {
            if let Some(task) = stream_task.take() { task.abort(); }
            if let Some(task) = history_task.take() { task.abort(); }
            stream_user = app.active_user_id().to_string();
            {
                let remote = app.remote.clone();
                let user = stream_user.clone(); let tx = stream_tx.clone();
                history_task = Some(tokio::spawn(async move {
                    loop {
                        if let Ok(messages) = remote.messages(&user).await {
                            if let Ok(data) = serde_json::to_string(&messages) {
                                if tx.send((user.clone(), "history".into(), data)).await.is_err() { break; }
                            }
                        }
                        if tx.is_closed() { break; }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }));
            }
            stream_task = Some(super::streaming::subscribe(app.gateway_url.clone(), app.gateway_api_key.clone(), stream_user.clone(), stream_tx.clone()));
        }
        app.sync_mouse_capture(&mut io::stdout(), &mut mouse_captured)?;
        app.flush_clipboard(&mut io::stdout());
        terminal.draw(|f| ui::draw(f, app))?;
        if app.should_quit {
            if let Some(task) = stream_task.take() { task.abort(); }
            if let Some(task) = history_task.take() { task.abort(); }
            break;
        }

        tokio::select! {
            Some((user, event, data)) = stream_rx.recv() => {
                if user == app.active_user_id() {
                    app.stream_event(&event, &data);
                }
            }
            // Keyboard / paste events.
            maybe_event = events.next() => {
                let Some(Ok(ev)) = maybe_event else { continue };
                handle_event(ev, app).await;
            }
            // Periodic poll of the DB for new messages produced by the agent.
            _ = tick.tick() => {
                app.refresh_transcript().await;
                // Activity is received from the gateway, not this client's
                // process-local registry (which never owns the remote task).
            }
            // Auto-clear the flash status message after a couple of seconds.
            _ = status_clear.tick() => {
                if let Some((_, when)) = &app.status_msg {
                    if when.elapsed() > Duration::from_secs(2) {
                        app.status_msg = None;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn handle_event(ev: Event, app: &mut App) {
    match ev {
        Event::Paste(_) if app.copy_selection.is_some() => {
            app.flash("Esc leaves copy mode before pasting into the draft");
        }
        Event::Paste(text) => {
            insert_str(app, &text.replace("\r\n", "\n").replace('\r', "\n"));
            app.update_popup();
        }
        Event::Key(k) if k.kind == KeyEventKind::Press => handle_key(app, k.code, k.modifiers).await,
        Event::Mouse(me) if app.mouse_capture => {
            match me.kind {
                MouseEventKind::ScrollUp => {
                    app.scroll_up(3);
                }
                MouseEventKind::ScrollDown => {
                    app.scroll_down(3);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    // Global keybinds first.
    if code == KeyCode::F(2) {
        app.toggle_mouse_capture();
        return;
    }
    if code == KeyCode::F(3) { app.toggle_copy_mode(); return; }
    // Tab toggles folding for tool output bubbles when no popup is active.
    if code == KeyCode::Tab && mods.is_empty() && matches!(app.popup, Popup::None) {
        // Fold/unfold the last visible tool bubble, or toggle all.
        let last_tool = app.transcript.iter().enumerate().rev()
            .find_map(|(i, b)| match b {
                Bubble::Tool { .. } | Bubble::ToolResult { .. } => Some(i),
                _ => None,
            });
        if let Some(idx) = last_tool {
            if app.folded_bubbles.contains(&idx) {
                app.folded_bubbles.remove(&idx);
            } else {
                app.folded_bubbles.insert(idx);
            }
        }
        return;
    }
    if code == KeyCode::Char('q') && mods == KeyModifiers::CONTROL {
        app.should_quit = true;
        app.pending_clipboard = None;
        return;
    }
    if let Some(selection) = &app.copy_selection {
        let index = selection.index;
        match code {
            KeyCode::Esc => { app.copy_selection = None; app.status_msg = None; }
            KeyCode::Char('c') if mods == KeyModifiers::CONTROL => app.request_copy(),
            KeyCode::Char('y') if mods.is_empty() => app.request_copy(),
            KeyCode::Up => app.select_copy_message(index.saturating_sub(1)),
            KeyCode::Down => app.select_copy_message((index + 1).min(app.transcript.len().saturating_sub(1))),
            KeyCode::Home => app.select_copy_message(0),
            KeyCode::End => app.select_copy_message(app.transcript.len().saturating_sub(1)),
            KeyCode::PageUp => app.scroll_up(5),
            KeyCode::PageDown => app.scroll_down(5),
            _ => {} // copy mode never edits/submits the draft or runs commands
        }
        return;
    }
    match (code, mods) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
            app.should_quit = true;
            return;
        }
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            app.show_sidebar = !app.show_sidebar;
            return;
        }
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            app.new_session().await;
            return;
        }
        (KeyCode::PageUp, _) => {
            app.scroll_up(5);
            return;
        }
        (KeyCode::PageDown, _) => {
            app.scroll_down(5);
            return;
        }
        (KeyCode::Char(c), KeyModifiers::ALT) if c.is_ascii_digit() => {
            let idx = (c as u8 - b'1') as usize;
            app.switch_session(idx).await;
            return;
        }
        _ => {}
    }

    // Popup-specific handling.
    let popup_active = !matches!(app.popup, Popup::None);
    if popup_active {
        match code {
            KeyCode::Esc => {
                app.popup = Popup::None;
                return;
            }
            KeyCode::Up => {
                bump_popup_selection(app, -1);
                return;
            }
            KeyCode::Down => {
                bump_popup_selection(app, 1);
                return;
            }
            KeyCode::Tab | KeyCode::Enter => {
                if accept_popup(app) {
                    return;
                }
            }
            _ => {}
        }
    }

    // Default editing behaviour.
    match code {
        KeyCode::Enter => {
            if mods.contains(KeyModifiers::SHIFT) {
                insert_char(app, '\n');
            } else {
                app.send_message().await;
            }
        }
        KeyCode::Char(c) => {
            insert_char(app, c);
            app.update_popup();
        }
        KeyCode::Backspace => {
            backspace(app);
            app.update_popup();
        }
        KeyCode::Delete => {
            delete_forward(app);
            app.update_popup();
        }
        KeyCode::Left => {
            if app.cursor > 0 {
                app.cursor -= 1;
            }
        }
        KeyCode::Right => {
            if app.cursor < app.input.chars().count() {
                app.cursor += 1;
            }
        }
        KeyCode::Home => app.cursor = 0,
        KeyCode::End => app.cursor = app.input.chars().count(),
        KeyCode::Esc => app.popup = Popup::None,
        _ => {}
    }
}

/// OSC 52 writes to the terminal's clipboard, including over SSH. Base64 keeps
/// untrusted message bytes out of the terminal control channel. Do not query the
/// clipboard, invoke external commands, bypass tmux, or claim OS acceptance.
fn write_clipboard(writer: &mut impl io::Write, text: &str) -> io::Result<()> {
    use base64::Engine;
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "clipboard payload too large"));
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    write!(writer, "\x1b]52;c;{encoded}\x07")?;
    writer.flush()
}

fn bump_popup_selection(app: &mut App, delta: i32) {
    match &mut app.popup {
        Popup::Slash { matches, selected } => {
            let len = matches.len();
            if len == 0 {
                return;
            }
            *selected = ((*selected as i32 + delta).rem_euclid(len as i32)) as usize;
        }
        Popup::File { matches, selected, .. } => {
            let len = matches.len();
            if len == 0 {
                return;
            }
            *selected = ((*selected as i32 + delta).rem_euclid(len as i32)) as usize;
        }
        Popup::None => {}
    }
}

fn accept_popup(app: &mut App) -> bool {
    let popup = std::mem::replace(&mut app.popup, Popup::None);
    match popup {
        Popup::Slash { matches, selected } => {
            let Some(&idx) = matches.get(selected) else {
                return false;
            };
            let cmd = SLASH_COMMANDS[idx].name.to_string();
            // Replace the slash-token at the start of input with the command + space.
            app.input.clear();
            app.input.push_str(&cmd);
            app.cursor = app.input.chars().count();
            true
        }
        Popup::File {
            prefix, matches, selected, ..
        } => {
            let Some(path) = matches.get(selected).cloned() else {
                return false;
            };
            // Replace the @<prefix> token before the cursor with @<path>.
            replace_at_token(app, &prefix, &path);
            // Refresh popup in case the user wants to keep navigating into a dir.
            app.update_popup();
            true
        }
        Popup::None => false,
    }
}

fn insert_char(app: &mut App, c: char) {
    let mut chars: Vec<char> = app.input.chars().collect();
    let pos = app.cursor.min(chars.len());
    chars.insert(pos, c);
    app.input = chars.into_iter().collect();
    app.cursor += 1;
}

fn insert_str(app: &mut App, s: &str) {
    let mut chars: Vec<char> = app.input.chars().collect();
    let pos = app.cursor.min(chars.len());
    for (i, c) in s.chars().enumerate() {
        chars.insert(pos + i, c);
    }
    let added = s.chars().count();
    app.input = chars.into_iter().collect();
    app.cursor += added;
}

fn backspace(app: &mut App) {
    if app.cursor == 0 {
        return;
    }
    let mut chars: Vec<char> = app.input.chars().collect();
    chars.remove(app.cursor - 1);
    app.input = chars.into_iter().collect();
    app.cursor -= 1;
}

fn delete_forward(app: &mut App) {
    let mut chars: Vec<char> = app.input.chars().collect();
    if app.cursor < chars.len() {
        chars.remove(app.cursor);
        app.input = chars.into_iter().collect();
    }
}

/// Replace the `@<prefix>` token immediately before the cursor with `@<replacement>`.
fn replace_at_token(app: &mut App, prefix: &str, replacement: &str) {
    let mut chars: Vec<char> = app.input.chars().collect();
    // Find `@` immediately preceding cursor (skipping the prefix).
    let token_len = prefix.chars().count() + 1; // +1 for '@'
    let start = app.cursor.saturating_sub(token_len);
    // Verify start is actually an '@'; if not, just append.
    if chars.get(start) != Some(&'@') {
        let appended = format!("@{replacement}");
        for c in appended.chars() {
            chars.insert(app.cursor, c);
            app.cursor += 1;
        }
        app.input = chars.into_iter().collect();
        return;
    }
    chars.drain(start..app.cursor);
    let new_token = format!("@{replacement}");
    let new_token_chars = new_token.chars().count();
    for (i, c) in new_token.chars().enumerate() {
        chars.insert(start + i, c);
    }
    app.cursor = start + new_token_chars;
    app.input = chars.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn selection_mouse_toggle_reports_native_selection_and_keeps_navigation() {
        let mut app = dummy_app();
        app.update_viewport(100, 60, 10);
        handle_key(&mut app, KeyCode::F(2), KeyModifiers::NONE).await;
        assert!(app.status_msg.as_ref().is_some_and(|(s, _)| s.contains("Mouse: OFF")),
            "F2 must release mouse capture for native selection");
        handle_key(&mut app, KeyCode::PageUp, KeyModifiers::NONE).await;
        assert_eq!(app.scroll_offset, 5);
        handle_key(&mut app, KeyCode::F(2), KeyModifiers::NONE).await;
        assert!(app.status_msg.as_ref().is_some_and(|(s, _)| s.contains("Mouse: ON")));
    }

    #[tokio::test]
    async fn selection_mouse_reporting_bytes_and_wheel_follow_the_toggle() {
        use crossterm::event::MouseEvent;
        let mut app = dummy_app();
        app.update_viewport(100, 60, 10);
        let mut bytes = Vec::new();
        let mut captured = true;
        let wheel = Event::Mouse(MouseEvent {kind: MouseEventKind::ScrollUp,
            column: 2, row: 3, modifiers: KeyModifiers::NONE});
        handle_event(wheel.clone(), &mut app).await;
        assert_eq!(app.scroll_offset, 3);
        handle_key(&mut app, KeyCode::F(2), KeyModifiers::NONE).await;
        app.sync_mouse_capture(&mut bytes, &mut captured).unwrap();
        let mut expected = Vec::new();
        execute!(&mut expected, DisableMouseCapture).unwrap();
        assert_eq!(bytes, expected);
        assert!(!captured);
        handle_event(wheel.clone(), &mut app).await;
        assert_eq!(app.scroll_offset, 3, "ignore queued wheel events after releasing capture");
        app.sync_mouse_capture(&mut bytes, &mut captured).unwrap();
        assert_eq!(bytes, expected, "do not emit mode changes on every redraw");
        handle_key(&mut app, KeyCode::F(2), KeyModifiers::NONE).await;
        app.sync_mouse_capture(&mut bytes, &mut captured).unwrap();
        execute!(&mut expected, EnableMouseCapture).unwrap();
        assert_eq!(bytes, expected);
        handle_event(wheel, &mut app).await;
        assert_eq!(app.scroll_offset, 6);
    }

    #[tokio::test]
    async fn selection_copy_does_not_quit_or_submit_the_draft() {
        for remote in [false, true] {
            let mut app = dummy_app();
            if remote { app.remote = crate::remote::Remote::new("http://127.0.0.1:0".into(), "synthetic".into()).unwrap(); }
            app.transcript.push(Bubble::Assistant { content: "line one\n  世界 👩‍💻".into() });
            app.input = "unsent draft".into();
            app.cursor = app.input.chars().count();
            handle_key(&mut app, KeyCode::F(3), KeyModifiers::NONE).await;
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
            assert!(!app.should_quit, "Ctrl+C in copy mode must copy, not exit");
            assert_eq!(app.input, "unsent draft");
            assert_eq!(app.transcript.len(), 1);
            assert!(!app.agent_active);
        }
    }

    #[tokio::test]
    async fn selection_baseline_paste_preserves_unicode_and_does_not_send() {
        let mut app = dummy_app();
        handle_event(Event::Paste("/quit\r\n  世界 👩‍💻\r\n\n".into()), &mut app).await;
        assert_eq!(app.input, "/quit\n  世界 👩‍💻\n\n");
        assert_eq!(app.cursor, app.input.chars().count());
        assert!(app.transcript.is_empty());
        assert!(!app.should_quit);
    }

    #[tokio::test]
    async fn selection_clipboard_is_explicit_exact_and_local_in_both_connection_modes() {
        use base64::Engine;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
        let server = MockServer::start().await;
        Mock::given(any()).respond_with(ResponseTemplate::new(500)).expect(0).mount(&server).await;
        let content = "  世界 e\u{301} 👩‍💻\n\n\tprintf 'literal \\n'\n\x1b]52;c;evil\x07";
        for remote in [false, true] {
            let mut app = dummy_app();
            app.gateway_url = server.uri();
            if remote { app.remote = crate::remote::Remote::new(server.uri(), "synthetic".into()).unwrap(); }
            app.transcript.push(Bubble::Assistant {content: content.into()});
            app.input = "draft".into(); app.cursor = 2;
            app.attachments.push("not-uploaded".into());
            handle_key(&mut app, KeyCode::F(3), KeyModifiers::NONE).await;
            let mut bytes = Vec::new();
            app.flush_clipboard(&mut bytes);
            assert!(bytes.is_empty(), "entering selection must not change the clipboard");
            handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE).await;
            handle_event(Event::Paste("/quit\nnot sent".into()), &mut app).await;
            assert!(app.pending_clipboard.is_none());
            assert_eq!(app.input, "draft"); assert_eq!(app.cursor, 2);
            assert_eq!(app.attachments, ["not-uploaded"]);
            // An unrelated live update must not change the selected snapshot.
            app.stream_event("char", "unselected streaming text");
            assert_eq!(app.live.text, "unselected streaming text");
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
            assert_eq!(app.pending_clipboard.as_deref(), Some(content));
            app.flush_clipboard(&mut bytes);
            let expected = format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(content));
            assert_eq!(bytes, expected.as_bytes());
            assert_eq!(bytes.iter().filter(|&&b| b == 0x1b).count(), 1);
            app.flush_clipboard(&mut bytes);
            assert_eq!(bytes, expected.as_bytes(), "write only once per explicit request");
            assert!(!app.should_quit); assert!(!app.agent_active);
            handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;
            handle_event(Event::Paste("\r\n世界".into()), &mut app).await;
            assert_eq!(app.input, "dr\n世界aft");
            assert_eq!(app.cursor, 5);
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
            assert!(app.should_quit, "original Ctrl+C quit stays intact outside selection");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn selection_navigation_bounds_snapshot_reset_and_intentional_quit() {
        let mut app = dummy_app();
        handle_key(&mut app, KeyCode::F(3), KeyModifiers::NONE).await;
        assert!(app.copy_selection.is_none());
        app.transcript = vec![Bubble::User {content: "one".into()},
            Bubble::Tool {name: "tool".into(), content: "two".into()},
            Bubble::Assistant {content: "three".into()}];
        app.run_slash("/copy").await;
        assert_eq!(app.copy_selection.as_ref().unwrap().index, 2);
        for (key, expected) in [(KeyCode::Down, 2), (KeyCode::Up, 1), (KeyCode::Home, 0),
            (KeyCode::Up, 0), (KeyCode::End, 2)] {
            handle_key(&mut app, key, KeyModifiers::NONE).await;
            assert_eq!(app.copy_selection.as_ref().unwrap().index, expected);
        }
        app.transcript[2] = Bubble::Assistant {content: "changed".into()};
        app.transcript.push(Bubble::Assistant {content: "new".into()});
        handle_key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE).await;
        assert_eq!(app.pending_clipboard.as_deref(), Some("three"));
        handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL).await;
        assert!(app.should_quit); assert!(app.pending_clipboard.is_none());
        app.should_quit = false;
        app.request_copy();
        app.reset_view();
        assert!(app.copy_selection.is_none()); assert!(app.pending_clipboard.is_none());
    }

    #[test]
    fn selection_clipboard_limits_and_write_failures_do_not_claim_success() {
        struct Broken;
        impl io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::ErrorKind::BrokenPipe.into()) }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let mut app = dummy_app();
        app.transcript.push(Bubble::Assistant {content: "世".repeat(MAX_CLIPBOARD_BYTES / 3 + 1)});
        app.select_copy_message(0);
        app.request_copy();
        assert!(app.pending_clipboard.is_none());
        assert!(app.status_msg.as_ref().unwrap().0.contains("refused"));
        let mut bytes = Vec::new();
        assert!(write_clipboard(&mut bytes, &"a".repeat(MAX_CLIPBOARD_BYTES + 1)).is_err());
        assert!(bytes.is_empty());
        assert!(write_clipboard(&mut bytes, &"a".repeat(MAX_CLIPBOARD_BYTES)).is_ok());
        app.pending_clipboard = Some("test".into());
        app.flush_clipboard(&mut Broken);
        assert!(app.pending_clipboard.is_none());
        assert!(app.status_msg.as_ref().unwrap().0.contains("failed"));
        app.mouse_capture = false;
        let mut captured = true;
        assert!(app.sync_mouse_capture(&mut Broken, &mut captured).is_err());
        assert!(captured, "failed mode write must not update tracked terminal state");
    }

    #[tokio::test]
    async fn selection_help_and_slash_mouse_are_discoverable() {
        let mut app = dummy_app();
        app.run_slash("/mouse").await;
        assert!(!app.mouse_capture);
        app.run_slash("/help").await;
        let Bubble::Banner {content, ..} = app.transcript.last().unwrap() else { panic!() };
        for hint in ["F2", "F3", "OSC 52", "Ctrl+Q", "PgUp", "Paste:", "/copy", "/mouse"] {
            assert!(content.contains(hint), "missing help: {hint}");
        }
    }

    #[test]
    fn split_at_attachments_basic() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let input = format!("hello @{} world", path);
        let (clean, atts) = split_at_attachments(&input);
        assert_eq!(clean, "hello world");
        assert_eq!(atts, vec![path]);
    }

    #[test]
    fn sending_preserves_line_breaks_blank_lines_and_indentation() {
        let text = "first\n\n    code  with  spaces\n\tmore\n";
        assert_eq!(split_at_attachments(text).0, text);
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let text = format!("first @{}\n\n    code\nlast", tmp.path().display());
        assert_eq!(split_at_attachments(&text).0, "first \n\n    code\nlast");
    }

    #[tokio::test]
    async fn multiline_input_is_sent_and_echoed_without_flattening() {
        use wiremock::{matchers::{method, body_partial_json}, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let text = "    first\n\nsecond\n";
        Mock::given(method("POST")).and(body_partial_json(serde_json::json!({"message":text})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":false,"error":"test failure"})))
            .expect(1).mount(&server).await;
        let mut app = dummy_app();
        app.gateway_url = server.uri();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        app.event_tx = Some(tx);
        app.input = text.into();
        app.send_message().await;
        assert!(matches!(&app.transcript[0], Bubble::User { content } if content == text));
        let (_, event, data) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        assert_eq!(event, "request_failed");
        app.stream_event(&event, &data);
        assert!(!app.agent_active);
        assert!(matches!(&app.transcript[1], Bubble::Banner {kind: BannerKind::Error,content} if content == "test failure"));
    }

    #[test]
    fn scrolling_clamps_and_failed_requests_clear_previews_without_duplicate_errors() {
        let mut app = dummy_app();
        app.update_viewport(100, 30, 10);
        app.scroll_up(usize::MAX);
        assert_eq!(app.scroll_offset, 90);
        app.scroll_down(5);
        assert_eq!(app.scroll_offset, 85);
        app.scroll_down(usize::MAX);
        assert!(app.at_bottom);
        app.live.apply("tool_call_delta", r#"{"index":0,"name":"read_file","arguments":"{"}"#);
        app.stream_event("feedback", "LLM request failed: specific cause");
        app.stream_event("request_failed", "LLM request failed: specific cause");
        assert!(app.live.bubbles().is_empty());
        assert_eq!(app.transcript.len(), 1);
    }

    fn saved_assistant(id: i64, content: &str) -> Message {
        let mut message = Message::assistant(content.into());
        message.id = Some(id);
        message
    }

    fn terminal_message(id: i64, tool: &str, command: &str, text: &str) -> Message {
        let mut message = saved_assistant(id, text);
        message.tool_calls = Some(vec![crate::model::ToolCallData {
            id: "reused-call-id".into(),
            function: crate::model::FunctionCallData {
                name: tool.into(),
                arguments: serde_json::json!({"command": command, "cwd": "/synthetic"}).to_string(),
            },
        }]);
        message
    }

    #[test]
    fn terminal_commands_survive_history_live_handoff_and_reload() {
        let command = "printf 'Grüße 世界'\n  printf '%s' 'C:\\new\\test'";
        for tool in ["execute_terminal", "run_background", "vm_shell"] {
            for history_first in [false, true] {
                for text in ["", "I'll run this command."] {
                    let mut app = dummy_app();
                    let message = terminal_message(10, tool, command, text);
                    app.stream_event("stream_start", "{}");
                    if history_first { app.append_messages(vec![message.clone()]); }
                    app.stream_event("assistant", text);
                    app.stream_event("stream_end", "{}");
                    app.stream_event("history", &serde_json::to_string(&vec![message.clone()]).unwrap());
                    app.append_messages(vec![message.clone()]);
                    let expected = usize::from(!text.is_empty()) + 1;
                    assert_eq!(app.transcript.len(), expected, "missing command for {tool}, history_first={history_first}, text={text:?}");
                    assert!(matches!(app.transcript.last(), Some(Bubble::Tool {name, content})
                        if name == &format!("{tool} — command") && content == command));
                    if !text.is_empty() {
                        assert!(matches!(&app.transcript[0], Bubble::Assistant {content} if content == text));
                    }
                    // Provider call IDs are not globally unique.
                    let mut repeated = message.clone(); repeated.id = Some(11);
                    app.append_messages(vec![repeated]);
                    assert_eq!(app.transcript.len(), 2 * expected);
                    app.reset_view();
                    app.append_messages(vec![message]);
                    assert_eq!(app.transcript.len(), expected);
                    assert!(matches!(app.transcript.last(), Some(Bubble::Tool {content, ..}) if content == command));
                }
            }
        }
    }

    #[test]
    fn terminal_history_preserves_call_order_results_and_unrelated_payload_privacy() {
        let first = terminal_message(1, "execute_terminal", "first\n  command", "Plan");
        let second = terminal_message(2, "vm_shell", "second 世界", "");
        let mut message = first.clone();
        message.tool_calls.as_mut().unwrap().extend(second.tool_calls.unwrap());
        // Nonterminal arguments must not leak into the transcript.
        let hidden = terminal_message(3, "write_file", "PRIVATE_UNRELATED_PAYLOAD", "");
        message.tool_calls.as_mut().unwrap().extend(hidden.tool_calls.unwrap());
        let mut result = Message::tool("result text".into(), "reused-call-id".into());
        result.id = Some(4);
        let mut app = dummy_app();
        app.append_messages(vec![message.clone(), result.clone()]);
        app.append_messages(vec![message, result]);
        assert_eq!(app.transcript.len(), 4);
        assert!(matches!(&app.transcript[0], Bubble::Assistant {content} if content == "Plan"));
        assert!(matches!(&app.transcript[1], Bubble::Tool {content, ..} if content == "first\n  command"));
        assert!(matches!(&app.transcript[2], Bubble::Tool {content, ..} if content == "second 世界"));
        assert!(matches!(&app.transcript[3], Bubble::ToolResult {content, ..} if content == "result text"));
    }

    #[tokio::test]
    async fn terminal_commands_arrive_via_authenticated_remote_history() {
        use wiremock::{matchers::{method, path, header}, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let message = terminal_message(21, "execute_terminal", "printf 'remote 世界'", "");
        Mock::given(method("GET")).and(path("/v1/messages/synthetic"))
            .and(header("Authorization", "Bearer synthetic-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"messages":[message]})))
            .expect(1).mount(&server).await;
        let remote = crate::remote::Remote::new(server.uri(), "synthetic-key".into()).unwrap();
        let mut app = dummy_app();
        app.remote = remote.clone();
        // The live event is only a truncated preview; use the same authoritative
        // HTTP history path as the remote poll task, never execute the preview.
        app.stream_event("tool_call", r#"{"tool":"execute_terminal","call_id":"reused-call-id","args_preview":{"args":"{\"command\":\"partial"}}"#);
        let history = remote.messages("synthetic").await.unwrap();
        app.stream_event("history", &serde_json::to_string(&history).unwrap());
        app.stream_event("history", &serde_json::to_string(&history).unwrap());
        assert_eq!(app.transcript.len(), 1);
        assert!(matches!(&app.transcript[0], Bubble::Tool {content, ..} if content == "printf 'remote 世界'"));
    }

    #[test]
    fn terminal_commands_require_complete_string_arguments_and_keep_legacy_identity() {
        let mut app = dummy_app();
        let args = [r#"{"command":"truncated"#,
            r#"{"command":42}"#, r#"{"cwd":"/private"}"#, r#"null"#, r#"["not a command"]"#];
        for (index, args) in args.iter().enumerate() {
            let mut message = terminal_message(index as i64 + 1, "execute_terminal", "", "");
            message.tool_calls.as_mut().unwrap()[0].function.arguments = (*args).into();
            app.append_messages(vec![message]);
        }
        assert_eq!(app.transcript.len(), args.len());
        for bubble in &app.transcript {
            assert!(matches!(bubble, Bubble::Tool {content, ..}
                if content == "(command unavailable: invalid or incomplete arguments)"));
        }
        let long = format!("printf '%s' '{}';\n  printf 'END 世界'", "x".repeat(1000));
        for command in ["", long.as_str()] {
            let mut message = terminal_message(100, "run_background", command, "");
            message.id = None; // older history endpoints may not send DB IDs
            app.append_messages(vec![message.clone(), message]);
        }
        assert_eq!(app.transcript.len(), args.len() + 2);
        assert!(matches!(&app.transcript[args.len()], Bubble::Tool {content, ..} if content.is_empty()));
        assert!(matches!(app.transcript.last(), Some(Bubble::Tool {content, ..}) if content == &long));
    }

    #[test]
    fn streamed_reply_survives_saved_history_handoff_in_both_event_orders() {
        for history_first in [false, true] {
            let mut app = dummy_app();
            // Exercise remote mode: there is no synchronous local DB refresh.
            app.remote = crate::remote::Remote::new("http://127.0.0.1:0".into(), "test".into()).unwrap();
            let message = saved_assistant(10, "line one\n\n    line two");
            app.stream_event("stream_start", "{}");
            app.stream_event("char", &message.content);
            if history_first { app.append_messages(vec![message.clone()]); }
            app.stream_event("assistant", &message.content);
            assert_eq!(app.transcript.len(), 1);
            assert!(app.live.text.is_empty());
            app.stream_event("assistant_saved", &serde_json::to_string(&message).unwrap());
            app.append_messages(vec![message.clone()]);
            app.stream_event("stream_start", "{}");
            assert_eq!(app.transcript.len(), 1);
            assert!(matches!(&app.transcript[0], Bubble::Assistant {content} if content == &message.content));
            assert!(app.pending_echoes.is_empty());
        }
    }

    #[test]
    fn saved_notification_installs_reply_immediately_and_ignores_malformed_events() {
        let mut app = dummy_app();
        app.stream_event("stream_start", "{}");
        app.stream_event("char", "complete text");
        app.stream_event("assistant_saved", "not JSON");
        assert_eq!(app.live.text, "complete text");
        app.stream_event("assistant_saved", &serde_json::to_string(&saved_assistant(1, "complete text")).unwrap());
        assert_eq!(app.transcript.len(), 1);
        assert!(app.live.text.is_empty());
        app.stream_event("stream_start", "{}");
        app.stream_event("char", "next response");
        app.stream_event("assistant_saved", &serde_json::to_string(&saved_assistant(1, "complete text")).unwrap());
        assert_eq!(app.live.text, "next response", "late saves must not erase a newer stream");
    }

    #[test]
    fn tool_continuations_and_reconnects_keep_validated_replies_visible() {
        let mut app = dummy_app();
        app.stream_event("stream_start", "{}");
        app.stream_event("assistant", "I'll check that.");
        app.stream_event("stream_start", "{}");
        assert_eq!(app.transcript.len(), 1);
        app.append_messages(vec![saved_assistant(1, "I'll check that.")]);
        app.stream_event("char", "partial");
        app.stream_event("stream_disconnected", "");
        assert_eq!(app.live.text, "partial");
        app.stream_event("char", "possibly gapped tail");
        assert_eq!(app.live.text, "partial");
        app.append_messages(vec![saved_assistant(2, "partial reply recovered from history")]);
        assert_eq!(app.transcript.len(), 2);
        assert!(app.live.bubbles().is_empty());
        app.stream_event("stream_abort", "{}");
        assert_eq!(app.transcript.len(), 2, "only provisional output may be aborted");
    }

    #[test]
    fn history_identity_does_not_collapse_repeated_messages_or_shared_prefixes() {
        let mut app = dummy_app();
        let first = format!("{} first ending", "same prefix ".repeat(10));
        let second = format!("{} second ending", "same prefix ".repeat(10));
        let messages = vec![saved_assistant(1, &first), saved_assistant(2, &second), saved_assistant(3, &second)];
        app.append_messages(messages.clone());
        app.append_messages(messages);
        assert_eq!(app.transcript.len(), 3);
        // Repeated optimistic user echoes must each bind to a different ID.
        for id in [4, 5] {
            app.pending_echoes.push(app.transcript.len());
            app.transcript.push(Bubble::User {content: "again".into()});
            let mut message = Message::user("again".into());
            message.id = Some(id);
            app.append_messages(vec![message]);
        }
        assert_eq!(app.transcript.len(), 5);
        assert!(app.pending_echoes.is_empty());
    }

    #[test]
    fn split_at_attachments_no_match_passes_through() {
        let (clean, atts) = split_at_attachments("hello @/totally/nope/file.txt world");
        assert_eq!(clean, "hello @/totally/nope/file.txt world");
        assert!(atts.is_empty());
    }

    #[test]
    fn file_matches_lists_cwd() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _f1 = std::fs::File::create(tmp.path().join("alpha.txt")).unwrap();
        let _f2 = std::fs::File::create(tmp.path().join("alphabet.txt")).unwrap();
        let prev = std::env::current_dir().ok();
        std::env::set_current_dir(tmp.path()).unwrap();
        let out = file_matches("al");
        if let Some(p) = prev {
            let _ = std::env::set_current_dir(p);
        }
        assert!(out.iter().any(|s| s.ends_with("alpha.txt")));
        assert!(out.iter().any(|s| s.ends_with("alphabet.txt")));
    }

    #[test]
    fn replace_at_token_works() {
        let mut app = dummy_app();
        app.input = "see @al".to_string();
        app.cursor = app.input.chars().count();
        replace_at_token(&mut app, "al", "/etc/hosts");
        assert_eq!(app.input, "see @/etc/hosts");
        assert_eq!(app.cursor, app.input.chars().count());
    }

    #[tokio::test]
    async fn structured_results_remote_history_keeps_raw_for_copy() {
        use wiremock::{matchers::{method, path, header}, Mock, MockServer, ResponseTemplate};
        let raw = serde_json::json!({"stdout":"first\n  世界\nlast","stderr":"","exit_code":0,
            "nested":{"key":"literal\\n"}}).to_string();
        {
            let mut app = dummy_app();
            let user = app.active_user_id().to_owned();
            let mut message = Message::tool(raw.clone(), "fixture".into());
            message.tool_name = Some("execute_terminal".into());
            message.id = Some(41);
            let server = MockServer::start().await;
            let history = {
                Mock::given(method("GET")).and(path(format!("/v1/messages/{user}")))
                    .and(header("Authorization", "Bearer synthetic-key"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"messages":[message]})))
                    .expect(1).mount(&server).await;
                let remote = crate::remote::Remote::new(server.uri(), "synthetic-key".into()).unwrap();
                app.remote = remote.clone();
                remote.messages(&user).await.unwrap()
            };
            for _ in 0..2 {
                app.stream_event("history", &serde_json::to_string(&history).unwrap());
            }
            assert_eq!(app.transcript.len(), 1);
            assert!(matches!(&app.transcript[0], Bubble::ToolResult {content,display,..}
                if content == &raw && display.contains("first\n  世界\nlast")));
            handle_key(&mut app, KeyCode::F(3), KeyModifiers::NONE).await;
            handle_key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE).await;
            assert_eq!(app.pending_clipboard.as_deref(), Some(raw.as_str()));
            assert_eq!(history[0].content, raw, "client formatting must not mutate API history");
            app.reset_view();
            app.stream_event("history", &serde_json::to_string(&history).unwrap());
            assert!(matches!(&app.transcript[0], Bubble::ToolResult {content,..} if content == &raw));
        }
    }

    fn dummy_app() -> App {
        // These helpers only exercise UI state; the gateway is never dialed.
        App::new(
            crate::remote::Remote::new("http://127.0.0.1:0".into(), "x".into()).unwrap(),
            "http://127.0.0.1:0".into(),
            "x".into(),
            ".".into(),
        )
    }
}


/// Minimal shell-like splitting: whitespace separated, single/double quotes group.
fn shell_words(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;
    for ch in input.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"') | (None, '\'') => { quote = Some(ch); has_token = true; }
            (None, c) if c.is_whitespace() => { if has_token || !current.is_empty() { out.push(std::mem::take(&mut current)); has_token = false; } }
            (None, c) => { current.push(c); has_token = true; }
        }
    }
    if has_token || !current.is_empty() { out.push(current); }
    out
}

/// Turn `/login …` / `/logout …` arguments into a gateway request.
/// `Ok(None)` means "show status".
fn parse_login_args(verb: &str, args: &[String]) -> Result<Option<serde_json::Value>, String> {
    let usage = "Usage: /login | /login codex [--device-auth | --auth-json '<json>'] [model] | /login <openai|anthropic|openrouter|minimax|mimo> <api_key> [model] [api_base] | /login <ollama|llamacpp> [api_base] [model] | /logout <provider>";
    let Some(provider) = args.first() else {
        return if verb == "/logout" { Err(usage.into()) } else { Ok(None) };
    };
    let provider = provider.to_ascii_lowercase();
    if verb == "/logout" {
        return Ok(Some(serde_json::json!({"provider": provider, "logout": true})));
    }
    let mut request = serde_json::json!({"provider": provider});
    let rest = &args[1..];
    match provider.as_str() {
        "codex" => {
            let mut args = rest.iter();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--auth-json" if request.get("auth_json").is_none() => {
                        request["auth_json"] = serde_json::json!(args.next().ok_or(usage)?);
                    }
                    "--device-auth" if request.get("device_auth").is_none() => {
                        request["device_auth"] = serde_json::json!(true);
                    }
                    _ if !arg.starts_with("--") && request.get("model").is_none() => {
                        request["model"] = serde_json::json!(arg);
                    }
                    _ => return Err(usage.into()),
                }
            }
            if request.get("auth_json").is_some() && request.get("device_auth").is_some() {
                return Err(usage.into());
            }
        }
        "ollama" | "llamacpp" => {
            for arg in rest {
                if arg.starts_with("http://") || arg.starts_with("https://") { request["api_base"] = serde_json::json!(arg); }
                else if request.get("model").is_none() { request["model"] = serde_json::json!(arg); }
                else { request["api_key"] = serde_json::json!(arg); }
            }
        }
        _ => {
            let key = rest.first().ok_or(usage)?;
            request["api_key"] = serde_json::json!(key);
            for arg in &rest[1..] {
                if arg.starts_with("http://") || arg.starts_with("https://") { request["api_base"] = serde_json::json!(arg); }
                else { request["model"] = serde_json::json!(arg); }
            }
        }
    }
    Ok(Some(request))
}

#[cfg(test)]
mod login_parse_tests {
    use super::*;

    #[test]
    fn login_arguments_map_to_gateway_requests() {
        assert_eq!(parse_login_args("/login", &[]).unwrap(), None);
        assert!(parse_login_args("/logout", &[]).is_err());
        let r = parse_login_args("/login", &shell_words("openai sk-test-1234 gpt-4.1 https://proxy.example/v1")).unwrap().unwrap();
        assert_eq!(r["api_key"], "sk-test-1234");
        assert_eq!(r["model"], "gpt-4.1");
        assert_eq!(r["api_base"], "https://proxy.example/v1");
        assert!(parse_login_args("/login", &shell_words("anthropic")).is_err(), "key required");
        let r = parse_login_args("/login", &shell_words("ollama http://gpu:11434 qwen3:8b")).unwrap().unwrap();
        assert_eq!(r["api_base"], "http://gpu:11434");
        assert_eq!(r["model"], "qwen3:8b");
        let r = parse_login_args("/login", &shell_words("codex --auth-json '{\"tokens\":{}}' gpt-5")).unwrap().unwrap();
        assert_eq!(r["auth_json"], "{\"tokens\":{}}");
        assert_eq!(r["model"], "gpt-5");
        let r = parse_login_args("/login", &shell_words("codex --auth-json '  {\"tokens\":{}}'")).unwrap().unwrap();
        assert!(r.get("model").is_none(), "auth JSON is consumed, never guessed to be a model");
        let r = parse_login_args("/login", &shell_words("codex --device-auth gpt-5-codex")).unwrap().unwrap();
        assert_eq!(r["device_auth"], true);
        assert_eq!(r["model"], "gpt-5-codex");
        for args in ["codex --unknown", "codex --auth-json", "codex --device-auth --auth-json '{}'", "codex model1 model2"] {
            assert!(parse_login_args("/login", &shell_words(args)).is_err());
        }
        let r = parse_login_args("/logout", &shell_words("Codex")).unwrap().unwrap();
        assert_eq!(r["logout"], true);
        assert_eq!(r["provider"], "codex");
    }
}
