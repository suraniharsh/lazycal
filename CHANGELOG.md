# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--sync` to sync and exit without opening the interface, `--offline` to show
  only the cache, plus `--help` and `--version`.
- A `sign-in needed` status, since Google expires refresh tokens weekly while
  an OAuth app's publishing status is "Testing".

### Fixed

- `--sync` no longer waits forever on a consent prompt nobody can answer; an
  unattended run fails fast with a non-zero exit instead.
- Event titles containing emoji presentation sequences (`⏱️`) no longer
  overflow into the next day's column.
- The selected day highlights only its date rather than filling the whole cell.
- The declared MSRV is 1.88, which the dependency tree actually requires.

## [0.1.0] - 2026-09-08

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

[Unreleased]: https://github.com/suraniharsh/lazycal/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/suraniharsh/lazycal/releases/tag/v0.1.0
