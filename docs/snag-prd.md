# Snag — Product Requirements Document

**Version:** 0.1
**Status:** Draft

---

## 1. Vision

Snag is a single CLI binary that lets you spawn, discover, attach to, and interact with any shell session on your machine — interactively or programmatically.

You open a shell somewhere. From anywhere else, you snag it. You send it commands, read its output, resize it, name it, kill it. You do this from another terminal, from a script, from another tool that shells out to `snag`. That's it.

Snag is not a terminal emulator. It is not a library. It does not do networking. It is a local PTY multiplexer exposed as a CLI tool — a building block that other projects can depend on the way they depend on `git` or `jq`.

---

## 2. Problem Statement

There is currently no simple, standalone tool that lets you:

- Spawn a named shell session that persists beyond the spawning terminal
- Discover and interact with existing shell sessions on the machine
- Programmatically send commands to a session and read its output, without attaching to it
- Attach multiple viewers to a single session simultaneously
- Do all of the above through a single binary with both human-friendly (TUI) and machine-friendly (CLI + JSON) interfaces

`tmux` and `screen` are the closest tools but they are full terminal multiplexers with their own windowing systems, keybinding layers, configuration languages, and conceptual overhead. They solve a much bigger problem. Snag solves a smaller, sharper one.

---

## 3. Target Users

- **Power users** who want fast, named, persistent shell sessions they can jump between
- **CLI tool authors** who need to programmatically drive shell sessions (send commands, read output, manage lifecycle)
- **Automation pipelines** that need to orchestrate multiple concurrent shell sessions
- **Other projects** that need a local session multiplexer as a dependency (remote shell apps, AI-assisted coding tools, mobile terminal clients, etc.)

---

## 4. Core Concepts

### Session

A session is a PTY-backed shell process managed by Snag. Each session has:

- **ID** — auto-generated unique identifier
- **Name** — optional human-friendly label (unique, mutable)
- **Shell** — the shell binary running in the session (`zsh`, `bash`, `sh`, etc.)
- **CWD** — current working directory of the shell process
- **State** — `running`, `exited` (with exit code)
- **Created at** — timestamp
- **Scrollback** — a ring buffer of recent output (configurable size)
- **Foreground process** — what's currently running in the shell (if detectable)
- **Attached clients** — how many viewers are currently attached

### Spawned vs. Adopted Sessions

**Spawned sessions** are created by Snag. Snag owns the PTY master fd and has full control: read, write, resize, kill.

**Adopted sessions** are pre-existing shell sessions on the machine that Snag discovers and takes control of. Snag locates the PTY master file descriptor held by the session's parent process (typically the terminal emulator), duplicates it via `pidfd_getfd()` (Linux 5.6+) or equivalent mechanism, and from that point has full read/write/resize control — functionally identical to a spawned session. Adoption requires same-user ownership (or root) and is subject to OS-level ptrace/pidfd permissions. Adopted sessions are a **core feature** — being able to snag any existing shell on the machine is fundamental to Snag's value proposition.

---

## 5. CLI Interface

### 5.1. Session Lifecycle

```
snag new [--name <n>] [--shell <shell>] [--cwd <path>]
```
Spawn a new session. Defaults to the user's default shell and current directory. Returns the session ID. Does **not** attach — the session runs in the background.

```
snag kill <id|name>
```
Kill a session. Sends SIGHUP to the shell process and cleans up.

```
snag rename <id|name> <new-name>
```
Rename a session.

---

### 5.2. Session Discovery

```
snag list [--json] [--all]
```
List all Snag-managed sessions. Shows ID, name, shell, CWD, state, foreground process, and attached client count. `--all` includes adopted (discovered) sessions. `--json` outputs machine-readable JSON.

```
snag info <id|name> [--json]
```
Detailed information about a single session.

```
snag scan
```
Discover existing PTY sessions on the machine that Snag could potentially adopt. Shows PTS device, PID, user, command, CWD, and adoption feasibility.

```
snag adopt <pts|pid> [--name <n>]
```
Adopt an existing session discovered via `snag scan`. Snag duplicates the PTY master fd and from that point manages the session like any other. Optionally name it on adoption. Once adopted, the session appears in `snag list` and supports all interaction commands (attach, send, output, etc.).

---

### 5.3. Session Interaction

