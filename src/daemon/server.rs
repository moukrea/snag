use crate::config::Config;
use crate::daemon::adopt;
use crate::daemon::pty;
use crate::daemon::registry::SessionRegistry;
use crate::daemon::session::{
    generate_session_id, validate_session_name, ClientId, Session, SessionState,
};
use crate::error::Result;
use crate::protocol::*;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

enum DaemonEvent {
    NewConnection(UnixStream),
    ClientMessage(ClientId, Request),
    ClientDisconnected(ClientId),
    PtyData(String, Vec<u8>), // session_id, data
    PtyEof(String),           // session_id
    ChildExited(String, i32), // session_id, exit_code
    Shutdown,
}

struct AttachedClient {
    tx: mpsc::Sender<Vec<u8>>,
    read_only: bool,
    session_id: Option<String>,
}

pub async fn run_daemon(config: Config) -> Result<()> {
    let socket_path = config.socket_path();

    // Create socket directory
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        // Set directory permissions to 0700
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(parent, perms)?;
        }
    }

    // Remove stale socket
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;

    // Write PID file
    let pid_path = config.pid_path();
    std::fs::write(&pid_path, std::process::id().to_string())?;

    eprintln!("snagd: listening on {}", socket_path.display());

    let mut registry = SessionRegistry::new();
    let mut clients: HashMap<ClientId, AttachedClient> = HashMap::new();
    let (event_tx, mut event_rx) = mpsc::channel::<DaemonEvent>(256);
    let start_time = Instant::now();
    let grace_period = Duration::from_secs(config.daemon_grace_period);
    let mut grace_deadline: Option<Instant> = None;
    let scrollback_bytes = config.scrollback_bytes;

    // Accept connections in a separate task
    let accept_tx = event_tx.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let _ = accept_tx.send(DaemonEvent::NewConnection(stream)).await;
                }
                Err(e) => {
                    eprintln!("snagd: accept error: {e}");
                }
            }
        }
    });

    // Set up SIGTERM handler
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigchld = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child())?;

    loop {
        let grace_sleep = match grace_deadline {
            Some(deadline) => tokio::time::sleep_until(deadline),
            None => tokio::time::sleep(Duration::from_secs(86400 * 365)),
        };
        let has_grace = grace_deadline.is_some();
        tokio::pin!(grace_sleep);

        tokio::select! {
            event = event_rx.recv() => {
                let Some(event) = event else { break; };
                match event {
                    DaemonEvent::NewConnection(stream) => {
                        let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
                        let (tx, rx) = mpsc::channel(64);
                        clients.insert(client_id, AttachedClient {
                            tx,
                            read_only: false,
                            session_id: None,
                        });
                        let event_tx = event_tx.clone();
                        tokio::spawn(handle_client_connection(client_id, stream, rx, event_tx));
                    }
                    DaemonEvent::ClientMessage(client_id, request) => {
                        let response = handle_request(
                            &mut registry,
                            &mut clients,
                            client_id,
                            request,
                            &config,
                            start_time,
                            &event_tx,
                            scrollback_bytes,
                        ).await;
                        if let Some(client) = clients.get(&client_id) {
                            let encoded = encode_response(&response).unwrap_or_default();
                            let _ = client.tx.send(encoded).await;
                        }
                        // Reset grace timer if we have sessions
                        if !registry.is_empty() {
                            grace_deadline = None;
                        }
                    }
                    DaemonEvent::ClientDisconnected(client_id) => {
                        // Remove client from any attached sessions
                        if let Some(client) = clients.remove(&client_id) {
                            if let Some(ref session_id) = client.session_id {
                                if let Some(session) = registry.get_mut(session_id) {
                                    session.attached_clients.retain(|&id| id != client_id);
                                }
                            }
                        }
                    }
                    DaemonEvent::PtyData(session_id, data) => {
                        if let Some(session) = registry.get_mut(&session_id) {
                            session.scrollback.write(&data);
                            // Fan out to attached clients
                            let attached: Vec<ClientId> = session.attached_clients.clone();
                            let output_frame = encode_response(&Response::PtyOutput(data.clone())).unwrap_or_default();
                            for cid in attached {
                                if let Some(client) = clients.get(&cid) {
                                    // Non-blocking send; drop data for slow clients
                                    let _ = client.tx.try_send(output_frame.clone());
                                }
                            }
                        }
                    }
                    DaemonEvent::PtyEof(ref session_id) | DaemonEvent::ChildExited(ref session_id, _) => {
                        let exit_code = match &event {
                            DaemonEvent::ChildExited(_, code) => *code,
                            _ => 0,
                        };
                        let session_id = session_id.clone();
                        if let Some(session) = registry.get_mut(&session_id) {
                            session.state = SessionState::Exited(exit_code);
                            // Notify attached clients
                            let event_msg = Response::SessionEvent {
                                event: "exited".to_string(),
                                session_id: session_id.clone(),
                            };
                            let encoded = encode_response(&event_msg).unwrap_or_default();
                            for cid in &session.attached_clients {
                                if let Some(client) = clients.get(cid) {
                                    let _ = client.tx.try_send(encoded.clone());
                                }
                            }
                        }
                        // Start grace timer if no sessions remain
                        if registry.iter().all(|s| matches!(s.state, SessionState::Exited(_))) {
                            grace_deadline = Some(Instant::now() + grace_period);
                        }
                    }
                    DaemonEvent::Shutdown => {
                        break;
                    }
                }
            }
            _ = sigterm.recv() => {
                eprintln!("snagd: received SIGTERM, shutting down");
                for session in registry.iter_mut() {
                    if session.registered {
                        teardown_output_capture(session);
                    } else if let Some(pid) = session.child_pid {
                        pty::kill_session(pid);
                    }
                }
                break;
            }
            _ = sigchld.recv() => {
                // Reap children
                let ids = registry.session_ids();
                for id in ids {
                    if let Some(session) = registry.get_mut(&id) {
                        if let Some(pid) = session.child_pid {
                            if let Some(code) = pty::reap_child(pid) {
                                session.state = SessionState::Exited(code);
                                let _ = event_tx.send(DaemonEvent::ChildExited(id.clone(), code)).await;
                            }
                        }
                    }
                }
            }
            _ = &mut grace_sleep, if has_grace => {
                eprintln!("snagd: grace period expired, shutting down");
                break;
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&pid_path);
    Ok(())
}

async fn handle_client_connection(
    client_id: ClientId,
    stream: UnixStream,
    mut rx: mpsc::Receiver<Vec<u8>>,
    event_tx: mpsc::Sender<DaemonEvent>,
) {
    let (reader, writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut writer = writer;

    let write_task = tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if writer.write_all(&data).await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
    });

    loop {
        match read_frame(&mut reader).await {
            Ok(Some((msg_type, payload))) => match decode_request(msg_type, &payload) {
                Ok(request) => {
                    if event_tx
                        .send(DaemonEvent::ClientMessage(client_id, request))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("snagd: decode error from client {client_id}: {e}");
                    break;
                }
            },
            Ok(None) => break,
            Err(_) => break,
        }
    }

    let _ = event_tx
        .send(DaemonEvent::ClientDisconnected(client_id))
        .await;
    write_task.abort();
}

