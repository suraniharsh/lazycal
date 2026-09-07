use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Days, NaiveDate, Utc};
use google_calendar3::api::EventListCall;

use crate::db::calendars::SyncState;
use crate::db::{SharedConnection, calendars, events};
use crate::google::{self, Connector, Hub};

/// How far back and forward a full sync fetches.
const BACKFILL_DAYS: u64 = 90;
const HORIZON_DAYS: u64 = 365;
/// Google forbids sending `timeMin`/`timeMax` alongside a sync token, so an
/// incremental sync can never widen the window a full sync established. Once
/// the stored horizon is this close, a fresh full sync extends it.
const HORIZON_REFRESH_DAYS: i64 = 60;

const EVENT_PAGE_SIZE: i32 = 250;

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

pub async fn sync_all(hub: &Hub, conn: &SharedConnection) -> Result<SyncReport> {
    // Must be the complete list: anything missing from it is treated below as
    // removed from the account and pruned.
    let listed = google::list_calendars(hub).await?;

    let mut report = SyncReport::default();
    let mut present = HashSet::with_capacity(listed.len());

    for entry in &listed {
        let Some(id) = entry.id.clone() else { continue };
        present.insert(id.clone());

        let summary = entry.summary.clone().unwrap_or_else(|| id.clone());
        {
            let guard = conn.lock().expect("db lock poisoned");
            calendars::upsert_calendar(
                &guard,
                &id,
                &summary,
                entry.background_color.as_deref(),
                entry.primary.unwrap_or(false),
            )?;
        }

        // One unreachable calendar (revoked access, a transient error) must not
        // stop the rest from syncing.
        match sync_calendar_events(hub, conn, &id).await {
            Ok(()) => report.synced += 1,
            Err(err) => {
                tracing::warn!("syncing calendar {id}: {err:#}");
                report.failed.push(summary);
            }
        }
    }

    prune_removed_calendars(conn, &present)?;
    Ok(report)
}

async fn sync_calendar_events(hub: &Hub, conn: &SharedConnection, id: &str) -> Result<()> {
    let state = {
        let guard = conn.lock().expect("db lock poisoned");
        calendars::sync_state(&guard, id)?
    };
    let today = Utc::now().date_naive();

    let window_is_running_out = state
        .horizon
        .is_none_or(|horizon| (horizon - today).num_days() < HORIZON_REFRESH_DAYS);
    if state.token.is_none() || window_is_running_out {
        return full_resync(hub, conn, id, today).await;
    }

    let token = state.token.as_deref().expect("checked above");
    match incremental_sync(hub, conn, id, token, state.horizon).await {
        Err(err) if needs_full_resync(&err) => {
            // Drop the rejected token first: if the resync then fails, the next
            // run must not retry the same dead token forever.
            {
                let guard = conn.lock().expect("db lock poisoned");
                calendars::clear_sync_token(&guard, id)?;
            }
            tracing::info!("sync token for {id} expired; falling back to a full resync");
            full_resync(hub, conn, id, today).await
        }
        result => result,
    }
}

/// Refetches the whole window and replaces the calendar's cached events.
///
/// A full resync starts a new baseline, so Google won't report deletions that
/// happened while the client was away — the old rows have to be dropped, and
/// that happens in the same transaction as the reinsert so a failure can't
/// leave the calendar empty.
async fn full_resync(hub: &Hub, conn: &SharedConnection, id: &str, today: NaiveDate) -> Result<()> {
    let horizon = today
        .checked_add_days(Days::new(HORIZON_DAYS))
        .unwrap_or(NaiveDate::MAX);
    let from = today
        .checked_sub_days(Days::new(BACKFILL_DAYS))
        .unwrap_or(NaiveDate::MIN);
    let encoded_id = google::encode_calendar_id(id);

    let mut fetched = Vec::new();
    let mut page_token: Option<String> = None;
    let next_sync_token;

    loop {
        let mut call = events_call(hub, &encoded_id)
            .time_min(midnight_utc(from))
            .time_max(midnight_utc(horizon));
        if let Some(token) = &page_token {
            call = call.page_token(token);
        }

        let (_, page) = call.doit().await.context("events.list (full sync)")?;
        fetched.extend(page.items.into_iter().flatten());

        match page.next_page_token {
            Some(token) => page_token = Some(token),
            None => {
                next_sync_token = page.next_sync_token;
                break;
            }
        }
    }

    let guard = conn.lock().expect("db lock poisoned");
    let tx = guard.unchecked_transaction()?;
    events::delete_for_calendar(&tx, id)?;
    for event in &fetched {
        events::apply(&tx, id, event)?;
    }
    calendars::set_sync_state(
        &tx,
        id,
        &SyncState {
            token: next_sync_token,
            horizon: Some(horizon),
        },
    )?;
    tx.commit()?;

    Ok(())
}

