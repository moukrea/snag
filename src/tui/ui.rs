use crate::tui::app::{App, InputMode};
use ansi_to_tui::IntoText;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Percentage(40),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // Session list (filtered by hide_snagged toggle)
    let visible = app.visible_sessions();
    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let name = s.name.as_deref().unwrap_or("-");
            let shell = s.shell.rsplit('/').next().unwrap_or(&s.shell);
            let fg = s.fg_process.as_deref().unwrap_or("idle");
            let cwd = shorten_path(&s.cwd, 30);

            let marker = if i == app.selected { "▸ " } else { "  " };
            let type_marker = if s.registered { " [R]" } else { "" };
            let snagged = s
                .snagged_by
                .as_deref()
                .map(|by| format!(" ← {by}"))
                .unwrap_or_default();

            let style = if i == app.selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if s.snagged_by.is_some() {
                Style::default().fg(Color::Magenta)
            } else if s.state != "running" {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };

            let id_short = &s.id[..8.min(s.id.len())];
            ListItem::new(Line::from(vec![Span::styled(
                format!("{marker}{id_short}  {name:<10} {shell:<6} {cwd:<25} {fg}{type_marker}{snagged}"),
                style,
            )]))
        })
        .collect();

    if items.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No sessions.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  [n] to spawn a new session",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  or add eval \"$(snag hook bash)\" to your .bashrc",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Snag — Sessions "),
        );
        frame.render_widget(empty, chunks[0]);
    } else {
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Snag — Sessions "),
        );
        frame.render_widget(list, chunks[0]);
    }

    // Preview pane
    let preview_title = app
        .selected_session()
        .map(|s| {
            format!(
                " Preview ({}) ",
                s.name.as_deref().unwrap_or(&s.id[..8.min(s.id.len())])
            )
        })
        .unwrap_or_else(|| " Preview ".to_string());

    // Pre-process: keep only SGR (color) sequences, strip everything else
    let cleaned = sanitize_for_preview(&app.preview_raw);
    let preview_text = cleaned
        .as_bytes()
        .into_text()
        .unwrap_or_else(|_| Text::raw(&cleaned));

    // Only show lines that fit in the preview area (take the last N lines)
    let available_height = chunks[1].height.saturating_sub(2) as usize; // minus borders
    let total_lines = preview_text.lines.len();
    let skip = total_lines.saturating_sub(available_height);
    let visible_lines: Vec<Line> = preview_text.lines.into_iter().skip(skip).collect();

    let preview = Paragraph::new(visible_lines)
        .block(Block::default().borders(Borders::ALL).title(preview_title));
    frame.render_widget(preview, chunks[1]);

    // Status bar
    let status_text = match app.input_mode {
        InputMode::Normal => {
            let hide_label = if app.hide_snagged {
                "[h]show snagged"
            } else {
                "[h]hide snagged"
            };
            format!(" [n]ew [x]kill [r]ename [s]end [Enter]attach {hide_label} [q]uit")
        }
        InputMode::Rename => {
            format!(" Rename: {}  [Enter]confirm [Esc]cancel", app.input_buffer)
        }
        InputMode::Send => {
            format!(
                " Send command: {}  [Enter]confirm [Esc]cancel",
                app.input_buffer
            )
        }
    };

    let status_style = match app.input_mode {
        InputMode::Normal => Style::default().fg(Color::DarkGray),
        _ => Style::default().fg(Color::Yellow),
    };

    let status = Paragraph::new(Line::from(vec![Span::styled(status_text, status_style)]));
    frame.render_widget(status, chunks[2]);
}

/// Sanitize raw PTY output for the preview pane.
/// Keeps SGR sequences (colors: `\x1b[...m`) and strips everything else:
/// cursor movement, screen clearing, bracketed paste, OSC title, private modes, etc.
fn sanitize_for_preview(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == 0x1b && i + 1 < len {
            if bytes[i + 1] == b'[' {
                // CSI sequence: \x1b[ ... <final byte>
                let start = i;
                i += 2; // skip \x1b[
                        // Skip parameter bytes and intermediate bytes
                while i < len
                    && (bytes[i] == b'?'
                        || bytes[i] == b'>'
                        || bytes[i] == b'='
                        || bytes[i] == b';'
                        || bytes[i].is_ascii_digit())
                {
                    i += 1;
                }
                // Final byte
                if i < len {
                    let final_byte = bytes[i];
                    i += 1;
                    if final_byte == b'm' {
                        // SGR — keep it (colors/styles)
                        out.push_str(&raw[start..i]);
                    }
                    // Everything else (H, J, K, A, B, C, D, h, l, etc.) — discard
                }
            } else if bytes[i + 1] == b']' {
                // OSC sequence: \x1b] ... (BEL or ST)
                i += 2;
                while i < len {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && i + 1 < len && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            } else if bytes[i + 1] == b'(' || bytes[i + 1] == b')' {
                // Character set designation — skip 3 bytes
                i += 3;
            } else {
                // Other escape — skip 2 bytes
                i += 2;
            }
        } else if bytes[i] < 0x20 && bytes[i] != b'\n' && bytes[i] != b'\t' && bytes[i] != b'\r' {
            // Strip control characters (except newline, tab, carriage return)
            i += 1;
        } else {
            // Regular character or \n/\t/\r — keep
            out.push(bytes[i] as char);
            i += 1;
        }
    }

    // Clean up: remove \r (CR) since ratatui handles newlines
    out = out.replace('\r', "");
    // Remove empty lines caused by cursor movement artifacts
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out
}

fn shorten_path(path: &str, max_len: usize) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = path.strip_prefix(&home) {
            let shortened = format!("~{rest}");
            if shortened.len() <= max_len {
                return shortened;
            }
        }
    }
    if path.len() <= max_len {
        path.to_string()
    } else {
        format!("...{}", &path[path.len() - max_len + 3..])
    }
}
