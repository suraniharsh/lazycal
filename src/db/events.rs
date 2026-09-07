use anyhow::Result;
use chrono::{Days, NaiveDate, TimeZone, Utc};
use google_calendar3::api::Event;
use rusqlite::{Connection, params};

pub struct EventRow {
    pub calendar_id: String,
    pub summary: String,
    pub start_utc: Option<i64>,
    pub end_utc: Option<i64>,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub all_day: bool,
    /// Google's per-event color override (`colorId`), which takes precedence
    /// over the calendar's own color.
    pub color_id: Option<String>,
}

/// Upserts a synced event, or deletes it locally if Google reports it
/// cancelled — incremental sync reports deletions that way rather than by
/// omitting the event.
pub fn apply(conn: &Connection, calendar_id: &str, event: &Event) -> Result<()> {
    let Some(id) = event.id.as_deref() else {
        return Ok(());
    };

    let status = event.status.as_deref().unwrap_or("confirmed");
    if status == "cancelled" {
        conn.execute(
            "DELETE FROM events WHERE calendar_id = ?1 AND id = ?2",
            params![calendar_id, id],
        )?;
        return Ok(());
    }

    let summary = event.summary.as_deref().unwrap_or("(no title)");
    let start = event.start.as_ref();
    let end = event.end.as_ref();
    let all_day = start.and_then(|slot| slot.date).is_some();

    let start_utc = start
        .and_then(|slot| slot.date_time)
        .map(|at| at.timestamp());
    let start_date = start.and_then(|slot| slot.date);

    // Ranges are queried half-open, so an event whose end is missing or not
    // after its start would never match any window. Google documents that it
    // always sends an end, but synthesising one keeps a malformed event
    // visible instead of silently dropping it.
    let end_utc = end
        .and_then(|slot| slot.date_time)
        .map(|at| at.timestamp())
        .filter(|end| start_utc.is_none_or(|start| *end > start))
        .or(start_utc);
    let end_date = end
        .and_then(|slot| slot.date)
        .filter(|end| start_date.is_none_or(|start| *end > start))
        .or_else(|| start_date.and_then(|start| start.checked_add_days(Days::new(1))));

    conn.execute(
        "INSERT INTO events (
             calendar_id, id, summary, start_utc, end_utc,
             start_date, end_date, all_day, status, color_id
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(calendar_id, id) DO UPDATE SET
             summary = excluded.summary,
             start_utc = excluded.start_utc,
             end_utc = excluded.end_utc,
             start_date = excluded.start_date,
             end_date = excluded.end_date,
             all_day = excluded.all_day,
             status = excluded.status,
             color_id = excluded.color_id",
        params![
            calendar_id,
            id,
            summary,
            start_utc,
            end_utc,
            start_date.map(|date| date.to_string()),
            end_date.map(|date| date.to_string()),
            all_day,
            status,
            event.color_id,
        ],
    )?;
    Ok(())
}

/// Clears a calendar's cached events. A full resync establishes a new
/// baseline and won't re-report deletions that happened while the client was
/// away, so stale rows have to go first or they linger as phantom events.
pub fn delete_for_calendar(conn: &Connection, calendar_id: &str) -> Result<usize> {
    let removed = conn.execute(
        "DELETE FROM events WHERE calendar_id = ?1",
        params![calendar_id],
    )?;
    Ok(removed)
}

/// Events whose span overlaps `[start, end)` in local dates. The timed-event
/// bounds are widened by a day so nothing near the edges is missed in any
/// timezone; exact placement happens in [`crate::data`].
pub fn events_between(
    conn: &Connection,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<EventRow>> {
    let widen_back = start.checked_sub_days(Days::new(1)).unwrap_or(start);
    let widen_forward = end.checked_add_days(Days::new(1)).unwrap_or(end);
    let start_bound = local_midnight_utc(widen_back).timestamp();
    let end_bound = local_midnight_utc(widen_forward).timestamp();

    let mut stmt = conn.prepare(
        "SELECT calendar_id, summary, start_utc, end_utc, start_date, end_date, all_day, color_id
         FROM events
         WHERE (all_day = 1 AND start_date < ?2 AND end_date > ?1)
            OR (all_day = 0 AND start_utc < ?4 AND end_utc > ?3)",
    )?;
    let rows = stmt
        .query_map(
            params![start.to_string(), end.to_string(), start_bound, end_bound],
            |row| {
                Ok(EventRow {
                    calendar_id: row.get(0)?,
                    summary: row.get(1)?,
                    start_utc: row.get(2)?,
                    end_utc: row.get(3)?,
                    start_date: parse_date(row.get::<_, Option<String>>(4)?.as_deref()),
                    end_date: parse_date(row.get::<_, Option<String>>(5)?.as_deref()),
                    all_day: row.get::<_, i64>(6)? != 0,
                    color_id: row.get(7)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Tolerates an unparseable stored date rather than panicking on the render
/// path; the event simply won't be placed.
fn parse_date(text: Option<&str>) -> Option<NaiveDate> {
    let text = text?;
    match NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        Ok(date) => Some(date),
        Err(err) => {
            tracing::warn!("ignoring unparseable stored date {text:?}: {err}");
            None
        }
    }
}

fn local_midnight_utc(date: NaiveDate) -> chrono::DateTime<Utc> {
    let naive = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists for every date");
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}
