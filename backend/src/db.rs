use crate::schema::{faces, files, people, settings, shots};
use diesel::prelude::*;
use diesel::r2d2::{self, ConnectionManager, CustomizeConnection, Pool};
use diesel::sqlite::SqliteConnection;
use diesel::Connection as DieselConnection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use std::path::{Path, PathBuf};
use tracing::info;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

pub type DbPool = Pool<ConnectionManager<SqliteConnection>>;

#[derive(Debug)]
struct SqlitePragmaCustomizer;

impl CustomizeConnection<SqliteConnection, r2d2::Error> for SqlitePragmaCustomizer {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> std::result::Result<(), r2d2::Error> {
        diesel::sql_query("PRAGMA journal_mode = WAL")
            .execute(conn)
            .map_err(|e| r2d2::Error::QueryError(e))?;
        diesel::sql_query("PRAGMA busy_timeout = 60000")
            .execute(conn)
            .map_err(|e| r2d2::Error::QueryError(e))?;
        Ok(())
    }
}

/// Create a Diesel r2d2 connection pool for the given database path.
/// Configures WAL mode and busy_timeout on each connection.
pub fn establish_pool<P: AsRef<Path>>(path: P) -> std::result::Result<DbPool, r2d2::PoolError> {
    let database_url = path.as_ref().to_string_lossy().to_string();
    let manager = ConnectionManager::<SqliteConnection>::new(database_url);
    Pool::builder()
        .max_size(2)
        .min_idle(Some(0))
        .connection_customizer(Box::new(SqlitePragmaCustomizer))
        .build(manager)
}

/// Run pending Diesel migrations on a pooled connection.
pub fn run_migrations(
    pool: &DbPool,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = pool
        .get()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    conn.run_pending_migrations(MIGRATIONS).map(|_| ())
}

/// Open a Diesel SQLite connection with WAL mode and busy timeout enabled.
/// Use this when scanner/worker threads need their own Diesel connection.
pub fn open_diesel_connection<P: AsRef<Path>>(path: P) -> anyhow::Result<SqliteConnection> {
    let mut conn = SqliteConnection::establish(&path.as_ref().to_string_lossy())?;
    diesel::sql_query("PRAGMA journal_mode = WAL").execute(&mut conn)?;
    diesel::sql_query("PRAGMA busy_timeout = 60000").execute(&mut conn)?;
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

/// Create schema (via Diesel migrations) and run data migrations.
/// Convenience function that does everything needed to initialize a database.
pub fn init_and_migrate<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
    let pool = establish_pool(&path)
        .map_err(|e| anyhow::anyhow!("Failed to create connection pool: {}", e))?;
    run_migrations(&pool).map_err(|e| anyhow::anyhow!("Failed to run migrations: {}", e))?;
    drop(pool);
    init_db(&path)
}

