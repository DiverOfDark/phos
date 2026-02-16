use rusqlite::{Connection, Result};
use std::path::Path;

/// Open a connection with WAL mode and busy timeout enabled.
/// Use this when worker threads need their own connection.
pub fn open_connection<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", "5000")?;
    Ok(conn)
}

pub fn init_db<P: AsRef<Path>>(path: P) -> Result<Connection> {
    tracing::info!("Initializing database at {:?}", path.as_ref());
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", "5000")?;

    // We store people (clusters)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS people (
            id TEXT PRIMARY KEY,
            name TEXT,
            thumbnail_face_id TEXT,
            representative_embedding BLOB,
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

    // Cached pairwise face distances for clustering
    conn.execute(
        "CREATE TABLE IF NOT EXISTS face_neighbors (
            face_id_a TEXT NOT NULL,
            face_id_b TEXT NOT NULL,
            distance REAL NOT NULL,
            PRIMARY KEY (face_id_a, face_id_b)
        )",
        [],
    )?;

    // Add representative_embedding column to people if it doesn't exist (migration)
    // SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we check via pragma
    let people_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(people)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !people_columns.contains(&"representative_embedding".to_string()) {
        conn.execute(
            "ALTER TABLE people ADD COLUMN representative_embedding BLOB",
            [],
        )?;
    }

    // Add score column to faces if it doesn't exist (migration)
    let faces_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(faces)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !faces_columns.contains(&"score".to_string()) {
        conn.execute("ALTER TABLE faces ADD COLUMN score REAL", [])?;
    }

    Ok(conn)
}
