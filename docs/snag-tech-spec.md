# Snag — Technical Specification

**Version:** 0.1
**Status:** Draft
**Companion document:** [Snag PRD](./snag-prd.md)

---

## 1. Overview

Snag is a single Rust binary (~2–4 MB static) that manages PTY sessions on a local Linux machine. It operates as two logical components compiled into the same binary:

- **`snag`** — the CLI/TUI client
- **`snagd`** (invoked as `snag daemon`) — a lightweight background process that owns PTY master file descriptors and multiplexes I/O

The daemon is auto-spawned on first use and transparent to the user. All communication between client and daemon happens over a Unix domain socket.

---

## 2. Design Principles

1. **Minimal footprint** — idle daemon < 1 MB RSS, near-zero CPU when no I/O flows
2. **Zero-copy where possible** — PTY bytes flow from kernel fd to client fd with minimal intermediate buffering
3. **No runtime dependencies** — static binary via `x86_64-unknown-linux-musl`, no libc dependency at runtime
4. **Single binary** — `snag` and `snagd` are the same executable, behavior determined by subcommand
5. **Crash-safe** — daemon crash loses active sessions (unavoidable — it holds the fds), but metadata is recoverable; client crash never affects sessions

---

## 3. Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  snag CLI   │     │  snag CLI   │     │  snag TUI   │
│  (client)   │     │  (client)   │     │  (client)   │
└──────┬──────┘     └──────┬──────┘     └──────┬──────┘
       │ Unix socket       │                    │
       └───────────┬───────┘────────────────────┘
                   │
          ┌────────▼────────┐
          │     snagd       │
          │   (daemon)      │
          │                 │
          │  ┌───────────┐  │
          │  │ Session 1 │──┼──► PTY master fd ──► /dev/pts/X ──► zsh
          │  ├───────────┤  │
          │  │ Session 2 │──┼──► PTY master fd ──► /dev/pts/Y ──► bash
          │  ├───────────┤  │
          │  │ Session 3 │──┼──► adopted fd    ──► /dev/pts/Z ──► zsh (pre-existing)
          │  └───────────┘  │
          └─────────────────┘