/// Run data migrations and cleanup on an existing database.
/// Schema creation is handled by Diesel migrations; this function handles:
/// - Legacy photos -> shots table migration
/// - Absolute -> relative path migration
/// - Dropping legacy tables
/// - Orphaned people cleanup
/// - VACUUM
pub fn init_db<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
    info!("Running data migrations on database at {:?}", path.as_ref());
    let mut conn = open_diesel_connection(&path)?;

    // Check if we need to migrate from old schema (photos -> shots)
    let has_photos_table: bool = diesel::sql_query(
        "SELECT COUNT(*) as cnt FROM sqlite_master WHERE type='table' AND name='photos'",
    )
    .get_result::<CountResult>(&mut conn)
    .map(|r| r.cnt > 0)
    .unwrap_or(false);

    let has_shots_table: bool = diesel::sql_query(
        "SELECT COUNT(*) as cnt FROM sqlite_master WHERE type='table' AND name='shots'",
    )
    .get_result::<CountResult>(&mut conn)
    .map(|r| r.cnt > 0)
    .unwrap_or(false);

    if has_photos_table && !has_shots_table {
        info!("Detected old schema with 'photos' table. Running migration to 'shots'...");
        migrate_photos_to_shots(&mut conn, &path)?;
    }

    // Drop legacy O(n²) pairwise distance cache — clustering now uses person centroids
    diesel::sql_query("DROP TABLE IF EXISTS face_neighbors").execute(&mut conn)?;

    // Migration: convert absolute file paths to relative paths.
    {
        let library_root = path.as_ref().parent().unwrap_or(Path::new("."));
        let prefix = format!("{}/", library_root.to_string_lossy());

        // Convert absolute paths to relative, skipping any that would conflict
        let files_migrated = diesel::sql_query(
            "UPDATE OR IGNORE files SET path = SUBSTR(path, LENGTH(?1) + 1) WHERE path LIKE (?1 || '%')",
        )
        .bind::<diesel::sql_types::Text, _>(&prefix)
        .execute(&mut conn)?;

        // Clean up remaining absolute-path duplicates that couldn't be converted.
        let duplicate_ids: Vec<String> = diesel::sql_query(
            "SELECT id FROM files WHERE path LIKE (?1 || '%')",
        )
        .bind::<diesel::sql_types::Text, _>(&prefix)
        .load::<IdResult>(&mut conn)?
        .into_iter()
        .map(|r| r.id)
        .collect();

        if !duplicate_ids.is_empty() {
            for file_id in &duplicate_ids {
                diesel::sql_query("DELETE FROM faces WHERE file_id = ?1")
                    .bind::<diesel::sql_types::Text, _>(file_id)
                    .execute(&mut conn)?;
                diesel::sql_query("DELETE FROM video_keyframes WHERE video_file_id = ?1")
                    .bind::<diesel::sql_types::Text, _>(file_id)
                    .execute(&mut conn)?;
                diesel::sql_query(
                    "UPDATE enhancement_tasks SET output_file_id = NULL WHERE output_file_id = ?1",
                )
                .bind::<diesel::sql_types::Text, _>(file_id)
                .execute(&mut conn)?;
                diesel::delete(files::table.filter(files::id.eq(file_id))).execute(&mut conn)?;
            }
            diesel::sql_query(
                "DELETE FROM shots WHERE id NOT IN (SELECT DISTINCT shot_id FROM files)",
            )
            .execute(&mut conn)?;
            info!(
                "Cleaned up {} duplicate absolute-path entries during migration",
                duplicate_ids.len()
            );
        }

        let faces_migrated = diesel::sql_query(
            "UPDATE OR IGNORE faces SET thumbnail_path = SUBSTR(thumbnail_path, LENGTH(?1) + 1) WHERE thumbnail_path LIKE (?1 || '%')",
        )
        .bind::<diesel::sql_types::Text, _>(&prefix)
        .execute(&mut conn)?;

        if files_migrated > 0 || faces_migrated > 0 {
            info!(
                "Migrated paths to relative: {} files, {} face thumbnails",
                files_migrated, faces_migrated
            );
        }
    }

    // Clean up people who lost all their shots (e.g. after shot merges)
    cleanup_orphaned_people(&mut conn)?;

    // Reclaim unused space
    info!("Running VACUUM on database");
    diesel::sql_query("VACUUM").execute(&mut conn)?;

    Ok(())
}

/// Get a setting value by key.
pub fn get_setting(conn: &mut SqliteConnection, key: &str) -> Option<String> {
    settings::table
        .filter(settings::key.eq(key))
        .select(settings::value)
        .first::<String>(conn)
        .ok()
}

