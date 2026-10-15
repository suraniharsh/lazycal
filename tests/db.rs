//! Integration tests for the SQLite cache layer.

use chrono::{NaiveDate, TimeZone, Utc};
use lazycal::db::calendars::SyncState;
use lazycal::db::{self, calendars, events, schema};
use lazycal::provider::{
    Attendee, Change, EventStatus, EventTime, Person, ProviderEvent, Response,
};
use rusqlite::Connection;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

/// A connection with the current schema, backed by memory.
fn memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    schema::init(&conn).expect("init schema");
    conn
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare table_info");
    stmt.query_map([], |row| row.get::<_, String>(1))
        .expect("query table_info")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect columns")
}

fn upsert(event: ProviderEvent) -> Change {
    Change::Upsert(Box::new(event))
}

fn all_day_event(id: &str, summary: &str, start: NaiveDate, end: NaiveDate) -> Change {
    upsert(ProviderEvent {
        id: id.to_owned(),
        summary: summary.to_owned(),
        start: EventTime::Date(start),
        end: EventTime::Date(end),
        status: EventStatus::Confirmed,
        color: None,
        description: None,
        location: None,
        organizer: None,
        attendees: Vec::new(),
    })
}

fn timed_event(id: &str, summary: &str, start: NaiveDate, hour: u32) -> Change {
    let at = |h: u32| Utc.from_utc_datetime(&start.and_hms_opt(h, 0, 0).expect("valid time"));
    upsert(ProviderEvent {
        id: id.to_owned(),
        summary: summary.to_owned(),
        start: EventTime::Instant(at(hour)),
        end: EventTime::Instant(at(hour + 1)),
        status: EventStatus::Confirmed,
        color: None,
        description: None,
        location: None,
        organizer: None,
        attendees: Vec::new(),
    })
}

#[test]
fn open_creates_and_migrates_a_database_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cache.db");

    let conn = db::open(&path).expect("open");
    assert!(columns(&conn, "calendars").contains(&"selected".to_owned()));
    let event_columns = columns(&conn, "events");
    for added in [
        "color_id",
        "description",
        "location",
        "organizer_email",
        "attendees",
    ] {
        assert!(
            event_columns.contains(&added.to_owned()),
            "{added} is missing"
        );
    }
    drop(conn);

    // Opening an existing database again must succeed unchanged.
    db::open(&path).expect("reopen");
}

#[test]
fn schema_init_is_idempotent() {
    let conn = memory_db();
    schema::init(&conn).expect("second init");
    schema::init(&conn).expect("third init");
}

#[test]
fn migration_adds_columns_to_a_legacy_database() {
    let conn = Connection::open_in_memory().expect("open");
    // The schema as it existed before `selected` and `color_id` were added.
    conn.execute_batch(
        "CREATE TABLE calendars (
             id TEXT PRIMARY KEY,
             summary TEXT NOT NULL,
             color TEXT,
             is_primary INTEGER NOT NULL DEFAULT 0,
             sync_token TEXT
         );
         CREATE TABLE events (
             calendar_id TEXT NOT NULL,
             id TEXT NOT NULL,
             summary TEXT NOT NULL,
             start_utc INTEGER,
             end_utc INTEGER,
             start_date TEXT,
             end_date TEXT,
             all_day INTEGER NOT NULL,
             status TEXT NOT NULL,
             PRIMARY KEY (calendar_id, id)
         );
         INSERT INTO calendars (id, summary, sync_token)
             VALUES ('cal-1', 'Legacy', 'resume-from-here');",
    )
    .expect("create legacy schema");

    schema::init(&conn).expect("migrate");

    assert!(columns(&conn, "calendars").contains(&"selected".to_owned()));
    let event_columns = columns(&conn, "events");
    for added in [
        "color_id",
        "description",
        "location",
        "organizer_email",
        "attendees",
    ] {
        assert!(
            event_columns.contains(&added.to_owned()),
            "{added} is missing"
        );
    }

    // Pre-existing rows pick up the column default rather than NULL.
    let cal = calendars::list_calendars(&conn).expect("list");
    assert_eq!(cal.len(), 1);
    assert!(cal[0].selected, "existing calendars default to visible");

    // The new columns are empty for every event already cached, and an
    // incremental sync only reports what changed — so they would stay empty
    // forever unless the next sync is made a full one.
    assert_eq!(
        calendars::sync_state(&conn, "cal-1").expect("state").token,
        None,
        "the cursor has to go, or the added columns never get filled"
    );
}