```

### 3.1. Daemon (`snagd`)

The daemon is the only process that holds PTY master file descriptors. It is responsible for:

- Spawning shell processes via `forkpty()`
- Adopting existing sessions via `pidfd_getfd()`
- Reading PTY output into per-session ring buffers
- Forwarding PTY output to attached clients
- Writing client input to PTY master fds
- Handling PTY resize (`TIOCSWINSZ`)
- Managing session lifecycle (tracking exits via `SIGCHLD` / `waitpid`)
- Serving the Unix socket protocol

**Process model:** Single-threaded async (tokio with `current_thread` runtime). PTY I/O is inherently fd-based and maps naturally to epoll. No thread pool needed — the workload is I/O-bound with negligible computation.

**Why single-threaded:** Eliminates all synchronization overhead. Session state is owned by the single task executor. The bottleneck is kernel PTY throughput, not CPU. A single thread can saturate PTY I/O for hundreds of concurrent sessions.

### 3.2. Client (`snag`)

The client is a short-lived process for non-interactive commands (`list`, `send`, `kill`, etc.) or a long-lived process for interactive commands (`attach`, TUI). It connects to the daemon's Unix socket, sends a request, and processes the response.

For `attach`, the client:
1. Puts the local terminal in raw mode (`cfmakeraw`)
2. Forwards local stdin to the daemon (which writes it to the PTY)
3. Receives PTY output from the daemon and writes it to local stdout
4. Forwards `SIGWINCH` to the daemon for PTY resize
5. Restores terminal state on detach

### 3.3. Unix Socket Location

Default: `$XDG_RUNTIME_DIR/snag/snag.sock` (typically `/run/user/<UID>/snag/snag.sock`).

Fallback: `/tmp/snag-<UID>/snag.sock` with `0700` directory permissions.

Override: `--socket <path>` or `SNAG_SOCKET` environment variable.

---

## 4. Wire Protocol

Binary protocol over Unix domain socket. Every message is framed as:

```
┌──────────┬──────────┬─────────────────┐
│ type(u8) │ len(u32) │ payload(bytes)  │
│  1 byte  │ 4 bytes  │ variable        │
│          │ little-endian              │
└──────────┴──────────┴─────────────────┘
```

Total header: 5 bytes. Payload is MessagePack-encoded for structured messages, raw bytes for PTY data streams.

### 4.1. Message Types

**Client → Daemon (requests):**

| Type | ID | Payload |
|---|---|---|
| `SessionNew` | `0x01` | `{ shell: Option<String>, name: Option<String>, cwd: Option<String> }` |
| `SessionKill` | `0x02` | `{ target: String }` (id or name) |
| `SessionRename` | `0x03` | `{ target: String, new_name: String }` |
| `SessionList` | `0x04` | `{ all: bool }` |
| `SessionInfo` | `0x05` | `{ target: String }` |
| `SessionAttach` | `0x06` | `{ target: String, read_only: bool }` |
| `SessionDetach` | `0x07` | `{}` |
| `SessionSend` | `0x08` | `{ target: String, input: String }` |
| `SessionOutput` | `0x09` | `{ target: String, lines: Option<u32>, follow: bool }` |
| `SessionCwd` | `0x0A` | `{ target: String }` |
| `SessionPs` | `0x0B` | `{ target: String }` |
| `SessionScan` | `0x0C` | `{}` |
| `SessionAdopt` | `0x0D` | `{ pts_or_pid: String, name: Option<String> }` |
| `Resize` | `0x0E` | `{ cols: u16, rows: u16 }` |
| `PtyInput` | `0x10` | raw bytes (no msgpack, payload is direct stdin bytes) |
| `DaemonStatus` | `0xF0` | `{}` |
| `DaemonStop` | `0xF1` | `{}` |

**Daemon → Client (responses):**

| Type | ID | Payload |
|---|---|---|
| `Ok` | `0x80` | `{ data: Value }` (command-specific response) |
| `Error` | `0x81` | `{ code: u16, message: String }` |
| `PtyOutput` | `0x82` | raw bytes (streamed PTY output during attach/follow) |
| `SessionEvent` | `0x83` | `{ event: String, session_id: String, ... }` (session exited, etc.) |

### 4.2. Attach Flow

```
Client                          Daemon
  │                                │
  │── SessionAttach ──────────────►│  daemon registers client as attached
  │◄── Ok { scrollback_lines } ───│  sends buffered scrollback
  │◄── PtyOutput (stream) ────────│  continuous output forwarding begins
  │── PtyInput (stream) ──────────►│  client forwards keystrokes
  │── Resize ─────────────────────►│  on SIGWINCH
  │                                │
  │── SessionDetach ──────────────►│  or client disconnects / escape sequence
  │◄── Ok ────────────────────────│
```

During attach, the connection switches to a bidirectional streaming mode. `PtyOutput` and `PtyInput` messages flow continuously with minimal framing overhead (5-byte header + raw bytes).

### 4.3. Why MessagePack

- Compact binary encoding (~30% smaller than JSON)
- Zero-copy deserialization possible via `rmp-serde`
- Schema-flexible (forward-compatible with new fields)
- Negligible encode/decode overhead

JSON is available as an output format for CLI consumers (`--json` flags) but is never used on the wire.

---

## 5. Session Management

### 5.1. Session Registry

The daemon maintains an in-memory `HashMap<SessionId, Session>`:

```rust
struct Session {
    id: SessionId,                    // 8-byte random ID, hex-encoded
    name: Option<String>,             // user-assigned label
    master_fd: OwnedFd,              // PTY master file descriptor
    child_pid: Option<Pid>,          // shell process PID (None for adopted sessions where unknown)
    shell: String,                    // shell binary path
    pts_path: PathBuf,               // /dev/pts/N
    state: SessionState,              // Running | Exited(i32)
    created_at: Instant,
    scrollback: RingBuffer,           // output ring buffer
    attached_clients: Vec<ClientId>,  // currently attached client connections
    adopted: bool,                    // whether this was adopted vs spawned
}

