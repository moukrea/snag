# Contributing to snag

Thank you for your interest in contributing to snag. This guide covers the development workflow and conventions used in the project.

## Prerequisites

- [Rust](https://rustup.rs/) stable toolchain
- Linux (snag uses Linux-specific APIs: PTY, `/proc`, `pidfd_getfd`)

## Building

```bash
cd snag/

# Debug build
cargo build

# Release build
cargo build --release
```

## Testing

```bash
# Run all tests
cargo test

# Run a specific test
cargo test <test_name>

# Run tests with stdout visible
cargo test -- --nocapture
```

## Code Quality

```bash
# Lint (warnings treated as errors)
cargo clippy -- -D warnings

# Check formatting
cargo fmt --check

# Auto-format
cargo fmt
```

All three checks must pass before a PR will be merged.

## Project Structure

```
snag/
├── src/
│   ├── main.rs              # Binary entry point, CLI dispatch
│   ├── cli/
│   │   ├── mod.rs           # Clap command definitions
│   │   ├── commands.rs      # Individual command handlers
│   │   └── output.rs        # JSON/human output formatting
│   ├── daemon/
│   │   ├── mod.rs           # Daemon module root
│   │   ├── server.rs        # Unix socket listener, event loop
│   │   ├── session.rs       # Session struct, spawn, register
│   │   ├── registry.rs      # Session registry (HashMap + name index)
│   │   ├── pty.rs           # PTY operations (openpty, fork, ioctl)
│   │   ├── adopt.rs         # Scan + pidfd_getfd for shell hook registration
│   │   └── ringbuf.rs       # Scrollback ring buffer
│   ├── protocol/
│   │   ├── mod.rs           # Protocol module root
│   │   ├── codec.rs         # Encode/decode (MessagePack + raw)
│   │   └── types.rs         # Shared request/response types
│   ├── tui/
│   │   ├── mod.rs           # TUI entry point
│   │   ├── app.rs           # Application state
│   │   └── ui.rs            # Ratatui rendering
│   ├── client.rs            # Daemon connection, request/response helpers
│   ├── config.rs            # Config file parsing
│   └── error.rs             # SnagError enum
└── tests/
```

## Conventions

### Session names

- Alphanumeric with dots, underscores, hyphens, spaces: `[a-zA-Z0-9._- ]`
- Maximum 64 characters
- Examples: `dev`, `ci-runner`, `staging.deploy`, `my project`

### Error handling

- All errors go to stderr with actionable messages
- Exit codes: `0` success, `1` general error
- Custom error enum with manual `Display` impl (no `thiserror` / `anyhow`)

### Wire protocol

- Binary protocol over Unix domain socket
- Framing: `type(u8) + len(u32 LE) + payload`
- Structured messages use MessagePack; PTY data is raw bytes

## Commit Messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`

Examples:

```
feat(daemon): add session auto-cleanup on exit
fix(attach): handle terminal resize during detach
docs(readme): add installation instructions
```

## Pull Requests

1. Fork the repository and create a branch from `main`
2. Make your changes
3. Ensure `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check` all pass
4. Write a clear PR description explaining what changed and why
5. Submit the PR

## Architecture Notes

Key design constraints:

- **Single-threaded async.** The daemon uses `tokio` with `current_thread` runtime. No multi-threading, no synchronization overhead.
- **Daemon holds all PTY fds.** Session state is owned by the daemon process. Client crash never affects sessions.
- **Zero-copy where possible.** PTY bytes flow from kernel fd to client fd with minimal intermediate buffering.
- **Linux-only.** Snag uses Linux-specific APIs (`pidfd_getfd`, `/proc` filesystem) and does not target other platforms.
