use rusqlite::{params, Connection, Result};
use std::path::{Path, PathBuf};

/// Open a connection with WAL mode and busy timeout enabled.
/// Use this when worker threads need their own connection.
pub fn open_connection<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", "60000")?;
    Ok(conn)
}

/// Convert an absolute filesystem path to a path relative to the library root.
/// Used when storing paths in the database for portability.
pub fn make_relative(library_root: &Path, abs_path: &Path) -> String {
    match abs_path.strip_prefix(library_root) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => abs_path.to_string_lossy().to_string(),
    }
}

/// Resolve a database path to an absolute filesystem path.
/// If the path is already absolute (pre-migration data), returns it as-is.
pub fn resolve_path(library_root: &Path, db_path: &str) -> PathBuf {
    let p = Path::new(db_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        library_root.join(p)
    }
}

pub fn init_db<P: AsRef<Path>>(path: P) -> Result<Connection> {
    tracing::info!("Initializing database at {:?}", path.as_ref());
    let conn = Connection::open(&path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", "5000")?;

    // Check if we need to migrate from old schema (photos -> shots)
    let has_photos_table: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='photos'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    let has_shots_table: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='shots'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if has_photos_table && !has_shots_table {
        tracing::info!("Detected old schema with 'photos' table. Running migration to 'shots'...");
        migrate_photos_to_shots(&conn, &path)?;
    }

    // We store people (clusters)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS people (
            id TEXT PRIMARY KEY,
            name TEXT,
            thumbnail_face_id TEXT,
            representative_embedding BLOB,
            folder_name TEXT UNIQUE,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    // A shot is a conceptual media item that can have multiple files (original + edits)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS shots (
            id TEXT PRIMARY KEY,
            main_file_id TEXT,
            timestamp DATETIME,
            width INTEGER,
            height INTEGER,
            latitude REAL,
            longitude REAL,
            primary_person_id TEXT,
            folder_number INTEGER,
            review_status TEXT DEFAULT 'pending',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(primary_person_id) REFERENCES people(id)
        )",
        [],
    )?;

    // Files physical on disk
    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            id TEXT PRIMARY KEY,
            shot_id TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            hash TEXT NOT NULL,
            mime_type TEXT,
            file_size INTEGER,
            is_original BOOLEAN DEFAULT 0,
            visual_embedding BLOB,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(shot_id) REFERENCES shots(id)
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

    // Drop legacy O(n²) pairwise distance cache — clustering now uses person centroids
    conn.execute("DROP TABLE IF EXISTS face_neighbors", [])?;

    // ComfyUI workflow templates
    conn.execute(
        "CREATE TABLE IF NOT EXISTS comfyui_workflows (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            workflow_json TEXT NOT NULL,
            inputs_json TEXT,
            outputs_json TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    // Enhancement tasks
    conn.execute(
        "CREATE TABLE IF NOT EXISTS enhancement_tasks (
            id TEXT PRIMARY KEY,
            shot_id TEXT NOT NULL,
            workflow_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            comfyui_prompt_id TEXT,
            text_overrides TEXT DEFAULT '{}',
            output_file_id TEXT,
            error_message TEXT,
            retry_count INTEGER DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            started_at DATETIME,
            completed_at DATETIME,
            FOREIGN KEY(shot_id) REFERENCES shots(id),
            FOREIGN KEY(workflow_id) REFERENCES comfyui_workflows(id),
            FOREIGN KEY(output_file_id) REFERENCES files(id)
        )",
        [],
    )?;

    // Ignored merges (for Variations Queue)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ignored_merges (
            shot_id_1 TEXT NOT NULL,
            shot_id_2 TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (shot_id_1, shot_id_2)
        )",
        [],
    )?;

    // Key-value settings (e.g. WebDAV credentials)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
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

    if !people_columns.contains(&"folder_name".to_string()) {
        conn.execute("ALTER TABLE people ADD COLUMN folder_name TEXT UNIQUE", [])?;
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

    // Add new shot columns if they don't exist (for existing shots tables without them)
    let shots_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(shots)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !shots_columns.is_empty() {
        if !shots_columns.contains(&"primary_person_id".to_string()) {
            conn.execute("ALTER TABLE shots ADD COLUMN primary_person_id TEXT", [])?;
        }
        if !shots_columns.contains(&"folder_number".to_string()) {
            conn.execute("ALTER TABLE shots ADD COLUMN folder_number INTEGER", [])?;
        }
        if !shots_columns.contains(&"review_status".to_string()) {
            conn.execute(
                "ALTER TABLE shots ADD COLUMN review_status TEXT DEFAULT 'pending'",
                [],
            )?;
        }
        if !shots_columns.contains(&"description".to_string()) {
            conn.execute("ALTER TABLE shots ADD COLUMN description TEXT", [])?;
        }
    }

    // Add updated_at column to people if it doesn't exist
    // Note: SQLite does not allow CURRENT_TIMESTAMP as a default in ALTER TABLE,
    // so we add the column without a default and backfill.
    if !people_columns.contains(&"updated_at".to_string()) {
        conn.execute(
            "ALTER TABLE people ADD COLUMN updated_at DATETIME",
            [],
        )?;
        conn.execute(
            "UPDATE people SET updated_at = COALESCE(created_at, CURRENT_TIMESTAMP) WHERE updated_at IS NULL",
            [],
        )?;
    }

    // Add updated_at column to shots if it doesn't exist
    if !shots_columns.contains(&"updated_at".to_string()) {
        conn.execute(
            "ALTER TABLE shots ADD COLUMN updated_at DATETIME",
            [],
        )?;
        conn.execute(
            "UPDATE shots SET updated_at = COALESCE(created_at, CURRENT_TIMESTAMP) WHERE updated_at IS NULL",
            [],
        )?;
    }

    // Add updated_at column to files if it doesn't exist
    let files_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(files)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !files_columns.contains(&"updated_at".to_string()) {
        conn.execute(
            "ALTER TABLE files ADD COLUMN updated_at DATETIME",
            [],
        )?;
        conn.execute(
            "UPDATE files SET updated_at = COALESCE(created_at, CURRENT_TIMESTAMP) WHERE updated_at IS NULL",
            [],
        )?;
    }

    // Create triggers to auto-update updated_at on modifications
    // Use a conditional check to avoid infinite recursion
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS people_updated_at AFTER UPDATE ON people
         FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at OR NEW.updated_at IS NULL
         BEGIN
             UPDATE people SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
         END;

         CREATE TRIGGER IF NOT EXISTS shots_updated_at AFTER UPDATE ON shots
         FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at OR NEW.updated_at IS NULL
         BEGIN
             UPDATE shots SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
         END;

         CREATE TRIGGER IF NOT EXISTS files_updated_at AFTER UPDATE ON files
         FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at OR NEW.updated_at IS NULL
         BEGIN
             UPDATE files SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
         END;"
    )?;

    // Add description column to comfyui_workflows if it doesn't exist
    let workflows_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(comfyui_workflows)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !workflows_columns.is_empty()
        && !workflows_columns.contains(&"description".to_string())
    {
        conn.execute(
            "ALTER TABLE comfyui_workflows ADD COLUMN description TEXT",
            [],
        )?;
    }

    // Migration: convert absolute file paths to relative paths.
    // Paths stored as absolute (e.g. /home/user/library/person/001/photo.jpg) become
    // relative to the library root (e.g. person/001/photo.jpg).
    // Idempotent: already-relative paths don't match the LIKE prefix.
    // Uses OR IGNORE to handle races where the scanner already stored relative paths.
    {
        let library_root = path.as_ref().parent().unwrap_or(Path::new("."));
        let prefix = format!("{}/", library_root.to_string_lossy());

        // Convert absolute paths to relative, skipping any that would conflict
        // with already-relative entries (e.g. from a scanner run before migration).
        let files_migrated = conn.execute(
            "UPDATE OR IGNORE files SET path = SUBSTR(path, LENGTH(?1) + 1) WHERE path LIKE (?1 || '%')",
            params![prefix],
        )?;

        // Clean up remaining absolute-path duplicates that couldn't be converted.
        // These are entries where a relative-path version already exists.
        let duplicate_ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM files WHERE path LIKE (?1 || '%')",
            )?;
            let ids: Vec<String> = stmt.query_map(params![prefix], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            ids
        };
        if !duplicate_ids.is_empty() {
            for file_id in &duplicate_ids {
                conn.execute("DELETE FROM faces WHERE file_id = ?", params![file_id])?;
                conn.execute("DELETE FROM video_keyframes WHERE video_file_id = ?", params![file_id])?;
                conn.execute("DELETE FROM files WHERE id = ?", params![file_id])?;
            }
            conn.execute(
                "DELETE FROM shots WHERE id NOT IN (SELECT DISTINCT shot_id FROM files)",
                [],
            )?;
            tracing::info!(
                "Cleaned up {} duplicate absolute-path entries during migration",
                duplicate_ids.len()
            );
        }

        let faces_migrated = conn.execute(
            "UPDATE OR IGNORE faces SET thumbnail_path = SUBSTR(thumbnail_path, LENGTH(?1) + 1) WHERE thumbnail_path LIKE (?1 || '%')",
            params![prefix],
        )?;

        if files_migrated > 0 || faces_migrated > 0 {
            tracing::info!(
                "Migrated paths to relative: {} files, {} face thumbnails",
                files_migrated,
                faces_migrated
            );
        }
    }

    // Indexes for join/filter performance (IF NOT EXISTS is idempotent)
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_faces_person_id ON faces(person_id);
         CREATE INDEX IF NOT EXISTS idx_faces_file_id ON faces(file_id);
         CREATE INDEX IF NOT EXISTS idx_files_shot_id ON files(shot_id);
         CREATE INDEX IF NOT EXISTS idx_shots_primary_person_id ON shots(primary_person_id);
         CREATE INDEX IF NOT EXISTS idx_shots_timestamp ON shots(timestamp);"
    )?;

    // Clean up people who lost all their shots (e.g. after shot merges)
    cleanup_orphaned_people(&conn)?;

    // Reclaim unused space
    tracing::info!("Running VACUUM on database");
    conn.execute_batch("VACUUM;")?;

    Ok(conn)
}

