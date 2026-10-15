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
use lazycal::draft::Field;
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

    db::calendars::upsert_calendar(
        &conn,
        &db::calendars::CalendarMeta {
            id: "cal-1",
            summary: "Work",
            color: Some("#112233"),
            is_primary: true,
            writable: true,
        },
    )
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
        conference_url: None,
        links: Vec::new(),
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
            conference_url: Some("https://meet.example.com/abc-defg-hij".to_owned()),
            links: Vec::new(),
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
    // Both, not one or the other: they shared a column once and the room won.
    assert_eq!(detail.location.as_deref(), Some("Meeting room 2"));
    assert_eq!(
        detail.conference_url.as_deref(),
        Some("https://meet.example.com/abc-defg-hij")
    );
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

/// Tabs the open form round to `field`, so a test doesn't depend on how many
/// rows happen to come before it.
fn focus_field(app: &mut App, field: Field) {
    for _ in 0..ORDER_LIMIT {
        if app.draft().expect("the form is open").focus == field {
            return;
        }
        press(app, KeyCode::Tab);
    }
    panic!("the form never focused {field:?}");
}

/// More tabs than the form has rows, so a wrong answer fails rather than loops.
const ORDER_LIMIT: usize = 32;

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
            conference_url: None,
            links: Vec::new(),
            attendees: Vec::new(),
        };
        db::events::apply(&conn, "cal-1", &Change::Upsert(Box::new(event))).expect("apply");
    }
    drop(conn);

    let app = App::open(&path).expect("open app");
    (app, dir, on)
}

/// An app with write access granted and one writable calendar, which is what
/// the new-event form needs to open at all.
fn writable_app() -> (App, tempfile::TempDir, NaiveDate) {
    let (mut app, dir, on) = busy_app();
    app.set_write_enabled(true);
    app.selected = on;
    (app, dir, on)
}

#[test]
fn the_new_event_form_draws_at_every_size() {
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    assert!(app.draft().is_some(), "the form opened");

    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }

    // All-day drops two rows, and a validation complaint adds two, so both
    // shapes get drawn.
    for _ in 0..3 {
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Char(' '));
    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }

    press(&mut app, KeyCode::Enter);
    assert!(
        app.draft().is_some_and(|draft| draft.error.is_some()),
        "an empty title is refused"
    );
    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }
}

#[test]
fn a_long_value_keeps_the_caret_on_screen() {
    // A meeting URL is longer than the field, so the value has to scroll or
    // the caret ends up outside the panel with nothing to type against.
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    focus_field(&mut app, Field::Location);
    let url = "https://meet.example.com/a-very-long-meeting-url-indeed";
    for character in url.chars() {
        press(&mut app, KeyCode::Char(character));
    }
    assert_eq!(
        app.draft().expect("the form is open").location.value(),
        url,
        "the typing did not reach the field under test"
    );
    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }
    assert!(
        has_reversed_cell(&app, 100, 30),
        "the caret is not drawn anywhere"
    );
}

#[test]
fn the_form_does_not_open_without_write_access() {
    // Read-only is the default, and `n` has to be inert there rather than
    // offering a form whose save would be refused.
    let (mut app, _dir, _on) = busy_app();
    press(&mut app, KeyCode::Char('n'));
    assert!(app.draft().is_none());
}

#[test]
fn the_form_does_not_open_without_a_writable_calendar() {
    // `busy_app` seeds one writable calendar; take that away and there is
    // nowhere to file an event.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cache.db");
    let conn = db::open(&path).expect("open db");
    db::calendars::upsert_calendar(
        &conn,
        &db::calendars::CalendarMeta {
            id: "feed",
            summary: "Holidays",
            color: None,
            is_primary: false,
            writable: false,
        },
    )
    .expect("insert calendar");
    drop(conn);

    let mut app = App::open(&path).expect("open app");
    app.set_write_enabled(true);
    press(&mut app, KeyCode::Char('n'));
    assert!(app.draft().is_none(), "a subscribed feed is not a target");
}

#[test]
fn the_form_swallows_keys_that_would_otherwise_navigate() {
    // Typing "w" into the title must not switch to the week view behind the
    // form.
    let (mut app, _dir, on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    for character in "week".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    assert_eq!(app.selected, on, "the day did not move");
    assert_eq!(
        app.draft().map(|draft| draft.summary.value()),
        Some("week"),
        "the keys went into the field"
    );
}

#[test]
fn esc_closes_the_form() {
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Esc);
    assert!(app.draft().is_none());
}

