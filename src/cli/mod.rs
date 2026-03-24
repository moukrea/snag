pub mod commands;
pub mod output;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "snag", about = "Snag shell sessions", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Override the default socket path
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,

    /// Override scrollback buffer size (bytes)
    #[arg(long, global = true)]
    pub scrollback: Option<usize>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Spawn a new session
    New {
        /// Session name
        #[arg(long)]
        name: Option<String>,
        /// Shell binary
        #[arg(long)]
        shell: Option<String>,
        /// Working directory
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Kill a session
    Kill {
        /// Session ID or name
        target: String,
    },
    /// Rename a session
    Rename {
        /// Session ID or name
        target: String,
        /// New name
        new_name: String,
    },
    /// List sessions
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show session details
    Info {
        /// Session ID or name
        target: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Attach to a session
    Attach {
        /// Session ID or name
        target: String,
        /// Read-only mode
        #[arg(long)]
        read_only: bool,
    },
    /// Send a command to a session
    Send {
        /// Session ID or name
        target: String,
        /// Command to send
        command: String,
    },
    /// Read session output
    Output {
        /// Session ID or name
        target: String,
        /// Number of lines
        #[arg(long)]
        lines: Option<u32>,
        /// Follow output
        #[arg(long)]
        follow: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print session working directory
    Cwd {
        /// Session ID or name
        target: String,
    },
    /// Print session foreground processes
    Ps {
        /// Session ID or name
        target: String,
    },
    /// Search session output for a pattern
    Grep {
        /// Pattern to search for
        pattern: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print shell hook code for automatic session registration
    Hook {
        /// Shell type (bash or zsh)
        shell: String,
    },
    /// Register the current shell session with the daemon (called by hook)
    Register {
        /// Shell PID (passed by the hook via $$)
        #[arg(long)]
        pid: Option<u32>,
        /// Session name
        #[arg(long)]
        name: Option<String>,
    },
    /// Unregister a shell session from the daemon (called by EXIT trap)
    Unregister {
        /// Session ID
        target: String,
    },
    /// Manage the daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// PTY proxy for shell hook (internal — called via exec from hook)
    #[command(hide = true)]
    Wrap {
        /// Capture file path
        #[arg(long)]
        capture: String,
    },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Start the daemon
    Start,
    /// Stop the daemon
    Stop,
    /// Show daemon status
    Status,
}
