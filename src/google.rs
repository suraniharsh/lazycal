use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::{Context, Result};
use chrono::{DateTime, Days, NaiveDate, Utc};
use google_calendar3::api::{
    CalendarListEntry, Event, EventAttendee, EventDateTime, EventListCall,
};
use google_calendar3::hyper_rustls::{self, HttpsConnector};
use google_calendar3::hyper_util::client::legacy::Client;
use google_calendar3::hyper_util::client::legacy::connect::HttpConnector;
use google_calendar3::hyper_util::rt::TokioExecutor;
use google_calendar3::{CalendarHub, yup_oauth2};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use yup_oauth2::{InstalledFlowAuthenticator, InstalledFlowReturnMethod};

use crate::provider::{
    Attendee, CalendarProvider, Change, EventStatus, EventTime, Person, ProviderCalendar,
    ProviderEvent, Response, SyncError, SyncKind, SyncOutcome, Window,
};

/// Read-only access is enough: calendar visibility is a local preference and
/// is never written back to Google.
///
/// Requested explicitly on every call, because yup-oauth2 caches tokens per
/// requested scope set — letting individual calls fall back to the generated
/// client's own per-method scope hint would ask for a fresh consent for each
/// distinct scope encountered.
pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";

/// Calendars per `calendarList.list` page (the API's maximum is 250).
const CALENDAR_PAGE_SIZE: i32 = 250;
/// Events per `events.list` page.
const EVENT_PAGE_SIZE: i32 = 250;

pub type Connector = HttpsConnector<HttpConnector>;
pub type Hub = CalendarHub<Connector>;

/// Marker in the error chain when sign-in is needed but couldn't be asked for.
const REAUTH_REQUIRED: &str = "authorization required";

/// Whether a connection is allowed to ask for consent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Interactive {
    /// Someone is watching a normal terminal, so consent can be requested.
    Yes,
    /// Unattended — from cron, or behind the TUI's alternate screen, where the
    /// consent URL would be invisible and the wait would never end.
    No,
}

/// Whether `err` means the user has to sign in again.
///
/// Google expires refresh tokens after seven days while an OAuth app's
/// publishing status is still "Testing", so this is a routine condition worth
/// telling the user about rather than a generic failure.
pub fn needs_reauthorization(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains(REAUTH_REQUIRED)
}

/// Declines the consent prompt instead of printing a URL and blocking.
struct RefuseConsent;

impl yup_oauth2::authenticator_delegate::InstalledFlowDelegate for RefuseConsent {
    fn present_user_url<'a>(
        &'a self,
        _url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send + 'a>> {
        Box::pin(async {
            Err(format!(
                "{REAUTH_REQUIRED}: run lazycal interactively once to sign in"
            ))
        })
    }
}

pub async fn connect(
    client_secret_path: &Path,
    token_cache_path: &Path,
    interactive: Interactive,
) -> Result<Hub> {
    let secret = yup_oauth2::read_application_secret(client_secret_path)
        .await
        .with_context(|| format!("reading {}", client_secret_path.display()))?;

    // Only reached when there's no usable cached token, so the choice of
    // method doesn't affect the normal path.
    let method = match interactive {
        // Captures the code automatically via a localhost redirect.
        Interactive::Yes => InstalledFlowReturnMethod::HTTPRedirect,
        // The redirect flow discards a delegate's refusal and then waits on
        // its local server forever, which would hang an unattended run; the
        // interactive method propagates the refusal as an error instead.
        Interactive::No => InstalledFlowReturnMethod::Interactive,
    };

    let mut builder = InstalledFlowAuthenticator::builder(secret, method)
        .persist_tokens_to_disk(token_cache_path);
    if interactive == Interactive::No {
        builder = builder.flow_delegate(Box::new(RefuseConsent));
    }

    let auth = builder
        .build()
        .await
        .context("building OAuth authenticator")?;

    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .context("loading native TLS roots")?
        .https_or_http()
        .enable_http2()
        .build();
    let client = Client::builder(TokioExecutor::new()).build(connector);

    Ok(CalendarHub::new(client, auth))
}

/// The generated client substitutes `{calendarId}` into the request path
/// without URL-encoding it, so a raw `#` in an id such as
/// `en.indian#holiday@group.v.calendar.google.com` is read as a fragment
/// delimiter and truncates the path into a 404. Encoding it here avoids that;
/// the server decodes it back, so over-encoding unreserved characters is
/// harmless.
pub fn encode_calendar_id(id: &str) -> String {
    utf8_percent_encode(id, NON_ALPHANUMERIC).to_string()
}

