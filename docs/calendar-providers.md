# Multi-provider calendar support: research summary

How `lazycal` can grow from Google-only to supporting Outlook, CalDAV and
plain `.ics` feeds.

Researched 2026-10-03 by three teams (one coordinator and three researchers
each). Raw material is in the session scratchpad as `google-*.md`,
`outlook-*.md` and `other-*.md`. Where this document says **verified**, it was
checked directly against the code in this repo or against crates.io, not just
reported by a researcher.

---

## Recommendation in one paragraph

Do the **provider trait refactor first** (2–4 days) while the codebase is still
~2,000 lines — it is the cheapest it will ever be, and every other item depends
on it. Then add **Outlook** (5–8 days), because it is the largest user
population and a second real backend is what proves the abstraction is honest.
Then build the **iCalendar/timezone/recurrence layer** (2–3 weeks), which is the
real price of admission for everything else, and which buys **ICS/webcal feeds**
cheaply (3–5 days) and **CalDAV** after it (2–3 weeks). CalDAV is one
implementation that reaches iCloud, Fastmail, Nextcloud, Radicale/Baikal, Zimbra
and SOGo.

---

## The single biggest finding

**Google and Outlook expand recurring events server-side. CalDAV does not.**

`lazycal` has never had to understand recurrence: `events_call()` passes
`single_events(true)` and Google returns individual instances
(verified in `src/sync.rs`). Microsoft Graph's `calendarView` behaves the same
way.

CalDAV servers return the master `VEVENT` with `RRULE`/`RDATE`/`EXDATE` plus
separate `RECURRENCE-ID` override events. The optional `<expand>` in
`calendar-query` is unevenly supported and does not work with `sync-collection`.
So the client must expand series itself, including exception merging, moved and
cancelled instances, DST edges, and **re-expansion every time the sync window
slides**.

Team Other estimates this at roughly **40% of the total multi-provider effort
and most of the bugs**. It is the main reason CalDAV is weeks rather than days.

**Architectural recommendation:** put expansion *inside* the CalDAV provider, so
the trait contract stays "returns expanded instances". The sync engine, the
cache schema and the whole TUI then stay unchanged, and recurrence stays one
module's problem instead of leaking through the app.

---

## Per-provider findings

### Google (already implemented)

| | |
|---|---|
| Status | Working; the model for the abstraction |
| Coupling to remove | `events::apply()` takes `google_calendar3::api::Event` directly; `sync.rs` matches `google_calendar3::Error` for 410 |
| Auth | Loopback/installed flow, PKCE, device code. OOB deprecated |
| Sync | Opaque `syncToken`; `timeMin`/`timeMax` forbidden alongside it, so the window is fixed at first full sync |
| Quota | 600 requests per user per minute |
| Push | `watch` channels need a public HTTPS endpoint — not viable for a local CLI. Polling only; keep it out of the trait |

Known operational wart, already handled in code: refresh tokens expire after
7 days while the OAuth app's publishing status is "Testing".

### Outlook / Microsoft 365

| | |
|---|---|
| Setup burden | **Lower than Google.** One Entra app registered by the project as a public multi-tenant client, client ID shipped; users just sign in. Google makes every user create their own GCP project |
| Caveat | Locked-down corporate tenants may demand admin approval — keep a bring-your-own-client-ID fallback |
| Auth | Authorization code + PKCE (loopback) or device code. `Calendars.Read` + `offline_access`. Access token ~60 min; refresh ~14 days sliding, **rotating** — the new refresh token must be persisted each time |
| Sync | `calendarView/delta`. Follow `@odata.nextLink` until `@odata.deltaLink` |
| Deletions | `@removed` objects carrying **only an id** — tombstone handling must key on id alone |
| Throttling | 429 + `Retry-After`; needs backoff the Google path doesn't have |

Two mismatches the trait must absorb:

