use std::collections::HashMap;

use anyhow::Result;
use chrono::{Days, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, params};

use crate::provider::{Attendee, Change, EventStatus, EventTime, Person};

pub struct EventRow {
    pub calendar_id: String,
    /// Provider-assigned id, unique within its calendar. Carried so the detail
    /// panel can fetch the heavy fields for one event instead of every render
    /// dragging every description through memory.
    pub id: String,
    pub summary: String,
    pub start_utc: Option<i64>,
    pub end_utc: Option<i64>,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub all_day: bool,
    /// Resolved `#rrggbb` when the event overrides its calendar's colour.
    pub color: Option<String>,
}

/// The fields only the detail panel needs, read one event at a time.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct EventDetail {
    pub description: Option<String>,
    pub location: Option<String>,
    /// Where to join, when the event has a video call attached.
    pub conference_url: Option<String>,
    /// Every link on the event, best first. See [`crate::link`].
    pub links: Vec<String>,
    pub organizer: Option<Person>,
    pub attendees: Vec<Attendee>,
}

/// Applies one synced change: an upsert, or a delete for a tombstone.
pub fn apply(conn: &Connection, calendar_id: &str, change: &Change) -> Result<()> {
    let event = match change {
        Change::Delete { id } => {
            conn.execute(
                "DELETE FROM events WHERE calendar_id = ?1 AND id = ?2",
                params![calendar_id, id],
            )?;
            return Ok(());
        }
        Change::Upsert(event) => event,
    };

    // The provider guarantees start and end are the same variant.
    let (start_utc, end_utc, start_date, end_date, all_day) = match (event.start, event.end) {
        (EventTime::Date(from), EventTime::Date(to)) => (None, None, Some(from), Some(to), true),
        (EventTime::Instant(from), EventTime::Instant(to)) => (
            Some(from.timestamp()),
            Some(to.timestamp()),
            None,
            None,
            false,
        ),
        // Mismatched variants would be a provider bug; skip rather than store
        // a row no window could ever match.
        _ => {
            tracing::warn!(
                "ignoring event {} with mismatched start/end kinds",
                event.id
            );
            return Ok(());
        }
    };

    let status = match event.status {
        EventStatus::Confirmed => "confirmed",
        EventStatus::Tentative => "tentative",
    };

    // An empty list stores as NULL rather than "[]", so the column stays
    // cheap for the holiday and fixtures feeds that have no attendees at all.
    let attendees = if event.attendees.is_empty() {
        None
    } else {
        match serde_json::to_string(&event.attendees) {
            Ok(json) => Some(json),
            Err(err) => {
                tracing::warn!("encoding attendees for {}: {err}", event.id);
                None
            }
        }
    };
    let organizer = event.organizer.clone().unwrap_or_default();
    // Same reasoning as `attendees`: an empty list stores as NULL, which is
    // every event in a holiday or fixtures feed that carries no link.
    let links = if event.links.is_empty() {
        None
    } else {
        match serde_json::to_string(&event.links) {
            Ok(json) => Some(json),
            Err(err) => {
                tracing::warn!("encoding links for {}: {err}", event.id);
                None
            }
        }
    };

    conn.execute(
        "INSERT INTO events (
             calendar_id, id, summary, start_utc, end_utc,
             start_date, end_date, all_day, status, color_id,
             description, location, organizer_name, organizer_email, attendees,
             conference_url, links
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT(calendar_id, id) DO UPDATE SET
             summary = excluded.summary,
             start_utc = excluded.start_utc,
             end_utc = excluded.end_utc,
             start_date = excluded.start_date,
             end_date = excluded.end_date,
             all_day = excluded.all_day,
             status = excluded.status,
             color_id = excluded.color_id,
             description = excluded.description,
             location = excluded.location,
             organizer_name = excluded.organizer_name,
             organizer_email = excluded.organizer_email,
             attendees = excluded.attendees,
             conference_url = excluded.conference_url,
             links = excluded.links",
        params![
            calendar_id,
            event.id,
            event.summary,
            start_utc,
            end_utc,
            start_date.map(|date| date.to_string()),
            end_date.map(|date| date.to_string()),
            all_day,
            status,
            event.color,
            event.description,
            event.location,
            organizer.name,
            organizer.email,
            attendees,
            event.conference_url,
            links,
        ],
    )?;
    Ok(())
}

