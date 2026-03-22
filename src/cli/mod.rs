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
        /// Include adopted sessions
        #[arg(long)]
        all: bool,
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
    /// Scan for adoptable PTY sessions
    Scan,
    /// Adopt an existing PTY session
    Adopt {
        /// PTS device number or PID
        pts_or_pid: String,
        /// Session name
        #[arg(long)]
        name: Option<String>,
    },
    /// Manage the daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
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
