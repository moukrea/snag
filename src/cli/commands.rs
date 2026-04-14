use crate::cli::output;
use crate::client::DaemonClient;
use crate::config::Config;
use crate::error::Result;
use crate::protocol::*;
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};

static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);
static SNAGGED: AtomicBool = AtomicBool::new(false);
static UNSNAGGED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigwinch(_: nix::libc::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::Relaxed);
}
extern "C" fn handle_sigusr1(_: nix::libc::c_int) {
    SNAGGED.store(true, Ordering::Relaxed);
}
extern "C" fn handle_sigusr2(_: nix::libc::c_int) {
    UNSNAGGED.store(true, Ordering::Relaxed);
}

pub fn cmd_wrap(capture: &str) -> Result<()> {
    use nix::libc;
    use nix::pty::openpty;
    use nix::unistd::{close, dup2, execvp, fork, setsid, ForkResult};
    use std::ffi::CString;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    // Create inner PTY pair
    let pty = openpty(None, None)?;

    // Copy outer terminal size to inner PTY
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(pty.master.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }
    }

    match unsafe { fork()? } {
        ForkResult::Child => {
            // Child: exec shell on inner PTY
            drop(pty.master);
            let slave_raw = pty.slave.as_raw_fd();
            setsid().ok();
            unsafe {
                libc::ioctl(slave_raw, libc::TIOCSCTTY, 0);
            }
            let _ = dup2(slave_raw, libc::STDIN_FILENO);
            let _ = dup2(slave_raw, libc::STDOUT_FILENO);
            let _ = dup2(slave_raw, libc::STDERR_FILENO);
            if slave_raw > libc::STDERR_FILENO {
                let _ = close(slave_raw);
            }
            let shell_cstr = match CString::new(shell.as_str()) {
                Ok(c) => c,
                Err(_) => {
                    eprintln!("error: SHELL contains invalid characters");
                    unsafe { libc::_exit(1) };
                }
            };
            let _ = execvp(&shell_cstr, std::slice::from_ref(&shell_cstr));
            unsafe {
                libc::_exit(127);
            }
        }
        ForkResult::Parent { child } => {
            drop(pty.slave);

            // Open capture file for writing
            let mut capture_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(capture)?;

            // Open /dev/tty once for output (bypasses any existing redirect)
            let mut tty_out = std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/tty")
                .unwrap_or_else(|_| unsafe { std::fs::File::from_raw_fd(libc::STDOUT_FILENO) });

            // Set initial terminal title to the shell name (not "snag")
            let shell_name = shell.rsplit('/').next().unwrap_or(&shell);
            let title_seq = format!("\x1b]0;{shell_name}\x07");
            let _ = std::io::Write::write_all(&mut tty_out, title_seq.as_bytes());
            let _ = std::io::Write::flush(&mut tty_out);

            // Put outer terminal in raw mode — but use minimal raw mode
            // (like script does) to preserve mouse tracking and scroll behavior.
            // crossterm's enable_raw_mode() is too aggressive for a PTY proxy.
            let saved_termios = nix::sys::termios::tcgetattr(std::io::stdin())?;
            let mut raw = saved_termios.clone();
            // Disable echo and canonical mode (input delivered char-by-char)
            raw.local_flags.remove(
                nix::sys::termios::LocalFlags::ECHO
                    | nix::sys::termios::LocalFlags::ICANON
                    | nix::sys::termios::LocalFlags::ISIG
                    | nix::sys::termios::LocalFlags::IEXTEN,
            );
            // Disable input processing that would mangle data
            raw.input_flags
                .remove(nix::sys::termios::InputFlags::ICRNL | nix::sys::termios::InputFlags::IXON);
            // Keep OPOST enabled — output processing must stay for proper
            // escape sequence handling by the terminal emulator
            raw.control_chars[nix::sys::termios::SpecialCharacterIndices::VMIN as usize] = 1;
            raw.control_chars[nix::sys::termios::SpecialCharacterIndices::VTIME as usize] = 0;
            nix::sys::termios::tcsetattr(
                std::io::stdin(),
                nix::sys::termios::SetArg::TCSANOW,
                &raw,
            )?;

            // Install signal handlers
            unsafe {
                libc::signal(
                    libc::SIGWINCH,
                    handle_sigwinch as *const () as libc::sighandler_t,
                );
                libc::signal(
                    libc::SIGUSR1,
                    handle_sigusr1 as *const () as libc::sighandler_t,
                );
                libc::signal(
                    libc::SIGUSR2,
                    handle_sigusr2 as *const () as libc::sighandler_t,
                );
            }

            let mut is_snagged = false;
            let mut tty_bytes_written: u64 = 0; // track how much was sent to tty

            // Set inner PTY master to non-blocking
            let master_fd = pty.master.as_raw_fd();
            unsafe {
                let flags = libc::fcntl(master_fd, libc::F_GETFL);
                libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }

            // Set stdin to non-blocking
            unsafe {
                let flags = libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL);
                libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }

            // Relay loop using poll
            let mut buf = [0u8; 4096];
            let mut pollfds = [
                libc::pollfd {
                    fd: libc::STDIN_FILENO,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: master_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            loop {
                pollfds[0].revents = 0;
                pollfds[1].revents = 0;

                let ret = unsafe { libc::poll(pollfds.as_mut_ptr(), 2, 100) };
                if ret < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        // EINTR from signal — check SIGWINCH and continue
                    } else {
                        break;
                    }
                }

                // Handle SIGWINCH: propagate terminal resize to inner PTY
                if SIGWINCH_RECEIVED.swap(false, Ordering::Relaxed) {
                    if let Ok((cols, rows)) = crossterm::terminal::size() {
                        let ws = libc::winsize {
                            ws_row: rows,
                            ws_col: cols,
                            ws_xpixel: 0,
                            ws_ypixel: 0,
                        };
                        unsafe {
                            libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
                        }
                    }
                }

                // Handle snag/unsnag signals
                if SNAGGED.swap(false, Ordering::Relaxed) && !is_snagged {
                    is_snagged = true;
                    // Switch to alternate screen (preserves original screen)
                    // and show snagged message
                    // Switch to alternate screen, clear it fully, then show message
                    let msg = "\x1b[?1049h\x1b[2J\x1b[H\x1b[1;33m\
                        === Session snagged by a remote client ===\r\n\r\n\
                        \x1b[0mThis session is being controlled remotely.\r\n\
                        It will resume when the remote client detaches.\r\n";
                    let _ = std::io::Write::write_all(&mut tty_out, msg.as_bytes());
                    let _ = std::io::Write::flush(&mut tty_out);
                }
                if UNSNAGGED.swap(false, Ordering::Relaxed) && is_snagged {
                    is_snagged = false;
                    // Restore original screen, then append missed output
                    let _ = std::io::Write::write_all(&mut tty_out, b"\x1b[?1049l");
                    if let Ok(mut replay) = std::fs::File::open(capture) {
                        use std::io::{Read, Seek, SeekFrom};
                        let _ = replay.seek(SeekFrom::Start(tty_bytes_written));
                        let mut replay_buf = Vec::new();
                        let _ = replay.read_to_end(&mut replay_buf);
                        let _ = std::io::Write::write_all(&mut tty_out, &replay_buf);
                        tty_bytes_written += replay_buf.len() as u64;
                    }
                    let _ = std::io::Write::flush(&mut tty_out);
                    // Re-apply current terminal size to the inner PTY so TUIs
                    // resize back to the original dimensions after detach
                    if let Ok((cols, rows)) = crossterm::terminal::size() {
                        let ws = libc::winsize {
                            ws_row: rows,
                            ws_col: cols,
                            ws_xpixel: 0,
                            ws_ypixel: 0,
                        };
                        unsafe {
                            libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
                        }
                    }
                }

                // stdin -> inner master (ALWAYS relay, even when snagged).
                // The daemon sends remote input through the original master fd,
                // which arrives here as stdin. Must relay to inner shell.
                if pollfds[0].revents & libc::POLLIN != 0 {
                    let n = unsafe {
                        libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len())
                    };
                    if n <= 0 {
                        break;
                    }
                    let _ = nix::unistd::write(&pty.master, &buf[..n as usize]);
                }

                // inner master -> capture file (always) + stdout (only when not snagged)
                if pollfds[1].revents & libc::POLLIN != 0 {
                    let n = unsafe { libc::read(master_fd, buf.as_mut_ptr().cast(), buf.len()) };
                    if n <= 0 {
                        break;
                    }
                    let data = &buf[..n as usize];
                    if !is_snagged {
                        let _ = std::io::Write::write_all(&mut tty_out, data);
                        let _ = std::io::Write::flush(&mut tty_out);
                        tty_bytes_written += data.len() as u64;
                    }
                    let _ = std::io::Write::write_all(&mut capture_file, data);
                    let _ = std::io::Write::flush(&mut capture_file);
                }

                // Check for HUP/ERR on inner master (shell exited)
                if pollfds[1].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                    // Drain remaining data
                    loop {
                        let n =
                            unsafe { libc::read(master_fd, buf.as_mut_ptr().cast(), buf.len()) };
                        if n <= 0 {
                            break;
                        }
                        let data = &buf[..n as usize];
                        let _ = std::io::Write::write_all(&mut tty_out, data);
                        let _ = std::io::Write::write_all(&mut capture_file, data);
                    }
                    let _ = std::io::Write::flush(&mut tty_out);
                    let _ = std::io::Write::flush(&mut capture_file);
                    break;
                }
            }

            // Restore terminal
            let _ = nix::sys::termios::tcsetattr(
                std::io::stdin(),
                nix::sys::termios::SetArg::TCSANOW,
                &saved_termios,
            );

            // Wait for child and exit with its status
            match nix::sys::wait::waitpid(child, None) {
                Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => {
                    std::process::exit(code);
                }
                Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => {
                    std::process::exit(128 + sig as i32);
                }
                _ => std::process::exit(0),
            }
        }
    }
}

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

