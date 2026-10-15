//! Renders the whole interface off-screen at awkward sizes.
//!
//! The views do their own slot and column arithmetic, so a terminal only a few
//! rows tall is the case most likely to produce an out-of-bounds rect. These
//! tests don't check what the frame says, only that every view and overlay can
//! draw at every size without panicking.

use chrono::{Days, Local, NaiveDate, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lazycal::app::App;
use lazycal::clock::Clock;
use lazycal::db;
use lazycal::provider::{
    Attendee, Change, EventStatus, EventTime, Person, ProviderEvent, Response,
};
use lazycal::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

/// Sizes worth trying: a single cell, a cramped pane, the classic 80x24, and
/// something generous.
const SIZES: [(u16, u16); 7] = [
    (1, 1),
    (4, 2),
    (12, 3),
    (30, 8),
    (80, 24),
    (100, 30),
    (200, 60),
];

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

/// A wall-clock time on `date` in the machine's own zone, as the UTC instant
/// the cache stores.
fn local(date: NaiveDate, hour: u32, minute: u32) -> chrono::DateTime<Utc> {
    Local
        .from_local_datetime(&date.and_hms_opt(hour, minute, 0).expect("valid time"))
        .earliest()
        .expect("representable local time")
        .with_timezone(&Utc)
}

/// An app whose cache holds a day busy enough to exercise overlap lanes, an
/// all-day band, and a description long enough to need scrolling.
fn busy_app() -> (App, tempfile::TempDir, NaiveDate) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cache.db");
    let conn = db::open(&path).expect("open db");
    let on = date(2026, 10, 12);

    db::calendars::upsert_calendar(&conn, "cal-1", "Work", Some("#112233"), true)
        .expect("insert calendar");

    // Local time, not UTC: these events are asserted about by the local day
    // they fall on, and a UTC instant lands on a different date either side
    // of the globe.
    let at = |hour: u32, minute: u32| EventTime::Instant(local(on, hour, minute));
    let base = ProviderEvent {
        id: String::new(),
        summary: String::new(),
        start: at(10, 0),
        end: at(11, 0),
        status: EventStatus::Confirmed,
        color: None,
        description: None,
        location: None,
        organizer: None,
        attendees: Vec::new(),
    };

    let events = [
        ProviderEvent {
            id: "all-day".to_owned(),
            summary: "Mahatma Gandhi Jayanti".to_owned(),
            start: EventTime::Date(on),
            end: EventTime::Date(on.checked_add_days(Days::new(1)).expect("next day")),
            ..base.clone()
        },
        // Two events in the same slot, so lanes and the `+N` collapse run.
        ProviderEvent {
            id: "standup".to_owned(),
            summary: "Standup with a title long enough to need truncating".to_owned(),
            location: Some("Meeting room 2".to_owned()),
            description: Some("Agenda.\n\n".to_owned() + &"detail ".repeat(200)),
            organizer: Some(Person {
                name: Some("Lead".to_owned()),
                email: Some("lead@example.com".to_owned()),
            }),
            attendees: (0..30)
                .map(|index| Attendee {
                    person: Person {
                        name: Some(format!("Person {index}")),
                        email: Some(format!("person{index}@example.com")),
                    },
                    response: Response::Accepted,
                    is_self: index == 0,
                    optional: index % 2 == 0,
                })
                .collect(),
            ..base.clone()
        },
        ProviderEvent {
            id: "overlap".to_owned(),
            summary: "Overlapping call \u{23f1}\u{fe0f}".to_owned(),
            start: at(10, 30),
            end: at(11, 30),
            ..base.clone()
        },
        // Crosses midnight, so it lands on two days.
        ProviderEvent {
            id: "overnight".to_owned(),
            summary: "Red-eye flight".to_owned(),
            start: at(22, 0),
            end: EventTime::Instant(local(
                on.checked_add_days(Days::new(1)).expect("next day"),
                8,
                0,
            )),
            ..base.clone()
        },
    ];
    for event in events {
        db::events::apply(&conn, "cal-1", &Change::Upsert(Box::new(event))).expect("apply");
    }
    drop(conn);

    let app = App::open(&path).expect("open app");
    (app, dir, on)
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn draw_at(app: &App, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| ui::draw(frame, app))
        .unwrap_or_else(|err| panic!("drawing at {width}x{height}: {err}"));
}

