# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Events can be added. `lazycal --enable-write` asks Google for the
  `calendar.events` scope, after which `n` opens a form for a title, calendar,
  date, times or all-day, guests, a video call, location and notes. Read-only
  remains the default and `lazycal --read-only` gives the access up again.
  Only calendars the account can write to are offered, a save needs the
  network, and a failed one hands the form back with the typing intact.
- Guests can be invited from the form: addresses separated by commas or
  spaces, deduplicated, and checked for being addresses at all before
  anything is sent. **A guest list means Google emails those people**, which
  the form says in as many words, with a count, before the save.
- `o` opens the selected event's link in the system browser, from the grid or
  from the details panel, and **asks which one when the event has several** —
  `j`/`k` to choose, `Enter` to open, a digit to open that row, `Esc` to
  cancel. A single link opens without asking, and the footer says `o: open…`
  when pressing it will ask. Only `http`/`https` schemes are opened, and the
  URL is passed as a single argument rather than through a shell, because an
  invitation is text somebody else wrote. The footer distinguishes nothing
  selected, no link on the event, and an opener that failed.
- Events carry the links found on them, collected during sync from the
  conferencing, the `href`s in the invitation's HTML, its readable text and
  its location.
- Times can be shown as `9:30am` instead of `09:30`: `lazycal --clock 12h`,
  remembered in `settings.json`. It covers the Week and Day hour gutters, the
  time on each event block, the details panel and the new-event form. The
  form's time fields accept either convention whichever is displayed, so
  `2pm` and `14:00` both work.
- `↑`/`↓` nudge the focused time field in the new-event form by 15
  minutes, and moving the start moves the end with it, keeping the gap. The
  length of the event is shown beside the end time.
- The new-event form's guest field completes from the people already on
  cached events, matched on name or address and ordered by how often they
  appear. `↑`/`↓` choose, `Tab` takes one. Nothing is fetched and no
  contacts scope is requested — the attendee lists were already synced with
  the events.
- The new-event form's bottom border names the keys the focused row responds
  to, rather than always saying the same three.
- Setting `Video call` to yes asks Google for a Meet link. Google generates it
  asynchronously, so it appears on the event's `Join` row after the sync that
  follows the save.
- `Tab` and `Shift-Tab` move a cursor through the events on screen, crossing
  days, and `Enter` opens the one under it. The panel shows the event's time
  and duration, where it is, the link to join if there is one, who was invited
  and how they replied, and the invitation text; `j`/`k` scroll a long
  description.
- The meeting in progress, or the next one left today, is marked
  automatically and the mark moves on as each finishes, without a keypress.
  All-day events are skipped, and nothing is marked once the day's meetings
  are done. Choosing an event by hand takes over, and `t` hands it back.
- Week and Day are now hour grids: a time axis, events positioned at their
  start and sized by duration, all-day events in a band above, overlapping
  events in lanes, and a marker for the current time. `j`/`k` scroll the grid
  in those views.
- `?` opens a help overlay listing every key.
- `--sync` to sync and exit without opening the interface, `--offline` to show
  only the cache, plus `--help` and `--version`.
- A `sign-in needed` status, since Google expires refresh tokens weekly while
  an OAuth app's publishing status is "Testing".

### Changed

- A meeting's join link is cached and shown separately from its location,
  instead of standing in for an empty one. An event with both a room and a
  video call used to show only the room.
- Calendars record whether the account may write to them, which an upgraded
  cache learns on its next sync.
- Upgrading rewrites every cached event once, because an incremental sync only
  reports what changed and would otherwise never fill in the new fields.

### Fixed

- A meeting link behind anchor text — `<a href="…">Click here to join the
  meeting</a>`, which is how a Teams or Zoom invitation written in a
  rich-text client arrives — was discarded on sync along with the rest of the
  markup, so the event appeared to have no way in. The `href`s are now
  collected before the markup is stripped.
- A URL in the form Outlook sends plain text —
  `Click here to join the meeting<https://…>`, with no space before the
  bracket — was not recognised as a link, because URLs were only looked for
  at the start of whitespace-separated words.
- `lazycal --enable-write` and `--read-only` wrote a whole fresh settings
  file, so granting or dropping write access would have discarded any other
  preference in it.
- Footer hints are dropped whole rather than cut mid-word when the terminal is
  narrow, and `?: help` and `q: quit` are kept whatever the width.
- `--sync` no longer waits forever on a consent prompt nobody can answer; an
  unattended run fails fast with a non-zero exit instead.
- Event titles containing emoji presentation sequences (`⏱️`) no longer
  overflow into the next day's column.
- The selected day highlights only its date rather than filling the whole cell,
  and today stays marked even when the cursor is on it.
- The declared MSRV is 1.88, which the dependency tree actually requires.

## 0.1.0 - 2026-09-08

Initial release.

### Added

- Google Calendar sync over OAuth (`calendar.readonly`), with an offline SQLite
  cache and incremental refresh via Google's sync tokens.
- Month, Week, Day and Agenda views rendered from the local cache.
- Instant startup: the UI renders from cache while a background task syncs,
  with a status indicator in the footer.
- Sidebar with a mini month calendar and a calendar list; calendars can be
  shown or hidden with `Space`, and the choice persists locally without being
  overwritten by a sync.
- Colors sourced from Google — per-calendar colors plus per-event `colorId`
  overrides — with UI chrome drawn from the terminal's own ANSI palette.
- Calendars removed from the account are pruned from the cache on sync.
- Manual re-sync with `r`, and a footer status that distinguishes syncing,
  synced, partly synced, failed and unconfigured.
- Multi-day and overnight events appear on every day they cover, with
  continued days marked.
- Overflowing views report a `+N more` count instead of clipping silently.

<!-- 0.1.0 was never tagged, so it has no link. Once a release is tagged,
     point Unreleased at compare/<tag>...HEAD and link each version to its
     own tag. -->

[Unreleased]: https://github.com/suraniharsh/lazycal/commits/main