enum SessionState {
    Running,
    Exited(i32),
}
```

### 5.2. Session ID Generation

8 random bytes, hex-encoded to 16 characters. Generated via `getrandom` crate (reads from `/dev/urandom`). Collision probability is negligible at the expected session count (< 1000).

Short prefixes are accepted for all commands — `snag attach a3f` resolves to the unique session whose ID starts with `a3f`. Ambiguity returns an error listing matches.

### 5.3. Name Resolution

All commands accepting `<id|name>` resolve in order:
1. Exact name match
2. Exact ID match
3. ID prefix match (minimum 3 characters)

### 5.4. Spawning Sessions

```rust
fn spawn_session(shell: &str, cwd: &Path) -> Result<Session> {
    // 1. openpty() to create master/slave pair
    let (master, slave) = openpty(None, None)?;

    // 2. fork()
    match unsafe { fork()? } {
        ForkResult::Child => {
            // Close master fd
            close(master)?;
            // Create new session (setsid)
            setsid()?;
            // Set controlling terminal (TIOCSCTTY)
            ioctl(slave, TIOCSCTTY, 0)?;
            // Dup slave to stdin/stdout/stderr
            dup2(slave, STDIN_FILENO)?;
            dup2(slave, STDOUT_FILENO)?;
            dup2(slave, STDERR_FILENO)?;
            if slave > STDERR_FILENO { close(slave)?; }
            // chdir
            chdir(cwd)?;
            // exec shell
            execvp(shell, &[shell, "-l"])?;
            unreachable!();
        }
        ForkResult::Parent { child } => {
            close(slave)?;
            // Register master_fd with epoll/tokio
            // Return Session with master_fd and child pid
        }
    }
}
```

Using raw `openpty` + `fork` instead of `forkpty` for explicit control over the PTY lifecycle. The `nix` crate provides safe wrappers for all these syscalls.

### 5.5. Adopting Sessions

Adoption flow:

```
1. snag scan
   └─► Walk /proc/*/fd/ to find processes holding PTY master fds
   └─► Cross-reference with /dev/pts/* to identify active sessions
   └─► Filter by UID (same user only)
   └─► Report: PTS device, holder PID, shell PID, command, CWD

2. snag adopt <pts|pid> --name dev
   └─► Identify the process holding the master fd for the target PTS
   └─► pidfd_open(holder_pid) to get a pidfd
   └─► pidfd_getfd(pidfd, master_fd_num) to duplicate the master fd into snagd
   └─► Register the duplicated fd as a new Session
   └─► Begin reading output and populating scrollback
```

**Scanning implementation:**

```rust
fn scan_pty_sessions() -> Result<Vec<DiscoveredSession>> {
    let uid = getuid();
    let mut sessions = Vec::new();

    // Iterate /proc entries for our user
    for entry in read_dir("/proc")? {
        let pid = parse_pid(&entry)?;
        if proc_uid(pid)? != uid { continue; }

        // Check each fd
        for fd_entry in read_dir(format!("/proc/{}/fd", pid))? {
            let link = readlink(&fd_entry.path())?;
            // Is this fd a PTY master? Check /dev/ptmx or /dev/pts/ptmx
            if is_pty_master(&link) {
                let pts_num = get_pts_number(pid, fd_entry)?;
                // Find the shell process on the slave side
                let shell_pid = find_slave_process(pts_num)?;
                sessions.push(DiscoveredSession {
                    pts: format!("/dev/pts/{}", pts_num),
                    holder_pid: pid,
                    holder_fd: fd_num,
                    shell_pid,
                    command: read_comm(shell_pid)?,
                    cwd: readlink(format!("/proc/{}/cwd", shell_pid))?,
                });
            }
        }
    }
    Ok(sessions)
}
```

**`pidfd_getfd` adoption:**

```rust
fn adopt_session(holder_pid: Pid, holder_fd: RawFd) -> Result<OwnedFd> {
    // Get a pidfd for the holder process
    let pidfd = unsafe { syscall(SYS_pidfd_open, holder_pid.as_raw(), 0) };
    if pidfd < 0 { return Err(last_errno()); }

    // Duplicate the master fd from the holder process into our process
    let our_fd = unsafe { syscall(SYS_pidfd_getfd, pidfd, holder_fd, 0) };
    if our_fd < 0 { return Err(last_errno()); }

    close(pidfd as RawFd)?;
    Ok(unsafe { OwnedFd::from_raw_fd(our_fd as RawFd) })
}
```

**Kernel requirements:**
- `pidfd_open`: Linux 5.3+
- `pidfd_getfd`: Linux 5.6+
- `ptrace_scope` sysctl must allow access (Yama LSM: `kernel.yama.ptrace_scope` ≤ 1, or same-parent relationship not required for `pidfd_getfd` in most configurations)

**Fallback (Linux < 5.6):** Use `ptrace(PTRACE_ATTACH)` + `process_vm_readv` to read the fd table, then `ptrace(PTRACE_SYSCALL)` to inject a `dup2` call. This is the approach `reptyr` uses. Complex but functional. Implemented as a secondary codepath behind a runtime kernel version check.

---

## 6. I/O Multiplexing

### 6.1. PTY Read Loop

Each session's master fd is registered with tokio's reactor (epoll-backed `AsyncFd`). When data is available:

```rust
async fn pty_read_loop(session: &mut Session, clients: &ClientRegistry) {
    let mut buf = [0u8; 4096]; // stack-allocated, no heap alloc per read
    loop {
        let ready = session.async_fd.readable().await?;
        match ready.try_io(|fd| {
            let n = nix::unistd::read(fd.as_raw_fd(), &mut buf)?;
            Ok(n)
        }) {
            Ok(Ok(0)) => { /* EOF — session exited */ break; }
            Ok(Ok(n)) => {
                let data = &buf[..n];
                // 1. Append to scrollback ring buffer
                session.scrollback.write(data);
                // 2. Fan-out to attached clients
                for client in &session.attached_clients {
                    client.send_pty_output(data).await;
                }
            }
            Ok(Err(e)) => { /* handle error */ }
            Err(_would_block) => { continue; }
        }
    }
}
```

### 6.2. Scrollback Ring Buffer

```rust
struct RingBuffer {
    buf: Box<[u8]>,     // fixed-size heap allocation
    write_pos: usize,   // current write position
    len: usize,         // bytes currently stored (up to capacity)
}
```

- Default capacity: 1 MB (configurable, represents ~10,000 lines of typical terminal output)
- Single contiguous allocation, no per-line overhead
- Write: memcpy to current position, wrap around
- Read last N lines: scan backwards from write position for `\n` bytes
- Zero-copy read: returns `(&[u8], &[u8])` — the two contiguous slices of the ring buffer that together represent the stored data in order

No line indexing. Line counting is done by scanning for `\n` in the buffer. For a 1 MB buffer this takes ~1μs on modern hardware — not worth the memory overhead of maintaining a line index.

### 6.3. Client I/O

Client connections are Unix domain socket streams. For attached clients, the daemon maintains a per-client write buffer and uses `writev` (scatter-gather I/O) to batch the 5-byte header + payload into a single syscall when possible.

Backpressure: if a client's socket write buffer fills up (slow consumer), the daemon skips PTY output delivery to that client rather than blocking. The client will have a gap in output but can re-sync from the scrollback buffer. This prevents a slow client from blocking the daemon's event loop.

---

## 7. Daemon Lifecycle

### 7.1. Auto-Start

When any `snag` command runs and no daemon is found at the socket path:

1. Client forks a child process
2. Child calls `setsid()` to detach from terminal
3. Child re-execs itself as `snag daemon --socket <path>`
4. Child writes a pidfile to `$XDG_RUNTIME_DIR/snag/snag.pid`
5. Parent polls the socket path until it appears (timeout: 2 seconds)
6. Parent connects and proceeds with the original command

### 7.2. Auto-Stop

When the last session exits and no sessions remain, the daemon starts a grace period timer (default: 30 seconds). If no new sessions are created within the grace period, the daemon exits cleanly. This avoids unnecessary daemon churn for rapid session create/destroy cycles.

### 7.3. Explicit Management

```
snag daemon start      # start daemon (no-op if running)
snag daemon stop       # graceful shutdown (kills all sessions)
snag daemon status     # print daemon PID, uptime, session count, memory usage
```

### 7.4. Signal Handling

| Signal | Behavior |
|---|---|
| `SIGTERM` | Graceful shutdown: send SIGHUP to all child shells, wait 5s, SIGKILL survivors, exit |
| `SIGCHLD` | Reap child processes, update session state to `Exited(code)` |
| `SIGPIPE` | Ignored (handled as write errors on individual client sockets) |
| `SIGHUP` | Ignored (daemon is detached) |

---

## 8. CLI Implementation

### 8.1. Argument Parsing

`clap` v4 with derive macros. Subcommand-based structure:

```rust
#[derive(Parser)]
#[command(name = "snag", about = "Snag shell sessions")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    #[arg(long, global = true)]
    scrollback: Option<usize>,
}

