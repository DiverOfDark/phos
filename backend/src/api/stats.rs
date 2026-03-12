use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::UState;

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
    let db = state.db.lock().await;

    let total_shots: i64 = db
        .query_row("SELECT COUNT(*) FROM shots", [], |r| r.get(0))
        .unwrap_or(0);
    let total_people: i64 = db
        .query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
        .unwrap_or(0);
    let total_files: i64 = db
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap_or(0);

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
    let db = state.db.lock().await;

    let total_shots: i64 = db
        .query_row("SELECT COUNT(*) FROM shots", [], |r| r.get(0))
        .unwrap_or(0);
    let total_files: i64 = db
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap_or(0);
    let total_people: i64 = db
        .query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
        .unwrap_or(0);
    let pending_review: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE review_status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let confirmed: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE review_status = 'confirmed'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let unsorted: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE primary_person_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let unnamed_people: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM people WHERE name IS NULL OR name = ''",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Json(OrganizeStatsResponse {
        total_shots,
        total_files,
        total_people,
        pending_review,
        confirmed,
        unsorted,
        unnamed_people,
    })
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
    std::thread::spawn(move || {
        if let Err(e) = crate::import::run_reorganize(&library_root, false) {
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
    let db_path_result: Result<String, rusqlite::Error> = {
        let db = state.db.lock().await;
        db.query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
    };

    if let Ok(db_path) = db_path_result {
        tokio::task::spawn_blocking(move || {
            let scanner = crate::scanner::Scanner::new(std::path::PathBuf::from(db_path), None);
            let _ = scanner.scan(std::path::Path::new(&payload.path));
        });
    }

    Json(serde_json::json!({"status": "started"}))
}
