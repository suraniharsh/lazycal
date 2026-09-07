use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, params};

pub struct CalendarRow {
    pub id: String,
    pub summary: String,
    pub color: Option<String>,
    pub is_primary: bool,
    pub selected: bool,
}

/// What a previous sync left behind for one calendar: the token to resume
/// from, and how far ahead that sync actually fetched.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncState {
    pub token: Option<String>,
    pub horizon: Option<NaiveDate>,
}

/// `selected`, `sync_token` and `sync_horizon` are locally owned, so they are
/// absent from both the insert (letting the column defaults apply) and the
/// conflict update: re-reading a calendar's metadata from Google must not
/// discard a visibility choice or a sync position. `color` is coalesced
/// because Google only returns `backgroundColor` in some configurations, and
/// a response without it shouldn't erase the color already known.
pub fn upsert_calendar(
    conn: &Connection,
    id: &str,
    summary: &str,
    color: Option<&str>,
    is_primary: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO calendars (id, summary, color, is_primary)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             summary = excluded.summary,
             color = COALESCE(excluded.color, calendars.color),
             is_primary = excluded.is_primary",
        params![id, summary, color, is_primary],
    )?;
    Ok(())
}

pub fn set_selected(conn: &Connection, id: &str, selected: bool) -> Result<()> {
    conn.execute(
        "UPDATE calendars SET selected = ?1 WHERE id = ?2",
        params![selected, id],
    )?;
    Ok(())
}

pub fn sync_state(conn: &Connection, id: &str) -> Result<SyncState> {
    let state = conn
        .query_row(
            "SELECT sync_token, sync_horizon FROM calendars WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?;

    Ok(match state {
        Some((token, horizon)) => SyncState {
            token,
            horizon: horizon.as_deref().and_then(parse_date),
        },
        None => SyncState::default(),
    })
}

pub fn set_sync_state(conn: &Connection, id: &str, state: &SyncState) -> Result<()> {
    conn.execute(
        "UPDATE calendars SET sync_token = ?1, sync_horizon = ?2 WHERE id = ?3",
        params![state.token, state.horizon.map(|date| date.to_string()), id],
    )?;
    Ok(())
}

/// Drops a stale token so a failure mid-resync can't leave it behind to be
/// retried (and rejected) forever.
pub fn clear_sync_token(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE calendars SET sync_token = NULL WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

pub fn list_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM calendars")?;
    let ids = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// Removes a calendar and its cached events. The schema declares no foreign
/// key, so the events have to go explicitly.
pub fn delete_calendar(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM events WHERE calendar_id = ?1", params![id])?;
    conn.execute("DELETE FROM calendars WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn list_calendars(conn: &Connection) -> Result<Vec<CalendarRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, summary, color, is_primary, selected
         FROM calendars
         ORDER BY is_primary DESC, summary",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CalendarRow {
                id: row.get(0)?,
                summary: row.get(1)?,
                color: row.get(2)?,
                is_primary: row.get::<_, i64>(3)? != 0,
                selected: row.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}