#[allow(clippy::too_many_arguments)]
async fn handle_request(
    registry: &mut SessionRegistry,
    clients: &mut HashMap<ClientId, AttachedClient>,
    client_id: ClientId,
    request: Request,
    config: &Config,
    start_time: Instant,
    event_tx: &mpsc::Sender<DaemonEvent>,
    scrollback_bytes: usize,
) -> Response {
    match request {
        Request::SessionNew { shell, name, cwd } => {
            handle_session_new(
                registry,
                config,
                shell,
                name,
                cwd,
                event_tx,
                scrollback_bytes,
            )
            .await
        }
        Request::SessionKill { target } => handle_session_kill(registry, &target),
        Request::SessionRename { target, new_name } => {
            handle_session_rename(registry, &target, new_name)
        }
        Request::SessionList => handle_session_list(registry),
        Request::SessionInfo { target } => handle_session_info(registry, &target),
        Request::SessionAttach { target, read_only } => {
            handle_session_attach(registry, clients, client_id, &target, read_only)
        }
        Request::SessionDetach => handle_session_detach(registry, clients, client_id),
        Request::SessionSend { target, input } => handle_session_send(registry, &target, &input),
        Request::SessionOutput {
            target,
            lines,
            follow,
        } => handle_session_output(registry, clients, client_id, &target, lines, follow),
        Request::SessionCwd { target } => handle_session_cwd(registry, &target),
        Request::SessionPs { target } => handle_session_ps(registry, &target),
        Request::SessionGrep { pattern } => handle_session_grep(registry, &pattern),
        Request::SessionRegister {
            pts,
            shell_pid,
            name,
        } => {
            handle_session_register(registry, &pts, shell_pid, name, scrollback_bytes, event_tx)
                .await
        }
        Request::SessionUnregister { target } => handle_session_unregister(registry, &target),
        Request::Resize { cols, rows } => handle_resize(registry, clients, client_id, cols, rows),
        Request::PtyInput(data) => handle_pty_input(registry, clients, client_id, &data),
        Request::DaemonStatus => handle_daemon_status(registry, start_time),
        Request::DaemonStop => {
            let _ = event_tx.send(DaemonEvent::Shutdown).await;
            Response::Ok(ResponseData::Empty)
        }
    }
}

