# Security Policy

## Reporting a vulnerability

Please report security issues privately rather than in a public issue:

- Use GitHub's [private vulnerability reporting](https://github.com/suraniharsh/lazycal/security/advisories/new), or
- email <harshsurani00@gmail.com>.

Please include a description of the issue, the steps to reproduce it, and the
version or commit you tested. You can expect an initial response within a few
days.

## Scope

`lazycal` runs locally and holds credentials for a Google account, so the
security-relevant surface is mainly:

- handling of `client_secret.json` and the cached OAuth token,
- what gets written to the log file and the SQLite cache,
- the OAuth flow itself.

## What lazycal does with your credentials

- **Read-only unless you ask otherwise.** By default only
  `https://www.googleapis.com/auth/calendar.readonly` is requested, so the
  token cannot be used to change your calendars. Running
  `lazycal --enable-write` additionally requests
  `https://www.googleapis.com/auth/calendar.events`, which lets lazycal add
  events; that needs a fresh consent from you, is recorded in
  `settings.json`, and is undone with `lazycal --read-only`. Nothing but a
  form you filled in and submitted causes a write.
- **Saving an event with guests emails them.** That is the only thing lazycal
  does that reaches anyone other than you, it happens only on a form you
  submitted with addresses in the Guests field, and the form counts the people
  it is about to mail before you press Enter. With the field empty, nothing is
  sent to anyone. Asking Google for a Meet link creates a conference on your
  own account and notifies nobody by itself.
- **Only `http`/`https` links are opened.** `o` hands a URL from the selected
  event to your system's opener. An invitation is text written by whoever sent
  it, so the scheme is checked against an allowlist first — a `file://`,
  `javascript:` or custom-scheme URL in an event is not treated as a link —
  and the URL is passed as a single argument to `xdg-open`, never through a
  shell. The opener's output is discarded so it cannot write over the
  interface.
- **Credentials stay local.** The OAuth client secret and the cached token are
  read from and written to your own config/data directories and are never sent
  anywhere except Google's own endpoints.
- **Nothing is committed.** `client_secret*.json`, `tokencache.json`, `*.db`
  and `*.log` are all in `.gitignore`.

## Hardening your own install

- `chmod 600 ~/.config/lazycal/client_secret.json` and
  `chmod 600 ~/.local/share/lazycal/tokencache.json`.
- To revoke access entirely, remove `lazycal` from
  [your Google account's third-party access list](https://myaccount.google.com/connections)
  and delete `~/.local/share/lazycal/tokencache.json`.
- `~/.local/share/lazycal/cache.db` contains your event titles, attendee
  email addresses and any meeting join links in plaintext; delete it if that
  matters on a shared machine.