/// Delete people who have no shots assigned to them and no faces referencing them.
/// This can happen when shots are merged and a person loses all their shots.
pub fn cleanup_orphaned_people(conn: &mut SqliteConnection) -> anyhow::Result<()> {
    // Collect people who still have at least one shot assigned
    let people_with_shots: Vec<String> = shots::table
        .select(shots::primary_person_id.assume_not_null())
        .filter(shots::primary_person_id.is_not_null())
        .distinct()
        .load::<String>(conn)?;

    // Unassign faces for people who have no shots
    let unassigned = diesel::update(
        faces::table
            .filter(faces::person_id.is_not_null())
            .filter(diesel::dsl::not(
                faces::person_id.eq_any(&people_with_shots),
            )),
    )
    .set(faces::person_id.eq(None::<String>))
    .execute(conn)?;
    if unassigned > 0 {
        info!("Unassigned {} faces from people with no shots", unassigned);
    }

    // Collect people who still have at least one face assigned
    let people_with_faces: Vec<String> = faces::table
        .select(faces::person_id.assume_not_null())
        .filter(faces::person_id.is_not_null())
        .distinct()
        .load::<String>(conn)?;

    // Delete people with no shots and no faces
    let deleted = diesel::delete(
        people::table
            .filter(diesel::dsl::not(people::id.eq_any(&people_with_shots)))
            .filter(diesel::dsl::not(people::id.eq_any(&people_with_faces))),
    )
    .execute(conn)?;
    if deleted > 0 {
        info!("Cleaned up {} orphaned people", deleted);
    }
    Ok(())
}

/// Migrate from the old `photos` table schema to the new `shots` table schema.
fn migrate_photos_to_shots<P: AsRef<Path>>(
    conn: &mut SqliteConnection,
    db_path: P,
) -> anyhow::Result<()> {
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
        info!("Database backed up to {:?}", backup_path);
    }

    diesel::sql_query("BEGIN TRANSACTION").execute(conn)?;

    // Create the new shots table with all new columns
    diesel::sql_query(
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
        )",
    )
    .execute(conn)?;

    diesel::sql_query(
        "INSERT INTO shots (id, main_file_id, timestamp, width, height, latitude, longitude, created_at)
            SELECT id, main_file_id, timestamp, width, height, latitude, longitude, created_at
            FROM photos",
    )
    .execute(conn)?;

    // Rename photo_id -> shot_id in files table (SQLite 3.25.0+)
    let has_photo_id: bool = diesel::sql_query(
        "SELECT COUNT(*) as cnt FROM pragma_table_info('files') WHERE name = 'photo_id'",
    )
    .get_result::<CountResult>(conn)
    .map(|r| r.cnt > 0)
    .unwrap_or(false);

    if has_photo_id {
        diesel::sql_query("ALTER TABLE files RENAME COLUMN photo_id TO shot_id").execute(conn)?;
    }

    // Add folder_name column to people if it doesn't exist
    let has_folder_name: bool = diesel::sql_query(
        "SELECT COUNT(*) as cnt FROM pragma_table_info('people') WHERE name = 'folder_name'",
    )
    .get_result::<CountResult>(conn)
    .map(|r| r.cnt > 0)
    .unwrap_or(false);

    if !has_folder_name {
        diesel::sql_query("ALTER TABLE people ADD COLUMN folder_name TEXT UNIQUE")
            .execute(conn)?;
    }

    // Populate folder_name from name or id
    diesel::sql_query("UPDATE people SET folder_name = COALESCE(name, id) WHERE folder_name IS NULL")
        .execute(conn)?;

    // Ensure exactly one is_original = 1 per shot.
    diesel::sql_query(
        "UPDATE files SET is_original = 1
         WHERE id IN (
             SELECT MIN(f.id) FROM files f
             LEFT JOIN (SELECT shot_id FROM files WHERE is_original = 1) o ON f.shot_id = o.shot_id
             WHERE o.shot_id IS NULL
             GROUP BY f.shot_id
         )",
    )
    .execute(conn)?;

    // Drop the old photos table
    diesel::sql_query("DROP TABLE photos").execute(conn)?;

    diesel::sql_query("COMMIT").execute(conn)?;

    info!("Migration from 'photos' to 'shots' completed successfully.");
    Ok(())
}

// Helper structs for sql_query results
#[derive(QueryableByName)]
struct CountResult {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    cnt: i64,
}

#[derive(QueryableByName)]
struct IdResult {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
}
