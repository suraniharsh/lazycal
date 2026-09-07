use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{event, execute};
use lazycal::app::{App, SyncStatus};
use lazycal::sync::SyncReport;
use lazycal::{config, db, google, sync, ui};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tracing::Level;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// How long to block on input before looping to check on the background sync.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[tokio::main]
async fn main() -> Result<()> {
    let paths = config::resolve()?;
    init_logging(&paths);

    if !paths.client_secret.exists() {
        eprintln!(
            "lazycal: no Google credentials at {}.\n\
             Showing cached events only — see the setup steps in the README.",
            paths.client_secret.display()
        );
    } else if !paths.token_cache.exists() {
        // yup-oauth2 prints the consent URL to stdout, which the alternate
        // screen would swallow, so first-time authorization has to happen
        // before the TUI takes over the terminal.
        println!("lazycal: first run, authorizing with Google…");
        match perform_sync(&paths).await {
            Ok(report) => println!("lazycal: synced {} calendar(s).", report.synced),
            Err(err) => eprintln!("lazycal: initial sync failed: {err:#}"),
        }
    }

    let mut terminal = init_terminal()?;
    let outcome = run(&mut terminal, &paths);
    let restored = restore_terminal(&mut terminal);
    // Report the run's failure ahead of any problem restoring the terminal.
    outcome.and(restored)
}

fn init_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(err) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(err.into());
    }

    // Without this a panic would leave the terminal in raw mode on the
    // alternate screen, and the user's shell unusable.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// The TUI owns stdout, so `tracing` output goes to a file instead.
fn init_logging(paths: &config::Paths) {
    let Some(dir) = paths.database.parent() else {
        return;
    };
    let Ok(file) = std::fs::File::create(dir.join("lazycal.log")) else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_max_level(Level::INFO)
        .try_init();
}

fn run(terminal: &mut Term, paths: &config::Paths) -> Result<()> {
    // Renders from whatever is cached so startup never waits on the network.
    let mut app = App::open(&paths.database)?;
    let configured = paths.client_secret.exists();
    if configured {
        spawn_background_sync(paths.clone(), app.sync_status_handle());
    } else {
        app.set_sync_status(SyncStatus::NotConfigured);
    }

    let mut last_status = app.sync_status();
    terminal.draw(|frame| ui::draw(frame, &app))?;

    while !app.should_quit {
        let mut redraw = false;

        if event::poll(POLL_INTERVAL)? {
            match event::read()? {
                event::Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                    app.on_key(key);
                    redraw = true;
                }
                event::Event::Resize(..) => redraw = true,
                _ => {}
            }
        }

        if app.take_refresh_request() && configured {
            app.set_sync_status(SyncStatus::Syncing);
            spawn_background_sync(paths.clone(), app.sync_status_handle());
            redraw = true;
        }

        // A finished sync wrote through its own connection, so reload the
        // calendar list to pick up anything added, renamed or removed.
        let status = app.sync_status();
        if status != last_status {
            last_status = status;
            app.reload_calendars();
            redraw = true;
        }

        if redraw {
            terminal.draw(|frame| ui::draw(frame, &app))?;
        }
    }
    Ok(())
}

fn spawn_background_sync(paths: config::Paths, status: Arc<Mutex<SyncStatus>>) {
    tokio::spawn(async move {
        let next = match perform_sync(&paths).await {
            Ok(report) if report.is_complete() => SyncStatus::Synced,
            Ok(report) => {
                tracing::warn!(
                    "these calendars failed to sync: {}",
                    report.failed.join(", ")
                );
                SyncStatus::Partial
            }
            Err(err) => {
                tracing::error!("sync failed: {err:#}");
                SyncStatus::Failed
            }
        };
        *status.lock().expect("sync status lock poisoned") = next;
    });
}

async fn perform_sync(paths: &config::Paths) -> Result<SyncReport> {
    let hub = google::connect(&paths.client_secret, &paths.token_cache).await?;
    // A connection of its own, so a slow sync write never blocks the UI.
    let conn = db::open_shared(&paths.database)?;
    sync::sync_all(&hub, &conn).await
}