#[test]
fn ctrl_c_still_quits_with_the_form_open() {
    // The form takes every key, but not the one that gets the user out.
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit);
}

#[test]
fn submitting_a_valid_form_hands_over_an_event() {
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    for character in "Pairing session".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    press(&mut app, KeyCode::Enter);

    assert!(app.draft().is_none(), "the form closed while it saves");
    let (calendar, event) = app.take_create_request().expect("an event to create");
    assert_eq!(calendar, "cal-1");
    assert_eq!(event.summary, "Pairing session");
}

#[test]
fn a_failed_save_hands_the_typing_back() {
    // Losing what someone typed because the network blinked is the worst
    // possible outcome here.
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    for character in "Hard-won title".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    focus_field(&mut app, Field::Guests);
    for character in "sam@example.com, alex@example.com".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    focus_field(&mut app, Field::Conference);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Enter);
    let _ = app.take_create_request();

    app.restore_failed_draft("the network is down".to_owned());
    let draft = app.draft().expect("the form came back");
    assert_eq!(draft.summary.value(), "Hard-won title");
    // A guest list is the most expensive thing on the form to retype.
    assert_eq!(draft.guests.value(), "sam@example.com, alex@example.com");
    assert!(draft.conference, "the video call choice came back too");
    assert_eq!(draft.error.as_deref(), Some("the network is down"));
    draw_at(&app, 100, 30);
}

#[test]
fn the_form_offers_guests_and_a_video_call() {
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    let rendered = rendered_text(&app, 100, 30);
    assert!(rendered.contains("Guests"), "{rendered}");
    assert!(rendered.contains("Video call"), "{rendered}");
}

#[test]
fn the_form_says_how_many_people_a_save_will_email() {
    // Inviting someone sends them mail, and finding that out afterwards is
    // not an acceptable way to learn it.
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    let quiet = rendered_text(&app, 100, 30);
    assert!(
        !quiet.contains("will be emailed"),
        "nobody is invited yet: {quiet}"
    );

    focus_field(&mut app, Field::Guests);
    for character in "sam@example.com".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    let one = rendered_text(&app, 100, 30);
    assert!(one.contains("1 guest will be emailed"), "{one}");

    for character in ", alex@example.com".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    let two = rendered_text(&app, 100, 30);
    assert!(two.contains("2 guests will be emailed"), "{two}");

    // The notice adds a row, so every size gets drawn again with it there.
    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }
}

#[test]
fn guests_and_a_video_call_reach_the_event_being_created() {
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    for character in "Design review".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    focus_field(&mut app, Field::Guests);
    for character in "Sam@example.com, sam@example.com alex@example.com".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    focus_field(&mut app, Field::Conference);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Enter);

    let (_, event) = app.take_create_request().expect("an event to create");
    assert_eq!(
        event.guests,
        vec!["Sam@example.com".to_owned(), "alex@example.com".to_owned()],
        "the repeat of the same address differing only in case was dropped"
    );
    assert!(event.conference, "the video call was asked for");
}

#[test]
fn a_guest_that_is_not_an_address_is_refused_before_anything_is_sent() {
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    for character in "Design review".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    focus_field(&mut app, Field::Guests);
    for character in "sam".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    press(&mut app, KeyCode::Enter);

    assert!(
        app.take_create_request().is_none(),
        "nothing may be sent with an unusable guest list"
    );
    let draft = app.draft().expect("the form stayed open");
    let error = draft.error.as_deref().expect("a complaint");
    assert!(
        error.contains("sam"),
        "the complaint names the offender: {error}"
    );
    assert_eq!(
        draft.summary.value(),
        "Design review",
        "the typing is intact"
    );
    draw_at(&app, 100, 30);
}

#[test]
fn the_detail_panel_shows_a_room_and_a_link_separately() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    let event = app.cursor_event().expect("an event under the cursor");
    assert!(event.summary.starts_with("Standup"), "{}", event.summary);

    press(&mut app, KeyCode::Enter);
    let rendered = rendered_text(&app, 100, 30);
    assert!(rendered.contains("Where"), "{rendered}");
    assert!(rendered.contains("Meeting room 2"), "{rendered}");
    assert!(rendered.contains("Join"), "{rendered}");
    assert!(rendered.contains("meet.example.com"), "{rendered}");
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

