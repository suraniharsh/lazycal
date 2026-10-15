//! Finding the link on an event, and handing it to the system browser.
//!
//! Both halves are wary on purpose. The text comes from a calendar
//! invitation, which means it comes from whoever sent it: anyone who can put
//! an event on your calendar can put a string here. So the URL is never
//! handed to a shell, and only schemes a browser is expected to handle are
//! opened at all.

use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::db::events::EventDetail;

/// Schemes worth opening.
///
/// An allowlist rather than a blocklist. `xdg-open` dispatches on scheme and
/// will happily act on `file://`, and a desktop handler for some scheme
/// nobody thought about is exactly the kind of thing an invitation from a
/// stranger shouldn't get to reach.
const OPENABLE: [&str; 2] = ["https://", "http://"];

/// The link to join an event, in the order worth trying.
///
/// `conference_url` is the one the service told us about. The others are
/// there because most invitations don't come from lazycal: an external
/// organiser's Zoom or Teams link arrives as plain text in the location or
/// the body, with nothing machine-readable about it.
pub fn find(detail: &EventDetail) -> Option<&str> {
    all(detail).into_iter().next()
}

/// Every link on an event, best first, deduplicated.
///
/// `links` is filled during sync, which is the only time the HTML body is
/// available — an `<a href>` is gone by the time the text reaches the cache.
/// The scan of the location and description is still here for a cache written
/// before that column existed and not yet resynced.
pub fn all(detail: &EventDetail) -> Vec<&str> {
    let mut links: Vec<&str> = Vec::new();
    let candidates = detail
        .conference_url
        .as_deref()
        .into_iter()
        .chain(detail.links.iter().map(String::as_str))
        .chain(detail.location.as_deref().into_iter().flat_map(urls_in))
        .chain(detail.description.as_deref().into_iter().flat_map(urls_in));
    for url in candidates {
        if is_openable(url) && !links.contains(&url) {
            links.push(url);
        }
    }
    links
}