async fn handle_session_new(
    registry: &mut SessionRegistry,
    config: &Config,
    shell: Option<String>,
    name: Option<String>,
    cwd: Option<String>,
    event_tx: &mpsc::Sender<DaemonEvent>,
    scrollback_bytes: usize,
) -> Response {
    // Validate name if provided
    if let Some(ref n) = name {
        if let Err(e) = validate_session_name(n) {
            return Response::Error {
                code: 1,
                message: e.to_string(),
            };
        }
        if registry.has_name(n) {
            return Response::Error {
                code: 2,
                message: format!("session name already in use: {n}"),
            };
        }
    }

    let shell = shell.unwrap_or_else(|| config.default_shell());
    let cwd = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

    match pty::spawn_shell(&shell, &cwd) {
        Ok(result) => {
            let id = generate_session_id();
            let session = Session::new_spawned(
                id.clone(),
                name,
                result.master_fd,
                result.child_pid,
                shell,
                result.pts_path,
                scrollback_bytes,
            );

            // Start PTY read loop
            let master_raw = session.raw_fd();
            let session_id = id.clone();
            let tx = event_tx.clone();
            tokio::spawn(async move {
                pty_read_loop(session_id, master_raw, tx).await;
            });

            registry.insert(session);
            Response::Ok(ResponseData::SessionCreated { id })
        }
        Err(e) => Response::Error {
            code: 3,
            message: format!("failed to spawn session: {e}"),
        },
    }
}

