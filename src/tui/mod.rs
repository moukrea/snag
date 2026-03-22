pub mod app;
pub mod ui;

use crate::client::DaemonClient;
use crate::config::Config;
use crate::error::Result;
use crate::protocol::*;
use app::App;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

pub async fn run_tui(config: &Config) -> Result<()> {
    let mut client = DaemonClient::connect(config).await?;

    // Setup terminal
    terminal::enable_raw_mode()?;
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.show_adopted = config.show_adopted;

    // Initial session load
    refresh_sessions(&mut client, &mut app).await?;
    refresh_preview(&mut client, &mut app).await?;

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        if event::poll(std::time::Duration::from_millis(500))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        app.should_quit = true;
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.should_quit = true;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.select_next();
                        refresh_preview(&mut client, &mut app).await?;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.select_prev();
                        refresh_preview(&mut client, &mut app).await?;
                    }
                    KeyCode::Enter => {
                        if let Some(id) = app.selected_id() {
                            // Exit TUI, attach to session
                            cleanup_terminal(&mut terminal)?;
                            crate::cli::commands::cmd_attach(config, id, false).await?;
                            // Re-enter TUI after detach
                            terminal::enable_raw_mode()?;
                            crossterm::execute!(
                                io::stdout(),
                                crossterm::terminal::EnterAlternateScreen
                            )?;
                            terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
                            refresh_sessions(&mut client, &mut app).await?;
                            refresh_preview(&mut client, &mut app).await?;
                        }
                    }
                    KeyCode::Char('n') => {
                        // Reconnect client if needed and create new session
                        client = DaemonClient::connect(config).await?;
                        let _ = client
                            .request(&Request::SessionNew {
                                shell: None,
                                name: None,
                                cwd: None,
                            })
                            .await?;
                        refresh_sessions(&mut client, &mut app).await?;
                        refresh_preview(&mut client, &mut app).await?;
                    }
                    KeyCode::Char('x') | KeyCode::Char('K') => {
                        if let Some(id) = app.selected_id() {
                            client = DaemonClient::connect(config).await?;
                            let _ = client.request(&Request::SessionKill { target: id }).await;
                            refresh_sessions(&mut client, &mut app).await?;
                            refresh_preview(&mut client, &mut app).await?;
                        }
                    }
                    KeyCode::Char('a') => {
                        app.show_adopted = !app.show_adopted;
                        refresh_sessions(&mut client, &mut app).await?;
                    }
                    _ => {}
                }
            }
        } else {
            // Periodic refresh
            let _ = refresh_sessions(&mut client, &mut app).await;
        }

        if app.should_quit {
            break;
        }
    }

    cleanup_terminal(&mut terminal)?;
    Ok(())
}

fn cleanup_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    terminal::disable_raw_mode()?;
    crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

async fn refresh_sessions(client: &mut DaemonClient, app: &mut App) -> Result<()> {
    // Reconnect if needed
    let config = Config::load();
    *client = DaemonClient::connect(&config).await?;

    let resp = client
        .request(&Request::SessionList {
            all: app.show_adopted,
        })
        .await?;

    if let Response::Ok(ResponseData::SessionList(sessions)) = resp {
        app.sessions = sessions;
        if app.selected >= app.sessions.len() && !app.sessions.is_empty() {
            app.selected = app.sessions.len() - 1;
        }
    }
    Ok(())
}

async fn refresh_preview(client: &mut DaemonClient, app: &mut App) -> Result<()> {
    app.preview_lines.clear();

    let Some(session) = app.selected_session() else {
        return Ok(());
    };

    let target = session.id.clone();
    let config = Config::load();
    *client = DaemonClient::connect(&config).await?;

    let resp = client
        .request(&Request::SessionOutput {
            target,
            lines: Some(20),
            follow: false,
        })
        .await?;

    if let Response::Ok(ResponseData::Output(text)) = resp {
        app.preview_lines = text.lines().map(|l| l.to_string()).collect();
    }
    Ok(())
}