/// Whether this is something to hand to a browser.
pub fn is_openable(url: &str) -> bool {
    let lowered = url.to_ascii_lowercase();
    // A scheme with nothing after it is not an address.
    OPENABLE
        .iter()
        .find_map(|scheme| lowered.strip_prefix(scheme))
        .is_some_and(|host| !host.is_empty())
        // Whitespace or a control character means this was mis-parsed out of
        // surrounding prose, or is trying to be two arguments.
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The first openable URL in a block of free text.
pub fn first_url(text: &str) -> Option<&str> {
    urls_in(text).into_iter().next()
}

/// Every openable URL in a block of free text, in the order they appear.
///
/// Scans for a scheme *anywhere* rather than splitting on whitespace, because
/// the most common way a meeting link arrives has no space before it:
/// Outlook writes `Click here to join the meeting<https://teams...>`, and a
/// whitespace split sees one token beginning with `Click`.
///
/// Where a URL ends is the whole difficulty. It runs to the first character
/// that cannot appear in one — whitespace, `<`, `>`, `"` — and then has its
/// trailing punctuation trimmed, so a link at the end of a sentence doesn't
/// keep the full stop.
pub fn urls_in(text: &str) -> Vec<&str> {
    let lowered = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut at = 0;

    while at < text.len() {
        // Find the next scheme, whichever comes first.
        let Some(start) = OPENABLE
            .iter()
            .filter_map(|scheme| lowered[at..].find(scheme).map(|found| at + found))
            .min()
        else {
            break;
        };

        let end = text[start..]
            .find(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"'))
            .map_or(text.len(), |offset| start + offset);
        let candidate = trim_punctuation(&text[start..end]);
        if is_openable(candidate) && !found.contains(&candidate) {
            found.push(candidate);
        }
        // Past this one even if it was rejected, so a malformed scheme can't
        // make this loop forever.
        at = end.max(start + 1);
    }
    found
}

/// Drops the punctuation a link picks up from the sentence around it, so
/// `(see https://example.com/a).` yields `https://example.com/a`.
///
/// Closing brackets only come off when the link doesn't contain the matching
/// opener, because a wiki-style URL can legitimately end in one.
fn trim_punctuation(candidate: &str) -> &str {
    let mut trimmed = candidate.trim_start_matches(['<', '"', '\'', '`', '(', '[', '{']);
    // One loop rather than a trim per character class, so punctuation that
    // arrives interleaved — `"https://example.com",` — comes off whole.
    while let Some(last) = trimmed.chars().next_back() {
        let opener = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '`' | '>' => None,
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => break,
        };
        if opener.is_some_and(|opener| trimmed.contains(opener)) {
            break;
        }
        trimmed = &trimmed[..trimmed.len() - last.len_utf8()];
    }
    trimmed
}

/// Hands `url` to whatever opens links on this system.
///
/// Spawned and not waited on: the opener may be a browser that runs for
/// hours. Its streams are discarded because the terminal belongs to the
/// alternate screen — `xdg-open` warning about a missing handler would
/// otherwise scribble across the calendar.
pub fn open(url: &str) -> Result<()> {
    open_with(opener(), url)
}

/// [`open`], with the command named.
///
/// Split out so the tests can exercise the spawn itself against a program
/// that does nothing. Without this seam the only way to cover this path would
/// be to open a browser on whoever is running the suite.
pub fn open_with(opener: &str, url: &str) -> Result<()> {
    if !is_openable(url) {
        bail!("not a link lazycal will open: {url}");
    }

    Command::new(opener)
        // One argument, no shell: the string came from an invitation, and
        // `sh -c` would make its contents executable.
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running {opener}"))?;
    Ok(())
}

/// The platform's open-this command.
const fn opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        // Not `start`, which is a shell builtin rather than a program.
        "explorer"
    } else {
        "xdg-open"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(
        conference: Option<&str>,
        location: Option<&str>,
        description: Option<&str>,
    ) -> EventDetail {
        EventDetail {
            conference_url: conference.map(str::to_owned),
            links: Vec::new(),
            location: location.map(str::to_owned),
            description: description.map(str::to_owned),
            ..EventDetail::default()
        }
    }

    #[test]
    fn the_outlook_angle_bracket_form_is_found() {
        // The commonest way a meeting link arrives, and the case a
        // whitespace split cannot see: no space before the bracket.
        assert_eq!(
            first_url("Click here to join the meeting<https://teams.microsoft.com/l/x>"),
            Some("https://teams.microsoft.com/l/x")
        );
    }

    #[test]
    fn every_link_in_a_body_is_listed_in_order() {
        let text = "Click here to join the meeting<https://teams.microsoft.com/l/x>\n\
                    Learn More<https://aka.ms/JoinTeamsMeeting>\n\
                    Agenda: https://notes.example.com/a.";
        assert_eq!(
            urls_in(text),
            [
                "https://teams.microsoft.com/l/x",
                "https://aka.ms/JoinTeamsMeeting",
                "https://notes.example.com/a",
            ]
        );
    }

    #[test]
    fn the_same_link_twice_is_listed_once() {
        let text = "https://example.com/a and again https://example.com/a";
        assert_eq!(urls_in(text), ["https://example.com/a"]);
    }

    #[test]
    fn a_url_with_an_encoded_query_survives_intact() {
        // A Teams link carries percent-encoded JSON, including `%22` quotes.
        let url = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_a@thread.v2/0?context=%7b%22Tid%22%3a%22b%22%7d";
        assert_eq!(urls_in(&format!("join<{url}>")), [url]);
        assert_eq!(urls_in(&format!("join {url} now")), [url]);
    }

    #[test]
    fn text_with_no_links_yields_none_and_terminates() {
        for text in [
            "",
            "no links here",
            "http",
            "https:/",
            "mailto:a@b.com",
            "<<<>>>",
        ] {
            assert!(urls_in(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn a_rejected_scheme_does_not_stall_the_scan() {
        // `https://` alone is not openable; the scan has to move past it and
        // still find the real link after it.
        assert_eq!(
            urls_in("broken https:// then https://example.com/a"),
            ["https://example.com/a"]
        );
    }

    #[test]
    fn every_link_on_an_event_is_offered_best_first() {
        let detail = EventDetail {
            conference_url: Some("https://meet.google.com/abc".to_owned()),
            links: vec![
                "https://meet.google.com/abc".to_owned(),
                "https://notes.example.com/a".to_owned(),
            ],
            location: Some("https://maps.example.com/office".to_owned()),
            description: Some("also https://notes.example.com/a".to_owned()),
            ..EventDetail::default()
        };
        assert_eq!(
            all(&detail),
            [
                "https://meet.google.com/abc",
                "https://notes.example.com/a",
                "https://maps.example.com/office",
            ],
            "deduplicated, conferencing first"
        );
        assert_eq!(find(&detail), Some("https://meet.google.com/abc"));
    }

    #[test]
    fn a_cache_written_before_the_links_column_still_works() {
        // `links` is empty until the next full sync, so the scan of the
        // location and description is still the fallback.
        let detail = EventDetail {
            links: Vec::new(),
            location: Some("https://zoom.example.com/j/1".to_owned()),
            description: Some("notes at https://notes.example.com/a".to_owned()),
            ..EventDetail::default()
        };
        assert_eq!(
            all(&detail),
            [
                "https://zoom.example.com/j/1",
                "https://notes.example.com/a"
            ]
        );
    }

    #[test]
    fn a_link_is_handed_to_the_opener() {
        // `true` stands in for the browser: the point is that the spawn
        // itself works and isn't waited on.
        assert!(open_with("true", "https://example.com/a").is_ok());
    }

    #[test]
    fn a_missing_opener_is_reported_rather_than_ignored() {
        let err = open_with("lazycal-no-such-opener", "https://example.com/a")
            .expect_err("there is no such program");
        assert!(
            err.to_string().contains("lazycal-no-such-opener"),
            "the message names what it tried to run: {err}"
        );
    }

    #[test]
    fn the_scheme_is_checked_before_anything_is_run() {
        // The opener here would succeed if it were ever reached.
        assert!(open_with("true", "file:///etc/passwd").is_err());
        assert!(open_with("true", "javascript:alert(1)").is_err());
    }

    #[test]
    fn the_services_own_link_wins() {
        let detail = detail(
            Some("https://meet.google.com/abc"),
            Some("https://zoom.example.com/j/1"),
            Some("https://notes.example.com"),
        );
        assert_eq!(find(&detail), Some("https://meet.google.com/abc"));
    }

    #[test]
    fn a_link_pasted_into_the_location_is_found() {
        // Most invitations don't come from lazycal, and an external
        // organiser's link arrives as plain text.
        let detail = detail(None, Some("https://zoom.example.com/j/1"), None);
        assert_eq!(find(&detail), Some("https://zoom.example.com/j/1"));
    }

    #[test]
    fn a_link_in_the_body_is_found_when_nothing_better_is_there() {
        let detail = detail(
            None,
            Some("Meeting room 2"),
            Some("Agenda in the doc.\nJoin: https://teams.example.com/l/42\nSee you there."),
        );
        assert_eq!(find(&detail), Some("https://teams.example.com/l/42"));
    }

    #[test]
    fn an_event_with_no_link_anywhere_has_none() {
        assert_eq!(
            find(&detail(None, Some("Meeting room 2"), Some("Bring notes."))),
            None
        );
        assert_eq!(find(&EventDetail::default()), None);
    }

    #[test]
    fn punctuation_from_the_sentence_around_a_link_comes_off() {
        for (text, expected) in [
            ("see https://example.com/a.", "https://example.com/a"),
            ("(https://example.com/a)", "https://example.com/a"),
            ("<https://example.com/a>", "https://example.com/a"),
            ("\"https://example.com/a\",", "https://example.com/a"),
            ("https://example.com/a;", "https://example.com/a"),
        ] {
            assert_eq!(first_url(text), Some(expected), "{text:?}");
        }
    }

    #[test]
    fn a_bracket_that_belongs_to_the_link_is_kept() {
        assert_eq!(
            first_url("https://example.com/Foo_(bar)"),
            Some("https://example.com/Foo_(bar)")
        );
    }

    #[test]
    fn only_browser_schemes_are_openable() {
        for url in [
            "https://example.com/a",
            "http://example.com/a",
            "HTTPS://EXAMPLE.COM/A",
        ] {
            assert!(is_openable(url), "{url:?} should open");
        }
        // An invitation is attacker-supplied text. None of these reach a
        // browser, and `open` refuses them a second time at the door.
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "vscode://file/etc/passwd",
            "smb://host/share",
            "ssh://host",
            "mailto:someone@example.com",
            "https://",
            "https:// example.com",
            "https://example.com/a\nhttps://evil.example.com",
            "https://example.com/a\u{0}",
            "",
            "Meeting room 2",
        ] {
            assert!(!is_openable(url), "{url:?} must not open");
            assert!(open(url).is_err(), "{url:?} must be refused");
        }
    }

    #[test]
    fn a_scheme_hidden_behind_prose_is_not_opened() {
        // `find` only ever returns something `is_openable` accepted, so a
        // body full of `file://` paths yields nothing rather than the path.
        let detail = detail(None, None, Some("logs at file:///var/log/x and notes"));
        assert_eq!(find(&detail), None);
    }

    #[test]
    fn a_conference_url_that_is_not_a_link_is_skipped_rather_than_trusted() {
        let detail = detail(
            Some("file:///etc/passwd"),
            Some("https://example.com/a"),
            None,
        );
        assert_eq!(find(&detail), Some("https://example.com/a"));
    }
}
