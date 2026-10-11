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
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProviderCalendar {
    pub id: String,
    pub name: String,
    /// Already resolved to `#rrggbb`.
    pub color: Option<String>,
    pub is_primary: bool,
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
pub trait CalendarProvider {
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