/// Every calendar on the account, following pagination to the end.
///
/// Completeness matters: the caller treats anything missing from this list as
/// removed from the account and prunes it from the cache.
pub async fn list_calendars(hub: &Hub) -> Result<Vec<CalendarListEntry>> {
    let mut calendars = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        let mut call = hub
            .calendar_list()
            .list()
            .max_results(CALENDAR_PAGE_SIZE)
            .add_scope(SCOPE);
        if let Some(token) = &page_token {
            call = call.page_token(token);
        }

        let (_, page) = call.doit().await.context("calendarList.list")?;
        calendars.extend(page.items.into_iter().flatten());

        page_token = page.next_page_token;
        if page_token.is_none() {
            return Ok(calendars);
        }
    }
}

/// Google Calendar as a [`CalendarProvider`].
pub struct GoogleProvider {
    hub: Hub,
}

impl GoogleProvider {
    pub async fn connect(
        client_secret_path: &Path,
        token_cache_path: &Path,
        interactive: Interactive,
    ) -> Result<Self> {
        Ok(Self {
            hub: connect(client_secret_path, token_cache_path, interactive).await?,
        })
    }

    fn events_call(&self, encoded_id: &str) -> EventListCall<'_, Connector> {
        self.hub
            .events()
            .list(encoded_id)
            // Recurring events are expanded server-side into instances, so the
            // rest of lazycal never has to understand RRULE.
            .single_events(true)
            // Incremental sync reports deletions as cancelled events.
            .show_deleted(true)
            .max_results(EVENT_PAGE_SIZE)
            .add_scope(SCOPE)
    }
}

impl CalendarProvider for GoogleProvider {
    async fn list_calendars(&self) -> Result<Vec<ProviderCalendar>, SyncError> {
        let entries = list_calendars(&self.hub).await.map_err(classify)?;
        Ok(entries.iter().filter_map(to_provider_calendar).collect())
    }

    async fn sync_calendar(
        &self,
        calendar_id: &str,
        cursor: Option<&str>,
        window: Window,
    ) -> Result<SyncOutcome, SyncError> {
        let encoded_id = encode_calendar_id(calendar_id);
        let kind = if cursor.is_some() {
            SyncKind::Incremental
        } else {
            SyncKind::Full
        };

        let mut changes = Vec::new();
        let mut page_token: Option<String> = None;
        let next_cursor;

        loop {
            let mut call = self.events_call(&encoded_id);
            match cursor {
                // Google rejects timeMin/timeMax alongside a sync token, so an
                // incremental pass inherits whatever window the full sync used.
                Some(token) => call = call.sync_token(token),
                None => {
                    call = call
                        .time_min(midnight_utc(window.from))
                        .time_max(midnight_utc(window.to));
                }
            }
            if let Some(token) = &page_token {
                call = call.page_token(token);
            }

            let (_, page) = call.doit().await.map_err(|err| classify(err.into()))?;
            changes.extend(page.items.iter().flatten().filter_map(to_change));

            match page.next_page_token {
                Some(token) => page_token = Some(token),
                None => {
                    // Only the last page carries the next sync token.
                    next_cursor = page.next_sync_token;
                    break;
                }
            }
        }

        Ok(SyncOutcome {
            changes,
            cursor: next_cursor,
            // An incremental pass doesn't widen the window, so it reports none
            // and the caller keeps the horizon it already had.
            covered: matches!(kind, SyncKind::Full).then_some(window),
            kind,
        })
    }
}

fn to_provider_calendar(entry: &CalendarListEntry) -> Option<ProviderCalendar> {
    Some(ProviderCalendar {
        id: entry.id.clone()?,
        name: entry
            .summary
            .clone()
            .unwrap_or_else(|| entry.id.clone().unwrap_or_default()),
        color: entry.background_color.clone(),
        is_primary: entry.primary.unwrap_or(false),
    })
}