#[test]
fn add_column_if_missing_is_a_no_op_when_present() {
    let conn = memory_db();
    let added =
        schema::add_column_if_missing(&conn, "calendars", "selected", "INTEGER NOT NULL DEFAULT 1")
            .expect("no-op add");
    assert!(!added, "the column was already there");
    let selected_columns = columns(&conn, "calendars")
        .iter()
        .filter(|c| *c == "selected")
        .count();
    assert_eq!(selected_columns, 1);
}

#[test]
fn upsert_preserves_locally_owned_columns() {
    let conn = memory_db();
    calendars::upsert_calendar(&conn, "cal-1", "Work", Some("#112233"), true).expect("insert");

    // Local-only state: a sync position and a hidden-in-the-sidebar choice.
    let state = SyncState {
        token: Some("token-abc".to_owned()),
        horizon: Some(date(2027, 8, 1)),
    };
    calendars::set_sync_state(&conn, "cal-1", &state).expect("set sync state");
    calendars::set_selected(&conn, "cal-1", false).expect("hide");

    // A later sync re-upserts the calendar's metadata from Google.
    calendars::upsert_calendar(&conn, "cal-1", "Work (renamed)", Some("#445566"), true)
        .expect("re-upsert");

    let rows = calendars::list_calendars(&conn).expect("list");
    assert_eq!(rows.len(), 1, "upsert must not duplicate the row");
    assert_eq!(rows[0].summary, "Work (renamed)", "metadata is refreshed");
    assert_eq!(rows[0].color.as_deref(), Some("#445566"));
    assert!(!rows[0].selected, "visibility choice survives a sync");
    assert_eq!(
        calendars::sync_state(&conn, "cal-1").expect("state"),
        state,
        "sync token and horizon survive a sync"
    );
}

#[test]
fn sync_state_is_empty_for_unknown_and_fresh_calendars() {
    let conn = memory_db();
    assert_eq!(
        calendars::sync_state(&conn, "nope").expect("missing"),
        SyncState::default()
    );

    calendars::upsert_calendar(&conn, "cal-1", "Work", None, false).expect("insert");
    assert_eq!(
        calendars::sync_state(&conn, "cal-1").expect("fresh"),
        SyncState::default()
    );
}

#[test]
fn clearing_the_sync_token_keeps_the_horizon_and_visibility() {
    let conn = memory_db();
    calendars::upsert_calendar(&conn, "cal-1", "Work", None, false).expect("insert");
    calendars::set_selected(&conn, "cal-1", false).expect("hide");
    calendars::set_sync_state(
        &conn,
        "cal-1",
        &SyncState {
            token: Some("dead-token".to_owned()),
            horizon: Some(date(2027, 8, 1)),
        },
    )
    .expect("set state");

    calendars::clear_sync_token(&conn, "cal-1").expect("clear");

    let state = calendars::sync_state(&conn, "cal-1").expect("state");
    assert_eq!(state.token, None, "the rejected token is gone");
    assert_eq!(
        state.horizon,
        Some(date(2027, 8, 1)),
        "the horizon is untouched"
    );
    assert!(!calendars::list_calendars(&conn).expect("list")[0].selected);
}

#[test]
fn upsert_keeps_a_known_color_when_google_omits_it() {
    let conn = memory_db();
    calendars::upsert_calendar(&conn, "cal-1", "Work", Some("#0b8043"), false).expect("insert");

    // A later listing without backgroundColor must not erase the color.
    calendars::upsert_calendar(&conn, "cal-1", "Work", None, false).expect("re-upsert");

    let rows = calendars::list_calendars(&conn).expect("list");
    assert_eq!(rows[0].color.as_deref(), Some("#0b8043"));
}

#[test]
fn delete_for_calendar_clears_only_that_calendars_events() {
    let conn = memory_db();
    events::apply(
        &conn,
        "cal-1",
        &all_day_event("a", "Mine", date(2026, 8, 26), date(2026, 8, 27)),
    )
    .expect("apply a");
    events::apply(
        &conn,
        "cal-2",
        &all_day_event("b", "Theirs", date(2026, 8, 26), date(2026, 8, 27)),
    )
    .expect("apply b");

    let removed = events::delete_for_calendar(&conn, "cal-1").expect("delete");
    assert_eq!(removed, 1);

    let left = events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].summary, "Theirs");
}

#[test]
fn list_calendars_puts_the_primary_first() {
    let conn = memory_db();
    calendars::upsert_calendar(&conn, "b", "Beta", None, false).expect("insert b");
    calendars::upsert_calendar(&conn, "a", "Alpha", None, false).expect("insert a");
    calendars::upsert_calendar(&conn, "p", "Zulu (primary)", None, true).expect("insert p");

    let names: Vec<_> = calendars::list_calendars(&conn)
        .expect("list")
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert_eq!(names, vec!["Zulu (primary)", "Alpha", "Beta"]);
}

