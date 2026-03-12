use axum::{extract::Query, http::StatusCode, Json};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::UState;

#[derive(Deserialize, IntoParams, ToSchema)]
pub(super) struct SyncQuery {
    /// ISO8601 timestamp. If omitted, returns everything (full sync).
    since: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SyncPerson {
    id: String,
    name: Option<String>,
    thumbnail_url: Option<String>,
    shot_count: i64,
    updated_at: Option<String>,
    deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SyncShot {
    id: String,
    timestamp: Option<String>,
    primary_person_id: Option<String>,
    review_status: Option<String>,
    updated_at: Option<String>,
    deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SyncFile {
    id: String,
    shot_id: String,
    mime_type: Option<String>,
    is_original: bool,
    file_size: Option<i64>,
    updated_at: Option<String>,
    deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SyncResponse {
    people: Vec<SyncPerson>,
    shots: Vec<SyncShot>,
    files: Vec<SyncFile>,
    /// ISO8601 timestamp to use as `since` in the next request.
    sync_token: String,
}

#[utoipa::path(
    get,
    path = "/api/sync",
    tag = "sync",
    summary = "Incremental sync",
    description = "Returns all people, shots, and files changed since the given timestamp. If no `since` parameter is provided, returns everything (full sync).",
    params(SyncQuery),
    responses(
        (status = 200, description = "Sync data", body = SyncResponse),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_sync(
    UState(state): UState,
    Query(params): Query<SyncQuery>,
) -> Result<Json<SyncResponse>, StatusCode> {
    let db = state.db.lock().await;
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

    let since = params.since.as_deref().unwrap_or("1970-01-01T00:00:00Z");

    // Query changed people
    let mut stmt = db
        .prepare(
            "SELECT p.id, p.name, p.thumbnail_face_id,
                COUNT(DISTINCT CASE WHEN s.id IS NOT NULL THEN s.id END) as shot_count,
                p.updated_at
         FROM people p
         LEFT JOIN shots s ON s.primary_person_id = p.id
         WHERE p.updated_at > ? OR p.created_at > ?
         GROUP BY p.id",
        )
        .map_err(|e| {
            tracing::error!("Sync people query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let people: Vec<SyncPerson> = stmt
        .query_map(params![since, since], |row| {
            let thumbnail_face_id: Option<String> = row.get(2)?;
            Ok(SyncPerson {
                id: row.get(0)?,
                name: row.get(1)?,
                thumbnail_url: thumbnail_face_id
                    .map(|fid| format!("/api/faces/{}/thumbnail", fid)),
                shot_count: row.get(3)?,
                updated_at: row.get(4)?,
                deleted: false,
            })
        })
        .map_err(|e| {
            tracing::error!("Sync people query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Query changed shots
    let mut stmt = db
        .prepare(
            "SELECT id, timestamp, primary_person_id, review_status, updated_at
         FROM shots
         WHERE updated_at > ? OR created_at > ?",
        )
        .map_err(|e| {
            tracing::error!("Sync shots query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let shots: Vec<SyncShot> = stmt
        .query_map(params![since, since], |row| {
            Ok(SyncShot {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                primary_person_id: row.get(2)?,
                review_status: row.get(3)?,
                updated_at: row.get(4)?,
                deleted: false,
            })
        })
        .map_err(|e| {
            tracing::error!("Sync shots query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Query changed files
    let mut stmt = db
        .prepare(
            "SELECT id, shot_id, mime_type, is_original, file_size, updated_at
         FROM files
         WHERE updated_at > ? OR created_at > ?",
        )
        .map_err(|e| {
            tracing::error!("Sync files query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let files: Vec<SyncFile> = stmt
        .query_map(params![since, since], |row| {
            Ok(SyncFile {
                id: row.get(0)?,
                shot_id: row.get(1)?,
                mime_type: row.get(2)?,
                is_original: row.get(3)?,
                file_size: row.get(4)?,
                updated_at: row.get(5)?,
                deleted: false,
            })
        })
        .map_err(|e| {
            tracing::error!("Sync files query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(SyncResponse {
        people,
        shots,
        files,
        sync_token: now,
    }))
}
