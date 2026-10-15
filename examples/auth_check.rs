//! Dev-only: verify Google OAuth + calendarList.list() against the real
//! account before any UI/sync code depends on it.
//! Run with: cargo run --example auth_check

use anyhow::Result;
use lazycal::provider::CalendarProvider;
use lazycal::{config, google};

#[tokio::main]
async fn main() -> Result<()> {
    let paths = config::resolve()?;
    println!("client secret: {}", paths.client_secret.display());
    println!("token cache:   {}", paths.token_cache.display());

    let provider = google::GoogleProvider::connect(
        &paths.client_secret,
        &paths.token_cache,
        google::Interactive::Yes,
        // These only read; granting write here would ask for consent the
        // example doesn't need.
        google::Access::ReadOnly,
    )
    .await?;
    let calendars = provider.list_calendars().await?;

    println!("\n{} calendar(s):", calendars.len());
    for cal in &calendars {
        println!(
            "  - {} (id: {}, color: {}, primary: {})",
            cal.name,
            cal.id,
            cal.color.as_deref().unwrap_or("-"),
            cal.is_primary,
        );
    }

    Ok(())
}