```
snag attach <id|name> [--read-only]
```
Attach the current terminal to a session. You see its output, your keystrokes go to it, your terminal size is forwarded. Multiple clients can attach simultaneously. Detach with a configurable escape sequence (default: `Ctrl+\` double-tap). `--read-only` disables input forwarding.

```
snag send <id|name> <command>
```
Send a line of input to a session without attaching. The command string is written to the PTY followed by a newline. Fire-and-forget from Snag's perspective — the shell executes it asynchronously.

```
snag output <id|name> [--lines <n>] [--follow] [--json]
```
Read recent output from a session's scrollback buffer. `--lines` controls how many lines (default: all available in buffer). `--follow` streams new output in real-time to stdout (like `tail -f`). `--json` wraps output with metadata (timestamp, session info).

```
snag cwd <id|name>
```
Print the current working directory of the session's shell process. Shortcut for a common query.

```
snag ps <id|name>
```
Print the foreground process tree of the session. Useful to check what's running before sending commands.

---

### 5.4. Interactive Mode (TUI)

```
snag
```
Launched with no subcommand, Snag enters interactive mode — a lightweight TUI that shows:

- A list of all sessions with their name, state, CWD, and foreground process
- Quick actions: attach (Enter), new session (n), kill (k), rename (r), send command (s)
- A preview pane showing the last N lines of the selected session's output
- Ability to toggle showing adopted sessions

The TUI is a convenience layer over the same operations available via subcommands. It does not replace the programmatic interface — it complements it for human navigation.

---

### 5.5. Global Options

```
snag --socket <path>    # Override the default socket path
snag --scrollback <n>   # Override scrollback buffer size (lines)
```

---

## 6. Programmatic Usage Patterns

Snag's CLI is designed to be composed. Some patterns other tools would use:

**Fire a command and capture output:**
```bash
snag send myproject "cargo test"
sleep 5
snag output myproject --lines 100
```

**Poll for a process to finish:**
```bash
while snag ps myproject | grep -q "cargo"; do
  sleep 1
done
echo "Build finished"
```

**Spin up a named session, use it, tear it down:**
```bash
SESSION=$(snag new --name ci-runner --cwd /home/user/project)
snag send ci-runner "make build && make test"
# ... later ...
snag kill ci-runner
```

**Get structured data:**
```bash
snag list --json | jq '.[] | select(.name == "dev") | .cwd'
```

---

## 7. Session Persistence

Spawned sessions must survive the terminal that created them. If you run `snag new`, close your terminal, open a new one, and run `snag list` — the session is still there.

This implies a lightweight background process that holds the PTY master file descriptors. This process is an **implementation detail**, not a user-facing concept. It is started automatically on first use and stopped when the last session exits (or can be managed via `snag daemon stop/start/status` if needed, but this is a low-priority escape hatch, not the primary workflow).

---

## 8. Configuration

Minimal configuration, sensible defaults. A config file (`~/.config/snag/config.toml` or similar) may allow overriding:

- Default shell
- Scrollback buffer size (default: 10,000 lines)
- Socket path
- Escape sequence for detach
- Default `--all` behavior (whether to always show adopted sessions)

Configuration is optional. Snag works out of the box with zero config.

---

## 9. Non-Goals

The following are explicitly **out of scope** for Snag:

- **Networking** — no TCP, no WebSocket, no SSH, no remote anything. Snag is local-only. Projects that want remote access build it on top of Snag.
- **Authentication / authorization** — the Unix socket is protected by filesystem permissions. That's it.
- **Terminal emulation / rendering** — Snag passes raw PTY bytes. It does not interpret escape sequences, render colors, or provide a terminal widget. That's the consumer's job.
- **Window management** — no splits, no panes, no tabs, no layouts. Snag manages sessions, not screen real estate.
- **Library / crate** — Snag is not designed to be embedded as a Rust dependency. It is a standalone binary. Other Rust projects interact with it by invoking the CLI or connecting to the Unix socket directly if they choose to (the protocol is simple enough), but this is their concern, not Snag's.
- **Shell integration** — no prompt injection, no shell functions, no PS1 manipulation. Snag is shell-agnostic.
- **Plugin system** — no hooks, no extensions, no scripting. Snag is a focused tool.

---

## 10. Naming

The name is **Snag** — as in "snag a session." Short, memorable, verb-oriented. No known conflicts with existing CLI tools, crates.io packages, npm packages, or system binaries.

The binary name is `snag`. The background process (if visible) is `snagd`.

---

## 11. Success Criteria

Snag is successful when:

1. A user can spawn a named session, close their terminal, reopen it, and reattach — in under 3 seconds total interaction time
2. A script can create a session, send 10 commands, read output, and tear it down — with no human interaction
3. Two terminals can attach to the same session simultaneously with real-time output sync
4. `snag list --json` returns structured data that any tool can parse
5. A user can `snag scan`, adopt an existing shell session, and interact with it — with the same capabilities as a Snag-spawned session
6. A user unfamiliar with Snag can understand and use it within 5 minutes, without reading docs beyond `snag --help`

---

## 12. Future Considerations

These are not in scope but are anticipated downstream uses that validate Snag's design:

- A **WebSocket bridge** binary that exposes Snag sessions over the network (separate project)
- A **mobile terminal app** that connects to the bridge and renders sessions with xterm.js (separate project)
- An **AI coding assistant** (Claude Code or similar) that uses `snag send` / `snag output` to drive multiple project sessions (separate project / integration)
- A **TUI dashboard** that shows real-time output from multiple Snag sessions side by side (could be a separate project or a Snag enhancement)
- **Session groups** — logically grouping sessions by project (potential future Snag feature)
- **Session recording / replay** — `script`-like capture of a session's full history (potential future Snag feature)
