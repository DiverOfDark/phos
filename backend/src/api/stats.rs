use axum::Json;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::UState;
use crate::schema::{files, people, shots};

#[utoipa::path(
    get,
    path = "/api/version",
    tag = "system",
    summary = "Get server version",
    description = "Return the current server version string.",
    responses(
        (status = 200, description = "Returns the server version", body = serde_json::Value)
    )
)]
pub(super) async fn get_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "version": env!("PHOS_VERSION") }))
}

/// Return aggregate stats about the library
#[derive(Serialize, ToSchema)]
pub(super) struct StatsResponse {
    total_shots: i64,
    total_people: i64,
    total_files: i64,
}

#[utoipa::path(
    get,
    path = "/api/stats",
    tag = "system",
    summary = "Get library statistics",
    description = "Return aggregate library statistics including total shots, people, and files.",
    responses(
        (status = 200, description = "Aggregate library statistics", body = StatsResponse),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_stats(UState(state): UState) -> Json<StatsResponse> {
    let mut conn = state.pool.get().unwrap();

    let total_shots: i64 = shots::table.count().get_result(&mut conn).unwrap_or(0);
    let total_people: i64 = people::table.count().get_result(&mut conn).unwrap_or(0);
    let total_files: i64 = files::table.count().get_result(&mut conn).unwrap_or(0);

    Json(StatsResponse {
        total_shots,
        total_people,
        total_files,
    })
}

/// Return detailed organize stats about the library
#[derive(Serialize, ToSchema)]
pub(super) struct OrganizeStatsResponse {
    total_shots: i64,
    total_files: i64,
    total_people: i64,
    pending_review: i64,
    confirmed: i64,
    unsorted: i64,
    unnamed_people: i64,
}

#[utoipa::path(
    get,
    path = "/api/organize/stats",
    tag = "system",
    summary = "Get organization statistics",
    description = "Return detailed organization statistics including pending reviews, confirmed shots, unsorted shots, and unnamed people.",
    responses(
        (status = 200, description = "Detailed organize statistics", body = OrganizeStatsResponse),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_organize_stats(UState(state): UState) -> Json<OrganizeStatsResponse> {
    let mut conn = state.pool.get().unwrap();
    Json(organize_stats(&mut conn))
}

/// The body of [`get_organize_stats`], separated from the extractor so the
/// counts can be tested against a database.
pub(super) fn organize_stats(conn: &mut diesel::SqliteConnection) -> OrganizeStatsResponse {
    let total_shots: i64 = shots::table.count().get_result(conn).unwrap_or(0);
    let total_files: i64 = files::table.count().get_result(conn).unwrap_or(0);
    let total_people: i64 = people::table.count().get_result(conn).unwrap_or(0);
    let pending_review: i64 = shots::table
        .filter(shots::review_status.eq("pending"))
        .count()
        .get_result(conn)
        .unwrap_or(0);
    let confirmed: i64 = shots::table
        .filter(shots::review_status.eq("confirmed"))
        .count()
        .get_result(conn)
        .unwrap_or(0);
    // Same rule as the Unsorted browse and the web's unsorted list, so the number
    // on the tile is the number of tiles behind it: owned by nobody — which
    // includes pointing at a person row that is gone — and having something left
    // to show.
    let unsorted: i64 = shots::table
        .filter(
            shots::primary_person_id
                .is_null()
                .or(diesel::dsl::not(diesel::dsl::exists(
                    people::table.filter(people::id.nullable().eq(shots::primary_person_id)),
                ))),
        )
        .filter(diesel::dsl::exists(
            files::table.filter(files::shot_id.eq(shots::id)),
        ))
        .count()
        .get_result(conn)
        .unwrap_or(0);
    let unnamed_people: i64 = people::table
        .filter(people::name.is_null().or(people::name.eq("")))
        .count()
        .get_result(conn)
        .unwrap_or(0);

    OrganizeStatsResponse {
        total_shots,
        total_files,
        total_people,
        pending_review,
        confirmed,
        unsorted,
        unnamed_people,
    }
}

/// Trigger filesystem reorganization in a background thread
#[utoipa::path(
    post,
    path = "/api/reorganize",
    tag = "system",
    summary = "Trigger reorganization",
    description = "Trigger a background filesystem reorganization that moves files into person-based folder structure.",
    responses(
        (status = 200, description = "Reorganization started", body = serde_json::Value),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn trigger_reorganize(UState(state): UState) -> Json<serde_json::Value> {
    let library_root = state.library_root.clone();
    let organizer = state.organizer.clone();
    std::thread::spawn(move || {
        if let Err(e) = organizer.run_now(&library_root) {
            tracing::error!("Background reorganize failed: {}", e);
        } else {
            tracing::info!("Background reorganize completed successfully");
        }
    });

    Json(serde_json::json!({"status": "started"}))
}

#[derive(Deserialize, ToSchema)]
pub(super) struct ScanParams {
    path: String,
}

#[utoipa::path(
    post,
    path = "/api/scan",
    tag = "system",
    summary = "Trigger library scan",
    description = "Trigger a background library scan on the specified path to detect new or changed media files and run face detection.",
    request_body = ScanParams,
    responses(
        (status = 200, description = "Scan started", body = serde_json::Value),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn trigger_scan(
    UState(state): UState,
    Json(payload): Json<ScanParams>,
) -> Json<serde_json::Value> {
    let scanner = state.scanner.clone();
    let scan_path = payload.path.clone();
    tokio::task::spawn_blocking(move || {
        let path = std::path::Path::new(&scan_path);
        if let Err(e) = scanner.scan(path) {
            tracing::error!("Triggered scan failed: {}", e);
        }
        if let Err(e) = scanner.caption_shots(path) {
            tracing::error!("Triggered captioning failed: {}", e);
        }
    });

    Json(serde_json::json!({"status": "started"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::connection::SimpleConnection;

    /// The same library the Unsorted browse is tested against: one sorted shot,
    /// two loose ones, one left pointing at a person who was deleted, and one
    /// whose files are gone.
    fn library() -> (tempfile::TempDir, diesel::SqliteConnection) {
        let tmp = tempfile::tempdir().expect("temp dir");
        let db_path = tmp.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).expect("schema");
        let mut conn = crate::db::open_diesel_connection(&db_path).expect("connection");

        conn.batch_execute(
            "INSERT INTO people (id, name) VALUES ('alice', 'Alice');
             INSERT INTO shots (id, primary_person_id) VALUES
                ('shot-alice', 'alice'),
                ('shot-loose-1', NULL),
                ('shot-loose-2', NULL),
                ('shot-orphan', 'ghost'),
                ('shot-fileless', NULL);
             INSERT INTO files (id, shot_id, path, hash, is_original) VALUES
                ('file-alice', 'shot-alice', 'a.jpg', 'h1', 1),
                ('file-loose-1', 'shot-loose-1', 'b.jpg', 'h2', 1),
                ('file-loose-2', 'shot-loose-2', 'c.jpg', 'h3', 1),
                ('file-orphan', 'shot-orphan', 'd.jpg', 'h4', 1);",
        )
        .expect("fixture");

        (tmp, conn)
    }

    /// The number on the Unsorted tile has to be the number of shots behind it,
    /// or the phone reads as if it were hiding photos.
    ///
    /// Both edge cases are in the fixture: the shot whose person was deleted
    /// counts (it is in nobody's grid), and the shot with no files does not
    /// (there is nothing to open).
    #[test]
    fn the_unsorted_count_matches_what_the_unsorted_view_can_show() {
        let (_tmp, mut conn) = library();

        let stats = organize_stats(&mut conn);

        let browsable = crate::api::people::browse_graph(
            &mut conn,
            crate::api::people::UNSORTED_BROWSE_ID,
        )
        .expect("browse")
        .shots
        .len();

        assert_eq!(3, stats.unsorted);
        assert_eq!(browsable as i64, stats.unsorted);
    }
}
