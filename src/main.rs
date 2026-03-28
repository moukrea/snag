mod cli;
mod client;
mod config;
mod daemon;
mod error;
mod protocol;
mod tui;

use clap::Parser;
use cli::{Cli, Command, DaemonAction};
use config::Config;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Cli::parse();
    let mut config = Config::load();

    // Apply CLI overrides
    if let Some(socket) = args.socket {
        config.socket = Some(socket);
    }
    if let Some(scrollback) = args.scrollback {
        config.scrollback_bytes = scrollback;
    }

    let result = match args.command {
        None => tui::run_tui(&config).await,
        Some(Command::New { name, shell, cwd }) => {
            cli::commands::cmd_new(&config, shell, name, cwd).await
        }
        Some(Command::Kill { target }) => cli::commands::cmd_kill(&config, target).await,
        Some(Command::Rename { target, new_name }) => {
            cli::commands::cmd_rename(&config, target, new_name).await
        }
        Some(Command::List { json }) => cli::commands::cmd_list(&config, json).await,
        Some(Command::Info { target, json }) => {
            cli::commands::cmd_info(&config, target, json).await
        }
        Some(Command::Attach {
            target,
            read_only,
            force,
        }) => cli::commands::cmd_attach(&config, target, read_only, force).await,
        Some(Command::Send { target, command }) => {
            cli::commands::cmd_send(&config, target, command).await
        }
        Some(Command::Output {
            target,
            lines,
            follow,
            json,
        }) => cli::commands::cmd_output(&config, target, lines, follow, json).await,
        Some(Command::Cwd { target }) => cli::commands::cmd_cwd(&config, target).await,
        Some(Command::Ps { target }) => cli::commands::cmd_ps(&config, target).await,
        Some(Command::Grep {
            pattern,
            sessions_only,
            last,
            json,
        }) => cli::commands::cmd_grep(&config, pattern, sessions_only, last, json).await,
        Some(Command::Hook { shell }) => cli::commands::cmd_hook(&shell),
        Some(Command::Register { pid, name }) => {
            cli::commands::cmd_register(&config, pid, name).await
        }
        Some(Command::Unregister { target }) => {
            cli::commands::cmd_unregister(&config, target).await
        }
        Some(Command::Wrap { capture }) => cli::commands::cmd_wrap(&capture),
        Some(Command::Daemon { action }) => match action {
            DaemonAction::Start => cli::commands::cmd_daemon_start(&config).await,
            DaemonAction::Stop => cli::commands::cmd_daemon_stop(&config).await,
            DaemonAction::Status => cli::commands::cmd_daemon_status(&config).await,
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