pub async fn cmd_attach(
    config: &Config,
    target: String,
    read_only: bool,
    force: bool,
) -> Result<()> {
    use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
    use crossterm::terminal;
    use futures_lite::StreamExt;

    let mut client = DaemonClient::connect(config).await?;

    // Send attach request
    let resp = client
        .request(&Request::SessionAttach {
            target: target.clone(),
            read_only,
            force,
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
                            let msg = if let Ok(resp) = decode_response(msg_type, &payload) {
                                match resp {
                                    Response::SessionEvent { event, .. } if event == "stolen" => {
                                        "\r\n\x1b[33m[Session stolen by another client]\x1b[0m\r\n"
                                    }
                                    _ => "\r\n\x1b[33m[Session killed by snag]\x1b[0m\r\n",
                                }
                            } else {
                                "\r\n\x1b[33m[Session ended]\x1b[0m\r\n"
                            };
                            let _ = std::io::Write::write_all(&mut tty_out, msg.as_bytes());
                            let _ = std::io::Write::flush(&mut tty_out);
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

pub async fn cmd_grep(
    config: &Config,
    pattern: String,
    sessions_only: bool,
    last: bool,
    json: bool,
) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;
    let resp = client.request(&Request::SessionGrep { pattern }).await?;
    match resp {
        Response::Ok(ResponseData::GrepResult(matches)) => {
            if json {
                output::print_grep_json(&matches, sessions_only, last);
            } else {
                output::print_grep(&matches, sessions_only, last);
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
  [ -n "$SNAG_SESSION" ] && return
  # Auto-start daemon if not running
  snag daemon status &>/dev/null || snag daemon start &>/dev/null &
  sleep 0.2
  local _snag_result
  _snag_result="$(snag register --pid $$ 2>/dev/null)"
  if [ $? -eq 0 ] && [ -n "$_snag_result" ]; then
    eval "$_snag_result"
    trap 'snag unregister $SNAG_SESSION 2>/dev/null' EXIT
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
  [[ -n "$SNAG_SESSION" ]] && return
  # Auto-start daemon if not running
  snag daemon status &>/dev/null || snag daemon start &>/dev/null &
  sleep 0.2
  local _snag_result
  _snag_result="$(snag register --pid $$ 2>/dev/null)"
  if [[ $? -eq 0 ]] && [[ -n "$_snag_result" ]]; then
    eval "$_snag_result"
    trap 'snag unregister $SNAG_SESSION 2>/dev/null' EXIT
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
        Response::Ok(ResponseData::SessionRegistered { id, .. }) => {
            // Only export the session ID — no exec, no redirect, no child
            // processes. The shell continues completely unmodified so titles,
            // CWD, colors, isatty(), and TUI apps all work normally.
            // Output capture is not available for hooked sessions (snag output
            // won't have scrollback), but ls/cwd/ps/attach/send all work via
            // /proc and the stolen master fd.
            println!("export SNAG_SESSION={id}");
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
