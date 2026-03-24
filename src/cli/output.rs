use crate::protocol::{GrepMatch, ProcessEntry, SessionInfo};

pub fn print_session_list(sessions: &[SessionInfo]) {
    if sessions.is_empty() {
        println!("No sessions.");
        return;
    }

    // Calculate column widths
    let id_width = 8;
    let name_width = sessions
        .iter()
        .map(|s| s.name.as_deref().unwrap_or("-").len())
        .max()
        .unwrap_or(4)
        .max(4);
    let shell_width = sessions
        .iter()
        .map(|s| short_shell(&s.shell).len())
        .max()
        .unwrap_or(5)
        .max(5);

    let header = format!(
        "{:<id_w$}  {:<name_w$}  {:<shell_w$}  {:<10}  {:<30}  FG PROCESS",
        "ID",
        "NAME",
        "SHELL",
        "STATE",
        "CWD",
        id_w = id_width,
        name_w = name_width,
        shell_w = shell_width,
    );
    println!("{header}");

    for s in sessions {
        let id_short = if s.id.len() > id_width {
            &s.id[..id_width]
        } else {
            &s.id
        };
        let name = s.name.as_deref().unwrap_or("-");
        let cwd = shorten_path(&s.cwd, 30);
        let fg = s.fg_process.as_deref().unwrap_or("idle");
        let status = &s.state;

        println!(
            "{:<id_w$}  {:<name_w$}  {:<shell_w$}  {:<10}  {:<30}  {}",
            id_short,
            name,
            short_shell(&s.shell),
            status,
            cwd,
            fg,
            id_w = id_width,
            name_w = name_width,
            shell_w = shell_width,
        );
    }
}

pub fn print_session_list_json(sessions: &[SessionInfo]) {
    let wrapper = serde_json::json!({ "sessions": sessions });
    println!("{}", serde_json::to_string_pretty(&wrapper).unwrap());
}

pub fn print_session_info(info: &SessionInfo) {
    println!("ID:           {}", info.id);
    println!("Name:         {}", info.name.as_deref().unwrap_or("(none)"));
    println!("Shell:        {}", info.shell);
    println!("CWD:          {}", info.cwd);
    println!("State:        {}", info.state);
    println!(
        "FG Process:   {}",
        info.fg_process.as_deref().unwrap_or("idle")
    );
    println!("Attached:     {}", info.attached);
    println!("Registered:   {}", info.registered);
    println!("Created:      {}", info.created_at);
}

pub fn print_session_info_json(info: &SessionInfo) {
    println!("{}", serde_json::to_string_pretty(info).unwrap());
}

pub fn print_grep(matches: &[GrepMatch], sessions_only: bool, last: bool) {
    if matches.is_empty() {
        println!("No matches.");
        return;
    }
    for (i, m) in matches.iter().enumerate() {
        if i > 0 && !sessions_only {
            println!();
        }
        let label = m
            .session_name
            .as_deref()
            .unwrap_or(&m.session_id[..8.min(m.session_id.len())]);
        if sessions_only {
            println!("{label}");
        } else if last {
            println!("[{label}]");
            if let Some(line) = m.lines.last() {
                println!("  {line}");
            }
        } else {
            println!("[{label}]");
            for line in &m.lines {
                println!("  {line}");
            }
        }
    }
}

pub fn print_grep_json(matches: &[GrepMatch], sessions_only: bool, last: bool) {
    if sessions_only {
        let ids: Vec<_> = matches
            .iter()
            .map(|m| {
                m.session_name
                    .as_deref()
                    .unwrap_or(&m.session_id)
                    .to_string()
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&ids).unwrap());
    } else if last {
        let trimmed: Vec<_> = matches
            .iter()
            .map(|m| {
                serde_json::json!({
                    "session_id": m.session_id,
                    "session_name": m.session_name,
                    "match": m.lines.last(),
                })
            })
            .collect();
        let wrapper = serde_json::json!({ "matches": trimmed });
        println!("{}", serde_json::to_string_pretty(&wrapper).unwrap());
    } else {
        let wrapper = serde_json::json!({ "matches": matches });
        println!("{}", serde_json::to_string_pretty(&wrapper).unwrap());
    }
}

pub fn print_process_list(entries: &[ProcessEntry]) {
    if entries.is_empty() {
        println!("No foreground processes.");
        return;
    }
    for e in entries {
        println!("{:>8}  {}", e.pid, e.command);
    }
}

fn short_shell(shell: &str) -> &str {
    shell.rsplit('/').next().unwrap_or(shell)
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