#[test]
fn the_form_shows_the_length_and_nudges_the_time() {
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    let rendered = rendered_text(&app, 100, 30);
    assert!(rendered.contains("30m"), "the default length: {rendered}");

    focus_field(&mut app, Field::End);
    for _ in 0..2 {
        press(&mut app, KeyCode::Up);
    }
    let rendered = rendered_text(&app, 100, 30);
    assert!(
        rendered.contains("1h"),
        "half an hour plus two nudges: {rendered}"
    );
    assert!(
        rendered.contains("to nudge"),
        "the border says so: {rendered}"
    );
}

#[test]
fn the_guest_field_completes_from_the_cache() {
    // The suggestions come from whoever is on the cached events, so this
    // drives the whole path: sync -> cache -> query -> form.
    let (mut app, _dir, _on) = writable_app();
    press(&mut app, KeyCode::Char('n'));
    focus_field(&mut app, Field::Guests);
    for character in "person1".chars() {
        press(&mut app, KeyCode::Char(character));
    }

    let rendered = rendered_text(&app, 100, 30);
    assert!(rendered.contains("person1@example.com"), "{rendered}");
    assert!(rendered.contains("to take the name"), "{rendered}");

    press(&mut app, KeyCode::Tab);
    let draft = app.draft().expect("the form is open");
    assert_eq!(draft.guests.value(), "person1@example.com, ");
    assert_eq!(
        draft.focus,
        Field::Guests,
        "tab completed rather than moved"
    );
    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }
}

thread_local! {
    /// What the stand-in opener was last handed, so a test can press `o`
    /// without a browser appearing on whoever is running the suite.
    static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn record_open(url: &str) -> anyhow::Result<()> {
    OPENED.with(|opened| opened.borrow_mut().push(url.to_owned()));
    Ok(())
}

fn fail_to_open(_: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("no browser here"))
}

fn opened() -> Vec<String> {
    OPENED.with(|opened| opened.borrow().clone())
}

#[test]
fn o_opens_the_link_on_the_event_under_the_cursor() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    app.set_opener(record_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    let event = app.cursor_event().expect("an event under the cursor");
    assert!(event.summary.starts_with("Standup"), "{}", event.summary);

    press(&mut app, KeyCode::Char('o'));
    assert_eq!(opened(), ["https://meet.example.com/abc-defg-hij"]);
    assert_eq!(app.flash(), Some("opening the link\u{2026}"));
}

#[test]
fn o_works_with_the_detail_panel_open_too() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    app.set_opener(record_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Enter);
    assert!(app.show_detail);

    press(&mut app, KeyCode::Char('o'));
    assert_eq!(opened().len(), 1, "{:?}", opened());
    assert!(app.show_detail, "opening a link does not close the panel");
}

#[test]
fn an_event_with_no_link_says_so_rather_than_doing_nothing() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    app.set_opener(record_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    let event = app.cursor_event().expect("an event under the cursor");
    assert!(event.all_day, "the all-day event, which has no link");

    press(&mut app, KeyCode::Char('o'));
    assert!(opened().is_empty(), "nothing was launched");
    assert_eq!(app.flash(), Some("no link on this event"));
    // The message is in the footer, not just in the struct.
    assert!(
        rendered_text(&app, 100, 30).contains("no link on this event"),
        "{}",
        rendered_text(&app, 100, 30)
    );
}

#[test]
fn o_with_nothing_selected_says_what_to_do_instead() {
    // An empty cache, so there is certainly no cursor to have a link.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::open(&dir.path().join("cache.db")).expect("open app");
    app.set_opener(record_open);
    assert!(app.cursor_event().is_none(), "nothing to select");

    press(&mut app, KeyCode::Char('o'));
    assert!(opened().is_empty());
    assert_eq!(
        app.flash(),
        Some("no event selected \u{2014} tab to pick one")
    );
    assert!(rendered_text(&app, 100, 30).contains("no event selected"));
}

#[test]
fn an_opener_that_fails_is_reported_in_the_footer() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    app.set_opener(fail_to_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('o'));

    let flash = app.flash().expect("a complaint");
    assert!(flash.starts_with("could not open it:"), "{flash}");
    assert!(rendered_text(&app, 100, 30).contains("could not open"));
}