/// Maps one Google event onto a neutral change, or nothing when it carries no
/// id or no usable times.
fn to_change(event: &Event) -> Option<Change> {
    let id = event.id.clone()?;
    let status = event.status.as_deref().unwrap_or("confirmed");
    // Incremental sync reports a removal as a cancelled event rather than by
    // omitting it.
    if status == "cancelled" {
        return Some(Change::Delete { id });
    }

    let (start, end) = event_times(event.start.as_ref()?, event.end.as_ref())?;
    Some(Change::Upsert(Box::new(ProviderEvent {
        id,
        summary: event
            .summary
            .clone()
            .unwrap_or_else(|| "(no title)".to_owned()),
        start,
        end,
        status: if status == "tentative" {
            EventStatus::Tentative
        } else {
            EventStatus::Confirmed
        },
        color: event
            .color_id
            .as_deref()
            .and_then(event_color)
            .map(str::to_owned),
        description: event
            .description
            .as_deref()
            .map(plain_text)
            .filter(|text| !text.is_empty()),
        location: text(event.location.as_deref()),
        organizer: event.organizer.as_ref().and_then(|organizer| {
            let person = Person {
                name: text(organizer.display_name.as_deref()),
                email: text(organizer.email.as_deref()),
            };
            (!person.is_anonymous()).then_some(person)
        }),
        attendees: event
            .attendees
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(to_attendee)
            .collect(),
    })))
}

/// Trims an optional string and discards it if nothing is left, so a blank
/// field from the API doesn't read as a present-but-empty one.
fn text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Maps one invitee, dropping entries that name nobody — Google emits those
/// for room and equipment bookings.
fn to_attendee(attendee: &EventAttendee) -> Option<Attendee> {
    let person = Person {
        name: text(attendee.display_name.as_deref()),
        email: text(attendee.email.as_deref()),
    };
    if person.is_anonymous() {
        return None;
    }
    Some(Attendee {
        person,
        response: match attendee.response_status.as_deref() {
            Some("accepted") => Response::Accepted,
            Some("declined") => Response::Declined,
            Some("tentative") => Response::Tentative,
            _ => Response::NeedsAction,
        },
        is_self: attendee.self_.unwrap_or(false),
        optional: attendee.optional.unwrap_or(false),
    })
}

/// Flattens a description into something a terminal can show.
///
/// Google stores HTML in `description` for anything created in its web UI, so
/// the raw field arrives full of `<br>` and `&amp;`. Text that carries no
/// recognisable tag is left exactly as it is, because a plain description may
/// legitimately contain `<` and `>` and must not have them eaten.
fn plain_text(description: &str) -> String {
    if !looks_like_html(description) {
        return description.trim().to_owned();
    }

    let mut out = String::with_capacity(description.len());
    let mut rest = description;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let Some(close) = after.find('>') else {
            // An unclosed `<` is content, not a tag.
            out.push_str(after);
            rest = "";
            break;
        };
        if breaks_line(&after[1..close]) {
            out.push('\n');
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);

    collapse_blank_lines(&unescape(&out)).trim().to_owned()
}

/// Whether the text contains a tag we recognise, as opposed to stray angle
/// brackets.
fn looks_like_html(text: &str) -> bool {
    const TAGS: [&str; 10] = [
        "<br", "<p>", "<p ", "<div", "<span", "<a ", "<ul", "<ol", "<li", "</",
    ];
    let lower = text.to_ascii_lowercase();
    TAGS.iter().any(|tag| lower.contains(tag))
}

/// Tags that end a line of text, so paragraphs don't run together.
fn breaks_line(tag: &str) -> bool {
    let name = tag
        .trim_start_matches('/')
        .split([' ', '/', '\t', '\n'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "br" | "p" | "div" | "li" | "tr" | "ul" | "ol" | "h1" | "h2" | "h3" | "blockquote"
    )
}

fn unescape(text: &str) -> String {
    // The entities Google actually emits. Ampersand is replaced last so
    // "&amp;lt;" survives as the literal "&lt;".
    const ENTITIES: [(&str, &str); 6] = [
        ("&nbsp;", " "),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
    ];
    let mut out = text.to_owned();
    for (entity, literal) in ENTITIES {
        if out.contains(entity) {
            out = out.replace(entity, literal);
        }
    }
    out.replace("&amp;", "&")
}

/// Markup generators are generous with `<div><br></div>`, which would
/// otherwise leave a screen of empty rows in the detail panel.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blanks = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Resolves start/end, synthesising an end Google didn't usably supply.
///
/// Ranges are queried half-open, so an end that precedes its start would make
/// the event match no window at all; falling back keeps it visible.
fn event_times(
    start: &EventDateTime,
    end: Option<&EventDateTime>,
) -> Option<(EventTime, EventTime)> {
    if let Some(from) = start.date {
        let to = end
            .and_then(|slot| slot.date)
            .filter(|to| *to > from)
            .or_else(|| from.checked_add_days(Days::new(1)))?;
        return Some((EventTime::Date(from), EventTime::Date(to)));
    }

    let from = start.date_time?;
    let to = end
        .and_then(|slot| slot.date_time)
        .filter(|to| *to > from)
        .unwrap_or(from);
    Some((EventTime::Instant(from), EventTime::Instant(to)))
}

/// Google's fixed event palette (`colorId` 1-11), which an event uses to
/// override its calendar's colour.
fn event_color(color_id: &str) -> Option<&'static str> {
    Some(match color_id {
        "1" => "#7986cb",  // Lavender
        "2" => "#33b679",  // Sage
        "3" => "#8e24aa",  // Grape
        "4" => "#e67c73",  // Flamingo
        "5" => "#f6bf26",  // Banana
        "6" => "#f4511e",  // Tangerine
        "7" => "#039be5",  // Peacock
        "8" => "#616161",  // Graphite
        "9" => "#3f51b5",  // Blueberry
        "10" => "#0b8043", // Basil
        "11" => "#d60000", // Tomato
        _ => return None,
    })
}

