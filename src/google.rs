use std::path::Path;

use anyhow::{Context, Result};
use google_calendar3::api::CalendarListEntry;
use google_calendar3::hyper_rustls::{self, HttpsConnector};
use google_calendar3::hyper_util::client::legacy::Client;
use google_calendar3::hyper_util::client::legacy::connect::HttpConnector;
use google_calendar3::hyper_util::rt::TokioExecutor;
use google_calendar3::{CalendarHub, yup_oauth2};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use yup_oauth2::{InstalledFlowAuthenticator, InstalledFlowReturnMethod};

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

pub type Connector = HttpsConnector<HttpConnector>;
pub type Hub = CalendarHub<Connector>;

pub async fn connect(client_secret_path: &Path, token_cache_path: &Path) -> Result<Hub> {
    let secret = yup_oauth2::read_application_secret(client_secret_path)
        .await
        .with_context(|| format!("reading {}", client_secret_path.display()))?;

    let auth = InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
        .persist_tokens_to_disk(token_cache_path)
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
