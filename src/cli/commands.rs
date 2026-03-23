use crate::cli::output;
use crate::client::DaemonClient;
use crate::config::Config;
use crate::error::Result;
use crate::protocol::*;
use std::os::fd::{AsRawFd, FromRawFd};

pub async fn cmd_new(
    config: &Config,
    shell: Option<String>,
    name: Option<String>,
    cwd: Option<String>,
) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client
        .request(&Request::SessionNew { shell, name, cwd })
        .await?;
    match resp {
        Response::Ok(ResponseData::SessionCreated { id }) => {
            println!("{id}");
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_kill(config: &Config, target: String) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::SessionKill { target }).await?;
    match resp {
        Response::Ok(_) => Ok(()),
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_rename(config: &Config, target: String, new_name: String) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client
        .request(&Request::SessionRename { target, new_name })
        .await?;
    match resp {
        Response::Ok(_) => Ok(()),
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_list(config: &Config, json: bool) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::SessionList).await?;
    match resp {
        Response::Ok(ResponseData::SessionList(sessions)) => {
            if json {
                output::print_session_list_json(&sessions);
            } else {
                output::print_session_list(&sessions);
            }
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_info(config: &Config, target: String, json: bool) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::SessionInfo { target }).await?;
    match resp {
        Response::Ok(ResponseData::SessionInfo(info)) => {
            if json {
                output::print_session_info_json(&info);
            } else {
                output::print_session_info(&info);
            }
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_attach(config: &Config, target: String, read_only: bool) -> Result<()> {
    use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
    use crossterm::terminal;
    use futures_lite::StreamExt;

    let mut client = DaemonClient::connect(config).await?;

    // Send attach request
    let resp = client
        .request(&Request::SessionAttach {
            target: target.clone(),
            read_only,
        })
        .await?;

    // Print initial scrollback directly to /dev/tty (bypasses tee redirect)
    let mut tty_init = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .unwrap_or_else(|_| unsafe { std::fs::File::from_raw_fd(std::io::stdout().as_raw_fd()) });
    match resp {
        Response::Ok(ResponseData::Output(scrollback)) => {
            let _ = std::io::Write::write_all(&mut tty_init, scrollback.as_bytes());
            let _ = std::io::Write::flush(&mut tty_init);
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {}
    }
    drop(tty_init);

    // Enter raw mode
    terminal::enable_raw_mode()?;

    // Send initial terminal size
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let _ = client.send_resize(cols, rows).await;

    let stream = client.into_stream();
    let (reader, writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut writer = writer;

    // Open /dev/tty directly for output — bypasses any stdout redirect
    let mut tty_out = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .unwrap_or_else(|_| unsafe { std::fs::File::from_raw_fd(std::io::stdout().as_raw_fd()) });

    // Track detach sequence
    let detach_timeout = std::time::Duration::from_millis(config.detach_timeout_ms);
    let mut last_escape: Option<std::time::Instant> = None;

    // Use async EventStream instead of blocking event::poll
    let mut events = EventStream::new();

    let result = loop {
        tokio::select! {
            // Read frames from the daemon (PTY output)
            frame = read_frame(&mut reader) => {
                match frame {
                    Ok(Some((msg_type, payload))) => {
                        if msg_type == MSG_PTY_OUTPUT {
                            let _ = std::io::Write::write_all(&mut tty_out, &payload);
                            let _ = std::io::Write::flush(&mut tty_out);
                        } else if msg_type == MSG_SESSION_EVENT {
                            break Ok(());
                        }
                        // MSG_OK/MSG_ERROR from control messages — ignore
                    }
                    Ok(None) => break Ok(()),
                    Err(e) => break Err(e),
                }
            }
            // Read keyboard events
            event = events.next() => {
                let Some(event) = event else { break Ok(()) };
                let Ok(event) = event else { continue };
                match event {
                    Event::Key(key_event) => {
                        let is_detach_key = (key_event.code == KeyCode::Char('\\')
                            && key_event.modifiers.contains(KeyModifiers::CONTROL))
                            || (key_event.code == KeyCode::Char('q')
                                && key_event.modifiers.contains(KeyModifiers::CONTROL));
                        if is_detach_key {
                            if let Some(last) = last_escape {
                                if last.elapsed() < detach_timeout {
                                    break Ok(());
                                }
                            }
                            last_escape = Some(std::time::Instant::now());
                            continue;
                        }
                        last_escape = None;
                        if read_only { continue; }
                        let bytes = key_event_to_bytes(&key_event);
                        if !bytes.is_empty() {
                            let req = Request::PtyInput(bytes);
                            let frame = match encode_request(&req) {
                                Ok(f) => f,
                                Err(e) => break Err(e),
                            };
                            if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut writer, &frame).await {
                                break Err(e.into());
                            }
                        }
                    }
                    Event::Resize(cols, rows) => {
                        let req = Request::Resize { cols, rows };
                        let frame = match encode_request(&req) {
                            Ok(f) => f,
                            Err(e) => break Err(e),
                        };
                        let _ = tokio::io::AsyncWriteExt::write_all(&mut writer, &frame).await;
                    }
                    _ => {}
                }
            }
        }
    };

    // Send detach
    let detach_req = Request::SessionDetach;
    if let Ok(frame) = encode_request(&detach_req) {
        let _ = tokio::io::AsyncWriteExt::write_all(&mut writer, &frame).await;
    }

    // Restore terminal
    terminal::disable_raw_mode()?;
    println!();

    result
}

fn key_event_to_bytes(event: &crossterm::event::KeyEvent) -> Vec<u8> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);

    match event.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl+A=1, Ctrl+B=2, ..., Ctrl+Z=26
                let byte = (c.to_ascii_lowercase() as u8)
                    .wrapping_sub(b'a')
                    .wrapping_add(1);
                if byte <= 26 {
                    vec![byte]
                } else {
                    vec![]
                }
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },
        _ => vec![],
    }
}

pub async fn cmd_send(config: &Config, target: String, command: String) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client
        .request(&Request::SessionSend {
            target,
            input: command,
        })
        .await?;
    match resp {
        Response::Ok(_) => Ok(()),
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_output(
    config: &Config,
    target: String,
    lines: Option<u32>,
    follow: bool,
    json: bool,
) -> Result<()> {
    use std::io::Write;

    let mut client = DaemonClient::connect(config).await?;
    let resp = client
        .request(&Request::SessionOutput {
            target: target.clone(),
            lines,
            follow,
        })
        .await?;

    match resp {
        Response::Ok(ResponseData::Output(text)) => {
            if json {
                let wrapper = serde_json::json!({
                    "session": target,
                    "output": text,
                });
                println!("{}", serde_json::to_string_pretty(&wrapper).unwrap());
            } else {
                print!("{text}");
                std::io::stdout().flush()?;
            }

            if follow {
                // Stream additional output
                let mut stream = client.into_stream();
                loop {
                    match read_frame(&mut stream).await {
                        Ok(Some((msg_type, payload))) => {
                            if msg_type == MSG_PTY_OUTPUT {
                                let mut stdout = std::io::stdout();
                                let _ = stdout.write_all(&payload);
                                let _ = stdout.flush();
                            } else if msg_type == MSG_SESSION_EVENT {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
            }

            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_cwd(config: &Config, target: String) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::SessionCwd { target }).await?;
    match resp {
        Response::Ok(ResponseData::Cwd(cwd)) => {
            println!("{cwd}");
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_ps(config: &Config, target: String) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::SessionPs { target }).await?;
    match resp {
        Response::Ok(ResponseData::ProcessInfo(entries)) => {
            output::print_process_list(&entries);
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_grep(config: &Config, pattern: String, json: bool) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::SessionGrep { pattern }).await?;
    match resp {
        Response::Ok(ResponseData::GrepResult(matches)) => {
            if json {
                output::print_grep_json(&matches);
            } else {
                output::print_grep(&matches);
            }
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub fn cmd_hook(shell: &str) -> Result<()> {
    match shell {
        "bash" => {
            print!(
                r#"_snag_hook() {{
  # Skip if already registered
  [ -n "$SNAG_SESSION" ] && return

  # Auto-start daemon if needed (snag register handles this)
  local _snag_result
  _snag_result="$(snag register --pid $$ 2>/dev/null)"
  if [ $? -eq 0 ] && [ -n "$_snag_result" ]; then
    eval "$_snag_result"
  fi
}}

_snag_hook
"#
            );
            Ok(())
        }
        "zsh" => {
            print!(
                r#"_snag_hook() {{
  # Skip if already registered
  [[ -n "$SNAG_SESSION" ]] && return

  # Auto-start daemon if needed (snag register handles this)
  local _snag_result
  _snag_result="$(snag register --pid $$ 2>/dev/null)"
  if [[ $? -eq 0 ]] && [[ -n "$_snag_result" ]]; then
    eval "$_snag_result"
  fi
}}

_snag_hook
"#
            );
            Ok(())
        }
        _ => {
            eprintln!("error: unsupported shell '{shell}' (supported: bash, zsh)");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_register(config: &Config, pid: Option<u32>, name: Option<String>) -> Result<()> {
    // Determine the current PTS
    let pts = get_current_pts();
    let Some(pts) = pts else {
        eprintln!("error: not running in a terminal");
        std::process::exit(1);
    };

    // Use --pid from the hook ($$), or fall back to our parent PID
    let shell_pid = pid.unwrap_or_else(|| nix::unistd::getppid().as_raw() as u32);

    let mut client = DaemonClient::connect(config).await?;
    let resp = client
        .request(&Request::SessionRegister {
            pts,
            shell_pid,
            name,
        })
        .await?;
    match resp {
        Response::Ok(ResponseData::SessionRegistered { id, capture_path }) => {
            // Print shell commands for the hook to eval
            println!("export SNAG_SESSION={id}");
            println!("export SNAG_CAPTURE={capture_path}");
            println!(
                "exec > >(tee -a '{}') 2>&1",
                capture_path.replace('\'', "'\\''")
            );
            println!("trap 'snag unregister {id} 2>/dev/null' EXIT");
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}

pub async fn cmd_unregister(config: &Config, target: String) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client
        .request(&Request::SessionUnregister { target })
        .await?;
    match resp {
        Response::Ok(_) => Ok(()),
        Response::Error { message, .. } => {
            // Silently ignore errors during EXIT trap
            eprintln!("error: {message}");
            Ok(())
        }
        _ => Ok(()),
    }
}

fn get_current_pts() -> Option<String> {
    // Read /proc/self/fd/0 symlink to get the TTY
    std::fs::read_link("/proc/self/fd/0").ok().and_then(|p| {
        let s = p.to_string_lossy().to_string();
        if s.starts_with("/dev/pts/") {
            Some(s)
        } else {
            None
        }
    })
}

pub async fn cmd_daemon_start(config: &Config) -> Result<()> {
    // Check if already running
    let socket_path = config.socket_path();
    if tokio::net::UnixStream::connect(&socket_path).await.is_ok() {
        eprintln!("daemon is already running");
        return Ok(());
    }

    // The daemon runs in this process (foreground)
    crate::daemon::server::run_daemon(config.clone()).await
}

pub async fn cmd_daemon_stop(config: &Config) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::DaemonStop).await?;
    match resp {
        Response::Ok(_) => {
            println!("daemon stopped");
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => Ok(()),
    }
}

pub async fn cmd_daemon_status(config: &Config) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::DaemonStatus).await?;
    match resp {
        Response::Ok(ResponseData::DaemonStatus {
            pid,
            uptime_secs,
            session_count,
        }) => {
            println!("PID:       {pid}");
            println!("Uptime:    {uptime_secs}s");
            println!("Sessions:  {session_count}");
            Ok(())
        }
        Response::Error { message, .. } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unexpected response");
            std::process::exit(1);
        }
    }
}
