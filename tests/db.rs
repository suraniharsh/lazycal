//! Integration tests for the SQLite cache layer.

use chrono::{NaiveDate, TimeZone, Utc};
use google_calendar3::api::{Event, EventDateTime};
use lazycal::db::calendars::SyncState;
use lazycal::db::{self, calendars, events, schema};
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

fn all_day_event(id: &str, summary: &str, start: NaiveDate, end: NaiveDate) -> Event {
    Event {
        id: Some(id.to_owned()),
        summary: Some(summary.to_owned()),
        start: Some(EventDateTime {
            date: Some(start),
            ..Default::default()
        }),
        end: Some(EventDateTime {
            date: Some(end),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn timed_event(id: &str, summary: &str, start: NaiveDate, hour: u32) -> Event {
    let at = |h: u32| Utc.from_utc_datetime(&start.and_hms_opt(h, 0, 0).expect("valid time"));
    Event {
        id: Some(id.to_owned()),
        summary: Some(summary.to_owned()),
        start: Some(EventDateTime {
            date_time: Some(at(hour)),
            ..Default::default()
        }),
        end: Some(EventDateTime {
            date_time: Some(at(hour + 1)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn open_creates_and_migrates_a_database_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cache.db");

    let conn = db::open(&path).expect("open");
    assert!(columns(&conn, "calendars").contains(&"selected".to_owned()));
    assert!(columns(&conn, "events").contains(&"color_id".to_owned()));
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
         INSERT INTO calendars (id, summary) VALUES ('cal-1', 'Legacy');",
    )
    .expect("create legacy schema");

    schema::init(&conn).expect("migrate");

    assert!(columns(&conn, "calendars").contains(&"selected".to_owned()));
    assert!(columns(&conn, "events").contains(&"color_id".to_owned()));

    // Pre-existing rows pick up the column default rather than NULL.
    let cal = calendars::list_calendars(&conn).expect("list");
    assert_eq!(cal.len(), 1);
    assert!(cal[0].selected, "existing calendars default to visible");
}

#[test]
fn add_column_if_missing_is_a_no_op_when_present() {
    let conn = memory_db();
    schema::add_column_if_missing(&conn, "calendars", "selected", "INTEGER NOT NULL DEFAULT 1")
        .expect("no-op add");
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
fn apply_upserts_then_deletes_a_cancelled_event() {
    let conn = memory_db();
    let event = all_day_event("e1", "Standup", date(2026, 8, 26), date(2026, 8, 27));

    events::apply(&conn, "cal-1", &event).expect("insert");
    let rows = events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary, "Standup");

    // Incremental sync reports a deletion as a cancelled event, not an omission.
    let cancelled = Event {
        status: Some("cancelled".to_owned()),
        ..event
    };
    events::apply(&conn, "cal-1", &cancelled).expect("cancel");

    let rows = events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert!(
        rows.is_empty(),
        "cancelled events are removed from the cache"
    );
}

#[test]
fn apply_ignores_an_event_without_an_id() {
    let conn = memory_db();
    let event = Event {
        summary: Some("No id".to_owned()),
        ..Default::default()
    };

    events::apply(&conn, "cal-1", &event).expect("apply");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 0);
}

#[test]
fn apply_records_a_per_event_color_override() {
    let conn = memory_db();
    let event = Event {
        color_id: Some("11".to_owned()),
        ..all_day_event("e1", "Tomato", date(2026, 8, 26), date(2026, 8, 27))
    };

    events::apply(&conn, "cal-1", &event).expect("apply");

    let rows = events::events_between(&conn, date(2026, 8, 26), date(2026, 8, 27)).expect("query");
    assert_eq!(rows[0].color_id.as_deref(), Some("11"));
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
