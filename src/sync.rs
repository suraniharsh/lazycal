use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{Days, NaiveDate, Utc};

use crate::db::calendars::SyncState;
use crate::db::{SharedConnection, calendars, events};
use crate::provider::{CalendarProvider, SyncError, SyncKind, Window};

/// How far back and forward a full sync fetches.
const BACKFILL_DAYS: u64 = 90;
const HORIZON_DAYS: u64 = 365;
/// A provider generally can't widen the window of an existing cursor, so the
/// window a full sync established is fixed until the next one. Once the stored
/// horizon is this close, resync from scratch to push it further out.
const HORIZON_REFRESH_DAYS: i64 = 60;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub synced: usize,
    /// Calendars whose events failed to sync; their details are logged.
    pub failed: Vec<String>,
}

impl SyncReport {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

pub async fn sync_all(
    provider: &impl CalendarProvider,
    conn: &SharedConnection,
) -> Result<SyncReport> {
    // Must be the complete list: anything missing from it is treated below as
    // removed from the account and pruned.
    let listed = provider.list_calendars().await?;

    let mut report = SyncReport::default();
    let mut present = HashSet::with_capacity(listed.len());

    for calendar in &listed {
        present.insert(calendar.id.clone());
        {
            let guard = conn.lock().expect("db lock poisoned");
            calendars::upsert_calendar(
                &guard,
                &calendar.id,
                &calendar.name,
                calendar.color.as_deref(),
                calendar.is_primary,
            )?;
        }

        // One unreachable calendar (revoked access, a transient error) must not
        // stop the rest from syncing.
        match sync_calendar(provider, conn, &calendar.id).await {
            Ok(()) => report.synced += 1,
            Err(err) => {
                tracing::warn!("syncing calendar {}: {err:#}", calendar.id);
                report.failed.push(calendar.name.clone());
            }
        }
    }

    prune_removed_calendars(conn, &present)?;
    Ok(report)
}

async fn sync_calendar(
    provider: &impl CalendarProvider,
    conn: &SharedConnection,
    id: &str,
) -> Result<()> {
    let state = {
        let guard = conn.lock().expect("db lock poisoned");
        calendars::sync_state(&guard, id)?
    };

    let today = Utc::now().date_naive();
    let window = Window {
        from: today
            .checked_sub_days(Days::new(BACKFILL_DAYS))
            .unwrap_or(NaiveDate::MIN),
        to: today
            .checked_add_days(Days::new(HORIZON_DAYS))
            .unwrap_or(NaiveDate::MAX),
    };

    // Resume only when there's a cursor and the window it was issued against
    // still reaches far enough ahead.
    let window_is_running_out = state
        .horizon
        .is_none_or(|horizon| (horizon - today).num_days() < HORIZON_REFRESH_DAYS);
    let cursor = if window_is_running_out {
        None
    } else {
        state.token.as_deref()
    };

    let outcome = match provider.sync_calendar(id, cursor, window).await {
        Err(SyncError::FullResyncRequired) if cursor.is_some() => {
            // Drop the rejected cursor before retrying: if the resync then
            // fails, the next run must not present the same dead cursor again.
            {
                let guard = conn.lock().expect("db lock poisoned");
                calendars::clear_sync_token(&guard, id)?;
            }
            tracing::info!("sync cursor for {id} expired; falling back to a full resync");
            provider.sync_calendar(id, None, window).await?
        }
        other => other?,
    };

    let guard = conn.lock().expect("db lock poisoned");
    let tx = guard.unchecked_transaction()?;

    // A full sync is a new baseline and won't report what was deleted while we
    // were away, so the old rows go in the same transaction as the reinsert.
    if outcome.kind == SyncKind::Full {
        events::delete_for_calendar(&tx, id)?;
    }
    for change in &outcome.changes {
        events::apply(&tx, id, change)?;
    }

    calendars::set_sync_state(
        &tx,
        id,
        &SyncState {
            token: outcome.cursor,
            // An incremental pass reports no window, so the horizon stands.
            horizon: outcome.covered.map(|window| window.to).or(state.horizon),
        },
    )?;
    tx.commit()?;

    Ok(())
}

/// Calendars can leave the account (unsubscribed, deleted, access revoked);
/// without this they would stay cached forever, since a sync only upserts what
/// the listing currently returns.
fn prune_removed_calendars(conn: &SharedConnection, present: &HashSet<String>) -> Result<()> {
    let guard = conn.lock().expect("db lock poisoned");
    for id in calendars::list_ids(&guard)? {
        if !present.contains(&id) {
            calendars::delete_calendar(&guard, &id)
                .with_context(|| format!("pruning calendar {id}"))?;
            tracing::info!("pruned calendar no longer on the account: {id}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_complete_only_without_failures() {
        assert!(
            SyncReport {
                synced: 3,
                failed: vec![]
            }
            .is_complete()
        );
        assert!(
            !SyncReport {
                synced: 3,
                failed: vec!["Work".into()]
            }
            .is_complete()
        );
    }
}
