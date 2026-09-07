# lazycal

A fast terminal UI for Google Calendar, backed by an offline SQLite cache.

`lazycal` starts instantly by rendering from its local cache, then refreshes from
Google in the background. Month, week, day and agenda views, per-calendar
visibility toggles, and colors taken from your own Google Calendar setup.

[![CI](https://github.com/suraniharsh/lazycal/actions/workflows/ci.yml/badge.svg)](https://github.com/suraniharsh/lazycal/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

<!-- Add a screenshot here, e.g.:
![lazycal](docs/screenshot.png)
Note: a screenshot of a real calendar exposes your event titles and account
email — consider using a throwaway account or blurring sensitive rows. -->

## Features

- **Instant startup** — renders from the local cache immediately; sync happens
  in a background task, with a live status indicator in the footer.
- **Four views** — Month, Week, Day and Agenda.
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

`lazycal` is **read-only**: it requests only the
`calendar.readonly` scope and cannot create, edit or delete events.

## Install

Requires Rust 1.85 or newer (2024 edition).

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
   Google account under **Test users**. Leaving the app in "Testing" is fine for
   personal use.
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

## Keys

| Key | Action |
| --- | --- |
| `m` / `w` / `d` / `a` | Month / Week / Day / Agenda view |
| `h` `l` or `←` `→` | Previous / next day |
| `j` `k` or `↓` `↑` | Next / previous week |
| `PgUp` / `PgDn` | Previous / next month |
| `t` | Jump to today |
| `c` | Focus the calendar sidebar |
| `r` | Sync now |
| `q` or `Ctrl-C` | Quit |

While the sidebar is focused:

| Key | Action |
| --- | --- |
| `j` `k` or `↓` `↑` | Move through the calendar list |
| `Space` / `Enter` | Show/hide that calendar |
| `Esc` / `c` | Return to the calendar |

## Files

Paths follow the platform conventions of the [`dirs`](https://docs.rs/dirs)
crate. On Linux:

| Path | Purpose |
| --- | --- |
| `~/.config/lazycal/client_secret.json` | Your Google OAuth client (you provide this) |
| `~/.local/share/lazycal/tokencache.json` | Cached OAuth tokens |
| `~/.local/share/lazycal/cache.db` | SQLite cache of calendars and events |
| `~/.local/share/lazycal/lazycal.log` | Log output (the TUI owns stdout) |

On macOS these live under `~/Library/Application Support/lazycal/`.

Deleting `cache.db` is safe — it is rebuilt on the next sync, though
per-calendar visibility choices are stored there and will reset.

## How it works

```
main.rs      terminal setup, event loop, spawns the background sync task
config.rs    resolves config/data paths
google.rs    OAuth (yup-oauth2) + Google Calendar client (google-calendar3)
sync.rs      incremental sync via sync tokens; prunes removed calendars
db/          SQLite schema, migrations, calendar and event queries
data.rs      groups cached events onto local calendar dates
app.rs       application state and key handling
ui/          rendering: month, week, day, agenda, sidebar
```

The app keeps two SQLite connections: one for the UI's reads and local writes,
and one owned by the background sync task, so a slow sync never blocks
rendering.

## Development

```bash
cargo test                  # unit + integration tests
cargo fmt --all             # format
cargo clippy --all-targets  # lint
```

Three examples double as manual verification tools against a real account:

```bash
cargo run --example auth_check   # verify OAuth and list your calendars
cargo run --example sync_once    # run one sync and report what was cached
cargo run --example preview      # render the TUI to stdout as plain text
```

`preview` renders through ratatui's `TestBackend`, which makes it possible to
inspect the layout without a real terminal.

## Limitations

- Read-only; no event creation or editing.
- Week and Day views are event lists, not hour grids.
- A sync covers 90 days back and 365 days forward. The window is extended
  automatically as that horizon approaches, but events further out than it
  aren't fetched.
- A sync runs at startup and on demand with `r`; there's no automatic periodic
  re-sync while the app is open.
- No scrolling: views that overflow show a `+N more` count instead.

## License

[GPL-3.0-only](LICENSE) © harsh surani
