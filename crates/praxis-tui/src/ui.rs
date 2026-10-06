//! Rendering for the TUI chat. Layout also reconciles the scroll viewport.

use crate::app::{App, BannerKind, Bubble, Popup, SLASH_COMMANDS};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;

// Palette — picked to harmonise with the web dashboard's purple/cyan accents.
const ACCENT_PURPLE: Color = Color::Rgb(0x9a, 0x8c, 0xff);
const ACCENT_CYAN: Color = Color::Rgb(0x6a, 0xea, 0xff);
const ACCENT_GREEN: Color = Color::Rgb(0x6a, 0xff, 0xa6);
const ACCENT_AMBER: Color = Color::Rgb(0xff, 0xc8, 0x6a);
const ACCENT_PINK: Color = Color::Rgb(0xff, 0x8a, 0xb6);
const TEXT_PRIMARY: Color = Color::Rgb(0xe6, 0xe6, 0xff);
const TEXT_DIM: Color = Color::Rgb(0x9a, 0x9a, 0xc0);
const BG_PANEL: Color = Color::Rgb(0x14, 0x14, 0x1f);
const BG_BUBBLE_USER: Color = Color::Rgb(0x1a, 0x1f, 0x36);
const BG_BUBBLE_BOT: Color = Color::Rgb(0x1d, 0x1a, 0x36);
const BG_BUBBLE_TOOL: Color = Color::Rgb(0x12, 0x24, 0x2c);

