//! Dev tool: run one sync against the real account and report what landed in
//! the cache. Run with: cargo run --example sync_once

use anyhow::Result;
use lazycal::db::calendars;
use lazycal::{config, db, google, sync};

#[tokio::main]
async fn main() -> Result<()> {
    let paths = config::resolve()?;
    println!("database: {}", paths.database.display());

    let hub = google::connect(
        &paths.client_secret,
        &paths.token_cache,
        google::Interactive::Yes,
    )
    .await?;
    let conn = db::open_shared(&paths.database)?;

    let report = sync::sync_all(&hub, &conn).await?;
    println!("synced {} calendar(s)", report.synced);
    if !report.is_complete() {
        println!("failed: {}", report.failed.join(", "));
    }

    let guard = conn.lock().expect("db lock poisoned");
    println!("\ncached:");
    for calendar in calendars::list_calendars(&guard)? {
        let events: i64 = guard.query_row(
            "SELECT COUNT(*) FROM events WHERE calendar_id = ?1",
            [&calendar.id],
            |row| row.get(0),
        )?;
        let state = calendars::sync_state(&guard, &calendar.id)?;
        println!(
            "  {:<32} {events:>4} events  token: {:<5} horizon: {}",
            calendar.summary,
            if state.token.is_some() { "set" } else { "none" },
            state
                .horizon
                .map_or_else(|| "-".to_owned(), |date| date.to_string()),
        );
    }

    Ok(())
}