#[test]
fn every_view_draws_at_every_size() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;

    for code in ['m', 'w', 'd', 'a'] {
        press(&mut app, KeyCode::Char(code));
        for (width, height) in SIZES {
            draw_at(&app, width, height);
        }
    }
}

#[test]
fn the_overlays_draw_at_every_size() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;

    for code in ['m', 'w', 'd', 'a'] {
        press(&mut app, KeyCode::Char(code));

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert!(app.show_detail, "the panel opened in {code:?} view");
        for (width, height) in SIZES {
            draw_at(&app, width, height);
        }
        // Scrolled to the end of a long description, which is where the
        // bottom-right marker is placed.
        for _ in 0..500 {
            press(&mut app, KeyCode::Char('j'));
        }
        for (width, height) in SIZES {
            draw_at(&app, width, height);
        }
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::Char('?'));
        for (width, height) in SIZES {
            draw_at(&app, width, height);
        }
        press(&mut app, KeyCode::Esc);
    }
}

#[test]
fn the_sidebar_and_an_empty_cache_draw_at_every_size() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::open(&dir.path().join("cache.db")).expect("open app");

    for code in ['m', 'w', 'd', 'a'] {
        press(&mut app, KeyCode::Char(code));
        for (width, height) in SIZES {
            draw_at(&app, width, height);
        }
    }

    press(&mut app, KeyCode::Char('c'));
    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }
}

#[test]
fn the_detail_panel_reports_what_it_holds() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    press(&mut app, KeyCode::Char('d'));

    // The all-day event sorts first, so the cursor starts on it.
    press(&mut app, KeyCode::Tab);
    let event = app.cursor_event().expect("an event under the cursor");
    assert_eq!(event.summary, "Mahatma Gandhi Jayanti");
    assert!(event.all_day);

    press(&mut app, KeyCode::Tab);
    let event = app.cursor_event().expect("an event under the cursor");
    assert!(event.summary.starts_with("Standup"), "{}", event.summary);

    let detail = app.event_detail(&event);
    assert_eq!(detail.attendees.len(), 30);
    assert_eq!(detail.location.as_deref(), Some("Meeting room 2"));
    assert!(detail.description.is_some());
}

/// The frame as text, for the assertions that are about wording rather than
/// about not panicking.
fn rendered_text(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether any cell in a rendered frame is drawn reversed, which is how the
/// event cursor marks itself.
fn has_reversed_cell(app: &App, width: u16, height: u16) -> bool {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .any(|(x, y)| buffer[(x, y)].modifier.contains(Modifier::REVERSED))
}

#[test]
fn the_event_cursor_is_visible_in_every_view() {
    // Tab has no feedback at all unless the event it moved to is marked, and
    // a text dump can't show a style, so the cells are checked directly.
    let (mut app, _dir, on) = busy_app();
    app.selected = on;

    for code in ['m', 'w', 'd', 'a'] {
        press(&mut app, KeyCode::Char(code));
        assert!(
            !has_reversed_cell(&app, 100, 30),
            "{code:?} view marks something before anything is picked"
        );

        press(&mut app, KeyCode::Tab);
        assert!(
            has_reversed_cell(&app, 100, 30),
            "{code:?} view draws no cursor on the first event"
        );

        // The second event is a timed one, so this covers the hour-grid block
        // as well as the all-day band.
        press(&mut app, KeyCode::Tab);
        assert!(
            has_reversed_cell(&app, 100, 30),
            "{code:?} view draws no cursor on a timed event"
        );

        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('h'));
        assert!(
            !has_reversed_cell(&app, 100, 30),
            "{code:?} view keeps a cursor after the day moved"
        );
    }
}