pub fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();
    if size.width == 0 || size.height == 0 {
        return;
    }

    // Outer split: optional sidebar + main column. On narrow terminals the
    // sidebar used to consume the whole frame, leaving the transcript/input at
    // width 0. Collapse it automatically below a practical breakpoint.
    let sidebar_visible = app.show_sidebar && size.width >= 72;
    let main_chunks = if sidebar_visible {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(1)])
            .split(size)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(0), Constraint::Min(1)])
            .split(size)
    };

    if sidebar_visible {
        draw_sidebar(f, app, main_chunks[0]);
    }
    draw_main(f, app, main_chunks[1]);
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_PURPLE))
        .title(Span::styled(
            " Sessions ",
            Style::default()
                .fg(ACCENT_PURPLE)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG_PANEL));

    let items: Vec<ListItem> = app
        .sessions
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let is_active = s.id == app.sessions.active;
            let marker = if is_active { "▌" } else { " " };
            let key_hint = if i < 9 {
                format!("M-{}", i + 1)
            } else {
                "   ".to_string()
            };
            let style = if is_active {
                Style::default()
                    .fg(ACCENT_CYAN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT_PRIMARY)
            };
            let line = Line::from(vec![
                Span::styled(marker, Style::default().fg(ACCENT_CYAN)),
                Span::raw(" "),
                Span::styled(pad_to_width(&truncate(&s.name, 18), 18), style),
                Span::styled(format!(" {}", key_hint), Style::default().fg(TEXT_DIM)),
            ]);
            ListItem::new(vec![line, Line::from(Span::styled(
                format!("  ctx: {}", s.id), Style::default().fg(TEXT_DIM)
            ))])
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);

    // User name footer.
    if area.height >= 4 {
        let footer_area = Rect {
            x: area.x + 1,
            y: area.y + area.height.saturating_sub(2),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        let footer = Paragraph::new(Line::from(vec![
            Span::styled("user: ", Style::default().fg(TEXT_DIM)),
            Span::styled(
                truncate(app.username(), 18),
                Style::default()
                    .fg(ACCENT_PINK)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        f.render_widget(footer, footer_area);
    }
}

fn draw_main(f: &mut Frame, app: &mut App, area: Rect) {
    // Reserve the composer before allocating transcript space. An unsatisfiable
    // Min(5) transcript constraint used to squeeze the input down to its borders.
    let header_height = if area.height >= 12 { 3 } else { 0 };
    let footer_height = if area.height >= 6 { 1 } else { 0 };
    let available = area.height.saturating_sub(header_height + footer_height);
    let transcript_min = u16::from(available >= 4);
    let composer_height =
        input_height(app, area.width).min(available.saturating_sub(transcript_min));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Length(available.saturating_sub(composer_height)),
            Constraint::Length(composer_height),
            Constraint::Length(footer_height),
        ])
        .split(area);

    draw_header(f, app, chunks[0]);
    draw_transcript(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);
    draw_footer(f, app, chunks[3]);

    // Popups float over the input row.
    match &app.popup {
        Popup::Slash { matches, selected } => {
            draw_slash_popup(f, app, chunks[2], matches, *selected)
        }
        Popup::File {
            matches, selected, ..
        } => draw_file_popup(f, app, chunks[2], matches, *selected),
        Popup::None => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let session_name = app
        .sessions
        .active_session()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| app.sessions.active.clone());
    let agent = if app.agent_active {
        Span::styled(
            " ● Agent active ",
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT_CYAN)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" ○ Idle ", Style::default().fg(TEXT_DIM))
    };
    // Show last response tokens/sec if available.
    let usage_span = if let Some(ref usage) = app.last_response_usage {
        if usage.tokens_per_sec > 0.0 {
            Span::styled(
                format!(" {:.1} tok/s ", usage.tokens_per_sec),
                Style::default().fg(ACCENT_GREEN),
            )
        } else {
            Span::raw("")
        }
    } else {
        Span::raw("")
    };
    let session_tokens = if app.session_total_tokens > 0 {
        Span::styled(
            format!(" {} tok ", app.session_total_tokens),
            Style::default().fg(TEXT_DIM),
        )
    } else {
        Span::raw("")
    };
    let title = Line::from(vec![
        Span::styled(
            "  Praxis ",
            Style::default()
                .fg(ACCENT_CYAN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Chat",
            Style::default()
                .fg(ACCENT_PURPLE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            format!("» {}  [context: {}] ", session_name, app.active_user_id()),
            Style::default().fg(TEXT_PRIMARY),
        ),
        Span::raw("  "),
        agent,
        usage_span,
        session_tokens,
    ]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(ACCENT_PURPLE));
    let para = Paragraph::new(title).block(block);
    f.render_widget(para, area);
}

fn draw_transcript(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0x33, 0x33, 0x55)))
        .style(Style::default().bg(BG_PANEL))
        // Keep the last line/bubble border clear of the composer, including
        // while streaming. Tiny terminals still get all available text rows.
        .padding(Padding::new(u16::from(area.width >= 6), u16::from(area.width >= 6), 0, u16::from(area.height >= 5)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() { return; }
    let bubble_max = inner.width as usize;

    // Build all lines, top-to-bottom.
    let mut lines: Vec<Line> = Vec::new();
    let mut selection_start = None;
    let live = app.live.bubbles();
    let transcript: Vec<_> = app.transcript.iter().chain(live.iter()).collect();
    if transcript.is_empty() {
        lines.push(Line::from(""));
        for (text, style) in [
            ("No messages yet — type a message and press Enter.", Style::default().fg(TEXT_DIM).add_modifier(Modifier::ITALIC)),
            ("Try `/help` for commands or attach files with @path", Style::default().fg(TEXT_DIM)),
        ] {
            lines.extend(wrap_text(text, bubble_max).into_iter().map(|text| Line::from(Span::styled(text, style))));
        }
    } else {
        for (idx, b) in transcript.iter().enumerate() {
            let selected = app.copy_selection.as_ref().filter(|selection| selection.index == idx);
            let mut snapshot;
            let rendered = if let Some(selection) = selected {
                snapshot = (*b).clone();
                if let Bubble::ToolResult {name, ..} = &snapshot {
                    snapshot = Bubble::Tool {name: format!("{name} — raw"), content: selection.content.clone()};
                }
                match &mut snapshot {
                    Bubble::User {content} | Bubble::Assistant {content} | Bubble::Thinking {content}
                    | Bubble::Tool {content, ..} | Bubble::ToolResult {content, ..} | Bubble::System {content} | Bubble::Banner {content, ..} => {
                        *content = selection.content.clone();
                    }
                }
                selection_start = Some(lines.len());
                &snapshot
            } else if app.folded_bubbles.contains(&idx) {
                // Show collapsed representation for folded tool outputs.
                let line_count = match b {
                    Bubble::Tool { content, .. } | Bubble::ToolResult { content, .. } => {
                        content.lines().count()
                    }
                    _ => 0,
                };
                snapshot = match b {
                    Bubble::Tool { name, .. } => Bubble::Banner {
                        kind: BannerKind::Info,
                        content: format!("⚙ {} — {} lines hidden (Tab/fold to expand)", name, line_count),
                    },
                    Bubble::ToolResult { name, .. } => Bubble::Banner {
                        kind: BannerKind::Info,
                        content: format!("⚙ {} — {} lines hidden (Tab/fold to expand)", name, line_count),
                    },
                    other => (*other).clone(),
                };
                &snapshot
            } else { b };
            let bubble_lines = render_bubble(rendered, bubble_max, app.username());
            for l in bubble_lines {
                lines.push(if selected.is_some() {
                    l.patch_style(Style::default().add_modifier(Modifier::REVERSED))
                } else { l });
            }
            if idx + 1 < transcript.len() {
                lines.push(Line::from(""));
            }
        }
    }

    // Count actual prewrapped rows, NOT the outer rectangle (borders/padding).
    // usize prevents transcripts over 65,535 rows from wrapping back to zero.
    app.update_viewport(lines.len(), inner.width, inner.height);
    if let Some(selection) = &mut app.copy_selection {
        if selection.reveal {
            if let Some(start) = selection_start {
                app.scroll_offset = lines.len().saturating_sub(inner.height as usize).saturating_sub(start);
                app.at_bottom = app.scroll_offset == 0;
            }
            selection.reveal = false;
        }
    }
    let from = lines.len().saturating_sub(inner.height as usize).saturating_sub(app.scroll_offset);
    let visible: Vec<Line> = lines.into_iter().skip(from).take(inner.height as usize).collect();
    // A second wrap here invalidates scrolling and can hide the tail of replies.
    f.render_widget(Paragraph::new(visible), inner);
}

/// Render a single bubble into 1+ ratatui lines.
fn render_bubble(b: &Bubble, max_width: usize, username: &str) -> Vec<Line<'static>> {
    match b {
        Bubble::User { content } => bubble_block(
            username,
            content,
            BubbleAlign::Right,
            ACCENT_CYAN,
            BG_BUBBLE_USER,
            max_width,
        ),
        Bubble::Assistant { content } => bubble_block(
            "praxis",
            content,
            BubbleAlign::Left,
            ACCENT_PURPLE,
            BG_BUBBLE_BOT,
            max_width,
        ),
        Bubble::Thinking { content } => bubble_block(
            "Thinking", content, BubbleAlign::Left, TEXT_DIM, BG_BUBBLE_TOOL, max_width,
        ),
        Bubble::Tool { name, content } => bubble_block(
            &format!("⚙ {name}"),
            content,
            BubbleAlign::Left,
            ACCENT_GREEN,
            BG_BUBBLE_TOOL,
            max_width,
        ),
        Bubble::ToolResult { name, display, .. } => bubble_block(
            &format!("⚙ {name} — F3 raw/copy"), display, BubbleAlign::Left,
            ACCENT_GREEN, BG_BUBBLE_TOOL, max_width,
        ),
        Bubble::System { content } => banner_line(content, ACCENT_GREEN, "·", max_width),
        Bubble::Banner { kind, content } => {
            let (color, icon) = match kind {
                BannerKind::AgentStart => (ACCENT_CYAN, "▶"),
                BannerKind::AgentStop => (ACCENT_PURPLE, "■"),
                BannerKind::Info => (TEXT_DIM, "ℹ"),
                BannerKind::Error => (ACCENT_PINK, "✕"),
            };
            banner_line(content, color, icon, max_width)
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum BubbleAlign {
    Left,
    Right,
}

/// Wrap `content` to `max_width - 4` columns and render as a chat bubble with
/// a label header. Bubbles are framed with rounded corners using box-drawing
/// characters for a soft, modern look.
fn bubble_block(
    label: &str,
    content: &str,
    align: BubbleAlign,
    accent: Color,
    bg: Color,
    max_width: usize,
) -> Vec<Line<'static>> {
    if max_width < 12 {
        return compact_bubble(label, content, accent, max_width.max(1));
    }

    // `max_width` is a terminal column budget. Keep every rendered line within
    // that budget, including borders, padding and wide glyphs.
    let max_inner = max_width.saturating_sub(2).max(1);
    let max_content = max_inner.saturating_sub(2).max(1);
    let wrapped = wrap_text(content, max_content);
    let widest_content = wrapped.iter().map(|l| display_width(l)).max().unwrap_or(0);
    let raw_label = format!(" {label} ");
    let label_budget = max_inner.saturating_sub(1).max(1);
    let label_text = truncate(&raw_label, label_budget);
    let label_width = display_width(&label_text);
    let inner_width = (widest_content + 2)
        .max(label_width + 1)
        .clamp(1, max_inner);
    let content_width = inner_width.saturating_sub(2).max(1);
    let wrapped = if content_width == max_content {
        wrapped
    } else {
        wrap_text(content, content_width)
    };

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Top border width: 1 + inner_width + 1.
    let top_fill = inner_width.saturating_sub(1 + label_width);
    let top = format!("╭─{label_text}{}╮", "─".repeat(top_fill));
    lines.push(align_line(
        top,
        align,
        max_width,
        Style::default()
            .fg(accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    ));

    // Body width: border + (space + content + pad + space) + border.
    let style_body = Style::default().fg(TEXT_PRIMARY).bg(bg);
    let style_border = Style::default().fg(accent).bg(bg);
    for content_line in wrapped {
        let cw = display_width(&content_line);
        let pad = content_width.saturating_sub(cw);
        let row = Line::from(vec![
            Span::styled("│".to_string(), style_border),
            Span::styled(" ".to_string(), style_body),
            Span::styled(content_line, style_body),
            Span::styled(" ".repeat(pad), style_body),
            Span::styled(" ".to_string(), style_body),
            Span::styled("│".to_string(), style_border),
        ]);
        lines.push(align_line_spans(row, align, max_width));
    }

    let bot = format!("╰{}╯", "─".repeat(inner_width));
    lines.push(align_line(
        bot,
        align,
        max_width,
        Style::default().fg(accent).bg(bg),
    ));

    lines
}

fn align_line(text: String, align: BubbleAlign, max_width: usize, style: Style) -> Line<'static> {
    align_line_spans(Line::from(Span::styled(text, style)), align, max_width)
}

fn align_line_spans(line: Line<'static>, align: BubbleAlign, max_width: usize) -> Line<'static> {
    let pad = max_width.saturating_sub(line.width());
    let mut spans: Vec<Span<'static>> = line.spans.into_iter().collect();
    match align {
        BubbleAlign::Left => spans.push(Span::raw(" ".repeat(pad))),
        BubbleAlign::Right => spans.insert(0, Span::raw(" ".repeat(pad))),
    }
    Line::from(spans)
}

fn banner_line(content: &str, color: Color, icon: &str, width: usize) -> Vec<Line<'static>> {
    let style = Style::default().fg(color).add_modifier(Modifier::BOLD | Modifier::ITALIC);
    wrap_text(&format!("{icon}  {content}"), width).into_iter()
        .map(|text| Line::from(Span::styled(text, style))).collect()
}

fn compact_bubble(
    label: &str,
    content: &str,
    accent: Color,
    max_width: usize,
) -> Vec<Line<'static>> {
    let width = max_width.max(1);
    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        truncate(label, width),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    )));
    for line in wrap_text(content, width) {
        out.push(Line::from(Span::styled(
            truncate(&line, width),
            Style::default().fg(TEXT_PRIMARY),
        )));
    }
    out
}

