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

- **Requested scope is read-only.** Only
  `https://www.googleapis.com/auth/calendar.readonly` is requested, so the
  token cannot be used to modify your calendars.
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
- `~/.local/share/lazycal/cache.db` contains your event titles in plaintext;
  delete it if that matters on a shared machine.
