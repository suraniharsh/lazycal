use anyhow::Result;
use rusqlite::Connection;

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS calendars (
            id          TEXT PRIMARY KEY,
            summary     TEXT NOT NULL,
            color       TEXT,
            is_primary  INTEGER NOT NULL DEFAULT 0,
            sync_token  TEXT
        );
        CREATE TABLE IF NOT EXISTS events (
            calendar_id TEXT NOT NULL,
            id          TEXT NOT NULL,
            summary     TEXT NOT NULL,
            start_utc   INTEGER,
            end_utc     INTEGER,
            start_date  TEXT,
            end_date    TEXT,
            all_day     INTEGER NOT NULL,
            status      TEXT NOT NULL,
            PRIMARY KEY (calendar_id, id)
        );
        CREATE INDEX IF NOT EXISTS events_calendar_idx ON events(calendar_id);
        CREATE INDEX IF NOT EXISTS events_start_utc_idx ON events(start_utc);
        CREATE INDEX IF NOT EXISTS events_start_date_idx ON events(start_date);
        ",
    )?;

    // Columns added after the initial release; `CREATE TABLE IF NOT EXISTS`
    // above only covers databases being created for the first time.
    add_column_if_missing(conn, "calendars", "selected", "INTEGER NOT NULL DEFAULT 1")?;
    add_column_if_missing(conn, "calendars", "sync_horizon", "TEXT")?;
    add_column_if_missing(conn, "events", "color_id", "TEXT")?;

    Ok(())
}

/// Adds a column to an existing table if it isn't there yet, so a database
/// written by an older version is upgraded in place instead of discarded.
pub fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    ddl_type: &str,
) -> Result<()> {
    // Table and column names are compile-time constants from this module, not
    // user input, so interpolating them is safe.
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    drop(stmt);

    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl_type}"),
            [],
        )?;
    }
    Ok(())
}