#[test]
fn delete_calendar_also_removes_its_events() {
    let conn = memory_db();
    calendars::upsert_calendar(&conn, "cal-1", "Work", None, false).expect("insert");
    events::apply(
        &conn,
        "cal-1",
        &all_day_event("e1", "Holiday", date(2026, 8, 26), date(2026, 8, 27)),
    )
    .expect("apply");

    calendars::delete_calendar(&conn, "cal-1").expect("delete");

    assert!(calendars::list_ids(&conn).expect("ids").is_empty());
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .expect("count");
    assert_eq!(
        remaining, 0,
        "orphaned events must not survive their calendar"
    );
}

#[test]
fn apply_upserts_then_applies_a_tombstone() {
    let conn = memory_db();
    let event = all_day_event("e1", "Standup", date(2026, 8, 26), date(2026, 8, 27));

    events::apply(&conn, "cal-1", &event).expect("insert");
    let rows = events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary, "Standup");

    // Incremental sync reports a removal as a tombstone, not an omission.
    let tombstone = Change::Delete {
        id: "e1".to_owned(),
    };
    events::apply(&conn, "cal-1", &tombstone).expect("delete");

    let rows = events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert!(
        rows.is_empty(),
        "a tombstone removes the event from the cache"
    );
}

#[test]
fn apply_records_a_resolved_event_color() {
    let conn = memory_db();
    let event = upsert(ProviderEvent {
        color: Some("#d60000".to_owned()),
        ..ProviderEvent {
            id: "e1".to_owned(),
            summary: "Tomato".to_owned(),
            start: EventTime::Date(date(2026, 8, 26)),
            end: EventTime::Date(date(2026, 8, 27)),
            status: EventStatus::Confirmed,
            color: None,
            description: None,
            location: None,
            organizer: None,
            attendees: Vec::new(),
        }
    });

    events::apply(&conn, "cal-1", &event).expect("apply");

    let rows = events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert_eq!(rows[0].color.as_deref(), Some("#d60000"));
}

#[test]
fn events_between_treats_all_day_end_dates_as_exclusive() {
    let conn = memory_db();
    // Google models a single all-day event on the 26th as [26th, 27th).
    events::apply(
        &conn,
        "cal-1",
        &all_day_event("e1", "Onam", date(2026, 8, 26), date(2026, 8, 27)),
    )
    .expect("apply");

    let on_the_day =
        events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert_eq!(on_the_day.len(), 1, "the event's own day is included");

    let day_after =
        events::events_between(&conn, date(2026, 8, 27), date(2026, 8, 28)).expect("query");
    assert!(
        day_after.is_empty(),
        "the exclusive end date must not leak into the next day"
    );

    let day_before =
        events::events_between(&conn, date(2026, 8, 25), date(2026, 8, 26)).expect("query");
    assert!(day_before.is_empty());
}

#[test]
fn events_between_includes_multi_day_all_day_events_overlapping_the_window() {
    let conn = memory_db();
    events::apply(
        &conn,
        "cal-1",
        &all_day_event("trip", "Trip", date(2026, 8, 20), date(2026, 8, 30)),
    )
    .expect("apply");

    // A window strictly inside the event's span still sees it.
    let inside =
        events::events_between(&conn, date(2026, 8, 24), date(2026, 8, 26)).expect("query");
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].start_date, Some(date(2026, 8, 20)));
    assert_eq!(inside[0].end_date, Some(date(2026, 8, 30)));
}

#[test]
fn events_between_finds_timed_events_by_instant() {
    let conn = memory_db();
    events::apply(
        &conn,
        "cal-1",
        &timed_event("e1", "Meeting", date(2026, 8, 26), 12),
    )
    .expect("apply");

    // A generous window keeps this independent of the machine's timezone.
    let found = events::events_between(&conn, date(2026, 8, 25), date(2026, 8, 28)).expect("query");
    assert_eq!(found.len(), 1);
    assert!(!found[0].all_day);
    assert!(found[0].start_utc.is_some());

    let elsewhere =
        events::events_between(&conn, date(2026, 9, 10), date(2026, 9, 20)).expect("query");
    assert!(elsewhere.is_empty());
}

