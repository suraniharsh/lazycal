//! Dev tool: renders the interface from invented data, for the README and for
//! eyeballing layout without a Google account.
//!
//! Unlike `preview`, this never touches your real cache — it seeds a throwaway
//! database in a temporary directory, so the output is safe to publish.
//!
//! The README's views are dumps of this, so regenerate them from here after
//! anything that changes the layout — including a version bump, which the
//! footer shows.
//!
//! Run with: cargo run --example demo [YYYY-MM-DD]

use anyhow::{Context, Result};
use chrono::{Days, Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lazycal::app::App;
use lazycal::draft::Field;
use lazycal::provider::{
    Attendee, Change, EventStatus, EventTime, Person, ProviderEvent, Response,
};
use lazycal::{db, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const WIDTH: u16 = 100;

/// Invented calendars: id, name, colour, whether it is the account's primary
/// one (which is what the sidebar sorts on), and whether it can be written to.
const CALENDARS: [(&str, &str, &str, bool, bool); 4] = [
    ("work", "Work", "#4285f4", true, true),
    ("personal", "Personal", "#0b8043", false, true),
    // A subscribed feed: readable, and never a place to put a meeting.
    ("holidays", "Holidays", "#8e24aa", false, false),
    ("oncall", "On-call rota", "#f4511e", false, false),
];

/// Left hidden, so the demo shows what a toggled-off calendar looks like.
const HIDDEN: &str = "oncall";

/// Invented events: calendar, title, day offset from the anchor, and the local
/// start and end times. `None` times make it an all-day event.
type Demo = (
    &'static str,
    &'static str,
    i64,
    Option<(u32, u32)>,
    Option<(u32, u32)>,
);
const EVENTS: [Demo; 26] = [
    ("work", "Sprint kickoff", -8, Some((9, 0)), Some((10, 0))),
    ("work", "Standup", -8, Some((9, 30)), Some((9, 45))),
    ("personal", "Climbing", -7, Some((18, 30)), Some((20, 0))),
    ("work", "Vendor call", -6, Some((15, 0)), Some((15, 45))),
    ("holidays", "Autumn bank holiday", -1, None, None),
    ("work", "Standup", 0, Some((9, 30)), Some((9, 45))),
    (
        "work",
        "Pairing: cache eviction",
        0,
        Some((10, 0)),
        Some((11, 0)),
    ),
    ("work", "Design review", 0, Some((10, 30)), Some((11, 30))),
    (
        "personal",
        "Lunch with Sam",
        0,
        Some((12, 30)),
        Some((13, 30)),
    ),
    ("work", "1:1 with Priya", 0, Some((14, 0)), Some((14, 30))),
    ("work", "Release 0.2.0", 0, Some((16, 30)), Some((17, 0))),
    ("work", "Standup", 1, Some((9, 30)), Some((9, 45))),
    ("work", "Roadmap workshop", 1, Some((11, 0)), Some((12, 30))),
    ("personal", "Dentist", 1, Some((15, 0)), Some((15, 45))),
    ("work", "Standup", 2, Some((9, 30)), Some((9, 45))),
    ("work", "Retro", 2, Some((16, 0)), Some((17, 0))),
    ("personal", "Sam's birthday", 3, None, None),
    ("work", "Quarterly planning", 5, Some((9, 0)), Some((12, 0))),
    ("work", "Standup", 8, Some((9, 30)), Some((9, 45))),
    ("work", "Architecture sync", 8, Some((14, 0)), Some((15, 0))),
    (
        "personal",
        "Train to Bristol",
        9,
        Some((7, 45)),
        Some((10, 15)),
    ),
    ("holidays", "Conference", 12, None, None),
    ("work", "Standup", 15, Some((9, 30)), Some((9, 45))),
    (
        "work",
        "Postmortem: sync tokens",
        15,
        Some((13, 0)),
        Some((14, 0)),
    ),
    (
        "personal",
        "Dinner with Alex",
        17,
        Some((19, 30)),
        Some((21, 0)),
    ),
    ("work", "Month-end review", 18, Some((16, 0)), Some((17, 0))),
];

fn main() -> Result<()> {
    let anchor = match std::env::args().nth(1) {
        Some(text) => text.parse().context("a date as YYYY-MM-DD")?,
        None => Local::now().date_naive(),
    };

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("demo.db");
    seed(&path, anchor)?;

    let mut app = App::open(&path)?;
    app.selected = anchor;
    // Nothing is syncing here, so don't leave the footer claiming it is.
    app.set_sync_status(lazycal::app::SyncStatus::Synced);
    // The clock-following cursor is deliberately not engaged: it marks an
    // event by reversing it and scrolls the grid to it, and a text dump keeps
    // neither, so the output would just look arbitrarily scrolled.

    show("month", &app, 26);
    press(&mut app, KeyCode::Char('d'));
    show("day", &app, 22);
    press(&mut app, KeyCode::Char('w'));
    show("week", &app, 22);

    // Three tabs from nothing reaches the one event seeded with a location,
    // an attendee list and a description.
    press(&mut app, KeyCode::Char('d'));
    for _ in 0..3 {
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Enter);
    show("event details", &app, 24);

    // The chooser, which `o` raises when an event carries more than one link.
    press(&mut app, KeyCode::Char('o'));
    show("choosing a link", &app, 22);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);

    // The new-event form, which needs write access to open at all. Guests are
    // filled in so the dump shows the warning about the mail a save sends.
    //
    // Opened on the day after the anchor, not on it: a form opened on the
    // real today defaults its start to the next half-hour slot, which would
    // make this dump change every thirty minutes.
    app.set_write_enabled(true);
    press(&mut app, KeyCode::Char('l'));
    press(&mut app, KeyCode::Char('n'));
    for character in "Pairing on the outbox".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    while app.draft().expect("the form is open").focus != Field::Guests {
        press(&mut app, KeyCode::Tab);
    }
    for character in "priya@example.com, sam".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    show("new event", &app, 26);

    // The same form on a 12-hour clock, which is a setting rather than a key.
    press(&mut app, KeyCode::Esc);
    app.clock = lazycal::clock::Clock::H12;
    press(&mut app, KeyCode::Char('n'));
    for character in "Pairing on the outbox".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    show("new event, twelve-hour clock", &app, 24);
    press(&mut app, KeyCode::Esc);
    show("day, twelve-hour clock", &app, 22);
    Ok(())
}

fn seed(path: &std::path::Path, anchor: NaiveDate) -> Result<()> {
    let conn = db::open(path)?;
    for (id, name, color, is_primary, writable) in CALENDARS {
        db::calendars::upsert_calendar(
            &conn,
            &db::calendars::CalendarMeta {
                id,
                summary: name,
                color: Some(color),
                is_primary,
                writable,
            },
        )?;
    }
    db::calendars::set_selected(&conn, HIDDEN, false)?;

    for (index, (calendar, summary, offset, from, to)) in EVENTS.iter().enumerate() {
        let day = shift(anchor, *offset);
        let (start, end) = match (from, to) {
            (Some((h1, m1)), Some((h2, m2))) => (
                EventTime::Instant(at(day, *h1, *m1)),
                EventTime::Instant(at(day, *h2, *m2)),
            ),
            // All-day events are stored half-open, ending the day after.
            _ => (EventTime::Date(day), EventTime::Date(shift(day, 1))),
        };
        let event = ProviderEvent {
            id: format!("demo-{index}"),
            summary: (*summary).to_owned(),
            start,
            end,
            status: EventStatus::Confirmed,
            color: None,
            description: (*summary == "Design review").then(|| {
                "Walk through the hour-grid layout before it goes in.\n\n\
                 Bring the overlap cases."
                    .to_owned()
            }),
            // A room *and* a link, which is the case the detail panel has to
            // show both halves of.
            location: (*summary == "Design review").then(|| "Meeting room 2".to_owned()),
            conference_url: (*summary == "Design review")
                .then(|| "https://meet.example.com/demo-abcd-efg".to_owned()),
            // As a sync would have left them: the conferencing first, then
            // whatever the invitation text pointed at.
            links: if *summary == "Design review" {
                vec![
                    "https://meet.example.com/demo-abcd-efg".to_owned(),
                    "https://notes.example.com/design-review".to_owned(),
                ]
            } else {
                Vec::new()
            },
            organizer: (*summary == "Design review").then(|| Person {
                name: Some("Priya".to_owned()),
                email: Some("priya@example.com".to_owned()),
            }),
            attendees: if *summary == "Design review" {
                vec![
                    attendee("You", "you@example.com", Response::Accepted, true),
                    attendee("Priya", "priya@example.com", Response::Accepted, false),
                    attendee("Sam", "sam@example.com", Response::Tentative, false),
                    attendee("Alex", "alex@example.com", Response::Declined, false),
                ]
            } else {
                Vec::new()
            },
        };
        db::events::apply(&conn, calendar, &Change::Upsert(Box::new(event)))?;
    }
    Ok(())
}

fn attendee(name: &str, email: &str, response: Response, is_self: bool) -> Attendee {
    Attendee {
        person: Person {
            name: Some(name.to_owned()),
            email: Some(email.to_owned()),
        },
        response,
        is_self,
        optional: false,
    }
}

/// A local wall-clock time on `date`, as the UTC instant the cache stores.
fn at(date: NaiveDate, hour: u32, minute: u32) -> chrono::DateTime<chrono::Utc> {
    Local
        .from_local_datetime(&date.and_hms_opt(hour, minute, 0).expect("valid time"))
        .earliest()
        .expect("representable local time")
        .with_timezone(&chrono::Utc)
}

fn shift(date: NaiveDate, days: i64) -> NaiveDate {
    let shifted = if days >= 0 {
        date.checked_add_days(Days::new(days.unsigned_abs()))
    } else {
        date.checked_sub_days(Days::new(days.unsigned_abs()))
    };
    shifted.expect("date within range")
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn show(label: &str, app: &App, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, height)).expect("test terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();

    println!("\n=== {label} ({WIDTH}x{height}) ===");
    for y in 0..height {
        let row: String = (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect();
        println!("{}", row.trim_end());
    }
}