#[test]
fn a_cursor_hidden_behind_an_overflow_count_is_still_shown() {
    // A month cell fits two or three event lines, so the eighth event of a day
    // is behind "+N more". The cursor must not vanish with it, because Enter
    // acts on whatever it is on.
    let (mut app, _dir, on) = crowded_app(8);
    app.selected = on;
    press(&mut app, KeyCode::Char('m'));

    for _ in 0..8 {
        press(&mut app, KeyCode::Tab);
    }
    assert_eq!(
        app.cursor_index_on(on),
        Some(7),
        "on the last of the eight events"
    );
    assert_eq!(app.selected, on, "still the same day");
    assert!(
        has_reversed_cell(&app, 100, 30),
        "the cursor fell behind the overflow count"
    );
}

#[test]
fn tabbing_to_an_event_scrolls_the_hour_grid_to_it() {
    // The regression: the grid opened on the day's first event, so tabbing to
    // a later one left the cursor off screen with nothing to show for it.
    let (mut app, _dir, on) = crowded_app(8);
    app.selected = on;

    for view in ['w', 'd'] {
        press(&mut app, KeyCode::Char(view));
        press(&mut app, KeyCode::Tab);
        for _ in 0..7 {
            press(&mut app, KeyCode::Tab);
        }
        assert_eq!(app.cursor_index_on(on), Some(7), "{view:?} view");
        assert!(
            has_reversed_cell(&app, 100, 30),
            "{view:?} view left the cursor outside the visible hours"
        );
    }
}

/// An app whose cache holds `count` timed events spread across one day, far
/// enough apart that no hour window can show them all at once.
fn crowded_app(count: u32) -> (App, tempfile::TempDir, NaiveDate) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cache.db");
    let conn = db::open(&path).expect("open db");
    let on = date(2026, 10, 12);

    for index in 0..count {
        // Spread over the whole day in local time, whatever the timezone.
        let start = Local
            .from_local_datetime(
                &on.and_hms_opt(index * (24 / count.max(1)), 0, 0)
                    .expect("valid time"),
            )
            .earliest()
            .expect("representable local time")
            .with_timezone(&Utc);
        let event = ProviderEvent {
            id: format!("e{index}"),
            summary: format!("Event {index}"),
            start: EventTime::Instant(start),
            end: EventTime::Instant(start + chrono::Duration::minutes(30)),
            status: EventStatus::Confirmed,
            color: None,
            description: None,
            location: None,
            organizer: None,
            attendees: Vec::new(),
        };
        db::events::apply(&conn, "cal-1", &Change::Upsert(Box::new(event))).expect("apply");
    }
    drop(conn);

    let app = App::open(&path).expect("open app");
    (app, dir, on)
}

#[test]
fn a_twelve_hour_clock_reaches_every_view() {
    // The setting is only honoured if every place that writes a time asks
    // for it, so this checks the views rather than the formatter.
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    app.clock = Clock::H12;

    for (code, needle) in [('d', "9am"), ('w', "9am")] {
        press(&mut app, KeyCode::Char(code));
        let rendered = rendered_text(&app, 200, 40);
        assert!(rendered.contains(needle), "{code}: {rendered}");
        assert!(
            !rendered.contains("09:00"),
            "{code} still shows a 24-hour gutter"
        );
    }

    // And the detail panel's when-line.
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Enter);
    let rendered = rendered_text(&app, 100, 30);
    assert!(
        rendered.contains("am") || rendered.contains("pm"),
        "{rendered}"
    );
}

#[test]
fn the_hour_gutter_does_not_change_width_with_the_clock() {
    // Both forms are drawn into the same six columns, so the grid beside
    // them must not shift when the setting changes.
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    press(&mut app, KeyCode::Char('d'));

    let column_of = |app: &App| {
        rendered_text(app, 100, 30)
            .lines()
            .find_map(|line| line.find("Standup"))
            .expect("the standup is on screen")
    };
    let twenty_four = column_of(&app);
    app.clock = Clock::H12;
    assert_eq!(column_of(&app), twenty_four);
}

thread_local! {
    /// What the stand-in opener was last handed, so a test can press `o`
    /// without a browser appearing on whoever is running the suite.
    static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}
