<p align="center">
  <strong>Spawn, attach to, and interact with any shell session on your machine.</strong>
</p>

<p align="center">
  <a href="#installation">Installation</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#commands">Commands</a> &bull;
  <a href="#programmatic-usage">Programmatic Usage</a> &bull;
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

---

## What is snag?

snag is a local PTY session multiplexer. You open a shell somewhere. From anywhere else, you snag it -- send it commands, read its output, resize it, name it, kill it. You do this from another terminal, from a script, from another tool that shells out to `snag`.

snag is not a terminal emulator. It is not a library. It does not do networking. It is a local PTY multiplexer exposed as a CLI tool -- a building block that other projects can depend on the way they depend on `git` or `jq`.

### Why not tmux?

tmux and screen are full terminal multiplexers with their own windowing systems, keybinding layers, configuration languages, and conceptual overhead. They solve a much bigger problem. Snag solves a smaller, sharper one:

- **Named persistent sessions** -- spawn a session, close your terminal, reopen, reattach
- **Programmatic control** -- send commands and read output without attaching
- **Shell hook integration** -- automatically register existing shell sessions
- **Multi-client attach** -- multiple terminals viewing the same session simultaneously
- **Machine-friendly output** -- `--json` for everything

## Installation

### Homebrew (Linux)

```bash
brew tap moukrea/tap
brew install snag
```

### Debian / Ubuntu

```bash
# Add GPG key
curl -fsSL https://moukrea.github.io/apt-repo/pubkey.gpg | sudo gpg --dearmor -o /usr/share/keyrings/moukrea.gpg

# Add repository
echo "deb [signed-by=/usr/share/keyrings/moukrea.gpg] https://moukrea.github.io/apt-repo stable main" | \
  sudo tee /etc/apt/sources.list.d/moukrea.list

# Install
sudo apt update && sudo apt install snag
```

### Fedora / RHEL

```bash
# Import GPG key and add repository
sudo rpm --import https://moukrea.github.io/rpm-repo/pubkey.gpg
sudo tee /etc/yum.repos.d/moukrea.repo << 'EOF'
[moukrea]
name=moukrea Repository
baseurl=https://moukrea.github.io/rpm-repo/
gpgcheck=0
repo_gpgcheck=1
gpgkey=https://moukrea.github.io/rpm-repo/pubkey.gpg
enabled=1
EOF

# Install
sudo dnf install snag
```

### Arch Linux

