use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::{Context, Result};
use chrono::{DateTime, Days, NaiveDate, Utc};
use google_calendar3::api::{
    CalendarListEntry, ConferenceData, ConferenceSolutionKey, CreateConferenceRequest, Event,
    EventAttendee, EventDateTime, EventListCall,
};
use google_calendar3::hyper_rustls::{self, HttpsConnector};
use google_calendar3::hyper_util::client::legacy::Client;
use google_calendar3::hyper_util::client::legacy::connect::HttpConnector;
use google_calendar3::hyper_util::rt::TokioExecutor;
use google_calendar3::{CalendarHub, yup_oauth2};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use yup_oauth2::{InstalledFlowAuthenticator, InstalledFlowReturnMethod};

use crate::provider::{
    Attendee, CalendarProvider, Change, EventStatus, EventTime, NewEvent, Person, ProviderCalendar,
    ProviderEvent, Response, SyncError, SyncKind, SyncOutcome, Window, WritableCalendar,
};

/// Reads everything: the calendar list, and the events on each calendar.
///
/// Requested explicitly on every call, because yup-oauth2 caches tokens per
/// requested scope set — letting individual calls fall back to the generated
/// client's own per-method scope hint would ask for a fresh consent for each
/// distinct scope encountered.
pub const SCOPE_READ: &str = "https://www.googleapis.com/auth/calendar.readonly";

/// Creates and edits events. Does **not** cover `calendarList.list`, which
/// accepts only the read, full, or calendar-list scopes — so writing needs
/// this *in addition to* [`SCOPE_READ`], never instead of it.
pub const SCOPE_WRITE: &str = "https://www.googleapis.com/auth/calendar.events";

/// What the signed-in session is allowed to do.
///
/// Read-only is the default because calendar visibility is a local preference
/// that is never written back, so most users never need more, and a narrower
/// scope is a smaller consent screen and a smaller blast radius.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Access {
    #[default]
    ReadOnly,
    ReadWrite,
}

impl Access {
    /// The scopes to request. Every call asks for the whole set rather than
    /// only what it needs, so that one cached token covers reads and writes
    /// and the user is asked to consent once.
    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::ReadOnly => &[SCOPE_READ],
            Self::ReadWrite => &[SCOPE_READ, SCOPE_WRITE],
        }
    }
}

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
pub async fn list_calendars(hub: &Hub, access: Access) -> Result<Vec<CalendarListEntry>> {
    let mut calendars = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        let mut call = hub
            .calendar_list()
            .list()
            .max_results(CALENDAR_PAGE_SIZE)
            .add_scopes(access.scopes());
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
    access: Access,
}

impl GoogleProvider {
    pub async fn connect(
        client_secret_path: &Path,
        token_cache_path: &Path,
        interactive: Interactive,
        access: Access,
    ) -> Result<Self> {
        Ok(Self {
            hub: connect(client_secret_path, token_cache_path, interactive).await?,
            access,
        })
    }

    fn events_call(&self, encoded_id: &str) -> EventListCall<'_, Connector> {
        let access = self.access;
        self.hub
            .events()
            .list(encoded_id)
            // Recurring events are expanded server-side into instances, so the
            // rest of lazycal never has to understand RRULE.
            .single_events(true)
            // Incremental sync reports deletions as cancelled events.
            .show_deleted(true)
            .max_results(EVENT_PAGE_SIZE)
            .add_scopes(access.scopes())
    }
}

impl CalendarProvider for GoogleProvider {
    fn can_write(&self) -> bool {
        self.access == Access::ReadWrite
    }

    async fn list_calendars(&self) -> Result<Vec<ProviderCalendar>, SyncError> {
        let entries = list_calendars(&self.hub, self.access)
            .await
            .map_err(classify)?;
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
        // "reader" and "freeBusyReader" cannot be written to. An absent role
        // is treated as read-only: refusing to offer a calendar is recoverable,
        // offering one that rejects every write is not.
        writable: matches!(entry.access_role.as_deref(), Some("owner" | "writer")),
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
        conference_url: conference_url(event),
        links: links(event),
        attendees: event
            .attendees
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(to_attendee)
            .collect(),
    })))
}

/// How many links to keep from one event.
///
/// A long invitation can carry a dozen — unsubscribe footers, help pages,
/// privacy notices — and the picker is for choosing between a handful.
const MAX_LINKS: usize = 8;