1. **The time window is baked into the delta token.** Graph cannot widen a
   window on an existing token, so extending the horizon *always* forces a full
   resync. On Google that is an occasional cost; on Graph it is permanent.
2. **The cursor is a full URL**, not an opaque token — so the trait's cursor
   type must be an opaque string, not something token-shaped.

All-day end dates are **exclusive, same as Google** — the trap I expected did
not materialise. The real trap is the format: Graph returns `dateTime` strings
plus a separate `timeZone` rather than bare dates, so you must branch on
`isAllDay` and truncate, never convert through UTC. Also: **no per-event
colour** in Graph (calendar-level colour plus named categories), so the neutral
model cannot assume Google's per-event `colorId`.

### CalDAV, and what it unlocks

One implementation reaches **iCloud, Fastmail, Nextcloud/ownCloud,
Radicale/Baikal, Zimbra, SOGo, Cyrus**. Suggested test order: Radicale (Docker
in CI) → Nextcloud → Fastmail → iCloud.

- Discovery: `/.well-known/caldav` → `current-user-principal` →
  `calendar-home-set`, with DNS SRV or a manual URL as fallback.
- Sync: `sync-collection` (RFC 6578) returns an opaque per-calendar token —
  this **matches the existing engine's model well**. Deletions arrive as a 404
  status on the member href; an expired token returns 412, mapping onto the
  existing "token rejected → full resync" path.
- But `sync-collection` has **no time window**, so the 90/365-day window must be
  applied client-side.
- Deletions carry only an href, so the cache must store **href and ETag per
  event** — a schema change.
- Auth in practice is Basic over TLS with app-specific passwords; credentials
  need keyring or a command hook. OAuth-over-CalDAV varies per provider — skip.

**Out of reach:** Proton (end-to-end encrypted, no API), Zoho (unreliable
CalDAV; would need a bespoke REST provider), Calendly (a scheduling product, not
a feed).

### ICS / webcal subscriptions — the cheap win

A plain `GET` (`webcal://` → `https://`), polled with `ETag`/`If-Modified-Since`,
whole feed replaced and diffed by UID. No auth, no tombstones, no write path.
Covers holidays, sports fixtures, university timetables and shared team
calendars. **3–5 days once the iCalendar parser exists** — but it does still
need the recurrence and timezone layer, because timetable feeds are heavily
recurring and often use floating times.

---

## The neutral model (reconciled across all three teams)

Google alone would not have revealed most of this.

**Calendar:** `provider`, `account`, `id` (opaque), `name`, `colour`
(resolved `#rrggbb`, optional), `is_primary`, `read_only`, plus local
`selected`, `cursor`, `covered_window`.

**Event:** calendar key, `id`, `summary`, `description`, `location`,
`start`/`end`, `status` {Confirmed, Tentative, Cancelled}, `colour`,
`series_id`, `recurrence_id`, `my_rsvp`, `organizer`, `url`, `etag`, `href`.

Four requirements that only appear once you look past Google:

1. **Four kinds of time**, not two: UTC instant, zoned (IANA `TZID`), *floating*
   (render as local wherever you are), and all-day `DATE`. The cache currently
   models two.
2. **Identity is composite.** Google is `(calendar_id, event_id)`; CalDAV needs
   `href` + `UID` + `RECURRENCE-ID` to distinguish a master from its overrides.
3. **Per-provider sync state**, not one shape: opaque token (Google, CalDAV),
   deltaLink URL (Outlook), CTag or whole-feed hash (ICS).
