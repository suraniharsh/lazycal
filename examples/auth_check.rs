//! Dev-only: verify Google OAuth + calendarList.list() against the real
//! account before any UI/sync code depends on it.
//! Run with: cargo run --example auth_check

use anyhow::Result;
use lazycal::{config, google};

#[tokio::main]
async fn main() -> Result<()> {
    let paths = config::resolve()?;
    println!("client secret: {}", paths.client_secret.display());
    println!("token cache:   {}", paths.token_cache.display());

    let hub = google::connect(
        &paths.client_secret,
        &paths.token_cache,
        google::Interactive::Yes,
    )
    .await?;
    let calendars = google::list_calendars(&hub).await?;

    println!("\n{} calendar(s):", calendars.len());
    for cal in &calendars {
        println!(
            "  - {} (id: {}, selected: {:?}, primary: {:?})",
            cal.summary.as_deref().unwrap_or("<no summary>"),
            cal.id.as_deref().unwrap_or("<no id>"),
            cal.selected,
            cal.primary,
        );
    }

    Ok(())
}