async fn pty_read_loop(session_id: String, master_fd: i32, event_tx: mpsc::Sender<DaemonEvent>) {
    // Set fd to non-blocking for use with AsyncFd
    unsafe {
        let flags = nix::libc::fcntl(master_fd, nix::libc::F_GETFL);
        nix::libc::fcntl(master_fd, nix::libc::F_SETFL, flags | nix::libc::O_NONBLOCK);
    }

    let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(master_fd) };
    let Ok(async_fd) = tokio::io::unix::AsyncFd::new(fd) else {
        eprintln!("snagd: failed to create AsyncFd for session {session_id}");
        return;
    };

    let mut buf = [0u8; 4096];
    loop {
        let mut ready = match async_fd.readable().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("snagd: readable error for session {session_id}: {e}");
                break;
            }
        };

        match ready.try_io(|inner| {
            let ret =
                unsafe { nix::libc::read(inner.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if ret < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(ret as usize)
            }
        }) {
            Ok(Ok(0)) => {
                let _ = event_tx.send(DaemonEvent::PtyEof(session_id)).await;
                break;
            }
            Ok(Ok(n)) => {
                let _ = event_tx
                    .send(DaemonEvent::PtyData(session_id.clone(), buf[..n].to_vec()))
                    .await;
            }
            Ok(Err(e)) => {
                // EIO is expected when the child exits
                if e.raw_os_error() == Some(nix::libc::EIO) {
                    let _ = event_tx.send(DaemonEvent::PtyEof(session_id)).await;
                } else {
                    eprintln!("snagd: read error for session {session_id}: {e}");
                    let _ = event_tx.send(DaemonEvent::PtyEof(session_id)).await;
                }
                break;
            }
            Err(_would_block) => continue,
        }
    }
}

