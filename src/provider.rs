//! The provider-neutral calendar interface.
//!
//! Everything above this boundary — the sync engine, the cache, the UI — is
//! written against these types. Everything a particular service does
//! differently (its auth, its sync cursor, its event shape, its colour scheme)
//! stays behind the [`CalendarProvider`] implementation.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// The span of time a sync should cover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

/// When an event starts or ends: either a whole day, or an instant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventTime {
    /// An all-day event's date. Ends are **exclusive**, matching iCalendar and
    /// both Google and Microsoft Graph.
    Date(NaiveDate),
    Instant(DateTime<Utc>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventStatus {
    Confirmed,
    Tentative,
}

/// How an invitee replied.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Accepted,
    Declined,
    Tentative,
    /// Invited and yet to reply, which is also what a provider that doesn't
    /// report replies at all maps to.
    #[default]
    NeedsAction,
}

/// Someone named on an event. Either field may be missing: a provider can give
/// a bare address with no display name, and Google sends entries with neither
/// for room and resource bookings.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Person {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl Person {
    /// Whether this names nobody, in which case there is nothing to display.
    pub fn is_anonymous(&self) -> bool {
        self.name.is_none() && self.email.is_none()
    }

    /// The best label available: a display name, else the address.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .or(self.email.as_deref())
            .unwrap_or("(unknown)")
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Attendee {
    #[serde(flatten)]
    pub person: Person,
    #[serde(default)]
    pub response: Response,
    /// Set on the signed-in user's own entry, so the UI can say "you".
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_self: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
}

/// Keeps the stored JSON to the fields that carry information; an attendee
/// list can run to hundreds of entries.
fn is_false(value: &bool) -> bool {
    !*value
}

/// An event as the cache wants it, with every provider quirk already resolved.
///
/// Providers must guarantee two things the storage layer relies on and cannot
/// check cheaply: `start` and `end` are the same variant, and `end` is never
/// *before* `start`. Ranges are queried half-open, so an event that doesn't
/// satisfy that would silently never match any window. A zero-length timed
/// event (`end == start`) is allowed and still matches the window it falls in;
/// an all-day event's exclusive end must be at least the day after its start.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProviderEvent {
    pub id: String,
    pub summary: String,
    pub start: EventTime,
    pub end: EventTime,
    pub status: EventStatus,
    /// Already resolved to `#rrggbb`; providers translate their own palettes.
    pub color: Option<String>,
    /// Free text, already plain: providers strip whatever markup their service
    /// stores (Google keeps HTML here for anything created in its web UI).
    pub description: Option<String>,
    pub location: Option<String>,
    pub organizer: Option<Person>,
    /// Where to join, when the event has a video call attached. Kept apart
    /// from `location` because an event can have both a room and a link, and
    /// folding one into the other loses whichever came second.
    pub conference_url: Option<String>,
    /// Every link the event carries, in the order a reader would meet them.
    ///
    /// Collected by the provider rather than scanned out of `description`
    /// later, because this is the only point at which the links are all still
    /// there: a service that sends an HTML body puts the real URL in an
    /// `href`, and nothing but the anchor text survives into the plain text
    /// the cache keeps.
    pub links: Vec<String>,
    /// Empty when nobody was invited, which is every event in a holiday or
    /// fixtures feed.
    pub attendees: Vec<Attendee>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProviderCalendar {
    pub id: String,
    pub name: String,
    /// Already resolved to `#rrggbb`.
    pub color: Option<String>,
    pub is_primary: bool,
    /// Whether the account may add events here. False for a subscribed feed
    /// such as a holiday or fixtures calendar, and for one shared read-only,
    /// so the new-event form can leave those out rather than offering a
    /// target the service is certain to refuse.
    pub writable: bool,
}

