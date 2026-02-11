use rusqlite::{Connection, Result};
use std::path::Path;

pub fn init_db<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;

    // We store people (clusters)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS people (
            id TEXT PRIMARY KEY,
            name TEXT,
            thumbnail_face_id TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    // A photo is a conceptual media item that can have multiple files (original + edits)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS photos (
            id TEXT PRIMARY KEY,
            main_file_id TEXT,
            timestamp DATETIME,
            width INTEGER,
            height INTEGER,
            latitude REAL,
            longitude REAL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    // Files physical on disk
    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            id TEXT PRIMARY KEY,
            photo_id TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            hash TEXT NOT NULL,
            mime_type TEXT,
            file_size INTEGER,
            is_original BOOLEAN DEFAULT 0,
            visual_embedding BLOB,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        )",
        [],
    )?;

    // Faces detected in files
    conn.execute(
        "CREATE TABLE IF NOT EXISTS faces (
            id TEXT PRIMARY KEY,
            file_id TEXT NOT NULL,
            person_id TEXT,
            box_x1 REAL,
            box_y1 REAL,
            box_x2 REAL,
            box_y2 REAL,
            embedding BLOB,
            thumbnail_path TEXT,
            FOREIGN KEY(file_id) REFERENCES files(id),
            FOREIGN KEY(person_id) REFERENCES people(id)
        )",
        [],
    )?;

    // Videos table for keyframes reference
    conn.execute(
        "CREATE TABLE IF NOT EXISTS video_keyframes (
            id TEXT PRIMARY KEY,
            video_file_id TEXT NOT NULL,
            timestamp_ms INTEGER,
            path TEXT NOT NULL,
            FOREIGN KEY(video_file_id) REFERENCES files(id)
        )",
        [],
    )?;

    Ok(conn)
}