/// Get a setting value by key.
pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?",
        params![key],
        |row| row.get(0),
    )
    .ok()
}

/// Set a setting value (insert or update).
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Delete a setting by key.
pub fn delete_setting(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?", params![key])?;
    Ok(())
}

/// Delete people who have no shots assigned to them and no faces referencing them.
/// This can happen when shots are merged and a person loses all their shots.
pub fn cleanup_orphaned_people(conn: &Connection) -> Result<()> {
    // First, unassign faces for people who have no shots
    let unassigned = conn.execute(
        "UPDATE faces SET person_id = NULL
         WHERE person_id IS NOT NULL
           AND person_id NOT IN (
               SELECT DISTINCT primary_person_id FROM shots WHERE primary_person_id IS NOT NULL
           )",
        [],
    )?;
    if unassigned > 0 {
        tracing::info!("Unassigned {} faces from people with no shots", unassigned);
    }

    // Then delete people with no shots and no faces
    let deleted = conn.execute(
        "DELETE FROM people
         WHERE id NOT IN (
             SELECT DISTINCT primary_person_id FROM shots WHERE primary_person_id IS NOT NULL
         )
         AND id NOT IN (
             SELECT DISTINCT person_id FROM faces WHERE person_id IS NOT NULL
         )",
        [],
    )?;
    if deleted > 0 {
        tracing::info!("Cleaned up {} orphaned people", deleted);
    }
    Ok(())
}