#[test]
fn the_footers_answer_clears_on_the_next_key() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    app.set_opener(record_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('o'));
    assert!(app.flash().is_some());
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(
        app.flash(),
        None,
        "it answered the key before, not this one"
    );
    assert!(
        rendered_text(&app, 200, 40).contains("m/w/d/a: view"),
        "the hints come back"
    );
}

#[test]
fn the_open_key_is_only_advertised_when_there_is_something_to_open() {
    let (mut app, _dir, on) = busy_app();
    app.selected = on;
    press(&mut app, KeyCode::Char('d'));

    press(&mut app, KeyCode::Tab);
    assert!(
        app.cursor_links().is_empty(),
        "the all-day event has no link"
    );
    let without = rendered_text(&app, 200, 40);
    assert!(!without.contains("o: open"), "{without}");

    press(&mut app, KeyCode::Tab);
    assert_eq!(app.cursor_links().len(), 1, "the standup has exactly one");
    let with = rendered_text(&app, 200, 40);
    assert!(with.contains("o: open"), "{with}");

    // And the detail panel's own border says it.
    press(&mut app, KeyCode::Enter);
    assert!(rendered_text(&app, 100, 30).contains("o to open"));
}

#[test]
fn a_link_pasted_in_the_body_is_what_o_opens() {
    // Invitations from other people arrive as prose with a link in it, which
    // is the common case rather than the exception.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cache.db");
    let conn = db::open(&path).expect("open db");
    db::calendars::upsert_calendar(
        &conn,
        &db::calendars::CalendarMeta {
            id: "cal-1",
            summary: "Work",
            color: None,
            is_primary: true,
            writable: false,
        },
    )
    .expect("calendar");
    let on = date(2026, 10, 12);
    db::events::apply(
        &conn,
        "cal-1",
        &Change::Upsert(Box::new(ProviderEvent {
            id: "zoom".to_owned(),
            summary: "Vendor call".to_owned(),
            start: EventTime::Instant(local(on, 10, 0)),
            end: EventTime::Instant(local(on, 11, 0)),
            status: EventStatus::Confirmed,
            color: None,
            description: Some(
                "Dial in:\nhttps://example.zoom.us/j/123456789?pwd=abc\nOr by phone.".to_owned(),
            ),
            location: Some("Zoom".to_owned()),
            conference_url: None,
            links: Vec::new(),
            organizer: None,
            attendees: Vec::new(),
        })),
    )
    .expect("apply");
    drop(conn);

    let mut app = App::open(&path).expect("open app");
    app.selected = on;
    app.set_opener(record_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(opened(), ["https://example.zoom.us/j/123456789?pwd=abc"]);
}

/// A cache holding one event with `count` links on it, as a sync would.
fn linked_app(count: usize) -> (App, tempfile::TempDir, NaiveDate) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cache.db");
    let conn = db::open(&path).expect("open db");
    db::calendars::upsert_calendar(
        &conn,
        &db::calendars::CalendarMeta {
            id: "cal-1",
            summary: "Work",
            color: None,
            is_primary: true,
            writable: false,
        },
    )
    .expect("calendar");
    let on = date(2026, 10, 12);
    db::events::apply(
        &conn,
        "cal-1",
        &Change::Upsert(Box::new(ProviderEvent {
            id: "teams".to_owned(),
            summary: "Vendor sync".to_owned(),
            start: EventTime::Instant(local(on, 10, 0)),
            end: EventTime::Instant(local(on, 11, 0)),
            status: EventStatus::Confirmed,
            color: None,
            description: Some("Click here to join the meeting".to_owned()),
            location: None,
            conference_url: None,
            links: (0..count)
                .map(|index| format!("https://example{index}.com/join"))
                .collect(),
            organizer: None,
            attendees: Vec::new(),
        })),
    )
    .expect("apply");
    drop(conn);

    let mut app = App::open(&path).expect("open app");
    app.selected = on;
    app.set_opener(record_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Tab);
    (app, dir, on)
}

#[test]
fn one_link_opens_without_asking() {
    let (mut app, _dir, _on) = linked_app(1);
    press(&mut app, KeyCode::Char('o'));
    assert!(app.link_choice().is_none(), "no question worth asking");
    assert_eq!(opened(), ["https://example0.com/join"]);
}

