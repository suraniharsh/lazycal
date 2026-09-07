pub mod calendars;
pub mod events;
pub mod schema;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// `rusqlite::Connection` is `Send` but not `Sync`, so a task that interleaves
/// API calls with database writes can't hold a plain `&Connection` across an
/// `.await` without making its future non-`Send`. Call sites lock briefly
/// around each synchronous operation instead.
pub type SharedConnection = Arc<Mutex<Connection>>;

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;

    // WAL keeps the UI's reads from blocking behind the sync task's writes.
    // `journal_mode` returns the new mode, so it needs a query, not an execute.
    let _: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .context("enabling WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .context("setting synchronous")?;

    schema::init(&conn)?;
    Ok(conn)
}

pub fn open_shared(path: &Path) -> Result<SharedConnection> {
    Ok(Arc::new(Mutex::new(open(path)?)))
}