fn handle_session_kill(registry: &mut SessionRegistry, target: &str) -> Response {
    match registry.resolve(target) {
        Ok(id) => {
            // Teardown capture before removal (needs mutable access to session)
            if let Some(session) = registry.get_mut(&id) {
                if session.registered {
                    teardown_output_capture(session);
                }
            }

            if let Some(session) = registry.remove(&id) {
                // For registered sessions, just drop the fd — don't kill the shell.
                // The shell continues running in the original terminal.
                if !session.registered {
                    if let Some(pid) = session.child_pid {
                        pty::kill_session(pid);
                    }
                }
                Response::Ok(ResponseData::Empty)
            } else {
                Response::Error {
                    code: 4,
                    message: format!("session not found: {target}"),
                }
            }
        }
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

fn handle_session_rename(
    registry: &mut SessionRegistry,
    target: &str,
    new_name: String,
) -> Response {
    if let Err(e) = validate_session_name(&new_name) {
        return Response::Error {
            code: 1,
            message: e.to_string(),
        };
    }
    match registry.rename(target, new_name) {
        Ok(()) => Response::Ok(ResponseData::Empty),
        Err(e) => Response::Error {
            code: 5,
            message: e.to_string(),
        },
    }
}

fn handle_session_list(registry: &SessionRegistry) -> Response {
    let sessions: Vec<_> = registry.iter().map(|s| s.to_info()).collect();
    Response::Ok(ResponseData::SessionList(sessions))
}

fn handle_session_info(registry: &SessionRegistry, target: &str) -> Response {
    match registry.resolve_session(target) {
        Ok(session) => Response::Ok(ResponseData::SessionInfo(session.to_info())),
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

fn handle_session_attach(
    registry: &mut SessionRegistry,
    clients: &mut HashMap<ClientId, AttachedClient>,
    client_id: ClientId,
    target: &str,
    read_only: bool,
) -> Response {
    match registry.resolve(target) {
        Ok(id) => {
            if let Some(session) = registry.get_mut(&id) {
                session.attached_clients.push(client_id);

                if let Some(client) = clients.get_mut(&client_id) {
                    client.session_id = Some(id.clone());
                    client.read_only = read_only;
                }

                // Send scrollback
                let scrollback = session.scrollback.all_bytes();
                let scrollback_str = String::from_utf8_lossy(&scrollback).to_string();

                Response::Ok(ResponseData::Output(scrollback_str))
            } else {
                Response::Error {
                    code: 4,
                    message: format!("session not found: {target}"),
                }
            }
        }
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

fn handle_session_detach(
    registry: &mut SessionRegistry,
    clients: &mut HashMap<ClientId, AttachedClient>,
    client_id: ClientId,
) -> Response {
    if let Some(client) = clients.get_mut(&client_id) {
        if let Some(ref session_id) = client.session_id.take() {
            if let Some(session) = registry.get_mut(session_id) {
                session.attached_clients.retain(|&id| id != client_id);
            }
        }
    }
    Response::Ok(ResponseData::Empty)
}

fn handle_session_send(registry: &mut SessionRegistry, target: &str, input: &str) -> Response {
    match registry.resolve(target) {
        Ok(id) => {
            if let Some(session) = registry.get(&id) {
                let data = format!("{input}\n");
                match nix::unistd::write(&session.master_fd, data.as_bytes()) {
                    Ok(_) => Response::Ok(ResponseData::Empty),
                    Err(e) => Response::Error {
                        code: 6,
                        message: format!("write error: {e}"),
                    },
                }
            } else {
                Response::Error {
                    code: 4,
                    message: format!("session not found: {target}"),
                }
            }
        }
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

fn handle_session_output(
    registry: &mut SessionRegistry,
    clients: &mut HashMap<ClientId, AttachedClient>,
    client_id: ClientId,
    target: &str,
    lines: Option<u32>,
    follow: bool,
) -> Response {
    match registry.resolve(target) {
        Ok(id) => {
            if let Some(session) = registry.get_mut(&id) {
                // For registered sessions without capture, warn
                if session.registered
                    && session.capture_path.is_none()
                    && session.scrollback.is_empty()
                {
                    return Response::Error {
                        code: 9,
                        message: "output capture not available: shell does not support \
                                  process substitution (requires bash or zsh)"
                            .to_string(),
                    };
                }

                let output = if let Some(n) = lines {
                    session.scrollback.last_n_lines(n as usize)
                } else {
                    session.scrollback.all_bytes()
                };

                if follow {
                    session.attached_clients.push(client_id);
                    if let Some(client) = clients.get_mut(&client_id) {
                        client.session_id = Some(id.clone());
                        client.read_only = true;
                    }
                }

                let output_str = String::from_utf8_lossy(&output).to_string();
                Response::Ok(ResponseData::Output(output_str))
            } else {
                Response::Error {
                    code: 4,
                    message: format!("session not found: {target}"),
                }
            }
        }
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

fn handle_session_cwd(registry: &SessionRegistry, target: &str) -> Response {
    match registry.resolve_session(target) {
        Ok(session) => {
            let cwd = session
                .child_pid
                .and_then(|pid| pty::read_cwd(pid.as_raw() as u32))
                .unwrap_or_else(|| "?".to_string());
            Response::Ok(ResponseData::Cwd(cwd))
        }
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

fn handle_session_ps(registry: &SessionRegistry, target: &str) -> Response {
    match registry.resolve_session(target) {
        Ok(session) => {
            let procs = pty::fg_process(&session.pts_path);
            let entries = procs
                .into_iter()
                .map(|(pid, command)| ProcessEntry { pid, command })
                .collect();
            Response::Ok(ResponseData::ProcessInfo(entries))
        }
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

fn handle_session_grep(registry: &SessionRegistry, pattern: &str) -> Response {
    let pattern_lower = pattern.to_lowercase();
    let mut matches = Vec::new();

    // Search managed sessions' scrollback buffers
    for session in registry.iter() {
        let raw = session.scrollback.all_bytes();
        if raw.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&raw);
        let stripped = strip_ansi(&text);
        let matching_lines: Vec<String> = stripped
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty() && trimmed.to_lowercase().contains(&pattern_lower)
            })
            .map(|l| l.to_string())
            .collect();

        if !matching_lines.is_empty() {
            matches.push(GrepMatch {
                session_id: session.id.clone(),
                session_name: session.name.clone(),
                lines: matching_lines,
            });
        }
    }

    Response::Ok(ResponseData::GrepResult(matches))
}

/// Strip ANSI escape sequences from text for plain-text searching.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI sequence: ESC [ ... final_byte
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if nc.is_ascii_alphabetic() || nc == '~' {
                        break;
                    }
                }
            // OSC sequence: ESC ] ... ST (BEL or ESC \)
            } else if chars.peek() == Some(&']') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if nc == '\x07' {
                        break;
                    }
                    if nc == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            } else {
                // Other escape: skip next char
                chars.next();
            }
        } else if c.is_control() && c != '\n' && c != '\t' {
            // Skip other control chars
        } else {
            out.push(c);
        }
    }
    out
}

async fn handle_session_register(
    registry: &mut SessionRegistry,
    pts: &str,
    shell_pid: u32,
    name: Option<String>,
    scrollback_bytes: usize,
    event_tx: &mpsc::Sender<DaemonEvent>,
) -> Response {
    // Validate name if provided
    if let Some(ref n) = name {
        if let Err(e) = validate_session_name(n) {
            return Response::Error {
                code: 1,
                message: e.to_string(),
            };
        }
        if registry.has_name(n) {
            return Response::Error {
                code: 2,
                message: format!("session name already in use: {n}"),
            };
        }
    }

    // Scan to find the terminal emulator holding the master fd for this PTS
    let sessions = match adopt::scan_pty_sessions() {
        Ok(s) => s,
        Err(e) => {
            return Response::Error {
                code: 7,
                message: e.to_string(),
            };
        }
    };

    // Find the session matching the given PTS path
    let target = sessions.into_iter().find(|s| s.pts == pts);

    let Some(discovered) = target else {
        return Response::Error {
            code: 4,
            message: format!("no PTY session found for: {pts}"),
        };
    };

    // Steal the master fd via pidfd_getfd
    match adopt::adopt_pty(discovered.holder_pid, discovered.holder_fd) {
        Ok(master_fd) => {
            let id = generate_session_id();

            // Set up capture file (the shell hook will exec tee on the client side,
            // but we still create the capture file and start reading it)
            let capture_path = setup_capture_file(&id);

            let shell_name = pty::read_comm(shell_pid).unwrap_or_else(|| "shell".to_string());
            let session = Session::new_registered(
                id.clone(),
                name,
                master_fd,
                Some(shell_pid),
                shell_name,
                PathBuf::from(&discovered.pts),
                scrollback_bytes,
                capture_path.clone(),
            );

            registry.insert(session);

            // Start capture file reader if capture was set up
            let capture_path_str = if let Some(ref path) = capture_path {
                let handle = tokio::spawn(capture_file_read_loop(
                    path.clone(),
                    id.clone(),
                    event_tx.clone(),
                ));
                if let Some(s) = registry.get_mut(&id) {
                    s.capture_abort = Some(handle.abort_handle());
                }
                path.to_string_lossy().to_string()
            } else {
                String::new()
            };

            Response::Ok(ResponseData::SessionRegistered {
                id,
                capture_path: capture_path_str,
            })
        }
        Err(e) => Response::Error {
            code: 8,
            message: e.to_string(),
        },
    }
}

fn handle_session_unregister(registry: &mut SessionRegistry, target: &str) -> Response {
    match registry.resolve(target) {
        Ok(id) => {
            if let Some(session) = registry.get(&id) {
                if !session.registered {
                    return Response::Error {
                        code: 10,
                        message: "cannot unregister a spawned session; use 'kill' instead"
                            .to_string(),
                    };
                }
            }

            // Teardown capture (no SIGHUP, just cleanup)
            if let Some(session) = registry.get_mut(&id) {
                teardown_output_capture(session);
            }

            // Remove from registry — drops the master fd without killing the shell
            if registry.remove(&id).is_some() {
                Response::Ok(ResponseData::Empty)
            } else {
                Response::Error {
                    code: 4,
                    message: format!("session not found: {target}"),
                }
            }
        }
        Err(e) => Response::Error {
            code: 4,
            message: e.to_string(),
        },
    }
}

/// Create a capture file for a registered session.
/// Returns the capture file path. The shell hook will set up `exec > >(tee ...)` itself.
fn setup_capture_file(session_id: &str) -> Option<PathBuf> {
    let capture_dir = capture_dir();
    if let Err(e) = std::fs::create_dir_all(&capture_dir) {
        eprintln!("snagd: failed to create capture directory: {e}");
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&capture_dir, std::fs::Permissions::from_mode(0o700));
    }

    let path = capture_dir.join(format!("capture-{session_id}"));
    if let Err(e) = std::fs::File::create(&path) {
        eprintln!("snagd: failed to create capture file: {e}");
        return None;
    }

    eprintln!("snagd: capture file created at {}", path.display());
    Some(path)
}

/// Tail a capture file and feed data into the daemon event loop as PtyData events.
async fn capture_file_read_loop(
    path: PathBuf,
    session_id: String,
    event_tx: mpsc::Sender<DaemonEvent>,
) {
    // Give the shell time to set up the tee command
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Use a dedicated blocking thread with std::fs::File to properly
    // maintain the read position when tailing an appended file.
    // tokio::fs::File can lose position across async read calls.
    // Use a dedicated OS thread for tailing the capture file.
    // We use std::fs::File to maintain a stable read position.
    // Communication back to the tokio runtime uses an unbounded channel
    // to avoid blocking_send deadlocks on current_thread runtime.
    let (file_tx, mut file_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let sid_thread = session_id.clone();
    std::thread::spawn(move || {
        let mut file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("snagd: failed to open capture file {}: {e}", path.display());
                return;
            }
        };

        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut file, &mut buf) {
                Ok(0) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Ok(n) => {
                    if file_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("snagd: capture read error for {sid_thread}: {e}");
                    break;
                }
            }
        }
    });

    // Bridge: forward data from the OS thread to the daemon event loop
    let sid_bridge = session_id.clone();
    while let Some(data) = file_rx.recv().await {
        if event_tx
            .send(DaemonEvent::PtyData(sid_bridge.clone(), data))
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Clean up output capture for a registered session.
fn teardown_output_capture(session: &mut Session) {
    // Abort the capture file reader
    if let Some(abort) = session.capture_abort.take() {
        abort.abort();
    }

    // Restore direct tty output in the shell (\x15 = Ctrl+U to clear input line)
    if session.capture_path.is_some() {
        let _ = nix::unistd::write(&session.master_fd, b"\x15exec > /dev/tty 2>&1\n");
    }

    // Remove capture file
    if let Some(ref path) = session.capture_path.take() {
        let _ = std::fs::remove_file(path);
    }
}

fn capture_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("snag")
    } else {
        let uid = nix::unistd::getuid();
        PathBuf::from(format!("/tmp/snag-{}", uid))
    }
}

fn handle_resize(
    registry: &mut SessionRegistry,
    clients: &HashMap<ClientId, AttachedClient>,
    client_id: ClientId,
    cols: u16,
    rows: u16,
) -> Response {
    let session_id = clients.get(&client_id).and_then(|c| c.session_id.clone());

    if let Some(id) = session_id {
        if let Some(session) = registry.get(&id) {
            match pty::set_winsize(session.raw_fd(), rows, cols) {
                Ok(()) => Response::Ok(ResponseData::Empty),
                Err(e) => Response::Error {
                    code: 6,
                    message: e.to_string(),
                },
            }
        } else {
            Response::Ok(ResponseData::Empty)
        }
    } else {
        Response::Ok(ResponseData::Empty)
    }
}

fn handle_pty_input(
    registry: &SessionRegistry,
    clients: &HashMap<ClientId, AttachedClient>,
    client_id: ClientId,
    data: &[u8],
) -> Response {
    let client = clients.get(&client_id);
    if let Some(client) = client {
        if client.read_only {
            return Response::Ok(ResponseData::Empty);
        }
        if let Some(ref session_id) = client.session_id {
            if let Some(session) = registry.get(session_id) {
                let _ = nix::unistd::write(&session.master_fd, data);
            }
        }
    }
    Response::Ok(ResponseData::Empty)
}

fn handle_daemon_status(registry: &SessionRegistry, start_time: Instant) -> Response {
    Response::Ok(ResponseData::DaemonStatus {
        pid: std::process::id(),
        uptime_secs: start_time.elapsed().as_secs(),
        session_count: registry.len(),
    })
}
