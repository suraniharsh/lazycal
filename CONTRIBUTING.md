# Contributing

Thanks for your interest in `lazycal`. Bug reports, ideas and patches are all
welcome.

## Getting set up

You need Rust 1.88 or newer and a C compiler (SQLite is built from source via
`rusqlite`'s `bundled` feature).

```bash
git clone https://github.com/suraniharsh/lazycal.git
cd lazycal
cargo build
cargo test
```

Running the app against a real account additionally needs a Google OAuth client
— see [Setup](README.md#setup).

## Before opening a pull request

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs exactly these, so a green local run should mean a green CI run.

## Guidelines

- **Keep it focused.** One logical change per pull request; unrelated cleanups
  are easier to review separately.
- **Add tests for logic.** Date math, text truncation, event bucketing and the
  database layer are all unit-testable without network access — see
  `src/data.rs` and `tests/` for the existing patterns.
- **Comment the "why", not the "what".** Explain non-obvious decisions,
  workarounds and invariants; skip comments that restate the code.
- **Don't hold a database lock across an `.await`.** The sync task shares its
  connection as `Arc<Mutex<Connection>>`; `rusqlite::Connection` is not `Sync`,
  so a guard held across a suspension point breaks `Send` for the whole future.
- **Verify UI changes.** `cargo run --example preview` dumps the rendered
  frames as text, which is usually enough to see a layout regression.

## Reporting bugs

Please include:

- what you expected and what happened instead,
- your OS, terminal and `rustc --version`,
- anything relevant from `~/.local/share/lazycal/lazycal.log`.

**Never paste your `client_secret.json`, `tokencache.json` or raw log lines
containing tokens into an issue.** Redact event titles you would rather not
publish.

## Security issues

Please do not open a public issue for a security problem — see
[SECURITY.md](SECURITY.md).