/// Migrate from the old `photos` table schema to the new `shots` table schema.
/// This creates a backup, renames tables/columns within a transaction, and drops the old table.
fn migrate_photos_to_shots<P: AsRef<Path>>(conn: &Connection, db_path: P) -> Result<()> {
    // Back up the database file
    let db_path = db_path.as_ref();
    let backup_path = db_path.with_extension("db.bak");
    if let Err(e) = std::fs::copy(db_path, &backup_path) {
        tracing::warn!(
            "Failed to create DB backup at {:?}: {}. Proceeding with migration anyway.",
            backup_path,
            e
        );
    } else {
        tracing::info!("Database backed up to {:?}", backup_path);
    }

    conn.execute_batch("BEGIN TRANSACTION")?;

    // Create the new shots table with all new columns
    conn.execute_batch(
        "CREATE TABLE shots (
            id TEXT PRIMARY KEY,
            main_file_id TEXT,
            timestamp DATETIME,
            width INTEGER,
            height INTEGER,
            latitude REAL,
            longitude REAL,
            primary_person_id TEXT,
            folder_number INTEGER,
            review_status TEXT DEFAULT 'pending',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(primary_person_id) REFERENCES people(id)
        );

        INSERT INTO shots (id, main_file_id, timestamp, width, height, latitude, longitude, created_at)
            SELECT id, main_file_id, timestamp, width, height, latitude, longitude, created_at
            FROM photos;"
    )?;

    // Rename photo_id -> shot_id in files table
    // SQLite ALTER TABLE RENAME COLUMN is supported since 3.25.0
    let files_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(files)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if files_columns.contains(&"photo_id".to_string()) {
        conn.execute_batch("ALTER TABLE files RENAME COLUMN photo_id TO shot_id")?;
    }

    // Add folder_name column to people if it doesn't exist
    let people_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(people)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !people_columns.contains(&"folder_name".to_string()) {
        conn.execute_batch("ALTER TABLE people ADD COLUMN folder_name TEXT UNIQUE")?;
    }

    // Populate folder_name from name or id
    conn.execute_batch(
        "UPDATE people SET folder_name = COALESCE(name, id) WHERE folder_name IS NULL",
    )?;

    // Ensure exactly one is_original = 1 per shot.
    // For shots where no file has is_original = 1, set the first file as original.
    conn.execute_batch(
        "UPDATE files SET is_original = 1
         WHERE id IN (
             SELECT MIN(f.id) FROM files f
             LEFT JOIN (SELECT shot_id FROM files WHERE is_original = 1) o ON f.shot_id = o.shot_id
             WHERE o.shot_id IS NULL
             GROUP BY f.shot_id
         )",
    )?;

    // Drop the old photos table
    conn.execute_batch("DROP TABLE photos")?;

    conn.execute_batch("COMMIT")?;

    tracing::info!("Migration from 'photos' to 'shots' completed successfully.");
    Ok(())
}
