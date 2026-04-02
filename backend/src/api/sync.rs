use axum::{extract::Query, http::StatusCode, Json};
use diesel::prelude::*;
use diesel::sql_types::{Integer, Nullable, Text};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::UState;
use crate::schema::{files, shots};

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

#[derive(QueryableByName)]
struct SyncPersonRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Nullable<Text>)]
    name: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    thumbnail_face_id: Option<String>,
    #[diesel(sql_type = Integer)]
    shot_count: i32,
    #[diesel(sql_type = Nullable<Text>)]
    updated_at: Option<String>,
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
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

    let since = params.since.as_deref().unwrap_or("1970-01-01T00:00:00Z");

    // Query changed people (complex aggregation — kept as sql_query)
    let person_rows: Vec<SyncPersonRow> = diesel::sql_query(
        "SELECT p.id, p.name, p.thumbnail_face_id,
                COUNT(DISTINCT CASE WHEN s.id IS NOT NULL THEN s.id END) as shot_count,
                p.updated_at
         FROM people p
         LEFT JOIN shots s ON s.primary_person_id = p.id
         WHERE p.updated_at > ?1 OR p.created_at > ?1
         GROUP BY p.id",
    )
    .bind::<Text, _>(since)
    .load(&mut conn)
    .map_err(|e| {
        tracing::error!("Sync people query failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let people: Vec<SyncPerson> = person_rows
        .into_iter()
        .map(|row| SyncPerson {
            id: row.id,
            name: row.name,
            thumbnail_url: row
                .thumbnail_face_id
                .map(|fid| format!("/api/faces/{}/thumbnail", fid)),
            shot_count: row.shot_count as i64,
            updated_at: row.updated_at,
            deleted: false,
        })
        .collect();

    // Query changed shots
    let shot_rows: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>)> =
        shots::table
            .filter(
                shots::updated_at
                    .gt(since)
                    .or(shots::created_at.gt(since)),
            )
            .select((
                shots::id,
                shots::timestamp,
                shots::primary_person_id,
                shots::review_status,
                shots::updated_at,
            ))
            .load(&mut conn)
            .map_err(|e| {
                tracing::error!("Sync shots query failed: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let shots_vec: Vec<SyncShot> = shot_rows
        .into_iter()
        .map(|(id, timestamp, ppid, status, updated)| SyncShot {
            id,
            timestamp,
            primary_person_id: ppid,
            review_status: status,
            updated_at: updated,
            deleted: false,
        })
        .collect();

    // Query changed files
    let file_rows: Vec<(String, String, Option<String>, Option<bool>, Option<i32>, Option<String>)> =
        files::table
            .filter(
                files::updated_at
                    .gt(since)
                    .or(files::created_at.gt(since)),
            )
            .select((
                files::id,
                files::shot_id,
                files::mime_type,
                files::is_original,
                files::file_size,
                files::updated_at,
            ))
            .load(&mut conn)
            .map_err(|e| {
                tracing::error!("Sync files query failed: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let files_vec: Vec<SyncFile> = file_rows
        .into_iter()
        .map(|(id, shot_id, mime_type, is_original, file_size, updated)| SyncFile {
            id,
            shot_id,
            mime_type,
            is_original: is_original.unwrap_or(false),
            file_size: file_size.map(|s| s as i64),
            updated_at: updated,
            deleted: false,
        })
        .collect();

    Ok(Json(SyncResponse {
        people,
        shots: shots_vec,
        files: files_vec,
        sync_token: now,
    }))
}