/// Google answers an expired sync token with HTTP 410 and `fullSyncRequired`.
///
/// The generated client decodes any error response that has a JSON body into
/// `Error::BadRequest`, which carries no status code, so the condition has to
/// be recognised from the payload; `Error::Failure` only happens when the body
/// isn't JSON.
fn is_full_resync_required(err: &google_calendar3::Error) -> bool {
    match err {
        google_calendar3::Error::BadRequest(body) => {
            let error = &body["error"];
            error["code"].as_i64() == Some(410)
                || error["errors"].as_array().is_some_and(|errors| {
                    errors
                        .iter()
                        .any(|e| e["reason"].as_str() == Some("fullSyncRequired"))
                })
        }
        google_calendar3::Error::Failure(response) => response.status().as_u16() == 410,
        _ => false,
    }
}

/// Translates a Google failure into the neutral error the engine acts on.
fn classify(err: anyhow::Error) -> SyncError {
    if needs_reauthorization(&err) {
        return SyncError::NeedsAuth;
    }
    match err.downcast_ref::<google_calendar3::Error>() {
        Some(google) if is_full_resync_required(google) => SyncError::FullResyncRequired,
        _ => SyncError::Other(err),
    }
}

fn midnight_utc(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight exists for every date")
        .and_utc()
}

#[cfg(test)]
mod tests {
    use google_calendar3::api::EventOrganizer;

    use super::*;

    /// The shape Google actually returns for an expired sync token.
    fn expired_token_body() -> serde_json::Value {
        serde_json::json!({
            "error": {
                "code": 410,
                "message": "Sync token is no longer valid, a full sync is required.",
                "errors": [{
                    "domain": "calendar",
                    "reason": "fullSyncRequired",
                    "message": "Sync token is no longer valid, a full sync is required."
                }]
            }
        })
    }

    #[test]
    fn recognises_an_expired_sync_token_from_a_json_error_body() {
        assert!(is_full_resync_required(
            &google_calendar3::Error::BadRequest(expired_token_body())
        ));
    }

    #[test]
    fn recognises_full_sync_required_without_a_code() {
        let body = serde_json::json!({ "error": { "errors": [{ "reason": "fullSyncRequired" }] } });
        assert!(is_full_resync_required(
            &google_calendar3::Error::BadRequest(body)
        ));
    }

    #[test]
    fn leaves_other_google_errors_alone() {
        let not_found =
            serde_json::json!({ "error": { "code": 404, "errors": [{ "reason": "notFound" }] } });
        assert!(!is_full_resync_required(
            &google_calendar3::Error::BadRequest(not_found)
        ));
        assert!(!is_full_resync_required(
            &google_calendar3::Error::MissingAPIKey
        ));
    }

    #[test]
    fn classifies_an_expired_cursor_as_needing_a_full_resync() {
        // Errors reach the engine wrapped in context, so classification has to
        // look through the chain.
        let err = anyhow::Error::new(google_calendar3::Error::BadRequest(expired_token_body()))
            .context("events.list");
        assert!(matches!(classify(err), SyncError::FullResyncRequired));
    }