/// Every link on an event, in the order a reader would meet them.
///
/// The `href` attributes come first and they are the point of this function.
/// Google stores a description composed in any rich-text client as HTML, and
/// a Teams or Zoom invitation written there puts the real URL in an `href`
/// behind words like "Click here to join the meeting". `plain_text` keeps the
/// words and drops the tag, so by the time anything downstream sees the
/// description the URL is simply gone. This is the only place it still
/// exists.
fn links(event: &Event) -> Vec<String> {
    let raw = event.description.as_deref().unwrap_or_default();
    // The sweep reads the *rendered* text, not the markup. Scanning the
    // markup finds every URL twice — once decoded out of its `href` and once
    // still escaped in the source — and picks up `src` attributes besides.
    let readable = plain_text(raw);
    let location = event.location.as_deref().unwrap_or_default();

    let mut links: Vec<String> = Vec::new();
    let candidates = conference_url(event)
        .into_iter()
        .chain(hrefs(raw))
        .chain(
            crate::link::urls_in(&readable)
                .into_iter()
                .map(str::to_owned),
        )
        .chain(
            crate::link::urls_in(location)
                .into_iter()
                .map(str::to_owned),
        );

    for url in candidates {
        if crate::link::is_openable(&url) && !links.contains(&url) {
            links.push(url);
        }
        if links.len() == MAX_LINKS {
            break;
        }
    }
    links
}

/// The `href` of every anchor in some HTML, in document order.
///
/// A deliberately small scanner rather than a parser: it looks for `href=`
/// inside a tag and takes the quoted value. Anything it can't make sense of
/// is skipped, and `urls_in` still sweeps the text afterwards.
fn hrefs(html: &str) -> Vec<String> {
    let lowered = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut at = 0;

    while let Some(offset) = lowered[at..].find("href") {
        let name_at = at + offset;
        let start = name_at + "href".len();
        at = start;
        // `data-xhref=` ends in "href" without being it, so the character
        // before has to be one that can't be part of an attribute name.
        if html[..name_at]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_alphanumeric() || matches!(before, '-' | '_' | ':'))
        {
            continue;
        }
        let rest = &html[start..];
        let Some(equals) = rest.find('=') else { break };
        // Only whitespace may sit between the name and the `=`, or this was
        // some other attribute that merely ends in "href".
        if !rest[..equals].chars().all(char::is_whitespace) {
            continue;
        }
        let value = rest[equals + 1..].trim_start();
        let url = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or_default(),
            // Unquoted, which is legal HTML and ends at whitespace or `>`.
            Some(_) => value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or_default(),
            None => break,
        };
        at = start + equals + 1 + url.len();
        if !url.is_empty() {
            found.push(unescape(url));
        }
    }
    found
}

