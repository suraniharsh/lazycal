use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Parser;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{event, execute};
use lazycal::app::{App, SyncStatus, WriteStatus};
use lazycal::clock::Clock;
use lazycal::config::Settings;
use lazycal::provider::{CalendarProvider, NewEvent, WritableCalendar};
use lazycal::sync::SyncReport;
use lazycal::{config, db, google, sync, ui};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tracing::Level;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// How long to block on input before looping to check on the background sync.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Parser)]
#[command(
    name = "lazycal",
    version,
    about = "A fast, offline-first calendar client for the terminal"
)]
struct Cli {
    /// Sync and exit without opening the interface, for use from cron
    #[arg(long)]
    sync: bool,

    /// Show only what's already cached, without syncing
    #[arg(long, conflicts_with = "sync")]
    offline: bool,

    /// Allow lazycal to add events, asking Google for consent again
    #[arg(long, conflicts_with_all = ["sync", "offline", "read_only"])]
    enable_write: bool,

    /// Give up the ability to add events, returning to read-only
    #[arg(long, conflicts_with_all = ["sync", "offline"])]
    read_only: bool,

    /// Show times as 24h (09:30) or 12h (9:30am), and remember the choice
    #[arg(long, value_name = "FORMAT", value_parser = parse_clock)]
    clock: Option<Clock>,
}