Download the `PKGBUILD` from the
[latest release](https://github.com/moukrea/snag/releases/latest) and build:

```bash
makepkg -si
```

### Pre-built Binaries

Download the archive for your platform from the
[latest release](https://github.com/moukrea/snag/releases/latest):

| Platform | Architecture | Archive |
|----------|-------------|---------|
| Linux | x86_64 | `snag-<version>-linux-x86_64.tar.gz` |
| Linux | aarch64 | `snag-<version>-linux-aarch64.tar.gz` |

Extract and copy the binary to your `PATH`:

```bash
tar xzf snag-<version>-linux-<arch>.tar.gz
sudo mv snag /usr/local/bin/
```

### From Source

Requires the [Rust toolchain](https://rustup.rs/) (stable).

```bash
git clone https://github.com/moukrea/snag.git
cd snag
cargo build --release
```

The binary is at `target/release/snag`. Copy it somewhere on your `PATH`.

### Requirements

- Linux (kernel 5.6+ for shell hook registration via `pidfd_getfd`)
- `kernel.yama.ptrace_scope` set to `0` (required for PTY adoption via `pidfd_getfd`)

Ubuntu and some other distributions set `ptrace_scope=1` by default, which prevents snag from adopting shell sessions. Check your current setting:

```bash
cat /proc/sys/kernel/yama/ptrace_scope
```

If the value is not `0`, fix it:

```bash
# Temporary (until reboot)
sudo sysctl kernel.yama.ptrace_scope=0

# Permanent
echo 'kernel.yama.ptrace_scope=0' | sudo tee /etc/sysctl.d/99-snag-ptrace.conf
sudo sysctl -p /etc/sysctl.d/99-snag-ptrace.conf
```

> **Note:** Setting `ptrace_scope=0` allows any process to ptrace other processes owned by the same user. This is the default on many distributions but Ubuntu enables stricter settings. See the [kernel docs](https://www.kernel.org/doc/Documentation/security/Yama.txt) for details.

## Quick Start

There are two ways to use snag: **explicit sessions** and **shell hook integration**.

### Explicit sessions

```bash
# 1. Spawn a named session
snag new --name dev

# 2. Send a command
snag send dev "echo hello from snag"

# 3. Read the output
snag output dev --lines 5

# 4. Attach interactively (Ctrl+q double-tap to detach)
snag attach dev

# 5. List all sessions
snag list

# 6. Kill the session
snag kill dev
```

### Shell hook (automatic registration)

Add this to your `.bashrc` or `.zshrc`:

```bash
eval "$(snag hook bash)"
```

Or for zsh:

```zsh
eval "$(snag hook zsh)"
```

This automatically registers every new shell with the snag daemon. When you open a terminal, the shell is registered and its output is captured. When you close the terminal, the session is unregistered cleanly.

If `snag list` shows no sessions after adding the hook, check that `kernel.yama.ptrace_scope` is set to `0` (see [Requirements](#requirements)). The hook silently skips registration when PTY adoption is blocked.

Once registered, you can interact with the shell from anywhere:

```bash
# List all sessions (spawned and registered)
snag list

# Search across all session output
snag grep "error"

# Read output from a registered shell
snag output <id> --lines 20
```

## Commands

### Session Lifecycle

| Command | Description |
|---------|-------------|
| `snag new [--name N] [--shell S] [--cwd P]` | Spawn a new session (returns session ID) |
| `snag kill <id\|name>` | Kill a session |
| `snag rename <id\|name> <new-name>` | Rename a session |

### Shell Hook Integration

| Command | Description |
|---------|-------------|
| `snag hook <shell>` | Print shell hook code (bash, zsh) |
| `snag register [--name N]` | Register the current shell (called by the hook) |
| `snag unregister <id>` | Unregister a shell session (called by EXIT trap) |

### Session Discovery

| Command | Description |
|---------|-------------|
| `snag list [--json]` | List all managed sessions |
| `snag info <id\|name> [--json]` | Detailed session information |
| `snag grep <pattern> [--json]` | Search session output for a pattern |

### Session Interaction

| Command | Description |
|---------|-------------|
| `snag attach <id\|name> [--read-only]` | Attach to a session (detach: Ctrl+q or Ctrl+\ double-tap) |
| `snag send <id\|name> <command>` | Send a command without attaching |
| `snag output <id\|name> [--lines N] [--follow] [--json]` | Read session output |
| `snag cwd <id\|name>` | Print the session's current working directory |
| `snag ps <id\|name>` | Print the session's foreground process tree |

### Interactive Mode

| Command | Description |
|---------|-------------|
| `snag` | Launch the TUI (session list, preview, quick actions) |

### Daemon Management

| Command | Description |
|---------|-------------|
| `snag daemon start` | Start the daemon (auto-started on first use) |
| `snag daemon stop` | Stop the daemon |
| `snag daemon status` | Show daemon status |

## Programmatic Usage

snag's CLI is designed to be composed. Some patterns:

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
snag list --json | jq '.sessions[] | select(.name == "dev") | .cwd'
```

## Configuration

Config file: `~/.config/snag/config.toml` (optional -- snag works with zero config)

```toml
# Default shell (default: $SHELL or /bin/sh)
shell = "/bin/zsh"

# Scrollback buffer size in bytes (default: 1048576 = 1MB)
scrollback_bytes = 1048576

# Detach escape sequence (default: Ctrl+q or Ctrl+\ double-tap within 500ms)
detach_key = "ctrl-\\"
detach_timeout_ms = 500

# Daemon grace period before auto-exit in seconds (default: 30)
daemon_grace_period = 30
```

## License

[MIT](LICENSE)
