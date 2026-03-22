use crate::tui::app::{App, InputMode};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
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

    // Session list
    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let name = s.name.as_deref().unwrap_or("-");
            let shell = s.shell.rsplit('/').next().unwrap_or(&s.shell);
            let fg = s.fg_process.as_deref().unwrap_or("idle");
            let cwd = shorten_path(&s.cwd, 30);

            let marker = if i == app.selected { "▸ " } else { "  " };
            let adopted_marker = if s.adopted { " [A]" } else { "" };

            let style = if i == app.selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if s.state != "running" {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(vec![Span::styled(
                format!("{marker}{name:<12} {shell:<6} {cwd:<30} {fg}{adopted_marker}"),
                style,
            )]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Snag — Sessions "),
    );
    frame.render_widget(list, chunks[0]);

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

    let preview_text: Vec<Line> = app
        .preview_lines
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect();

    let preview = Paragraph::new(preview_text)
        .block(Block::default().borders(Borders::ALL).title(preview_title));
    frame.render_widget(preview, chunks[1]);

    // Status bar
    let status_text = match app.input_mode {
        InputMode::Normal => {
            let show_adopted_indicator = if app.show_adopted {
                "[a]dopted:ON"
            } else {
                "[a]dopted:OFF"
            };
            format!(" [n]ew [x]kill [r]ename [s]end [Enter]attach {show_adopted_indicator} [q]uit")
        }
        InputMode::Rename => {
            format!(" Rename: {}█  [Enter]confirm [Esc]cancel", app.input_buffer)
        }
        InputMode::Send => {
            format!(
                " Send command: {}█  [Enter]confirm [Esc]cancel",
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