/// One change from a sync.
///
/// Incremental sync reports removals as tombstones rather than by omission, so
/// a page is a list of changes, not a list of events.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    Upsert(Box<ProviderEvent>),
    Delete { id: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncKind {
    /// Everything in the window; the caller should clear what it had first.
    Full,
    /// Only what changed since the cursor.
    Incremental,
}

pub struct SyncOutcome {
    pub changes: Vec<Change>,
    /// Opaque resume point — a Google sync token, a Graph `deltaLink` URL, a
    /// CalDAV sync-token. Only meaningful to the provider that issued it.
    pub cursor: Option<String>,
    /// The window actually fetched, which the caller stores so it can tell when
    /// the horizon needs extending. `None` when the provider doesn't track one.
    pub covered: Option<Window>,
    pub kind: SyncKind,
}

#[derive(Debug)]
pub enum SyncError {
    /// The cursor is no longer usable and the caller must resync from scratch.
    FullResyncRequired,
    /// Credentials expired or were revoked; the user has to sign in again.
    NeedsAuth,
    RateLimited {
        retry_after: Option<Duration>,
    },
    /// The credentials are valid but don't carry the access this call needs,
    /// which is what a read-only sign-in gets for a write. Distinct from
    /// [`SyncError::NeedsAuth`] because signing in again changes nothing
    /// unless the user grants more.
    Forbidden,
    Other(anyhow::Error),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FullResyncRequired => write!(f, "sync cursor expired; a full resync is required"),
            Self::NeedsAuth => write!(f, "authorization required"),
            Self::RateLimited { retry_after } => match retry_after {
                Some(wait) => write!(f, "rate limited; retry after {}s", wait.as_secs()),
                None => write!(f, "rate limited"),
            },
            Self::Forbidden => write!(f, "the signed-in account may not make this change"),
            Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Other(err) => err.source(),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for SyncError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

/// A calendar service lazycal can read.
///
/// Implementations own their own paging: `sync_calendar` follows the service's
/// pagination to the end and returns the accumulated changes, because a resume
/// cursor is generally only valid once the whole stream has been consumed.
/// The futures are explicitly `Send` because syncing runs in a spawned task.
/// Spelling that out here means a provider whose future isn't `Send` fails at
/// its own `impl` rather than at the far-away spawn site.
/// An event the user has asked to create, before any service has seen it.
///
/// Deliberately smaller than [`ProviderEvent`]: this is what a person types
/// into a form, not what comes back from a service. No id, because the
/// provider assigns one; no status or colour, because neither is settable
/// here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewEvent {
    pub summary: String,
    pub start: EventTime,
    pub end: EventTime,
    pub location: Option<String>,
    pub description: Option<String>,
    /// Addresses to invite, already validated and deduplicated. Bare
    /// addresses rather than [`Attendee`]s: nobody has replied yet, and a
    /// reply is the only thing that type adds.
    ///
    /// **A non-empty list means the service will email these people.** That
    /// is the point of inviting them, and a provider must not quietly skip it
    /// — an invitation nobody is told about is worse than no invitation.
    pub guests: Vec<String>,
    /// Ask the service to attach its own video call: Google Meet, a Teams
    /// link, whatever the provider hosts. The URL comes back on the created
    /// event rather than being chosen here.
    pub conference: bool,
}

/// A provider that can be written to.
///
/// Separate from [`CalendarProvider`] because read-only providers exist and
/// always will: a subscribed `webcal://` feed is a static file on someone
/// else's server. Implementing this says the *service* supports writing —
/// whether the signed-in session may actually write is
/// [`CalendarProvider::can_write`], which is a property of the credentials.
pub trait WritableCalendar: CalendarProvider {
    /// Creates an event and returns the id the service gave it.
    ///
    /// `id` is supplied by the caller so that a retry after a lost response
    /// doesn't create the event twice. Providers that can't accept a
    /// caller-chosen id may ignore it and return their own.
    fn create_event(
        &self,
        calendar_id: &str,
        id: &str,
        event: &NewEvent,
    ) -> impl Future<Output = Result<String, SyncError>> + Send;
}

pub trait CalendarProvider {
    /// Whether this session may write, which needs both a service that
    /// supports it and credentials that were granted the access. A provider
    /// implementing [`WritableCalendar`] can still answer `false` here after
    /// a read-only sign-in.
    fn can_write(&self) -> bool {
        false
    }

    /// Every calendar on the account.
    ///
    /// Must be complete — the caller treats anything missing as removed from
    /// the account and prunes it from the cache.
    fn list_calendars(
        &self,
    ) -> impl Future<Output = Result<Vec<ProviderCalendar>, SyncError>> + Send;

    /// Changes for one calendar. With no `cursor`, fetches `window` in full.
    fn sync_calendar(
        &self,
        calendar_id: &str,
        cursor: Option<&str>,
        window: Window,
    ) -> impl Future<Output = Result<SyncOutcome, SyncError>> + Send;
}