    #[test]
    fn classifies_a_refused_consent_prompt_as_needing_auth() {
        let err = anyhow::anyhow!("{REAUTH_REQUIRED}: run lazycal interactively once to sign in");
        assert!(matches!(classify(err), SyncError::NeedsAuth));
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn all_day(start: NaiveDate, end: Option<NaiveDate>) -> Event {
        Event {
            id: Some("e1".to_owned()),
            summary: Some("Onam".to_owned()),
            start: Some(EventDateTime {
                date: Some(start),
                ..Default::default()
            }),
            end: end.map(|date| EventDateTime {
                date: Some(date),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// A confirmed, timed event with no detail fields set, for tests that care
    /// about one field at a time.
    fn event_fixture() -> Event {
        Event {
            id: Some("e1".to_owned()),
            summary: Some("Standup".to_owned()),
            start: Some(EventDateTime {
                date: Some(date(2026, 10, 12)),
                ..Default::default()
            }),
            location: Some("Room 4".to_owned()),
            ..Default::default()
        }
    }

    fn upserted(change: Change) -> ProviderEvent {
        match change {
            Change::Upsert(event) => *event,
            Change::Delete { .. } => panic!("expected an upsert"),
        }
    }

    #[test]
    fn maps_an_all_day_event_keeping_the_exclusive_end() {
        let event =
            to_change(&all_day(date(2026, 8, 26), Some(date(2026, 8, 27)))).expect("mapped");
        let event = upserted(event);
        assert_eq!(event.start, EventTime::Date(date(2026, 8, 26)));
        assert_eq!(event.end, EventTime::Date(date(2026, 8, 27)));
        assert_eq!(event.summary, "Onam");
    }

    #[test]
    fn synthesises_an_all_day_end_that_is_missing_or_not_after_the_start() {
        for end in [None, Some(date(2026, 8, 26)), Some(date(2026, 8, 25))] {
            let event = upserted(to_change(&all_day(date(2026, 8, 26), end)).expect("mapped"));
            assert_eq!(
                event.end,
                EventTime::Date(date(2026, 8, 27)),
                "an unusable end ({end:?}) must still leave a matchable range"
            );
        }
    }

    #[test]
    fn maps_a_timed_event_and_tolerates_a_zero_length_one() {
        let at = |h: u32| {
            date(2026, 8, 26)
                .and_hms_opt(h, 0, 0)
                .expect("valid time")
                .and_utc()
        };
        let timed = |end: Option<DateTime<Utc>>| Event {
            id: Some("e1".to_owned()),
            summary: Some("Standup".to_owned()),
            start: Some(EventDateTime {
                date_time: Some(at(9)),
                ..Default::default()
            }),
            end: end.map(|at| EventDateTime {
                date_time: Some(at),
                ..Default::default()
            }),
            ..Default::default()
        };

        let event = upserted(to_change(&timed(Some(at(10)))).expect("mapped"));
        assert_eq!(event.start, EventTime::Instant(at(9)));
        assert_eq!(event.end, EventTime::Instant(at(10)));

        // No usable end falls back to the start rather than inverting the range.
        let event = upserted(to_change(&timed(None)).expect("mapped"));
        assert_eq!(event.end, EventTime::Instant(at(9)));
    }

    #[test]
    fn maps_a_cancelled_event_to_a_tombstone() {
        let event = Event {
            status: Some("cancelled".to_owned()),
            ..all_day(date(2026, 8, 26), Some(date(2026, 8, 27)))
        };
        assert_eq!(
            to_change(&event),
            Some(Change::Delete {
                id: "e1".to_owned()
            })
        );
    }

    #[test]
    fn skips_an_event_without_an_id() {
        let event = Event {
            id: None,
            ..all_day(date(2026, 8, 26), Some(date(2026, 8, 27)))
        };
        assert_eq!(to_change(&event), None);
    }

    #[test]
    fn resolves_the_event_colour_from_googles_palette() {
        let tomato = Event {
            color_id: Some("11".to_owned()),
            ..all_day(date(2026, 8, 26), Some(date(2026, 8, 27)))
        };
        assert_eq!(
            upserted(to_change(&tomato).expect("mapped"))
                .color
                .as_deref(),
            Some("#d60000")
        );

        // An unknown id falls back to the calendar's colour rather than failing.
        let unknown = Event {
            color_id: Some("99".to_owned()),
            ..all_day(date(2026, 8, 26), Some(date(2026, 8, 27)))
        };
        assert_eq!(upserted(to_change(&unknown).expect("mapped")).color, None);
    }

    #[test]
    fn carries_tentative_status_through() {
        let event = Event {
            status: Some("tentative".to_owned()),
            ..all_day(date(2026, 8, 26), Some(date(2026, 8, 27)))
        };
        assert_eq!(
            upserted(to_change(&event).expect("mapped")).status,
            EventStatus::Tentative
        );
    }

    #[test]
    fn detects_reauthorization_through_wrapped_context() {
        // The marker has to survive the context layers the error picks up on
        // its way out of the sync engine.
        let err = anyhow::anyhow!("{REAUTH_REQUIRED}: run lazycal interactively once to sign in")
            .context("Token retrieval failed")
            .context("calendarList.list");
        assert!(needs_reauthorization(&err));
    }

    #[test]
    fn leaves_unrelated_failures_alone() {
        let err = anyhow::anyhow!("connection reset by peer").context("events.list");
        assert!(!needs_reauthorization(&err));
        assert!(!needs_reauthorization(&anyhow::anyhow!("404 not found")));
    }

    #[test]
    fn encodes_a_calendar_id_that_would_otherwise_truncate_the_path() {
        // '#' would be read as a fragment delimiter and lose the rest.
        let encoded = encode_calendar_id("en.indian#holiday@group.v.calendar.google.com");
        assert!(!encoded.contains('#'));
        assert!(encoded.contains("%23"));
    }

    #[test]
    fn leaves_plain_ids_recoverable() {
        let id = "harshsurani@gmail.com";
        let encoded = encode_calendar_id(id);
        // Over-encoding is fine; the server decodes back to the original.
        assert_eq!(
            percent_encoding::percent_decode_str(&encoded)
                .decode_utf8()
                .expect("utf-8"),
            id
        );
    }

    #[test]
    fn a_plain_description_is_left_exactly_as_it_is() {
        // Angle brackets in prose are content, not markup, and eating them
        // would quietly corrupt the text.
        assert_eq!(
            plain_text("budget < 500 > last year"),
            "budget < 500 > last year"
        );
        assert_eq!(plain_text("  padded  "), "padded");
    }

    #[test]
    fn google_html_descriptions_become_readable_text() {
        let html = "<div>Standup.<br>Agenda in the <a href=\"http://x\">doc</a>.</div>";
        assert_eq!(plain_text(html), "Standup.\nAgenda in the doc.");
    }

    #[test]
    fn entities_are_unescaped_once() {
        assert_eq!(plain_text("<p>Tom &amp; Jerry</p>"), "Tom & Jerry");
        assert_eq!(plain_text("<p>a&nbsp;b</p>"), "a b");
        // The literal text "&lt;" was written as "&amp;lt;", and must survive
        // as "&lt;" rather than unescaping twice into "<".
        assert_eq!(plain_text("<p>&amp;lt;</p>"), "&lt;");
    }

    #[test]
    fn runs_of_empty_markup_collapse() {
        let html = "<div>One</div><div><br></div><div><br></div><div>Two</div>";
        assert_eq!(plain_text(html), "One\n\nTwo");
    }

    #[test]
    fn an_unclosed_angle_bracket_is_kept_as_content() {
        assert_eq!(plain_text("<p>see <attachment"), "see <attachment");
    }

    #[test]
    fn an_event_carries_its_detail_fields_across() {
        let mut event = event_fixture();
        event.description = Some("<p>Notes &amp; more</p>".to_owned());
        event.location = Some("  Room 4  ".to_owned());
        event.organizer = Some(EventOrganizer {
            display_name: Some("Lead".to_owned()),
            email: Some("lead@example.com".to_owned()),
            ..Default::default()
        });
        event.attendees = Some(vec![
            EventAttendee {
                email: Some("lead@example.com".to_owned()),
                response_status: Some("accepted".to_owned()),
                self_: Some(true),
                ..Default::default()
            },
            // A room booking, which names nobody and is dropped.
            EventAttendee {
                resource: Some(true),
                ..Default::default()
            },
        ]);

        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.description.as_deref(), Some("Notes & more"));
        assert_eq!(mapped.location.as_deref(), Some("Room 4"));
        assert_eq!(
            mapped.organizer.as_ref().map(|person| person.label()),
            Some("Lead")
        );
        assert_eq!(mapped.attendees.len(), 1, "the resource row is dropped");
        assert_eq!(mapped.attendees[0].response, Response::Accepted);
        assert!(mapped.attendees[0].is_self);
    }

    #[test]
    fn an_unanswered_invitation_defaults_to_needing_action() {
        let mut event = event_fixture();
        event.attendees = Some(vec![EventAttendee {
            email: Some("someone@example.com".to_owned()),
            response_status: None,
            ..Default::default()
        }]);
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.attendees[0].response, Response::NeedsAction);
    }
}
