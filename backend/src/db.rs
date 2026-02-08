use rusqlite::{Connection, Result};
use std::path::Path;

pub fn init_db<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS people (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS photos (
            id TEXT PRIMARY KEY,
            person_id TEXT,
            original_file_hash TEXT UNIQUE,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(person_id) REFERENCES people(id)
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            id TEXT PRIMARY KEY,
            photo_id TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            hash TEXT NOT NULL,
            mime_type TEXT,
            width INTEGER,
            height INTEGER,
            is_original BOOLEAN DEFAULT 0,
            embedding BLOB,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        )",
        [],
    )?;

    Ok(conn)
}