#[test]
fn several_links_ask_which_one() {
    let (mut app, _dir, _on) = linked_app(3);
    press(&mut app, KeyCode::Char('o'));
    assert!(opened().is_empty(), "nothing opened until one was chosen");

    let choice = app.link_choice().expect("the chooser is up");
    assert_eq!(choice.links.len(), 3);
    assert_eq!(choice.highlighted, 0);

    let rendered = rendered_text(&app, 100, 30);
    assert!(rendered.contains("Open which link?"), "{rendered}");
    for index in 0..3 {
        assert!(
            rendered.contains(&format!("{}. https://example{index}.com/join", index + 1)),
            "{rendered}"
        );
    }

    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.link_choice().expect("still up").highlighted, 2);
    press(&mut app, KeyCode::Enter);
    assert!(app.link_choice().is_none(), "it closed after choosing");
    assert_eq!(opened(), ["https://example2.com/join"]);
}

#[test]
fn the_chooser_wraps_at_both_ends() {
    let (mut app, _dir, _on) = linked_app(3);
    press(&mut app, KeyCode::Char('o'));
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(
        app.link_choice().expect("up").highlighted,
        2,
        "wrapped back"
    );
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.link_choice().expect("up").highlighted, 0, "and forward");
}

#[test]
fn a_digit_opens_that_link_directly() {
    let (mut app, _dir, _on) = linked_app(3);
    press(&mut app, KeyCode::Char('o'));
    press(&mut app, KeyCode::Char('2'));
    assert_eq!(opened(), ["https://example1.com/join"]);
    assert!(app.link_choice().is_none());
}

#[test]
fn a_digit_past_the_end_of_the_list_does_nothing() {
    let (mut app, _dir, _on) = linked_app(2);
    press(&mut app, KeyCode::Char('o'));
    press(&mut app, KeyCode::Char('9'));
    assert!(opened().is_empty());
    assert!(app.link_choice().is_some(), "the question is still open");
}

#[test]
fn esc_cancels_the_chooser_without_opening_anything() {
    let (mut app, _dir, _on) = linked_app(2);
    press(&mut app, KeyCode::Char('o'));
    press(&mut app, KeyCode::Esc);
    assert!(app.link_choice().is_none());
    assert!(opened().is_empty());
}

#[test]
fn the_chooser_swallows_keys_that_would_otherwise_navigate() {
    // Navigating behind it would leave it asking about an event that is no
    // longer selected.
    let (mut app, _dir, on) = linked_app(2);
    press(&mut app, KeyCode::Char('o'));
    for code in [
        KeyCode::Char('l'),
        KeyCode::Char('m'),
        KeyCode::Char('t'),
        KeyCode::Char('?'),
        KeyCode::Char('n'),
    ] {
        press(&mut app, code);
    }
    assert_eq!(app.selected, on, "the date did not move");
    assert!(!app.show_help, "the help overlay did not open");
    assert!(app.draft().is_none(), "the form did not open");
    assert!(app.link_choice().is_some(), "and the question is still up");
}

#[test]
fn ctrl_c_still_quits_with_the_chooser_up() {
    let (mut app, _dir, _on) = linked_app(2);
    press(&mut app, KeyCode::Char('o'));
    app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit);
}

#[test]
fn the_footer_says_whether_o_will_ask() {
    let (app, _dir, _on) = linked_app(1);
    assert!(rendered_text(&app, 200, 40).contains("o: open"));
    assert!(!rendered_text(&app, 200, 40).contains("o: open\u{2026}"));

    let (app, _dir, _on) = linked_app(3);
    assert!(rendered_text(&app, 200, 40).contains("o: open\u{2026}"));
}

#[test]
fn the_detail_panel_says_so_too() {
    let (mut app, _dir, _on) = linked_app(3);
    press(&mut app, KeyCode::Enter);
    assert!(rendered_text(&app, 100, 30).contains("o to open a link"));

    let (mut app, _dir, _on) = linked_app(1);
    press(&mut app, KeyCode::Enter);
    let rendered = rendered_text(&app, 100, 30);
    assert!(rendered.contains("o to open"), "{rendered}");
    assert!(!rendered.contains("o to open a link"), "{rendered}");
}

#[test]
fn the_chooser_draws_at_every_size() {
    let (mut app, _dir, _on) = linked_app(4);
    press(&mut app, KeyCode::Char('o'));
    for (width, height) in SIZES {
        draw_at(&app, width, height);
    }
}