/// The detail fields for one event, or `None` if it is no longer cached — a
/// sync can remove the event the panel is open on.
pub fn detail(conn: &Connection, calendar_id: &str, id: &str) -> Result<Option<EventDetail>> {
    let mut stmt = conn.prepare(
        "SELECT description, location, organizer_name, organizer_email, attendees,
                conference_url, links
         FROM events WHERE calendar_id = ?1 AND id = ?2",
    )?;
    let mut rows = stmt.query(params![calendar_id, id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };

    let organizer = Person {
        name: row.get(2)?,
        email: row.get(3)?,
    };
    Ok(Some(EventDetail {
        description: row.get(0)?,
        location: row.get(1)?,
        conference_url: row.get(5)?,
        links: parse_links(row.get::<_, Option<String>>(6)?.as_deref()),
        organizer: (!organizer.is_anonymous()).then_some(organizer),
        attendees: parse_attendees(row.get::<_, Option<String>>(4)?.as_deref()),
    }))
}

/// Tolerates unreadable stored JSON the same way attendees do.
fn parse_links(json: Option<&str>) -> Vec<String> {
    let Some(json) = json else {
        return Vec::new();
    };
    match serde_json::from_str(json) {
        Ok(links) => links,
        Err(err) => {
            tracing::warn!("ignoring unreadable stored links: {err}");
            Vec::new()
        }
    }
}

/// Tolerates unreadable stored JSON rather than failing the whole panel; the
/// event simply shows no attendees.
fn parse_attendees(json: Option<&str>) -> Vec<Attendee> {
    let Some(json) = json else {
        return Vec::new();
    };
    match serde_json::from_str(json) {
        Ok(attendees) => attendees,
        Err(err) => {
            tracing::warn!("ignoring unreadable stored attendees: {err}");
            Vec::new()
        }
    }
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
        "SELECT calendar_id, id, summary, start_utc, end_utc, start_date, end_date, all_day,
                color_id
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
                    id: row.get(1)?,
                    summary: row.get(2)?,
                    start_utc: row.get(3)?,
                    end_utc: row.get(4)?,
                    start_date: parse_date(row.get::<_, Option<String>>(5)?.as_deref()),
                    end_date: parse_date(row.get::<_, Option<String>>(6)?.as_deref()),
                    all_day: row.get::<_, i64>(7)? != 0,
                    color: row.get(8)?,
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
        .expect("00:00 is a valid time of day");
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}

/// Everyone who appears on a cached event, most often first.
///
/// The guest field's suggestions come from here: the people already on the
/// calendar are overwhelmingly the people the next invitation goes to, and
/// they cost nothing extra to know — they were synced with the events.
///
/// The signed-in user is left out. Google puts them on their own events as
/// organizer, so they would otherwise top the list, and adding yourself as a
/// guest is not a thing anyone means to do.
pub fn known_people(conn: &Connection, limit: usize) -> Result<Vec<Person>> {
    let mut stmt = conn.prepare(
        "SELECT attendees FROM events
         WHERE attendees IS NOT NULL
         ORDER BY COALESCE(start_utc, 0) DESC",
    )?;
    let rows = stmt
        .query_map([], |row| row.get::<_, Option<String>>(0))?
        .filter_map(Result::ok);

    // Keyed on the lowercased address, because the same person comes back
    // from Google spelled whichever way whoever invited them typed it.
    let mut seen: HashMap<String, (usize, Person)> = HashMap::new();
    for json in rows {
        for attendee in parse_attendees(json.as_deref()) {
            if attendee.is_self {
                continue;
            }
            let Some(email) = attendee.person.email.as_deref() else {
                continue;
            };
            let entry = seen
                .entry(email.to_ascii_lowercase())
                .or_insert_with(|| (0, attendee.person.clone()));
            entry.0 += 1;
            // Rows are newest first, so a name only fills a gap left by a
            // more recent appearance that had none.
            if entry.1.name.is_none() {
                entry.1.name = attendee.person.name.clone();
            }
        }
    }

    let mut people: Vec<(usize, Person)> = seen.into_values().collect();
    // Most frequent first, then alphabetically, so the order is stable
    // between runs rather than following the hash map.
    people.sort_by(|(left_count, left), (right_count, right)| {
        right_count.cmp(left_count).then_with(|| {
            left.label()
                .to_ascii_lowercase()
                .cmp(&right.label().to_ascii_lowercase())
        })
    });
    people.truncate(limit);
    Ok(people.into_iter().map(|(_, person)| person).collect())
}
