# lazycal

A fast, offline-first calendar client for the terminal.

`lazycal` starts instantly by rendering from its local cache, then refreshes in
the background. Month, week, day and agenda views, per-calendar visibility
toggles, and colors taken from your own calendar setup.

Google Calendar is the provider supported today; the sync engine is built to
take others.

[![CI](https://github.com/suraniharsh/lazycal/actions/workflows/ci.yml/badge.svg)](https://github.com/suraniharsh/lazycal/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

<!-- These are dumps of `cargo run --example demo`, which renders the real
     interface from invented data. A plain-text dump keeps neither the colours
     nor the cursor highlight, so the running app looks better than this. -->

```text
                                            October 2026
┌ October 2026 ──────────┐┌ Month ─────────────────────────────────────────────────────────────────┐
│Su Mo  Tu We  Th Fr  Sa ││Su        Mo         Tu        We        Th        Fr         Sa        │
│27 28  29 30  1  2   3  ││27        28         29        30        1         2          3         │
│4  5   6  7   8  9   10 ││                                                                        │
│11 12  13 14  15 16  17 ││                                                                        │
│18 19  20 21  22 23  24 ││                                                                        │
│25 26  27 28  29 30  31 ││4         5          6         7         8         9          10        │
│1  2   3  4   5  6   7  ││          ● Sprint … ● Climbi… ● Vendor…                                │
│                        ││          ● Standup                                                     │
└────────────────────────┘│11        12         13        14        15        16         17        │
                          │          Autumn ba… ● Standup ● Standup ● Standup Sam's bir…           │
┌ Calendars (c) ─────────┐│                     ● Pairin… ● Roadma… ● Retro                        │
│■ Work                  ││                     +4 more   ● Dentist                                │
│■ Holidays              ││18        19         20        21        22        23         24        │
│□ On-call rota          ││● Quarte…                      ● Standup ● Train …                      │
│■ Personal              ││                               ● Archit…                                │
│                        ││25        26         27        28        29        30         31        │
│                        ││Conferen…                      ● Standup           ● Dinner … ● Month-… │
│                        ││                               ● Postmo…                                │
│                        ││                                                                        │
│                        ││1         2          3         4         5         6          7         │
│                        ││                                                                        │
│                        ││                                                                        │
└────────────────────────┘└────────────────────────────────────────────────────────────────────────┘
m/w/d/a: view   h/l: day   j/k: week   enter: event   ?: help   q: quit     lazycal v0.1.0  ✓ synced
```

Week and Day are hour grids: events are placed at their start, sized by their
duration, and overlapping ones share the width in lanes. The line across the
grid is the current time.

```text
                                          October 13, 2026
┌ October 2026 ──────────┐┌ Day ───────────────────────────────────────────────────────────────────┐
│Su Mo  Tu We  Th Fr  Sa ││09:00                                                                   │
│27 28  29 30  1  2   3  ││      Standup                                                           │
│4  5   6  7   8  9   10 ││10:00 Pairing: cache eviction                                           │
│11 12  13 14  15 16  17 ││      10:00–11:00                      Design review                    │
│18 19  20 21  22 23  24 ││11:00                                  10:30–11:30                      │
│25 26  27 28  29 30  31 ││                                                                        │
│1  2   3  4   5  6   7  ││12:00                                                                   │
│                        ││      Lunch with Sam                                                    │
└────────────────────────┘│13:00 12:30–13:30                                                       │
                          │                                                                        │
┌ Calendars (c) ─────────┐│14:00 1:1 with Priya                                                    │
│■ Work                  ││                                                                        │
│■ Holidays              ││15:00                                                                   │
│□ On-call rota          ││                                                                        │
│■ Personal              ││16:00                                                                   │
│                        ││      Release 0.2.0                                                     │
│                        ││17:00                                                                   │
│                        ││                                                                        │
└────────────────────────┘└────────────────────────────────────────────────────────────────────────┘
m/w/d/a: view   h/l: day   j/k: time   enter: event   ?: help   q: quit     lazycal v0.1.0  ✓ synced
```

`Tab` walks through the events on screen and `Enter` opens one.

```text
                                          October 13, 2026
┌ October 2026 ──────────┐┌ Day ───────────────────────────────────────────────────────────────────┐
│Su Mo  Tu We  Th Fr  Sa ││10:00 Pairing: cache eviction                                           │
│27 28  29 3┌ Design review ───────────────────────────────────────────────────────────┐           │
│4  5   6  7│Tue 13 Oct 2026 · 10:30–11:30 · 1h                                        │           │
│11 12  13 1│Work                                                                      │           │
│18 19  20 2│                                                                          │           │
│25 26  27 2│Where  Meeting room 2                                                     │           │
│1  2   3  4│                                                                          │           │
│           │Join   https://meet.example.com/demo-abcd-efg                             │           │
└───────────│                                                                          │           │
            │Who    ✓ You (you)                                                        │           │
┌ Calendars │       ✓ Priya (organizer)                                                │           │
│■ Work     │       ? Sam                                                              │           │
│■ Holidays │       ✗ Alex                                                             │           │
│□ On-call r│                                                                          │           │
│■ Personal │Notes  Walk through the hour-grid layout before it goes in.               │           │
│           │                                                                          │           │
│           │       Bring the overlap cases.                                           │           │
│           └ o to open a link · esc to close · tab for the next event ────────────────┘           │
│                        ││19:00                                                                   │
│                        ││                                                                        │
└────────────────────────┘└────────────────────────────────────────────────────────────────────────┘
m/w/d/a: view   h/l: day   j/k: time   enter: event   ?: help   q: quit     lazycal v0.1.0  ✓ synced
```

## Features

- **Instant startup** — renders from the local cache immediately; sync happens
  in a background task, with a live status indicator in the footer.
- **Four views** — Month, plus Week and Day as hour grids with events
  placed and sized by time, and Agenda.
- **Add events and invite people** — opt in with `lazycal --enable-write` and
  `n` opens a form: title, calendar, date, times or all-day, guests, a Google
  Meet link, where and notes. Arrow keys nudge a time and the end follows the
  start; the guest field completes from the people already on your calendar.
  Read-only otherwise, so the default install cannot change your calendar at
  all.
- **12- or 24-hour clock** — `lazycal --clock 12h` switches every time the
  interface shows, and is remembered. The form keeps accepting both.
- **Event details** — `Tab` walks through the events on screen and `Enter`
  opens one: its time and duration, where it is, the link to join if there is
  one, who was invited and how they replied, and the invitation text with its
  markup stripped out.
- **Open a link with one key** — `o` opens the event's link in your browser.
  Links are collected during sync from the event's conferencing, the `href`s
  in its invitation, its text and its location, so a Teams or Zoom invite from
  someone else works as well as a Meet link. With more than one, `o` asks
  which.
- **Tracks what's next** — the meeting happening now, or the next one left
  today, is marked without you doing anything, and the mark moves on by itself
  as each one finishes. Once the day's meetings are over nothing is marked,
  rather than reaching into tomorrow. Picking an event yourself takes over;
  `t` hands it back.
- **Offline capable** — events are cached in SQLite, so navigation works with no
  network.
- **Incremental sync** — uses Google's sync tokens, so refreshes transfer only
  what changed, and falls back to a full resync if a token expires.
- **Per-calendar visibility** — toggle any calendar on/off from the sidebar; the
  choice is stored locally and never overwritten by a sync.
- **Your colors** — per-calendar colors come from Google, and an individual
  event's own color override is honored.
- **Terminal-native theming** — UI chrome uses your terminal's ANSI palette
  instead of hardcoded colors.

`lazycal` is **read-only by default**: it requests only the
`calendar.readonly` scope and cannot change anything. Adding events is opt-in
— see [Writing events](#writing-events).

## Install

Requires Rust 1.88 or newer (2024 edition).

```bash
git clone https://github.com/suraniharsh/lazycal.git
cd lazycal
cargo install --path .
```

Or just run it from the checkout:

```bash
cargo run --release
```

SQLite is compiled in via `rusqlite`'s `bundled` feature, so you only need a C
compiler available at build time.

## Setup

`lazycal` talks to Google as your own OAuth application, so you need to create
one once. It is free and takes a few minutes.

1. **Create a project** at
   [console.cloud.google.com](https://console.cloud.google.com/projectcreate).
2. **Enable the Calendar API** for that project:
   [Google Calendar API → Enable](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com).
3. **Configure the OAuth consent screen**
   ([here](https://console.cloud.google.com/apis/credentials/consent)):
   choose **External**, fill in an app name and your email, and add your own
   Google account under **Test users**. See the note below about publishing
   status before you leave it on "Testing".
4. **Create the credential**
   ([here](https://console.cloud.google.com/apis/credentials)):
   **Create Credentials → OAuth client ID**, application type **Desktop app**,
   then **Download JSON**.
5. **Install it** where `lazycal` looks for it:

   ```bash
   mkdir -p ~/.config/lazycal
   mv ~/Downloads/client_secret_*.json ~/.config/lazycal/client_secret.json
   chmod 600 ~/.config/lazycal/client_secret.json
   ```

On first run a browser window opens for consent. The resulting token is cached,
so later runs need no interaction.

> **Testing status expires your sign-in every 7 days.** While the OAuth consent
> screen's publishing status is **Testing**, Google issues refresh tokens that
> stop working after a week, and `lazycal` will show `⚠ sign-in needed` until
> you run it interactively again. To avoid that, set the publishing status to
> **In production** on the consent screen. Calendar counts as a sensitive
> scope, so Google may show an "unverified app" warning you have to click
> through (fine for personal use) or ask you to submit the app for
> verification.

## Usage

```
lazycal                  # open the interface
lazycal --sync           # sync and exit, for cron
lazycal --offline        # show the cache without syncing
lazycal --enable-write   # allow adding events (asks Google again)
lazycal --read-only      # give that up again
lazycal --clock 12h      # show times as 9:30am, and remember that
lazycal --clock 24h      # back to 09:30 (the default)
lazycal --help
```

`--clock` records the choice in `settings.json` and then carries on opening the
interface, so there is no second command to run. It changes the hour gutter in
Week and Day, the time on each event block, the event details panel, and the
new-event form's own fields — which still accept either convention however
they are displayed, so `2pm` works on a 24-hour clock and `14:00` on a
12-hour one.

```text
                                          October 14, 2026
┌ October 2026 ──────────┐┌ Day ───────────────────────────────────────────────────────────────────┐
│Su Mo  Tu We  Th Fr  Sa ││  9am                                                                   │
│27 28  29 30  1  2   3  ││      Standup                                                           │
│4  5   6  7   8  9   10 ││ 10am                                                                   │
│11 12  13 14  15 16  17 ││                                                                        │
│18 19  20 21  22 23  24 ││ 11am Roadmap workshop                                                  │
│25 26  27 28  29 30  31 ││      11:00am–12:30pm                                                   │
│1  2   3  4   5  6   7  ││ 12pm                                                                   │
│                        ││                                                                        │
└────────────────────────┘│  1pm                                                                   │
                          │                                                                        │
┌ Calendars (c) ─────────┐│  2pm                                                                   │
│■ Work                  ││                                                                        │
│■ Holidays              ││  3pm Dentist                                                           │
│□ On-call rota          ││                                                                        │
│■ Personal              ││  4pm                                                                   │
│                        ││                                                                        │
│                        ││  5pm                                                                   │
│                        ││                                                                        │
└────────────────────────┘└────────────────────────────────────────────────────────────────────────┘
m/w/d/a: view   h/l: day   j/k: time   enter: event   ?: help   q: quit     lazycal v0.1.0  ✓ synced
```

`--sync` never prompts for consent, so it fails fast with a non-zero exit
instead of waiting on a browser when your sign-in has expired — which makes it
safe to schedule:

```cron
*/30 * * * * /usr/local/bin/lazycal --sync >/dev/null
```

## Keys

| Key | Action |
| --- | --- |
| `m` / `w` / `d` / `a` | Month / Week / Day / Agenda view |
| `h` `l` or `←` `→` | Previous / next day |
| `j` `k` or `↓` `↑` | Next / previous week — **scrolls the hour grid** in Week and Day |
| `PgUp` / `PgDn` | Previous / next month |
| `t` | Jump to today, and go back to tracking what's next |
| `Tab` / `Shift-Tab` | Next / previous event on screen |
| `Enter` | Open the event under the cursor |
| `o` | Open the event's link, or choose when it has several |
| `n` | Add an event (needs `--enable-write`) |
| `c` | Focus the calendar sidebar |
| `r` | Sync now |
| `?` | Show all keys |
| `q` or `Ctrl-C` | Quit |

While an event's details are open, `Tab` and `Shift-Tab` move straight on to
the next event, `j` `k` scroll a long description, and `Esc` closes the panel.

While the sidebar is focused:

| Key | Action |
| --- | --- |
| `j` `k` or `↓` `↑` | Move through the calendar list |
| `Space` / `Enter` | Show/hide that calendar |
| `Esc` / `c` | Return to the calendar |

## Writing events

Adding events is off by default. `lazycal` requests the read-only scope, which
Google will not let it use to change anything, so the default install is safe
by construction rather than by good behaviour.

To turn it on:

```bash
lazycal --enable-write
```

That asks Google for `calendar.events` as well, which means a fresh consent
screen, and records the choice in `~/.config/lazycal/settings.json`. Then `n`
opens a form in the interface:

```text
                                          October 14, 2026
┌ October 2026 ──────────┐┌ Day ───────────────────────────────────────────────────────────────────┐
│Su Mo  Tu We  Th Fr  Sa ││09:00                                                                   │
│27 28  29 30  1  2   3  ││      Standup                                                           │
│4  5   6  7   8  9   10 ││10:00                                                                   │
│11 12  13 14  15 16  17 ││                                                                        │
│18 19  20 21  2┌ New event ───────────────────────────────────────────────────────┐               │
│25 26  27 28  2│Title       Pairing on the outbox                                 │               │
│1  2   3  4   5│Calendar    Work                                                  │               │
│               │Date        2026-10-14                                            │               │
└───────────────│All day     no                                                    │               │
                │Start       09:00                                                 │               │
┌ Calendars (c) │End         09:30     30m                                         │               │
│■ Work         │Guests      priya@example.com, sam                                │               │
│■ Holidays     │›           Sam <sam@example.com>                                 │               │
│□ On-call rota │Video call  no                                                    │               │
│■ Personal     │Where                                                             │               │
│               │Notes                                                             │               │
│               └ tab to take the name · ↑↓ to choose · esc to cancel ─────────────┘               │
│                        ││                                                                        │
│                        ││18:00                                                                   │
│                        ││                                                                        │
│                        ││19:00                                                                   │
│                        ││                                                                        │
└────────────────────────┘└────────────────────────────────────────────────────────────────────────┘
m/w/d/a: view   h/l: day   j/k: time   enter: event   ?: help   q: quit     lazycal v0.1.0  ✓ synced
```

| Key | Action |
| --- | --- |
| `Tab` / `Shift-Tab`, `↓` `↑` | Move between fields |
| `←` `→`, `Home`, `End` | Move within a field |
| `←` `→` or `Space` | Change the calendar, or toggle all-day or the video call |
| `↑` `↓` | In **Start** or **End**, nudge the time by 15 minutes |
| `Tab` | In **Guests**, take the highlighted suggestion |
| `Enter` | Save |
| `Esc` | Cancel |

The panel's bottom border says which of these the row you are on responds to.

Moving **Start** moves **End** with it, keeping the gap: a half-hour meeting
nudged an hour later is still a half-hour meeting. The length is shown beside
the end time so you don't have to work it out.

### Guests

Type addresses into **Guests**, separated by commas or spaces. Duplicates are
dropped, and anything that isn't an address is refused before the save goes
anywhere — with the offending text named, and your typing left alone.

As you type, lazycal offers people who are already on your cached events,
matching either their name or their address, most frequently seen first.
`↑`/`↓` choose and `Tab` takes one. These come out of the cache that is
already on disk — nothing extra is fetched, no contacts scope is requested,
and nobody who isn't already on one of your events is suggested. Your own
address is left out of the list.

**Saving an event with guests emails them an invitation.** That is the point
of inviting someone, so lazycal does not offer to do it quietly; the form
counts the people it is about to mail on the row under the fields, and sends
`sendUpdates=all` when that count is more than zero. Google notes that some
mail may go out even when notifications are suppressed, so there is no setting
here that promises silence.

Setting **Video call** to `yes` asks Google for a Meet link. Google generates
it a moment after the event is created, so it appears on the `Join` row of the
event's details once the sync that follows the save has caught up — not
instantly.

`lazycal --read-only` gives the ability up again. That stops lazycal asking
for the scope, but the access you already granted is only truly revoked at
[myaccount.google.com/connections](https://myaccount.google.com/connections).

Some limits worth knowing:

- **Only calendars you can write to are offered** — a subscribed holiday or
  fixtures feed is read-only to your account, so it isn't in the list.
- **A save needs the network.** There is no outbox yet: offline, the save
  fails, says so, and gives you the form back with what you typed still in it.
- **No editing or deleting** existing events yet — including the guest list,
  so an invitation goes out as typed or not at all.
- **Guests are invited, not managed.** You can't mark one optional, see who
  has replied to an event you just created until the next sync, or add a
  guest to an event that already exists.
- All-day events are a single day.
- **No tasks, no out-of-office, no focus time.** Google Tasks is a separate
  API that the Calendar API does not expose at all; out-of-office, focus-time
  and working-location events can only live on a primary calendar and are
  read here as ordinary events.

## Opening links

With an event under the cursor, `o` opens its link in your browser — the way
into a meeting, or whatever a subscribed feed points at, which for a fixtures
or holiday calendar is usually a page about the event. It works from the grid
and from the details panel.

**When an event carries more than one link, `o` asks which one**, and the
footer says `o: open…` with the ellipsis to tell you that in advance. A Teams
invitation is the usual reason: it carries the join link, an `aka.ms` help
page, and often a dial-in page.

```text
                                          October 13, 2026
┌ October 2026 ──────────┐┌ Day ───────────────────────────────────────────────────────────────────┐
│Su Mo  Tu W┌ Design review ───────────────────────────────────────────────────────────┐           │
│27 28  29 3│Tue 13 Oct 2026 · 10:30–11:30 · 1h                                        │           │
│4  5   6  7│Work                                                                      │           │
│11 12  13 1│                                                                          │           │
│18 19  20 2│Where  Meeting room 2                                                     │           │
│25 26  27 2│                                                                          │           │
│1  2   3  4│Join   https://meet.example.com/demo-abcd-efg                             │           │
│           ┌ Open which link? ────────────────────────────────────────────────────────┐           │
└───────────│1. https://meet.example.com/demo-abcd-efg                                 │           │
            │2. https://notes.example.com/design-review                                │           │
┌ Calendars └ enter to open · j/k to choose · esc to cancel ───────────────────────────┘           │
│■ Work     │       ✗ Alex                                                             │           │
│■ Holidays │                                                                          │           │
│□ On-call r│Notes  Walk through the hour-grid layout before it goes in.               │           │
│■ Personal │                                                                          │           │
│           │       Bring the overlap cases.                                           │           │
│           └ o to open a link · esc to close · tab for the next event ────────────────┘           │
│                        ││                                                                        │
└────────────────────────┘└────────────────────────────────────────────────────────────────────────┘
m/w/d/a: view   h/l: day   j/k: time   enter: event   ?: help   q: quit     lazycal v0.1.0  ✓ synced
```

`j`/`k` or the arrows choose, `Enter` opens, a digit opens that row directly,
and `Esc` cancels. A single link opens straight away rather than asking a
question with one answer.

The URL is shown rather than a friendly name, because the host is what tells a
join link from a help page — and inventing labels for links somebody else
wrote would be guessing.

### Where the links come from

They are collected **during sync**, in this order, deduplicated:

1. the conferencing the service attached to the event;
2. every `href` in the invitation's HTML;
3. every link in the invitation's readable text;
4. every link in the **location**.

Step 2 is the one that matters most and the reason this happens at sync time
rather than on demand. Google stores a description written in any rich-text
client as HTML, and a Teams or Zoom invitation composed there puts the real
URL in an `href` behind words like *"Click here to join the meeting"*. The
cache keeps the readable text, so by the time anything reads the description
the URL is gone — sync is the only point at which it still exists.

Step 3 handles the plain-text form, where Outlook writes the URL in angle
brackets with no space before them:
`Click here to join the meeting<https://teams.microsoft.com/...>`.
Punctuation from the surrounding sentence is stripped, so `(see
https://example.com/a).` still opens, and at most eight links are kept per
event so an unsubscribe footer can't fill the list.

**Only `http://` and `https://` links are opened.** An invitation is text
somebody else wrote — anyone who can put an event on your calendar can put a
string in it — so the scheme is checked against a short allowlist, and the
URL is handed to `xdg-open` (or `open`, or `explorer`) as a single argument,
never through a shell. A `file://` or `vscode://` link in an invitation is
treated as not a link at all, and the footer says there is nothing to open.

If the opener isn't installed, or fails, the footer says so rather than
leaving you guessing. The link itself is always on the `Join` row of the
details panel, so you can read it off and open it yourself.

## Files

Paths follow the platform conventions of the [`dirs`](https://docs.rs/dirs)
crate. On Linux:

| Path | Purpose |
| --- | --- |
| `~/.config/lazycal/client_secret.json` | Your Google OAuth client (you provide this) |
| `~/.local/share/lazycal/tokencache.json` | Cached OAuth tokens |
| `~/.config/lazycal/settings.json` | Write access, and the 12/24-hour clock |
| `~/.local/share/lazycal/cache.db` | SQLite cache of calendars and events |
| `~/.local/share/lazycal/lazycal.log` | Log output (the TUI owns stdout) |

On macOS these live under `~/Library/Application Support/lazycal/`.

The cache holds everything the detail panel shows, which includes invitation
text, the addresses of everyone invited, and the link to join any meeting with
a video call. It is a plain SQLite file with no encryption, so treat it as
being as sensitive as your calendar itself — a join link is usable by anyone
who has it.

Deleting `cache.db` is safe — it is rebuilt on the next sync, though
per-calendar visibility choices are stored there and will reset.

## How it works

```
main.rs      terminal setup, event loop, spawns the sync and write tasks
config.rs    config/data paths, and the stored settings
provider.rs  the trait and types a calendar service is mapped onto. Writing
             is a second trait: a subscribed feed can never implement it
google.rs    OAuth (yup-oauth2) + Google Calendar client (google-calendar3)
sync.rs      incremental sync via sync tokens; prunes removed calendars
db/          SQLite schema, migrations, calendar and event queries
data.rs      groups cached events onto local calendar dates
link.rs      finds the URLs in an event, and hands one to the system opener
draft.rs     the new-event form's state: parsing typed times, validating it
app.rs       application state, key handling, and the clock-following cursor
ui/          rendering: month, week, day, agenda, the hour grid shared by the
             last two, the sidebar, the detail panel, the new-event form, the
             link chooser, the help
clock.rs     formatting and parsing times on a 12- or 24-hour clock
text.rs      display-width-aware truncation, padding and wrapping
theme.rs     colour parsing and the ANSI palette used for chrome
```

Everything a provider has to supply is behind the `CalendarProvider` trait in
`provider.rs`, so `sync.rs` and the cache never mention Google. See
[docs/calendar-providers.md](docs/calendar-providers.md) for what adding
another service involves.

The app keeps two SQLite connections: one for the UI's reads and local writes,
and one owned by the background sync task, so a slow sync never blocks
rendering.

## Development

```bash
cargo test                  # unit + integration tests
cargo fmt --all             # format
cargo clippy --all-targets  # lint
```

Four examples double as manual verification tools:

```bash
cargo run --example demo         # render invented data; needs no account
cargo run --example auth_check   # verify OAuth and list your calendars
cargo run --example sync_once    # run one sync and report what was cached
cargo run --example preview      # render your own cache as plain text
```

`demo` and `preview` both render through ratatui's `TestBackend`, so the layout
can be inspected without a real terminal. `demo` seeds a throwaway database and
is what the views in this file were generated from; `preview` reads
your real cache, so its output is not safe to paste anywhere.

The test suite is also run across a spread of timezones in CI — including
half- and quarter-hour offsets and both ends of the date line — because almost
every bug in a calendar is a date-arithmetic bug:

```bash
TZ=Pacific/Chatham cargo test
```

## Limitations

- Events can be added once write access is granted, with guests and a Meet
  link, but not edited or deleted, and a save needs the network. See
  [Writing events](#writing-events).
- A sync covers 90 days back and 365 days forward. The window is extended
  automatically as that horizon approaches, but events further out than it
  aren't fetched.
- A sync runs at startup and on demand with `r`; there's no automatic periodic
  re-sync while the app is open (schedule `--sync` if you want one).
- `o` opens a link but can't copy one to the clipboard yet; the `Join` row of
  the details panel is where to read it from.
- Month and Agenda don't scroll; they show a `+N more` count instead. Week and
  Day scroll through the day with `j`/`k`.
- Overlapping events share the width in lanes only while each lane stays at
  least 8 columns wide. A Week column on a narrow terminal is below that, so
  it shows the first event plus a `+N` count instead; the Day view almost
  always has the room.

## Contributing

Bug reports, ideas and patches are all welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md). To report a security issue privately, see
[SECURITY.md](SECURITY.md).

## License

[GPL-3.0-only](LICENSE) © harsh surani