#[test]
fn detail_fields_survive_a_round_trip() {
    let conn = memory_db();
    let event = upsert(ProviderEvent {
        description: Some("Agenda in the doc.".to_owned()),
        location: Some("Meeting room 2".to_owned()),
        organizer: Some(Person {
            name: Some("Lead".to_owned()),
            email: Some("lead@example.com".to_owned()),
        }),
        attendees: vec![
            Attendee {
                person: Person {
                    name: None,
                    email: Some("lead@example.com".to_owned()),
                },
                response: Response::Accepted,
                is_self: true,
                optional: false,
            },
            Attendee {
                person: Person {
                    name: Some("Other".to_owned()),
                    email: None,
                },
                response: Response::Declined,
                is_self: false,
                optional: true,
            },
        ],
        ..plain_event()
    });
    events::apply(&conn, "cal-1", &event).expect("apply");

    let detail = events::detail(&conn, "cal-1", "e1")
        .expect("read detail")
        .expect("the event is cached");
    assert_eq!(detail.description.as_deref(), Some("Agenda in the doc."));
    assert_eq!(detail.location.as_deref(), Some("Meeting room 2"));
    assert_eq!(
        detail.organizer.as_ref().map(|person| person.label()),
        Some("Lead")
    );
    assert_eq!(detail.attendees.len(), 2);
    assert!(detail.attendees[0].is_self);
    assert_eq!(detail.attendees[0].response, Response::Accepted);
    assert_eq!(detail.attendees[1].person.name.as_deref(), Some("Other"));
    assert!(detail.attendees[1].optional);
}

#[test]
fn an_event_with_no_details_reads_back_empty() {
    let conn = memory_db();
    events::apply(&conn, "cal-1", &upsert(plain_event())).expect("apply");

    let detail = events::detail(&conn, "cal-1", "e1")
        .expect("read detail")
        .expect("the event is cached");
    assert_eq!(detail, Default::default());
}

#[test]
fn detail_reports_an_event_that_is_no_longer_cached() {
    // The panel can be open when a sync removes the event underneath it.
    let conn = memory_db();
    assert_eq!(events::detail(&conn, "cal-1", "gone").expect("query"), None);
}

#[test]
fn an_upsert_replaces_the_previous_details() {
    let conn = memory_db();
    events::apply(
        &conn,
        "cal-1",
        &upsert(ProviderEvent {
            location: Some("Room 4".to_owned()),
            ..plain_event()
        }),
    )
    .expect("apply");
    // The meeting moved online, so the room has to go rather than linger.
    events::apply(&conn, "cal-1", &upsert(plain_event())).expect("apply");

    let detail = events::detail(&conn, "cal-1", "e1")
        .expect("read detail")
        .expect("still cached");
    assert_eq!(detail.location, None);
}

/// An event with no detail fields set, for tests that set one at a time.
fn plain_event() -> ProviderEvent {
    ProviderEvent {
        id: "e1".to_owned(),
        summary: "Standup".to_owned(),
        start: EventTime::Date(date(2026, 10, 12)),
        end: EventTime::Date(date(2026, 10, 13)),
        status: EventStatus::Confirmed,
        color: None,
        description: None,
        location: None,
        organizer: None,
        attendees: Vec::new(),
    }
}

#[test]
fn adding_the_detail_columns_forces_the_next_sync_to_be_a_full_one() {
    // An incremental sync only reports events that changed, so a database
    // written before these columns existed would never have them filled in.
    let conn = Connection::open_in_memory().expect("open in-memory db");
    conn.execute_batch(
        "CREATE TABLE calendars (
             id TEXT PRIMARY KEY, summary TEXT NOT NULL, color TEXT,
             is_primary INTEGER NOT NULL DEFAULT 0, sync_token TEXT
         );",
    )
    .expect("create a pre-upgrade calendars table");
    conn.execute(
        "INSERT INTO calendars (id, summary, sync_token) VALUES ('cal-1', 'Work', 'token-1')",
        [],
    )
    .expect("insert");

    schema::init(&conn).expect("migrate");

    let token: Option<String> = conn
        .query_row(
            "SELECT sync_token FROM calendars WHERE id = 'cal-1'",
            [],
            |row| row.get(0),
        )
        .expect("read token");
    assert_eq!(
        token, None,
        "the cursor was dropped so the next sync refills"
    );
}

#[test]
fn a_second_init_leaves_the_sync_token_alone() {
    // Only the upgrade clears it; every later start must not throw away a
    // perfectly good cursor and resync everything.
    let conn = memory_db();
    calendars::upsert_calendar(&conn, "cal-1", "Work", None, true).expect("insert");
    calendars::set_sync_state(
        &conn,
        "cal-1",
        &SyncState {
            token: Some("token-1".to_owned()),
            horizon: None,
        },
    )
    .expect("store token");

    schema::init(&conn).expect("re-init");

    let state = calendars::sync_state(&conn, "cal-1").expect("read state");
    assert_eq!(state.token.as_deref(), Some("token-1"));
}