/// `--clock 12h`, spelled the handful of ways someone might reach for.
fn parse_clock(text: &str) -> Result<Clock, String> {
    match text.to_ascii_lowercase().as_str() {
        "12" | "12h" | "12hr" | "ampm" | "am/pm" => Ok(Clock::H12),
        "24" | "24h" | "24hr" => Ok(Clock::H24),
        other => Err(format!("expected 12h or 24h, not {other:?}")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = config::resolve()?;
    init_logging(&paths);

    // A preference, so it is recorded and the run carries on with it rather
    // than exiting — `lazycal --clock 12h` should open the calendar.
    if let Some(clock) = cli.clock {
        set_clock(&paths, clock)?;
    }

    if cli.enable_write || cli.read_only {
        return set_write_access(&paths, cli.enable_write).await;
    }

    let settings = Settings::load(&paths.settings);

    if cli.sync {
        return sync_and_exit(&paths, settings).await;
    }

    if cli.offline {
        // Nothing to announce; the footer reports the offline status.
    } else if !paths.client_secret.exists() {
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
        match perform_sync(&paths, google::Interactive::Yes, settings.access()).await {
            Ok(report) => println!("lazycal: synced {} calendar(s).", report.synced),
            Err(err) => eprintln!("lazycal: initial sync failed: {err:#}"),
        }
    }

    let mut terminal = init_terminal()?;
    let outcome = run(&mut terminal, &paths, cli.offline, settings);
    let restored = restore_terminal(&mut terminal);
    // Report the run's failure ahead of any problem restoring the terminal.
    outcome.and(restored)
}

/// Syncs without opening the interface, reporting on stdout and exiting
/// non-zero if anything failed so a scheduler notices.
async fn sync_and_exit(paths: &config::Paths, settings: Settings) -> Result<()> {
    if !paths.client_secret.exists() {
        bail!(
            "no Google credentials at {} — see the setup steps in the README",
            paths.client_secret.display()
        );
    }

    let report = perform_sync(paths, google::Interactive::No, settings.access()).await?;
    println!("synced {} calendar(s)", report.synced);
    if !report.is_complete() {
        bail!(
            "these calendars failed to sync: {}",
            report.failed.join(", ")
        );
    }
    Ok(())
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

fn run(
    terminal: &mut Term,
    paths: &config::Paths,
    offline: bool,
    settings: Settings,
) -> Result<()> {
    // Renders from whatever is cached so startup never waits on the network.
    let mut app = App::open(&paths.database)?;
    app.clock = settings.clock;
    app.set_write_enabled(settings.write && !offline);
    let configured = !offline && paths.client_secret.exists();
    if configured {
        spawn_background_sync(paths.clone(), app.sync_status_handle(), settings.access());
    } else if offline {
        app.set_sync_status(SyncStatus::Offline);
    } else {
        app.set_sync_status(SyncStatus::NotConfigured);
    }

    let mut last_status = app.sync_status();
    let mut last_write = app.write_status();
    // Point the cursor at the next meeting before the first frame, so it is
    // already marked rather than appearing a moment later.
    app.follow_clock();
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

        // Before the refresh check, so a saved event's resync happens in the
        // same pass rather than a poll later.
        if let Some((calendar_id, event)) = app.take_create_request() {
            let handle = app.write_status_handle();
            let paths = paths.clone();
            tokio::spawn(create_event(paths, calendar_id, event, handle));
            redraw = true;
        }

        let write = app.write_status();
        if write != last_write {
            last_write = write.clone();
            match write {
                // The service is the authority on what it stored, so the
                // event arrives by sync rather than being guessed locally.
                WriteStatus::Saved => {
                    app.set_write_status(WriteStatus::Idle);
                    last_write = WriteStatus::Idle;
                    app.request_refresh();
                }
                WriteStatus::Failed(message) => {
                    app.set_write_status(WriteStatus::Idle);
                    last_write = WriteStatus::Idle;
                    app.restore_failed_draft(message);
                }
                WriteStatus::Idle | WriteStatus::Saving => {}
            }
            redraw = true;
        }

        if app.take_refresh_request() && configured {
            app.set_sync_status(SyncStatus::Syncing);
            spawn_background_sync(paths.clone(), app.sync_status_handle(), settings.access());
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

        // Last, so it sees the keypress and the reload: a meeting ending has
        // to move the highlight on with nobody pressing anything.
        if app.follow_clock() {
            redraw = true;
        }

        if redraw {
            terminal.draw(|frame| ui::draw(frame, &app))?;
        }
    }
    Ok(())
}

fn spawn_background_sync(
    paths: config::Paths,
    status: Arc<Mutex<SyncStatus>>,
    access: google::Access,
) {
    tokio::spawn(async move {
        let next = match perform_sync(&paths, google::Interactive::No, access).await {
            Ok(report) if report.is_complete() => SyncStatus::Synced,
            Ok(report) => {
                tracing::warn!(
                    "these calendars failed to sync: {}",
                    report.failed.join(", ")
                );
                SyncStatus::Partial
            }
            // Google expires refresh tokens weekly while the OAuth app is in
            // "Testing", so say so rather than reporting a generic failure.
            Err(err) if google::needs_reauthorization(&err) => {
                tracing::warn!("sign-in needed: {err:#}");
                SyncStatus::NeedsAuth
            }
            Err(err) => {
                tracing::error!("sync failed: {err:#}");
                SyncStatus::Failed
            }
        };
        *status.lock().expect("sync status lock poisoned") = next;
    });
}

/// Grants or gives up write access, then exits.
///
/// Granting needs fresh consent, because the token Google issued covers only
/// the scopes it was asked for. That happens here rather than inside the TUI:
/// yup-oauth2 prints the consent URL to stdout, which the alternate screen
/// would swallow.
async fn set_write_access(paths: &config::Paths, write: bool) -> Result<()> {
    if write && !paths.client_secret.exists() {
        bail!(
            "no Google credentials at {} — see the setup steps in the README",
            paths.client_secret.display()
        );
    }

    // Read-modify-write, not a fresh `Settings`: the file holds the clock
    // preference too, and granting write access must not quietly reset it.
    let settings = Settings {
        write,
        ..Settings::load(&paths.settings)
    };
    settings.save(&paths.settings)?;

    if !write {
        println!(
            "lazycal: back to read-only. The access already granted stays until you remove\n\
             lazycal at https://myaccount.google.com/connections."
        );
        return Ok(());
    }

    println!("lazycal: asking Google for permission to add events…");
    let provider = google::GoogleProvider::connect(
        &paths.client_secret,
        &paths.token_cache,
        google::Interactive::Yes,
        google::Access::ReadWrite,
    )
    .await?;

    // Connecting builds the authenticator but asks for nothing: yup-oauth2
    // fetches a token on the first call that needs one. So make a call, which
    // is what actually triggers consent and caches a token covering both
    // scopes — otherwise this would report success and the first save inside
    // the TUI would fail, where no consent prompt can be shown.
    let calendars = provider
        .list_calendars()
        .await
        .map_err(|err| anyhow::anyhow!("signing in with write access: {err}"))?;

    let writable: Vec<&str> = calendars
        .iter()
        .filter(|calendar| calendar.writable)
        .map(|calendar| calendar.name.as_str())
        .collect();
    if writable.is_empty() {
        bail!(
            "signed in, but none of your {} calendar(s) accept new events",
            calendars.len()
        );
    }

    println!(
        "lazycal: write access granted. Press n to add an event to: {}.",
        writable.join(", ")
    );
    // The one thing this grant lets lazycal do that reaches other people, so
    // it is said once here as well as on the form itself.
    println!(
        "lazycal: filling in the form's Guests field invites those people,          which means Google emails them. Leave it empty and nothing is sent."
    );
    // Google's granular consent lets a scope be unticked on the way through,
    // which can't be told apart from a read-only token until something is
    // written, so say what that looks like rather than promise it worked.
    println!(
        "lazycal: if saving reports a permission error, the add-events \
         permission was not ticked — run this again."
    );
    Ok(())
}

/// Records the clock preference, leaving the rest of the file alone.
fn set_clock(paths: &config::Paths, clock: Clock) -> Result<()> {
    let settings = Settings {
        clock,
        ..Settings::load(&paths.settings)
    };
    settings.save(&paths.settings)?;
    println!("lazycal: showing times as {}.", clock.as_str());
    Ok(())
}

/// Sends one event, reporting through the shared status the UI watches.
async fn create_event(
    paths: config::Paths,
    calendar_id: String,
    event: NewEvent,
    status: Arc<Mutex<WriteStatus>>,
) {
    // A fresh id, so a retry after a lost response can't create the event
    // twice. Google asks for base32hex — digits and the letters a to v — and
    // recommends a UUID; a UUID's hex digits are a subset of that alphabet
    // once the hyphens are gone.
    let id = uuid::Uuid::new_v4().simple().to_string();
    let next = match google::GoogleProvider::connect(
        &paths.client_secret,
        &paths.token_cache,
        google::Interactive::No,
        google::Access::ReadWrite,
    )
    .await
    {
        Ok(provider) => match provider.create_event(&calendar_id, &id, &event).await {
            Ok(_) => WriteStatus::Saved,
            Err(err) => {
                tracing::warn!("creating event: {err}");
                WriteStatus::Failed(err.to_string())
            }
        },
        Err(err) => {
            tracing::warn!("connecting to create an event: {err:#}");
            WriteStatus::Failed(if google::needs_reauthorization(&err) {
                "Sign-in needed: run lazycal --enable-write once.".to_owned()
            } else {
                format!("{err}")
            })
        }
    };
    *status.lock().expect("write status lock poisoned") = next;
}

async fn perform_sync(
    paths: &config::Paths,
    interactive: google::Interactive,
    access: google::Access,
) -> Result<SyncReport> {
    let provider = google::GoogleProvider::connect(
        &paths.client_secret,
        &paths.token_cache,
        interactive,
        access,
    )
    .await?;
    // A connection of its own, so a slow sync write never blocks the UI.
    let conn = db::open_shared(&paths.database)?;
    sync::sync_all(&provider, &conn).await
}
