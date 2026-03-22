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
                // Kill all sessions
                for session in registry.iter() {
                    if let Some(pid) = session.child_pid {
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
        Request::SessionList { all } => handle_session_list(registry, all),
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
        Request::SessionScan => handle_session_scan(),
        Request::SessionAdopt { pts_or_pid, name } => {
            handle_session_adopt(registry, &pts_or_pid, name, scrollback_bytes)
        }
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
    use std::os::fd::BorrowedFd;
    use tokio::io::unix::AsyncFd;

    let fd = unsafe { BorrowedFd::borrow_raw(master_fd) };
    let Ok(async_fd) = AsyncFd::new(fd) else {
        return;
    };

    let mut buf = [0u8; 4096];
    loop {
        let mut ready = match async_fd.readable().await {
            Ok(r) => r,
            Err(_) => break,
        };

        match ready.try_io(|inner| {
            let n = nix::unistd::read(inner.as_raw_fd(), &mut buf)
                .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
            Ok(n)
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
            Ok(Err(_)) => {
                let _ = event_tx.send(DaemonEvent::PtyEof(session_id)).await;
                break;
            }
            Err(_would_block) => continue,
        }
    }
}

fn handle_session_kill(registry: &mut SessionRegistry, target: &str) -> Response {
    match registry.resolve(target) {
        Ok(id) => {
            if let Some(session) = registry.remove(&id) {
                if let Some(pid) = session.child_pid {
                    pty::kill_session(pid);
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

fn handle_session_list(registry: &SessionRegistry, all: bool) -> Response {
    let sessions: Vec<_> = registry
        .iter()
        .filter(|s| all || !s.adopted)
        .map(|s| s.to_info())
        .collect();
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
                let output = if let Some(n) = lines {
                    session.scrollback.last_n_lines(n as usize)
                } else {
                    session.scrollback.all_bytes()
                };

                if follow {
                    // Attach client for follow mode (read-only)
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

fn handle_session_scan() -> Response {
    match adopt::scan_pty_sessions() {
        Ok(sessions) => Response::Ok(ResponseData::ScanResult(sessions)),
        Err(e) => Response::Error {
            code: 7,
            message: e.to_string(),
        },
    }
}

fn handle_session_adopt(
    registry: &mut SessionRegistry,
    pts_or_pid: &str,
    name: Option<String>,
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

    // Scan to find the target
    let sessions = match adopt::scan_pty_sessions() {
        Ok(s) => s,
        Err(e) => {
            return Response::Error {
                code: 7,
                message: e.to_string(),
            };
        }
    };

    // Find matching session by PTS path or PID
    let target = sessions.into_iter().find(|s| {
        s.pts.ends_with(&format!("/{pts_or_pid}"))
            || s.pts == format!("/dev/pts/{pts_or_pid}")
            || s.holder_pid.to_string() == pts_or_pid
            || s.shell_pid.map(|p| p.to_string()) == Some(pts_or_pid.to_string())
    });

    let Some(discovered) = target else {
        return Response::Error {
            code: 4,
            message: format!("no adoptable session found for: {pts_or_pid}"),
        };
    };

    // Adopt the PTY master fd
    match adopt::adopt_pty(discovered.holder_pid, discovered.holder_fd) {
        Ok(master_fd) => {
            let id = generate_session_id();
            let session = Session::new_adopted(
                id.clone(),
                name,
                master_fd,
                discovered.shell_pid,
                discovered.command.clone(),
                PathBuf::from(&discovered.pts),
                scrollback_bytes,
            );
            registry.insert(session);
            Response::Ok(ResponseData::SessionCreated { id })
        }
        Err(e) => Response::Error {
            code: 8,
            message: e.to_string(),
        },
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