/// Word-wrap by terminal columns, preserving explicit/blank lines, indentation
/// and repeated spaces. Only the separator at a soft wrap is discarded.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw_line in display_text(text).split('\n') {
        let mut current = String::new();
        let mut columns = 0;
        for (index, word) in raw_line.split(' ').enumerate() {
            let ww = display_width(word);
            let separator = usize::from(index > 0);
            if columns + separator + ww <= width {
                if separator > 0 { current.push(' '); }
                current.push_str(word);
                columns += separator + ww;
            } else {
                if !current.is_empty() { out.push(std::mem::take(&mut current)); }
                let mut chunks = chunks_of(word, width);
                current = chunks.pop().unwrap_or_default();
                out.extend(chunks);
                columns = display_width(&current);
            }
        }
        out.push(current);
    }
    out
}

fn display_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ")
        .chars().filter(|ch| *ch == '\n' || !ch.is_control()).collect()
}

fn chunks_of(s: &str, n: usize) -> Vec<String> {
    let width = n.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut columns = 0;
    for grapheme in s.graphemes(true) {
        let gw = display_width(grapheme);
        if columns + gw > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            columns = 0;
        }
        if gw > width {
            chunks.push("…".into());
        } else {
            current.push_str(grapheme);
            columns += gw;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn display_width(s: &str) -> usize {
    Line::from(s).width()
}

fn input_height(app: &App, frame_width: u16) -> u16 {
    let inner_width = frame_width.saturating_sub(4).max(1) as usize;
    let mut lines = input_visual_lines(app, inner_width).0.len().min(10) as u16;
    if !app.attachments.is_empty() {
        lines += 1;
    }
    (lines + 2).clamp(3, 12)
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.height < 3 || area.width < 5 {
        let (visual, cursor_row) = input_visual_lines(app, area.width as usize);
        let rows = area.height as usize;
        let start = cursor_row.saturating_sub(rows.saturating_sub(1));
        let lines: Vec<Line> = visual
            .into_iter()
            .skip(start)
            .take(rows)
            .map(|text| Line::from(Span::styled(text, Style::default().fg(TEXT_PRIMARY))))
            .collect();
        f.render_widget(Paragraph::new(lines), area);
        return;
    }
    let title_str = if app.attachments.is_empty() {
        " Message ".to_string()
    } else {
        format!(" Message · {} attachment(s) ", app.attachments.len())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_CYAN))
        .title(Span::styled(
            title_str,
            Style::default()
                .fg(ACCENT_CYAN)
                .add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    let inner_width = inner.width.max(1) as usize;
    let visible_rows = inner.height.max(1) as usize;
    let show_attachments = !app.attachments.is_empty() && visible_rows > 1;
    let mut lines: Vec<Line> = Vec::new();
    if show_attachments {
        let span_text = app
            .attachments
            .iter()
            .map(|p| {
                let basename = std::path::Path::new(p)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.clone());
                format!("📎 {basename}")
            })
            .collect::<Vec<_>>()
            .join("   ");
        lines.push(Line::from(Span::styled(
            truncate(&span_text, inner_width),
            Style::default().fg(ACCENT_AMBER),
        )));
    }

    let input_style = Style::default().fg(TEXT_PRIMARY);
    if app.input.is_empty() {
        let mut spans = vec![Span::styled("█", Style::default().fg(ACCENT_CYAN))];
        if inner_width > 1 {
            spans.push(Span::styled(
                truncate(
                    " Type a message…  (try /help, @path, Shift+Enter for newline)",
                    inner_width - 1,
                ),
                Style::default().fg(TEXT_DIM).add_modifier(Modifier::ITALIC),
            ));
        }
        lines.push(Line::from(spans));
    } else {
        let input_rows = visible_rows
            .saturating_sub(usize::from(show_attachments))
            .max(1);
        let (visual, cursor_row) = input_visual_lines(app, inner_width);
        let start = cursor_row.saturating_sub(input_rows - 1);
        for l in visual.into_iter().skip(start).take(input_rows) {
            lines.push(Line::from(Span::styled(l, input_style)));
        }
    }

    // Input is already hard-wrapped without discarding spaces. Re-wrapping it
    // would invalidate the cursor row and can hide it again.
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = if let Some(selection) = &app.copy_selection {
        vec![Span::styled(format!("COPY {}/{} ^C/y copy Esc back  ↑/↓ select PgUp/Dn scroll ^Q quit",
            selection.index + 1, app.transcript.len()), Style::default().fg(ACCENT_CYAN))]
    } else { vec![
        Span::raw(format!("F2 mouse:{}  F3 copy  /help  ", if app.mouse_capture { "ON" } else { "OFF" })),
        keybind("Enter", "send"),
        Span::raw("  "),
        keybind("Sh+Enter", "newline"),
        Span::raw("  "),
        keybind("Tab", "complete"),
        Span::raw("  "),
        keybind("^N", "new session"),
        Span::raw("  "),
        keybind("M-1..9", "switch"),
        Span::raw("  "),
        keybind("^B", "sidebar"),
        Span::raw("  "),
        keybind("^C", "quit"),
    ] };
    if let Some((msg, _)) = &app.status_msg {
        spans.insert(0, Span::raw("  ·  "));
        spans.insert(0, Span::styled(
            msg.clone(),
            Style::default()
                .fg(ACCENT_AMBER)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let para = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Left)
        .style(Style::default().fg(TEXT_DIM));
    f.render_widget(para, area);
}

fn keybind<'a>(key: &'a str, label: &'a str) -> Span<'a> {
    Span::raw(format!("{key} {label}"))
}

fn draw_slash_popup(
    f: &mut Frame,
    _app: &App,
    input_area: Rect,
    matches: &[usize],
    selected: usize,
) {
    // Popups must fit ABOVE the composer, even on small/resized terminals.
    // Clamping only y to zero let Clear paint over the input box itself.
    let height = (matches.len().min(8) as u16 + 2)
        .min(input_area.y.saturating_sub(f.area().y));
    let width = 50.min(input_area.width.saturating_sub(2));
    if height < 3 || width < 4 { return; }
    let popup_area = Rect {
        x: input_area.x + 1,
        y: input_area.y.saturating_sub(height),
        width,
        height,
    };
    f.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(i, &cmd_idx)| {
            let cmd = &SLASH_COMMANDS[cmd_idx];
            let is_sel = i == selected;
            let style = if is_sel {
                Style::default()
                    .bg(ACCENT_PURPLE)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT_PRIMARY)
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    pad_to_width(&format!(" {}", cmd.name), 13),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", cmd.desc),
                    if is_sel {
                        Style::default().bg(ACCENT_PURPLE).fg(Color::Black)
                    } else {
                        Style::default().fg(TEXT_DIM)
                    },
                ),
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_PURPLE))
        .title(Span::styled(
            " / commands ",
            Style::default()
                .fg(ACCENT_PURPLE)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG_PANEL));
    let list = List::new(items).block(block);
    f.render_widget(list, popup_area);
}

fn draw_file_popup(
    f: &mut Frame,
    _app: &App,
    input_area: Rect,
    matches: &[String],
    selected: usize,
) {
    let height = (matches.len().min(10) as u16 + 2)
        .min(input_area.y.saturating_sub(f.area().y));
    let width = 70.min(input_area.width.saturating_sub(2));
    if height < 3 || width < 4 { return; }
    let popup_area = Rect {
        x: input_area.x + 1,
        y: input_area.y.saturating_sub(height),
        width,
        height,
    };
    f.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let is_sel = i == selected;
            let display = truncate(p, width.saturating_sub(4) as usize);
            let icon = if p.ends_with('/') { "📁" } else { "📄" };
            let style = if is_sel {
                Style::default()
                    .bg(ACCENT_CYAN)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT_PRIMARY)
            };
            ListItem::new(Line::from(vec![Span::styled(
                format!(" {icon} {display}"),
                style,
            )]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_CYAN))
        .title(Span::styled(
            " @ files ",
            Style::default()
                .fg(ACCENT_CYAN)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG_PANEL));
    let list = List::new(items).block(block);
    f.render_widget(list, popup_area);
}

fn pad_to_width(s: &str, width: usize) -> String {
    let mut out = truncate(s, width);
    let pad = width.saturating_sub(display_width(&out));
    out.push_str(&" ".repeat(pad));
    out
}

fn input_visual_lines(app: &App, width: usize) -> (Vec<String>, usize) {
    let width = width.max(1);
    let chars: Vec<char> = app.input.chars().collect();
    let cursor_pos = app.cursor.min(chars.len());
    let before = display_text(&chars[..cursor_pos].iter().collect::<String>());
    let after = display_text(&chars[cursor_pos..].iter().collect::<String>());
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut columns = 0;
    let mut cursor_row = 0;
    for (index, grapheme) in format!("{before}█{after}").grapheme_indices(true) {
        if grapheme == "\n" {
            lines.push(std::mem::take(&mut current));
            columns = 0;
            continue;
        }
        let piece = truncate(grapheme, width);
        let pw = display_width(&piece);
        if !current.is_empty() && columns + pw > width {
            lines.push(std::mem::take(&mut current));
            columns = 0;
        }
        if index == before.len() {
            cursor_row = lines.len();
        }
        current.push_str(&piece);
        columns += pw;
    }
    lines.push(current);
    (lines, cursor_row)
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let budget = max - 1;
    let mut columns = 0;
    for grapheme in s.graphemes(true) {
        let gw = display_width(grapheme);
        if columns + gw > budget { break; }
        out.push_str(grapheme);
        columns += gw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_width(line: &Line<'_>) -> usize {
        line.width()
    }

    fn assert_lines_fit(lines: &[Line<'_>], width: usize) {
        for line in lines {
            assert!(
                line_width(line) <= width,
                "line width {} > {width}: {:?}",
                line_width(line),
                line
            );
        }
    }

    #[test]
    fn explicit_newlines_indentation_and_graphemes_are_preserved() {
        assert_eq!(wrap_text("first\r\n\r\n    indented  text\n\tcode\n", 80),
            vec!["first", "", "    indented  text", "    code", ""]);
        assert_eq!(chunks_of("ae\u{301}👩‍💻z", 2), vec!["ae\u{301}", "👩‍💻", "z"]);
        let banners = banner_line("first\n\n    second\nlast", TEXT_DIM, "·", 30);
        let texts: Vec<_> = banners.iter().map(ToString::to_string).collect();
        assert_eq!(texts, vec!["·  first", "", "    second", "last"]);
        assert_lines_fit(&banner_line(&"very long error ".repeat(100), TEXT_DIM, "✕", 20), 20);
    }

    #[test]
    fn every_message_kind_renders_explicit_line_breaks_and_blank_lines() {
        let content = "FIRST\n\n    code\nLAST".to_string();
        for bubble in [
            Bubble::User {content: content.clone()},
            Bubble::Assistant {content: content.clone()},
            Bubble::Thinking {content: content.clone()},
            Bubble::Tool {name: "read_file".into(), content: content.clone()},
            Bubble::System {content: content.clone()},
            Bubble::Banner {kind: BannerKind::Error, content},
        ] {
            let lines = render_bubble(&bubble, 30, "user");
            assert_lines_fit(&lines, 30);
            let rows: Vec<_> = lines.iter().map(ToString::to_string).collect();
            let first = rows.iter().position(|row| row.contains("FIRST")).unwrap();
            assert!(!rows[first + 1].chars().any(char::is_alphanumeric), "{rows:?}");
            assert!(rows[first + 2].contains("    code"), "{rows:?}");
            assert!(rows[first + 3].contains("LAST"), "{rows:?}");
        }
    }

    fn screen_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        buffer.content().chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect()).collect()
    }

    #[test]
    fn long_live_reply_and_error_tail_stay_above_bottom_padding_after_resize() {
        let (mut app, _dir) = dummy_app();
        app.live.text = format!("{}\nLAST-LINE", "long line with 日本語 👩‍💻\n".repeat(100));
        for (width, height) in [(24, 10), (40, 8), (80, 24), (20, 7)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw_transcript(f, &mut app, f.area())).unwrap();
            let rows = screen_rows(&terminal);
            assert!(rows.iter().any(|row| row.contains("LAST-LINE")), "{rows:?}");
            assert!(rows[height as usize - 3].contains('╰'), "bubble border missing: {rows:?}");
            assert!(rows[height as usize - 2].contains(&" ".repeat(width as usize - 2)), "no bottom gap: {rows:?}");
        }
        app.live = Default::default();
        app.transcript.push(Bubble::Banner {kind: BannerKind::Error,
            content: format!("{}\nERROR-TAIL", "wide error details ".repeat(500))});
        let mut terminal = Terminal::new(TestBackend::new(24, 10)).unwrap();
        terminal.draw(|f| draw_transcript(f, &mut app, f.area())).unwrap();
        assert!(screen_rows(&terminal).iter().any(|row| row.contains("ERROR-TAIL")));
    }

    #[test]
    fn scrolling_handles_more_than_u16_rows_and_stream_growth() {
        let (mut app, _dir) = dummy_app();
        app.live.text = format!("TOP\n{}TAIL", "row\n".repeat(66_000));
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        terminal.draw(|f| draw_transcript(f, &mut app, f.area())).unwrap();
        assert!(screen_rows(&terminal).iter().any(|row| row.contains("TAIL")));
        app.scroll_offset = usize::MAX;
        app.at_bottom = false;
        terminal.draw(|f| draw_transcript(f, &mut app, f.area())).unwrap();
        assert!(app.scroll_offset > u16::MAX as usize);
        let before = screen_rows(&terminal);
        assert!(before.iter().any(|row| row.contains("TOP")));
        app.live.text.push_str("\nnew\nnew");
        terminal.draw(|f| draw_transcript(f, &mut app, f.area())).unwrap();
        assert_eq!(before, screen_rows(&terminal), "streaming moved a scrolled-up reader");
        app.scroll_offset = 0;
        app.at_bottom = true;
        terminal.draw(|f| draw_transcript(f, &mut app, f.area())).unwrap();
        assert!(screen_rows(&terminal).iter().any(|row| row.contains("new")));
    }

    #[test]
    fn wrap_basic() {
        let lines = wrap_text("hello world this is a test", 10);
        assert_eq!(lines, vec!["hello", "world this", "is a test"]);
    }

    #[test]
    fn wrap_preserves_paragraphs() {
        let lines = wrap_text("a\n\nb", 80);
        assert_eq!(lines, vec!["a", "", "b"]);
    }

    #[test]
    fn column_width_helpers_handle_wide_and_combining_text() {
        assert_eq!(Line::from("日本").width(), 4);
        assert_eq!(display_width("é"), Line::from("é").width());
        assert_eq!(display_width("e\u{301}"), Line::from("e\u{301}").width());
        assert_eq!(display_width("🌵"), Line::from("🌵").width());
        for width in [1, 8, 12, 24, 40] {
            for chunk in chunks_of("日本語🌵e\u{301}abcdef", width) {
                assert!(display_width(&chunk) <= width, "{chunk:?} exceeds {width}");
            }
            for line in wrap_text("日本語 🌵🌵🌵 e\u{301}e\u{301} supercalifragilistic", width)
            {
                assert!(display_width(&line) <= width, "{line:?} exceeds {width}");
            }
        }
    }

    #[test]
    fn truncate_works_by_columns() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
        assert!(Line::from(truncate("日本語🌵", 5)).width() <= 5);
        assert!(Line::from(truncate("e\u{301}e\u{301}e\u{301}", 3)).width() <= 3);
    }

    #[test]
    fn bubbles_fit_all_small_widths_with_wide_text_and_long_labels() {
        for width in [1, 8, 12, 24, 40] {
            for align in [BubbleAlign::Left, BubbleAlign::Right] {
                let lines = bubble_block(
                    "praxis-日本語🌵-very-long-label",
                    "日本語🌵 e\u{301} accents supercalifragilisticexpialidocious",
                    align,
                    ACCENT_PURPLE,
                    BG_BUBBLE_BOT,
                    width,
                );
                assert_lines_fit(&lines, width);
            }
        }
    }

    fn dummy_app() -> (App, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let app = App::new(
            crate::remote::Remote::new("http://127.0.0.1:0".into(), "x".into()).unwrap(),
            "http://127.0.0.1:0".into(),
            "x".into(),
            dir.path().to_string_lossy().into_owned(),
        );
        (app, dir)
    }

    fn buffer_contains_cursor(terminal: &Terminal<TestBackend>) -> bool {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() == "█")
    }

    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn input_height_accounts_for_soft_wraps_and_cursor_stays_visible() {
        for (width, height, input) in [
            (1, 1, "abc"),
            (8, 4, "日本語🌵"),
            (24, 10, ""),
            (24, 10, "日本語🌵"),
            (24, 10, "long-input-"),
            (40, 12, "first line\nsecond 日本語 line\nthird line"),
        ] {
            let (mut app, _dir) = dummy_app();
            app.show_sidebar = false;
            app.input = if input == "long-input-" {
                format!("{}", "日本語🌵abcdef".repeat(10))
            } else {
                input.to_string()
            };
            app.attachments
                .push("/synthetic/very-long-attachment-name.txt".into());
            assert!(input_height(&app, width) >= 3);
            for cursor in [0, app.input.chars().count() / 2, app.input.chars().count()] {
                app.cursor = cursor;
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|f| draw(f, &mut app)).unwrap();
                assert!(
                    buffer_contains_cursor(&terminal),
                    "cursor missing for width={width} input={input:?} cursor={cursor}"
                );
            }
        }
    }

    #[test]
    fn streaming_and_completion_popups_never_cover_the_composer() {
        let (mut app, _dir) = dummy_app();
        app.show_sidebar = false;
        for (width, height) in [(24, 7), (40, 8), (80, 14), (100, 30)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for multiline in [false, true] {
                app.input = if multiline { "line\n".repeat(12) } else { "draft".into() };
                app.cursor = app.input.chars().count();
                for popup in [
                    Popup::None,
                    Popup::Slash {matches: (0..SLASH_COMMANDS.len()).collect(), selected: 0},
                    Popup::File {prefix: "".into(), matches: vec!["example.rs".into(); 25], selected: 0},
                ] {
                    app.popup = popup;
                    app.stream_event("stream_start", "{}");
                    for _ in 0..15 {
                        app.stream_event("char", "streamed text 日本語\n");
                        terminal.draw(|f| draw(f, &mut app)).unwrap();
                        assert!(buffer_contains_cursor(&terminal), "composer cursor hidden at {width}x{height}");
                        assert!(screen_rows(&terminal).iter().any(|row| row.contains("Message")), "composer border/title hidden");
                    }
                }
            }
        }
    }

    #[test]
    fn saved_reply_handoff_does_not_remove_its_bubble_for_a_frame() {
        let (mut app, _dir) = dummy_app();
        app.show_sidebar = false;
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let text = "FIRST\n\n    code\nLAST";
        app.stream_event("stream_start", "{}");
        app.stream_event("char", text);
        for (event, data) in [
            ("stream_end", "{}".to_string()),
            ("assistant", text.to_string()),
            ("assistant_saved", serde_json::json!({"id":7,"role":"assistant","content":text}).to_string()),
            ("history", serde_json::json!([{"id":7,"role":"assistant","content":text}]).to_string()),
            ("stream_start", "{}".to_string()),
        ] {
            app.stream_event(event, &data);
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            let rows = screen_rows(&terminal);
            assert_eq!(rows.iter().filter(|row| row.contains("FIRST")).count(), 1, "{event}: {rows:?}");
            assert!(rows.iter().any(|row| row.contains("LAST")), "{event}: {rows:?}");
            assert!(buffer_contains_cursor(&terminal));
        }
    }

    #[test]
    fn selection_is_highlighted_revealed_and_shows_keyboard_help_after_resize() {
        let (mut app, _dir) = dummy_app();
        app.show_sidebar = false;
        app.transcript.push(Bubble::Assistant {content: "ORIGINAL\n  世界 e\u{301} 👩‍💻".into()});
        for _ in 0..30 { app.transcript.push(Bubble::User {content: "unselected\nmessage".into()}); }
        for width in [24, 40, 80, 100] {
            app.copy_selection = Some(crate::app::CopySelection {
                index: 0, content: "SNAPSHOT\n  世界 e\u{301} 👩‍💻".into(), reveal: true,
            });
            let mut terminal = Terminal::new(TestBackend::new(width, 22)).unwrap();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            let rows = screen_rows(&terminal);
            assert!(rows.iter().any(|row| row.contains("SNAPSHOT")), "{rows:?}");
            assert!(!rows.iter().any(|row| row.contains("ORIGINAL")));
            assert!(rows.last().unwrap().contains("COPY 1/31"), "{rows:?}");
            assert!(rows.last().unwrap().contains("^C/y"));
            assert!(terminal.backend().buffer().content().iter().any(|cell|
                cell.symbol() == "S" && cell.modifier.contains(Modifier::REVERSED)));
            assert!(buffer_contains_cursor(&terminal));
            assert!(app.scroll_offset > 0);
            // Live growth keeps the reader on the selected snapshot.
            app.live.text.push_str("new streamed text\n");
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            assert!(screen_rows(&terminal).iter().any(|row| row.contains("SNAPSHOT")));
            app.copy_selection = None;
            app.mouse_capture = false;
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            assert!(screen_rows(&terminal).last().unwrap().contains("F2 mouse:OFF  F3 copy"));
            app.status_msg = Some(("Copy requested (OSC 52)".into(), tokio::time::Instant::now()));
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            assert!(screen_rows(&terminal).last().unwrap().contains("Copy requested"));
            app.status_msg = None;
        }
    }

    #[test]
    fn selection_remains_visible_when_terminal_reflows_while_copying() {
        let (mut app, _dir) = dummy_app();
        app.show_sidebar = false;
        app.transcript.push(Bubble::Assistant {content: "SELECTED".into()});
        for _ in 0..30 { app.transcript.push(Bubble::User {content: "long unselected message ".repeat(4)}); }
        app.copy_selection = Some(crate::app::CopySelection {index: 0, content: "SELECTED".into(), reveal: true});
        let mut terminal = Terminal::new(TestBackend::new(100, 22)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen_rows(&terminal).iter().any(|row| row.contains("SELECTED")));
        terminal.backend_mut().resize(24, 22);
        terminal.resize(Rect::new(0, 0, 24, 22)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen_rows(&terminal).iter().any(|row| row.contains("SELECTED")),
            "reflow must reveal the selected message rather than an unrelated row");
    }

    #[test]
    fn terminal_command_history_is_visible_after_live_preview_and_reload() {
        let (mut app, _dir) = dummy_app();
        app.show_sidebar = false;
        let command = "printf 'FIRST 世界'\n  printf 'LAST'; # <script> & C:\\new";
        let args = serde_json::json!({"command": command, "unrelated": "DO_NOT_DISPLAY"}).to_string();
        let history = serde_json::json!([{"id":31,"role":"assistant","content":"",
            "tool_calls":[{"id":"call-1","function":{"name":"execute_terminal","arguments":args}}]}]).to_string();
        app.stream_event("stream_start", "{}");
        app.stream_event("tool_call_delta", &serde_json::json!({"index":0,"name":"execute_terminal","arguments":"{\"command\":\"partial"}).to_string());
        app.stream_event("stream_end", "{}");
        app.stream_event("tool_call", &serde_json::json!({"tool":"execute_terminal","call_id":"call-1",
            "args_preview":{"args":"{\"command\":\"partial"}}).to_string());
        for width in [40, 80, 100] {
            for reload in [false, true] {
                if reload { app.reset_view(); }
                app.stream_event("history", &history);
                app.stream_event("history", &history);
                let mut terminal = Terminal::new(TestBackend::new(width, 22)).unwrap();
                terminal.draw(|f| draw(f, &mut app)).unwrap();
                let rows = screen_rows(&terminal);
                // TestBackend includes a continuation cell after each wide glyph.
                assert_eq!(rows.iter().filter(|row| row.contains("FIRST")).count(), 1, "{rows:?}");
                assert!(rows.iter().any(|row| row.contains('世') && row.contains('界')), "{rows:?}");
                assert!(rows.iter().any(|row| row.contains("LAST")), "{rows:?}");
                assert!(!rows.iter().any(|row| row.contains("DO_NOT_DISPLAY") || row.contains("partial")));
                assert!(buffer_contains_cursor(&terminal));
                assert_eq!(app.transcript.len(), 1);
            }
        }
    }

    #[test]
    fn draw_regression_narrow_and_wide_terminals() {
        let (mut app, _dir) = dummy_app();
        app.transcript.push(Bubble::Assistant {
            content: "日本語🌵 A very long line_that_must_wrap_without_panicking_or_painting_past_the_terminal_width".into(),
        });
        app.input = "hello from a narrow terminal 日本語🌵".into();
        app.cursor = app.input.chars().count();

        for (w, h) in [(24, 10), (40, 8), (100, 30)] {
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            assert!(buffer_contains_cursor(&terminal));
        }
    }
    #[test]
    fn structured_results_history_renders_decoded_lines_without_mutating_raw() {
        let (mut app, _dir) = dummy_app();
        app.show_sidebar = false;
        let raw = serde_json::json!({"stdout":"FIRST\nSECOND 世界","stderr":"warning\nlast","exit_code":3}).to_string();
        let history = serde_json::json!([{"id":41,"role":"tool","tool_name":"execute_terminal",
            "tool_call_id":"result","content":raw}]).to_string();
        app.stream_event("history", &history);
        app.stream_event("history", &history);
        assert_eq!(app.transcript.len(), 1);
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let rows = screen_rows(&terminal);
        let first = rows.iter().position(|r| r.contains("FIRST")).unwrap();
        let second = rows.iter().position(|r| r.contains("SECOND")).unwrap();
        assert_ne!(first, second, "decoded stdout newlines must be separate rows: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains(r#"\n"#)), "raw JSON escapes leaked into presentation");
        app.copy_selection = Some(crate::app::CopySelection {index:0,content:raw.clone(),reveal:true});
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen_rows(&terminal).iter().any(|r| r.contains(r#"\n"#)), "copy mode must show the exact raw snapshot");
    }

    #[test]
    fn structured_results_commands_and_live_arguments_are_not_decoded() {
        let command = r#"{"stdout":"FIRST\nSECOND"}"#;
        for name in ["execute_terminal — command", "execute_terminal — generating, not executed"] {
            let lines = render_bubble(&Bubble::Tool {name:name.into(),content:command.into()},120,"user");
            assert!(lines.iter().any(|line|line.to_string().contains(command)));
        }
        for width in 1..80 {
            let bubble = Bubble::ToolResult {name:"execute_terminal".into(),content:command.into(),
                display:"stdout:\nFIRST\n世界 👩‍💻\n  LAST".into()};
            assert_lines_fit(&render_bubble(&bubble,width,"user"),width);
        }
    }

}
