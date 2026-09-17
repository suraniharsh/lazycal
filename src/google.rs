use std::future::Future;
use std::path::Path;
use std::pin::Pin;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