#[derive(Subcommand)]
enum Command {
    New { ... },
    Kill { target: String },
    Rename { target: String, new_name: String },
    List { #[arg(long)] json: bool, #[arg(long)] all: bool },
    Info { target: String, #[arg(long)] json: bool },
    Attach { target: String, #[arg(long)] read_only: bool },
    Send { target: String, command: String },
    Output { target: String, #[arg(long)] lines: Option<u32>, #[arg(long)] follow: bool, #[arg(long)] json: bool },
    Cwd { target: String },
    Ps { target: String },
    Scan,
    Adopt { pts_or_pid: String, #[arg(long)] name: Option<String> },
    Daemon { #[command(subcommand)] action: DaemonAction },
}
```

When invoked with no subcommand (`snag`), the TUI launches.

### 8.2. JSON Output

All `--json` outputs use `serde_json` with a consistent envelope:

```json
{
  "sessions": [
    {
      "id": "a3f7b2c1e9d04816",
      "name": "dev",
      "shell": "/bin/zsh",
      "cwd": "/home/user/project",
      "state": "running",
      "fg_process": "cargo build",
      "attached": 1,
      "adopted": false,
      "created_at": "2026-03-22T14:30:00Z"
    }
  ]
}
```

### 8.3. TUI

Built with `ratatui` + `crossterm`. Layout:

```
┌─ Snag ────────────────────────────────────────────┐
│ Sessions                                          │
│ ──────────────────────────────────────────────── │
│ ▸ dev       zsh   ~/project     cargo build      │
│   ci        bash  ~/ci-runner   idle             │
│   staging   zsh   ~/deploy      kubectl logs     │
│                                                   │
│ Preview (dev) ─────────────────────────────────── │
│   Compiling snag v0.1.0 (/home/user/project)     │
│   warning: unused variable `x`                    │
│     --> src/main.rs:42:9                          │
│                                                   │
│ [n]ew [k]ill [r]ename [s]end [Enter]attach [q]uit│
└───────────────────────────────────────────────────┘
```

Minimal dependency surface — `ratatui` and `crossterm` are the only TUI crates. No custom widget framework.

---

## 9. Process Introspection

### 9.1. Current Working Directory

```rust
fn session_cwd(shell_pid: Pid) -> Result<PathBuf> {
    readlink(format!("/proc/{}/cwd", shell_pid))
}
```

For adopted sessions where the shell PID might not be directly known, walk `/proc/<PID>/stat` to find the foreground process group of the PTY's slave side.

### 9.2. Foreground Process

```rust
fn session_fg_process(pts_path: &Path) -> Result<ProcessInfo> {
    // Get the foreground process group of the PTY
    let fg_pgid = tcgetpgrp(slave_fd)?;

    // Find the process(es) in that group
    // Walk /proc, match pgid
    // Return the leader's comm and cmdline
}
```

For spawned sessions, we can use `TIOCGPGRP` on the master fd. For adopted sessions, we parse `/dev/pts/N`'s device number and match it against `/proc/*/stat` field 7 (tty_nr).

### 9.3. Efficient `/proc` Access

All `/proc` reads use `openat2` where possible to minimize path resolution overhead. For `scan`, batch all reads per-PID to exploit dentry cache locality. Total scan time target: < 50ms for a system with 500 processes.

---

## 10. Security

### 10.1. Socket Permissions

The Unix socket directory is created with `0700` permissions. The socket itself inherits these permissions. Only the owning user (or root) can connect.

### 10.2. No Privilege Escalation

Snag never runs as root and never requests elevated privileges. Session adoption via `pidfd_getfd` is governed by kernel-level permission checks (same UID, ptrace scope). Snag does not bypass these — if the kernel denies access, adoption fails with a clear error message.

### 10.3. Input Sanitization

Session names are restricted to `[a-zA-Z0-9._-]`, max 64 characters. Shell paths must be absolute and exist on disk. CWD paths are validated with `stat()` before use.

---

## 11. Configuration

Config file: `$XDG_CONFIG_HOME/snag/config.toml` (default: `~/.config/snag/config.toml`).

```toml
# Default shell (default: $SHELL or /bin/sh)
shell = "/bin/zsh"

# Scrollback buffer size in bytes (default: 1048576 = 1MB)
scrollback_bytes = 1048576

# Socket path (default: $XDG_RUNTIME_DIR/snag/snag.sock)
# socket = "/run/user/1000/snag/snag.sock"

# Detach escape sequence (default: Ctrl+\ double-tap within 500ms)
detach_key = "ctrl-\\"
detach_timeout_ms = 500

# Always show adopted sessions in `snag list` (default: false)
show_adopted = false

# Daemon grace period before auto-exit in seconds (default: 30)
daemon_grace_period = 30
```

Parsed with `toml` crate. Missing file = all defaults. Unknown keys are ignored (forward compatibility).

---

## 12. Dependencies

Minimal, audited dependency tree:

| Crate | Purpose | Justification |
|---|---|---|
| `tokio` | Async runtime | `current_thread` + `io-util` + `net` + `signal` features only. No `rt-multi-thread`. |
| `nix` | Unix syscall wrappers | Safe wrappers for `openpty`, `fork`, `ioctl`, `setsid`, signal handling |
| `clap` | CLI argument parsing | Derive-based, compile-time validated |
| `rmp-serde` | MessagePack serialization | Wire protocol encoding/decoding |
| `serde` / `serde_json` | Serialization framework | JSON output for `--json` flags |
| `ratatui` | TUI framework | Interactive mode only |
| `crossterm` | Terminal manipulation | Raw mode, input events, for TUI and attach |
| `toml` | Config file parsing | Configuration |
| `getrandom` | Random ID generation | Session IDs |

**Not included:**
- No `tracing` / `log` — daemon logs to a file via simple `eprintln!` redirected at startup. Logging frameworks add ~200KB to binary size for negligible benefit in a single-purpose daemon.
- No `anyhow` / `thiserror` — custom error enum with `Display` impl. Keeps error handling explicit and binary small.
- No `async-trait` — use `impl Future` returns or manual trait object boxing where needed.

### 12.1. Build Configuration

```toml
[profile.release]
opt-level = "z"         # optimize for size
lto = true              # link-time optimization
codegen-units = 1       # single codegen unit for maximum optimization
panic = "abort"         # no unwinding, saves ~10KB
strip = true            # strip symbols
```

Target: `x86_64-unknown-linux-musl` for fully static binary. Expected binary size: 2–4 MB.

---

## 13. Error Handling

### 13.1. Error Types

```rust
enum SnagError {
    // Daemon errors
    SessionNotFound(String),
    SessionNameConflict(String),
    SessionAmbiguousTarget(String, Vec<String>),
    SessionExited(String, i32),
    DaemonNotRunning,
    DaemonStartFailed(String),

    // Adoption errors
    AdoptionFailed(String),
    KernelTooOld { required: &'static str, found: String },
    PermissionDenied(String),

    // System errors
    Io(std::io::Error),
    Nix(nix::Error),

    // Protocol errors
    ProtocolError(String),
    ConnectionLost,
}
```

All errors produce human-readable messages. No stack traces in release builds. Exit codes follow sysexits conventions where applicable.

---

## 14. Testing Strategy

### 14.1. Unit Tests

- Ring buffer: write/read/wrap correctness, line counting accuracy
- Protocol: encode/decode roundtrip for all message types
- Name resolution: exact match, prefix match, ambiguity detection
- Config: parsing, defaults, override precedence

### 14.2. Integration Tests

- Spawn session → send command → read output → kill
- Multiple clients attach simultaneously
- Daemon auto-start on first command
- Daemon auto-stop after last session exits
- Session survives client disconnect
- `snag scan` discovers sessions from other terminal emulators
- `snag adopt` successfully adopts and interacts with discovered session
- JSON output parseable by `jq`
- Large output (1 MB+) flows without blocking or data loss

### 14.3. Stress Tests

- 100 concurrent sessions, each producing continuous output
- 10 clients attached to the same session
- Rapid create/destroy cycles (1000 sessions in 10 seconds)
- Scrollback buffer wrap-around correctness under sustained write pressure

---

## 15. Project Structure

```
snag/
├── Cargo.toml
├── src/
│   ├── main.rs              # entry point, CLI dispatch
│   ├── cli/
│   │   ├── mod.rs           # clap definitions
│   │   ├── commands.rs      # individual command handlers
│   │   └── output.rs        # JSON/human output formatting
│   ├── daemon/
│   │   ├── mod.rs           # daemon entry point, event loop
│   │   ├── server.rs        # Unix socket listener, connection handling
│   │   ├── session.rs       # Session struct, spawn, adopt
│   │   ├── registry.rs      # session registry (HashMap + name index)
│   │   ├── pty.rs           # PTY operations (openpty, forkpty, ioctl)
│   │   ├── adopt.rs         # scan + pidfd_getfd adoption logic
│   │   └── ringbuf.rs       # scrollback ring buffer
│   ├── protocol/
│   │   ├── mod.rs           # message types, framing
│   │   ├── codec.rs         # encode/decode (MessagePack + raw)
│   │   └── types.rs         # shared request/response types
│   ├── tui/
│   │   ├── mod.rs           # TUI entry point
│   │   ├── app.rs           # application state
│   │   └── ui.rs            # ratatui rendering
│   ├── client.rs            # daemon connection, request/response helpers
│   ├── config.rs            # config file parsing
│   └── error.rs             # SnagError enum
└── tests/
    ├── integration/
    │   ├── spawn_test.rs
    │   ├── attach_test.rs
    │   ├── adopt_test.rs
    │   └── daemon_test.rs
    └── stress/
        └── concurrent_test.rs
```

---

## 16. Implementation Phases

### Phase 1 — Core (MVP)

**Goal:** spawned sessions work end-to-end.

- Daemon: spawn sessions, read/write PTY I/O, scrollback buffer
- Protocol: framed Unix socket communication
- CLI: `new`, `list`, `kill`, `send`, `output`, `attach`
- Daemon auto-start/stop
- Config file

**Deliverable:** a user can `snag new`, `snag send`, `snag attach`, `snag kill`.

### Phase 2 — Discovery & Adoption

**Goal:** adopt existing sessions.

- `snag scan` — `/proc` walking, PTY master fd detection
- `snag adopt` — `pidfd_getfd` implementation + ptrace fallback
- `snag ps`, `snag cwd` — process introspection
- `snag info`, `snag rename`
- Session name resolution (prefix matching)

**Deliverable:** a user can snag any shell on their machine.

### Phase 3 — TUI & Polish

**Goal:** interactive mode, UX polish.

- TUI with ratatui: session list, preview pane, quick actions
- `--json` output for all commands
- `--read-only` attach mode
- `--follow` mode for `snag output`
- Escape sequence detection for detach
- Multi-client attach (fan-out output, conflict-free input)

**Deliverable:** full PRD feature set.

### Phase 4 — Hardening

**Goal:** production-grade reliability.

- Stress testing (100+ concurrent sessions)
- Edge case handling (PTY EOF races, partial writes, SIGCHLD coalescing)
- Memory profiling and optimization
- Binary size optimization
- Man page generation
- Shell completions (`bash`, `zsh`, `fish`)
- CI: cross-compile for musl, run integration tests
- Packaging: `cargo install`, AUR, `.deb`