4. **Colour is not universal** — per-event on Google, calendar-level on Outlook,
   non-standard on CalDAV (only Apple's `calendar-color`).

⚠️ **Schema migration warning:** the storage key must become
`(provider, account, calendar_id, event_id)`, otherwise ids from different
providers can collide. Changing a primary key in SQLite needs a **table
rebuild** — the existing `add_column_if_missing()` helper cannot do it.

---

## Crate licences — independently verified on crates.io

All GPL-3.0 compatible except where flagged.

| Crate | Version | Licence | Updated | Role |
|---|---|---|---|---|
| `icalendar` | 0.17.14 | MIT/Apache-2.0 | 2026-09-29 | iCalendar parsing — actively maintained |
| `rrule` | 0.14.0 | MIT OR Apache-2.0 | 2025-04-20 | Recurrence expansion — ⚠️ ~17 months stale |
| `libdav` | 0.11.0 | ISC | 2026-09-05 | CalDAV client — only ~49k downloads |
| `chrono-tz` | 0.10.4 | MIT OR Apache-2.0 | 2025-07-11 | IANA timezones |
| `oauth2` | 5.0.0 | MIT OR Apache-2.0 | 2025-01-21 | Outlook auth |
| `quick-xml` | 0.42.0 | MIT | 2026-08-22 | If hand-rolling CalDAV |
| `ical` | 0.11.0 | **non-standard** ⚠️ | 2024-03-13 | Suggested as a fallback parser — **avoid**: unclear licence and stale |

Recommended: hand-roll Outlook with `reqwest` + `oauth2`. **Skip
`graph-rs-sdk`** — community-maintained (not Microsoft, despite a researcher's
claim), heavy, with reported build failures. No crate maps Windows timezone
names to IANA; forcing UTC via `Prefer: outlook.timezone="UTC"` sidesteps it.

---

## Effort

| Piece | Estimate |
|---|---|
| Provider trait refactor | 2–4 days |
| Outlook provider | 5–8 days |
| iCalendar + timezone + recurrence layer | 2–3 weeks |
| ICS/webcal provider (on top of that layer) | 3–5 days |
| CalDAV provider | 2–3 weeks |
| Auth and setup UX | ~1 week |
| Per-server test matrix | ~1 week |

Solo-developer estimates, and the recurrence layer is the one most likely to
overrun.

## Top risks

1. **Recurrence correctness**, especially re-expansion as the window slides.
2. **Schema migration** — a primary-key change needs a table rebuild, and
   `sync.rs` holds the trickiest invariants (token, horizon, prune,
   transactional resync).
3. **Per-server CalDAV quirks** — missing `sync-collection`, iCloud auth,
   SOGo deletion edge cases.
4. **Over-generalising too early**: design the trait against Google *and*
   Outlook, but don't finalise it before CalDAV's constraints are real.
5. Owning an Entra registration and handling corporate consent blocks.

## Claims rejected during review

Recorded because they were plausible and wrong:

- *"`singleEvents` cannot be used with a sync token."* **False, verified in
  this repo:** `events_call()` sets `single_events(true)` and the incremental
  path adds `sync_token()`; this has synced a real account repeatedly. It only
  has to stay consistent between initial and incremental calls.
- *"`graph-rs-sdk` is Microsoft-maintained."* It is community-maintained.
- *"CalDAV covers 80%+ of addressable users."* Unsupported — Google and Outlook
  dominate; CalDAV's real win is Apple plus the self-hosting and privacy crowd.
- *"Google deprecated CalDAV"* and *"Microsoft offers OAuth-over-CalDAV."*
  Both unverified and doubted by the reviewing coordinator.
- *"Calendly needs a 2–3 day REST provider."* Unsupported; it is a scheduling
  product, not a calendar feed.
- Market-share percentages and Fastmail/Proton user counts in the raw files are
  researcher inference, not sourced figures.

## Open decisions

1. Ship a verified Google client ID and an Entra registration, or keep
   bring-your-own for both? Who owns verification and its renewal?
2. Multiple accounts per provider in scope, or one account each for now?
3. Design the write path now, or keep the trait read-only and add a separate
   write capability later?
4. When the window outgrows its horizon: immediate full resync, or a background
   one?
5. Keyring or file-based credential storage for CalDAV Basic auth?