/// Applies only what changed since `token`, keeping the window `horizon`
/// established by the last full sync.
async fn incremental_sync(
    hub: &Hub,
    conn: &SharedConnection,
    id: &str,
    token: &str,
    horizon: Option<NaiveDate>,
) -> Result<()> {
    let encoded_id = google::encode_calendar_id(id);
    let mut page_token: Option<String> = None;
    let next_sync_token;

    loop {
        let mut call = events_call(hub, &encoded_id).sync_token(token);
        if let Some(token) = &page_token {
            call = call.page_token(token);
        }

        let (_, page) = call
            .doit()
            .await
            .context("events.list (incremental sync)")?;

        {
            // One transaction per page rather than per event, so a large delta
            // doesn't mean thousands of separate commits.
            let guard = conn.lock().expect("db lock poisoned");
            let tx = guard.unchecked_transaction()?;
            for event in page.items.iter().flatten() {
                events::apply(&tx, id, event)?;
            }
            tx.commit()?;
        }

        match page.next_page_token {
            Some(token) => page_token = Some(token),
            None => {
                next_sync_token = page.next_sync_token;
                break;
            }
        }
    }

    let guard = conn.lock().expect("db lock poisoned");
    calendars::set_sync_state(
        &guard,
        id,
        &SyncState {
            token: next_sync_token,
            horizon,
        },
    )?;
    Ok(())
}

fn events_call<'a>(hub: &'a Hub, encoded_id: &str) -> EventListCall<'a, Connector> {
    hub.events()
        .list(encoded_id)
        // Recurring events are expanded server-side into individual instances.
        .single_events(true)
        // Incremental sync reports deletions as cancelled events.
        .show_deleted(true)
        .max_results(EVENT_PAGE_SIZE)
        .add_scope(google::SCOPE)
}

/// Calendars can leave the account (unsubscribed, deleted, access revoked);
/// without this they would stay cached forever, since a sync only upserts what
/// the listing currently returns.
fn prune_removed_calendars(conn: &SharedConnection, present: &HashSet<String>) -> Result<()> {
    let guard = conn.lock().expect("db lock poisoned");
    for id in calendars::list_ids(&guard)? {
        if !present.contains(&id) {
            calendars::delete_calendar(&guard, &id)?;
            tracing::info!("pruned calendar no longer on the account: {id}");
        }
    }
    Ok(())
}

fn needs_full_resync(err: &anyhow::Error) -> bool {
    err.downcast_ref::<google_calendar3::Error>()
        .is_some_and(is_full_resync_required)
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

fn midnight_utc(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight exists for every date")
        .and_utc()
}

#[cfg(test)]
mod tests {
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
        let err = google_calendar3::Error::BadRequest(expired_token_body());
        assert!(is_full_resync_required(&err));
    }

    #[test]
    fn recognises_full_sync_required_without_a_code() {
        let body = serde_json::json!({
            "error": { "errors": [{ "reason": "fullSyncRequired" }] }
        });
        assert!(is_full_resync_required(
            &google_calendar3::Error::BadRequest(body)
        ));
    }

    #[test]
    fn leaves_other_errors_alone() {
        let not_found = serde_json::json!({
            "error": { "code": 404, "errors": [{ "reason": "notFound" }] }
        });
        assert!(!is_full_resync_required(
            &google_calendar3::Error::BadRequest(not_found)
        ));

        assert!(!is_full_resync_required(
            &google_calendar3::Error::MissingAPIKey
        ));
        assert!(!is_full_resync_required(
            &google_calendar3::Error::Cancelled
        ));
    }

    #[test]
    fn a_full_resync_is_detected_through_anyhow_context() {
        // Errors reach the caller wrapped in context, so the check has to look
        // through the chain.
        let err = anyhow::Error::new(google_calendar3::Error::BadRequest(expired_token_body()))
            .context("events.list (incremental sync)");
        assert!(needs_full_resync(&err));
    }

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