/// The URL to join an event's video call.
///
/// `conferenceData` is the current field and carries one entry point per way
/// in — a URL, a phone number, a SIP address — so the video one is picked out
/// by type. `hangoutLink` is the older field, still populated for Meet, and
/// is the fallback for anything synced before conferencing was read.
fn conference_url(event: &Event) -> Option<String> {
    event
        .conference_data
        .as_ref()
        .and_then(|data| data.entry_points.as_deref())
        .unwrap_or_default()
        .iter()
        .find(|entry| entry.entry_point_type.as_deref() == Some("video"))
        .and_then(|entry| text(entry.uri.as_deref()))
        .or_else(|| text(event.hangout_link.as_deref()))
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

/// Whether `err` is Google refusing the request rather than failing it: the
/// token is good but doesn't carry the scope, or the account may not write to
/// this calendar (a subscribed holiday feed, or one shared read-only).
fn is_forbidden(err: &google_calendar3::Error) -> bool {
    match err {
        google_calendar3::Error::BadRequest(body) => {
            matches!(body["error"]["code"].as_i64(), Some(401 | 403))
        }
        google_calendar3::Error::Failure(response) => {
            matches!(response.status().as_u16(), 401 | 403)
        }
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
        Some(google) if is_forbidden(google) => SyncError::Forbidden,
        _ => SyncError::Other(err),
    }
}

/// Maps a draft onto the shape `events.insert` wants.
fn to_google_event(id: &str, event: &NewEvent) -> Event {
    let slot = |time: EventTime| match time {
        EventTime::Date(date) => EventDateTime {
            date: Some(date),
            ..Default::default()
        },
        EventTime::Instant(at) => EventDateTime {
            date_time: Some(at),
            ..Default::default()
        },
    };
    Event {
        id: Some(id.to_owned()),
        summary: Some(event.summary.clone()),
        start: Some(slot(event.start)),
        end: Some(slot(event.end)),
        location: event.location.clone(),
        description: event.description.clone(),
        attendees: (!event.guests.is_empty()).then(|| {
            event
                .guests
                .iter()
                .map(|email| EventAttendee {
                    email: Some(email.clone()),
                    ..Default::default()
                })
                .collect()
        }),
        conference_data: event.conference.then(|| ConferenceData {
            create_request: Some(CreateConferenceRequest {
                // The event id doubles as the request id, which is what makes
                // a retry after a lost response reuse the same conference
                // instead of booking a second one.
                request_id: Some(id.to_owned()),
                conference_solution_key: Some(ConferenceSolutionKey {
                    type_: Some("hangoutsMeet".to_owned()),
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// What to tell Google about notifying people.
///
/// An invitation nobody is told about is worse than no invitation, so a guest
/// list means mail goes out. With nobody invited there is nobody to notify,
/// and `"none"` is passed explicitly rather than left to default because the
/// API documents it as having effects of its own.
fn send_updates(event: &NewEvent) -> &'static str {
    if event.guests.is_empty() {
        "none"
    } else {
        "all"
    }
}

fn midnight_utc(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("00:00 is a valid time of day")
        .and_utc()
}

impl WritableCalendar for GoogleProvider {
    async fn create_event(
        &self,
        calendar_id: &str,
        id: &str,
        event: &NewEvent,
    ) -> Result<String, SyncError> {
        if !self.can_write() {
            // Asking anyway would spend a round trip to be told the same, and
            // the message Google returns is less useful than this one.
            return Err(SyncError::Forbidden);
        }

        let (_, created) = self
            .hub
            .events()
            .insert(to_google_event(id, event), &encode_calendar_id(calendar_id))
            .send_updates(send_updates(event))
            // Without this the request succeeds with the `conferenceData`
            // silently dropped: no error, no link. Set whether or not a call
            // was asked for, so the failure mode can't come back.
            .conference_data_version(1)
            .add_scopes(self.access.scopes())
            .doit()
            .await
            .context("events.insert")
            .map_err(classify)?;

        // Google echoes the id back; falling back to the one we chose keeps
        // the caller able to find the event either way. The conference URL is
        // deliberately not read here: Google generates it asynchronously and
        // the insert response often has none yet. The resync that follows a
        // save is what picks it up.
        Ok(created.id.unwrap_or_else(|| id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use google_calendar3::api::{EntryPoint, EventOrganizer};

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

    /// A minimal event to create, which each test then varies.
    fn new_event() -> NewEvent {
        NewEvent {
            summary: "Design review".to_owned(),
            start: EventTime::Date(NaiveDate::from_ymd_opt(2026, 10, 20).expect("valid date")),
            end: EventTime::Date(NaiveDate::from_ymd_opt(2026, 10, 21).expect("valid date")),
            location: None,
            description: None,
            guests: Vec::new(),
            conference: false,
        }
    }

    #[test]
    fn guests_become_attendees() {
        let mut event = new_event();
        event.guests = vec!["sam@example.com".to_owned(), "alex@example.com".to_owned()];
        let sent = to_google_event("abc", &event);
        let attendees = sent.attendees.expect("an attendee list");
        assert_eq!(
            attendees
                .iter()
                .map(|attendee| attendee.email.as_deref().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["sam@example.com", "alex@example.com"]
        );
    }

    #[test]
    fn inviting_nobody_sends_no_attendee_list_at_all() {
        // An empty array and an absent field mean the same thing to Google,
        // but the absent one is what an uninvited event actually is.
        assert!(to_google_event("abc", &new_event()).attendees.is_none());
    }

    #[test]
    fn a_guest_list_means_google_sends_the_invitations() {
        let mut event = new_event();
        assert_eq!(send_updates(&event), "none", "nobody to notify");
        event.guests = vec!["sam@example.com".to_owned()];
        assert_eq!(
            send_updates(&event),
            "all",
            "an invitation nobody is told about is worse than none"
        );
    }

    #[test]
    fn asking_for_a_video_call_sends_a_meet_create_request() {
        let mut event = new_event();
        event.conference = true;
        let sent = to_google_event("abc123", &event);
        let request = sent
            .conference_data
            .expect("conference data")
            .create_request
            .expect("a create request");
        assert_eq!(
            request.conference_solution_key.and_then(|key| key.type_),
            Some("hangoutsMeet".to_owned())
        );
        assert_eq!(
            request.request_id.as_deref(),
            Some("abc123"),
            "the event id doubles as the request id, so a retry reuses the call"
        );
    }

    #[test]
    fn an_event_with_no_video_call_sends_no_conference_data() {
        assert!(
            to_google_event("abc", &new_event())
                .conference_data
                .is_none()
        );
    }

    /// A Teams invitation as Google stores one composed in Outlook: HTML,
    /// with the real URL behind the words "Click here to join the meeting".
    const TEAMS_URL: &str = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_NzY4OTRkYTk@thread.v2/0?context=%7b%22Tid%22%3a%22aaaa%22%7d";

    #[test]
    fn a_teams_link_behind_anchor_text_survives_the_sync() {
        // This is the case that was silently lost: `plain_text` keeps the
        // words and drops the tag, so without `links` the URL reached nothing
        // downstream and the event looked like it had no way in.
        let mut event = event_fixture();
        event.description = Some(format!(
            "<div>Microsoft Teams meeting</div>\
             <div><a href=\"{TEAMS_URL}\">Click here to join the meeting</a></div>\
             <div>Meeting ID: 123 456 789</div>"
        ));
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.links, [TEAMS_URL.to_owned()]);
        assert!(
            mapped
                .description
                .as_deref()
                .is_some_and(|text| text.contains("Click here to join the meeting")),
            "the readable text is still the readable text"
        );
        assert!(
            mapped
                .description
                .as_deref()
                .is_some_and(|text| !text.contains("teams.microsoft.com")),
            "and the URL is not inlined into it"
        );
    }

    #[test]
    fn the_outlook_plain_text_form_is_found_too() {
        // Not every invitation is HTML. Outlook's text version puts the URL
        // in angle brackets with no space before them.
        let mut event = event_fixture();
        event.description = Some(format!(
            "Microsoft Teams meeting\nClick here to join the meeting<{TEAMS_URL}>\nMeeting ID: 1"
        ));
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.links, [TEAMS_URL.to_owned()]);
    }

    #[test]
    fn every_link_on_an_invitation_is_kept_in_reading_order() {
        let mut event = event_fixture();
        event.location = Some("https://maps.example.com/office".to_owned());
        event.description = Some(format!(
            "<a href=\"{TEAMS_URL}\">Click here to join the meeting</a>\
             <a href=\"https://aka.ms/JoinTeamsMeeting\">Learn More</a>\
             <div>Agenda: https://notes.example.com/agenda.</div>"
        ));
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(
            mapped.links,
            [
                TEAMS_URL.to_owned(),
                "https://aka.ms/JoinTeamsMeeting".to_owned(),
                "https://notes.example.com/agenda".to_owned(),
                "https://maps.example.com/office".to_owned(),
            ],
            "hrefs, then the body text, then the location"
        );
    }

    #[test]
    fn the_conferencing_link_leads_the_list() {
        let mut event = event_fixture();
        event.hangout_link = Some("https://meet.google.com/abc-defg-hij".to_owned());
        event.description = Some("<a href=\"https://notes.example.com/x\">Notes</a>".to_owned());
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.links[0], "https://meet.google.com/abc-defg-hij");
    }

    #[test]
    fn a_link_written_twice_is_listed_once() {
        // Google's own invitations put the Meet URL in `hangoutLink`, in an
        // anchor, and again as the anchor's text.
        let mut event = event_fixture();
        event.hangout_link = Some("https://meet.google.com/abc".to_owned());
        event.description = Some(
            "<a href=\"https://meet.google.com/abc\">https://meet.google.com/abc</a>".to_owned(),
        );
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.links, ["https://meet.google.com/abc".to_owned()]);
    }

    #[test]
    fn a_hostile_scheme_in_an_href_is_not_collected() {
        let mut event = event_fixture();
        event.description = Some(
            "<a href=\"file:///etc/passwd\">Open</a><a href=\"javascript:alert(1)\">Click</a>\
             <a href=\"https://example.com/ok\">Fine</a>"
                .to_owned(),
        );
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.links, ["https://example.com/ok".to_owned()]);
    }

    #[test]
    fn an_html_entity_in_an_href_is_decoded() {
        // Google escapes the query separators when it stores the body.
        let mut event = event_fixture();
        event.description =
            Some("<a href=\"https://example.com/a?x=1&amp;y=2\">Join</a>".to_owned());
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.links, ["https://example.com/a?x=1&y=2".to_owned()]);
    }

    #[test]
    fn the_href_scanner_copes_with_the_shapes_html_allows() {
        assert_eq!(
            hrefs("<a href='https://example.com/a'>x</a>"),
            ["https://example.com/a".to_owned()],
            "single quotes"
        );
        assert_eq!(
            hrefs("<a href=https://example.com/a>x</a>"),
            ["https://example.com/a".to_owned()],
            "unquoted"
        );
        assert_eq!(
            hrefs("<a  href =  \"https://example.com/a\" >x</a>"),
            ["https://example.com/a".to_owned()],
            "whitespace around the equals"
        );
        assert!(
            hrefs("<div data-xhref=\"https://example.com/a\">x</div>").is_empty(),
            "an attribute that merely ends in href is not one"
        );
        assert!(hrefs("no markup at all").is_empty());
        assert!(hrefs("<a href=\"\">empty</a>").is_empty());
        assert!(hrefs("<a href").is_empty(), "truncated, and must not loop");
    }

    #[test]
    fn a_long_invitation_does_not_keep_every_link_it_has() {
        let mut event = event_fixture();
        let many: String = (0..20)
            .map(|index| format!("<a href=\"https://example.com/{index}\">x</a>"))
            .collect();
        event.description = Some(many);
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.links.len(), MAX_LINKS);
    }

    #[test]
    fn an_event_with_nothing_to_open_has_no_links() {
        let mut event = event_fixture();
        event.description = Some("Bring notes. Room 2.".to_owned());
        event.location = Some("Meeting room 2".to_owned());
        let mapped = upserted(to_change(&event).expect("a change"));
        assert!(mapped.links.is_empty());
    }

    #[test]
    fn an_older_hangout_link_is_still_read_as_the_way_in() {
        // `hangoutLink` predates `conferenceData` and is still populated for
        // Meet, so it remains the fallback.
        let mut event = event_fixture();
        event.location = None;
        event.hangout_link = Some("https://meet.google.com/abc-defg-hij".to_owned());
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(
            mapped.conference_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert_eq!(mapped.location, None, "a link is not a place");
    }

    #[test]
    fn the_video_entry_point_is_preferred_over_the_phone_one() {
        let mut event = event_fixture();
        event.conference_data = Some(ConferenceData {
            entry_points: Some(vec![
                EntryPoint {
                    entry_point_type: Some("phone".to_owned()),
                    uri: Some("tel:+1-555-0100".to_owned()),
                    ..Default::default()
                },
                EntryPoint {
                    entry_point_type: Some("video".to_owned()),
                    uri: Some("https://meet.google.com/xyz-1234-abc".to_owned()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        });
        // Set too, and ignored: `conferenceData` is the current field.
        event.hangout_link = Some("https://meet.google.com/stale".to_owned());
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(
            mapped.conference_url.as_deref(),
            Some("https://meet.google.com/xyz-1234-abc")
        );
    }

    #[test]
    fn a_room_and_a_link_both_survive() {
        // The two used to share one field, and the room won.
        let mut event = event_fixture();
        event.location = Some("Meeting room 2".to_owned());
        event.hangout_link = Some("https://meet.google.com/abc-defg-hij".to_owned());
        let mapped = upserted(to_change(&event).expect("a change"));
        assert_eq!(mapped.location.as_deref(), Some("Meeting room 2"));
        assert_eq!(
            mapped.conference_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
    }

    #[test]
    fn an_event_with_no_conferencing_has_no_join_url() {
        let mapped = upserted(to_change(&event_fixture()).expect("a change"));
        assert_eq!(mapped.conference_url, None);
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
