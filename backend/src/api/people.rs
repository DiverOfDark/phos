use axum::{
    extract::Path,
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use diesel::sql_types::{Integer, Nullable, Text};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::UState;
use super::shots::ShotBrief;
use crate::schema::{faces, files, people, shots};

#[derive(Serialize, ToSchema)]
pub(super) struct PersonBrief {
    id: String,
    name: Option<String>,
    face_count: i64,
    thumbnail_url: Option<String>,
    shot_count: i64,
    pending_count: i64,
    updated_at: Option<String>,
    cover_shot_thumbnail_url: Option<String>,
}

#[derive(QueryableByName)]
struct PersonBriefRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Nullable<Text>)]
    name: Option<String>,
    #[diesel(sql_type = Integer)]
    face_count: i32,
    #[diesel(sql_type = Nullable<Text>)]
    thumbnail_face_id: Option<String>,
    #[diesel(sql_type = Integer)]
    shot_count: i32,
    #[diesel(sql_type = Integer)]
    pending_count: i32,
    #[diesel(sql_type = Nullable<Text>)]
    updated_at: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    cover_file_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/people",
    tag = "people",
    summary = "List people",
    description = "List all detected people with their name, face count, and a representative thumbnail face ID.",
    responses(
        (status = 200, description = "List all people", body = Vec<PersonBrief>),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_people(UState(state): UState) -> Result<Json<Vec<PersonBrief>>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows: Vec<PersonBriefRow> = diesel::sql_query(
        "SELECT p.id, p.name, COUNT(DISTINCT fa.id) as face_count, p.thumbnail_face_id,
                COUNT(DISTINCT CASE WHEN s_primary.id IS NOT NULL THEN s_primary.id END) as shot_count,
                COUNT(DISTINCT CASE WHEN s_primary.id IS NOT NULL AND s_primary.review_status = 'pending' THEN s_primary.id END) as pending_count,
                p.updated_at,
                (SELECT f_cover.id FROM shots s_cover
                 JOIN files f_cover ON s_cover.main_file_id = f_cover.id
                 WHERE s_cover.primary_person_id = p.id
                 ORDER BY s_cover.timestamp DESC LIMIT 1) as cover_file_id
         FROM people p
         LEFT JOIN faces fa ON fa.person_id = p.id
         LEFT JOIN shots s_primary ON s_primary.primary_person_id = p.id
         GROUP BY p.id
         ORDER BY p.name ASC NULLS LAST, face_count DESC",
    )
    .load(&mut conn)
    .map_err(|e| {
        tracing::error!("Failed to query people: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let people: Vec<PersonBrief> = rows
        .into_iter()
        .map(|row| PersonBrief {
            id: row.id,
            name: row.name,
            face_count: row.face_count as i64,
            thumbnail_url: row.thumbnail_face_id.map(|fid| format!("/api/faces/{}/thumbnail", fid)),
            shot_count: row.shot_count as i64,
            pending_count: row.pending_count as i64,
            updated_at: row.updated_at,
            cover_shot_thumbnail_url: row.cover_file_id.map(|fid| format!("/api/files/{}/thumbnail", fid)),
        })
        .collect();

    Ok(Json(people))
}

/// Create a new person with a name
#[derive(Deserialize, ToSchema)]
pub(super) struct CreatePersonPayload {
    name: String,
}

#[utoipa::path(
    post,
    path = "/api/people",
    tag = "people",
    summary = "Create a person",
    description = "Create a new named person record that faces can be assigned to.",
    request_body = CreatePersonPayload,
    responses(
        (status = 200, description = "Success"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn create_person(
    UState(state): UState,
    Json(payload): Json<CreatePersonPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let id = uuid::Uuid::new_v4().to_string();

    diesel::insert_into(people::table)
        .values((people::id.eq(&id), people::name.eq(&name)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to create person: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"id": id, "name": name})))
}

#[derive(QueryableByName)]
struct PersonShotRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Nullable<Text>)]
    timestamp: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    primary_person_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    review_status: Option<String>,
    #[diesel(sql_type = Nullable<Integer>)]
    folder_number: Option<i32>,
    #[diesel(sql_type = Nullable<Text>)]
    main_file_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    person_name: Option<String>,
    #[diesel(sql_type = Integer)]
    file_count: i32,
    #[diesel(sql_type = Nullable<Text>)]
    description: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/people/{id}",
    tag = "people",
    summary = "Get person's shots",
    description = "Retrieve all shots associated with a specific person, ordered by date.",
    params(
        ("id" = String, Path, description = "Person ID")
    ),
    responses(
        (status = 200, description = "Shots for a person", body = Vec<ShotBrief>),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_person_shots(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<Vec<ShotBrief>>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows: Vec<PersonShotRow> = diesel::sql_query(
        "SELECT DISTINCT s.id, s.timestamp, s.primary_person_id, s.review_status, s.folder_number,
                f.id AS main_file_id, p.name AS person_name,
                (SELECT COUNT(*) FROM files WHERE shot_id = s.id) AS file_count,
                s.description
         FROM shots s
         LEFT JOIN files f ON s.main_file_id = f.id
         LEFT JOIN people p ON s.primary_person_id = p.id
         WHERE s.primary_person_id = ?1
         ORDER BY s.timestamp DESC",
    )
    .bind::<Text, _>(&id)
    .load(&mut conn)
    .map_err(|e| {
        tracing::error!("Failed to query person shots: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let shots: Vec<ShotBrief> = rows
        .into_iter()
        .map(|row| ShotBrief {
            id: row.id,
            timestamp: row.timestamp,
            primary_person_id: row.primary_person_id,
            review_status: row.review_status,
            folder_number: row.folder_number.map(|n| n as i64),
            thumbnail_url: row
                .main_file_id
                .map(|fid| format!("/api/files/{}/thumbnail", fid))
                .unwrap_or_default(),
            primary_person_name: row.person_name,
            file_count: row.file_count as i64,
            description: row.description,
        })
        .collect();

    Ok(Json(shots))
}

/// Get up to 12 face IDs with thumbnail URLs for a person
#[derive(Serialize, ToSchema)]
pub(super) struct PersonFaceBrief {
    id: String,
    thumbnail_url: String,
}

#[utoipa::path(
    get,
    path = "/api/people/{id}/faces",
    tag = "people",
    summary = "Get person's faces",
    description = "Retrieve all face thumbnail IDs belonging to a specific person.",
    params(
        ("id" = String, Path, description = "Person ID")
    ),
    responses(
        (status = 200, description = "Face thumbnails for a person", body = Vec<PersonFaceBrief>),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_person_faces(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<Vec<PersonFaceBrief>>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let face_ids: Vec<String> = faces::table
        .filter(faces::person_id.eq(&id))
        .select(faces::id)
        .limit(12)
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query person faces: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let face_briefs: Vec<PersonFaceBrief> = face_ids
        .into_iter()
        .map(|face_id| PersonFaceBrief {
            thumbnail_url: format!("/api/faces/{}/thumbnail", face_id),
            id: face_id,
        })
        .collect();

    Ok(Json(face_briefs))
}

/// Rename a person
#[derive(Deserialize, ToSchema)]
pub(super) struct RenamePersonPayload {
    name: String,
}

#[utoipa::path(
    put,
    path = "/api/people/{id}",
    tag = "people",
    summary = "Rename a person",
    description = "Rename an existing person.",
    params(
        ("id" = String, Path, description = "Person ID")
    ),
    request_body = RenamePersonPayload,
    responses(
        (status = 200, description = "Success"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn rename_person(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<RenamePersonPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    crate::import::rename_person_folder(&db, &state.library_root, &id, &payload.name).map_err(|e| {
        tracing::error!("Failed to rename person folder: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Merge two people: move all faces from source to target, then delete source
#[derive(Deserialize, ToSchema)]
pub(super) struct MergePeoplePayload {
    source_id: String,
    target_id: String,
}

#[utoipa::path(
    post,
    path = "/api/people/merge",
    tag = "people",
    summary = "Merge people",
    description = "Merge two or more people into a single person. All faces from the source people are reassigned to the target person.",
    request_body = MergePeoplePayload,
    responses(
        (status = 200, description = "Success"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn merge_people(
    UState(state): UState,
    Json(payload): Json<MergePeoplePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.source_id == payload.target_id {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    diesel::update(faces::table.filter(faces::person_id.eq(&payload.source_id)))
        .set(faces::person_id.eq(&payload.target_id))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to reassign faces during merge: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Update shots that reference the source person as their primary
    diesel::update(shots::table.filter(shots::primary_person_id.eq(&payload.source_id)))
        .set(shots::primary_person_id.eq(&payload.target_id))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to reassign shots during merge: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    diesel::delete(people::table.filter(people::id.eq(&payload.source_id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete merged person: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Browse a person: returns person metadata + all shots + all files per shot.
/// Solves the N+1 problem for the mobile client.
#[derive(Serialize, ToSchema)]
pub(super) struct PersonMeta {
    id: String,
    name: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct BrowseFileDetail {
    id: String,
    mime_type: Option<String>,
    is_original: bool,
    file_size: Option<i64>,
    thumbnail_url: String,
}

#[derive(Serialize, ToSchema)]
pub(super) struct BrowseShotDetail {
    id: String,
    timestamp: Option<String>,
    review_status: Option<String>,
    files: Vec<BrowseFileDetail>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct PersonBrowseResponse {
    person: PersonMeta,
    shots: Vec<BrowseShotDetail>,
}

#[derive(QueryableByName)]
struct BrowseRow {
    #[diesel(sql_type = Text)]
    shot_id: String,
    #[diesel(sql_type = Nullable<Text>)]
    timestamp: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    review_status: Option<String>,
    #[diesel(sql_type = Text)]
    file_id: String,
    #[diesel(sql_type = Nullable<Text>)]
    mime_type: Option<String>,
    #[diesel(sql_type = Nullable<diesel::sql_types::Bool>)]
    is_original: Option<bool>,
    #[diesel(sql_type = Nullable<Integer>)]
    file_size: Option<i32>,
}

#[utoipa::path(
    get,
    path = "/api/people/{id}/browse",
    tag = "people",
    summary = "Get person browse graph",
    description = "Returns a complete person browse graph with all shots and their file variants in a single response. Designed for offline-first browsing clients.",
    params(
        ("id" = String, Path, description = "Person ID")
    ),
    responses(
        (status = 200, description = "Person browse graph", body = PersonBrowseResponse),
        (status = 404, description = "Person not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_person_browse(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<PersonBrowseResponse>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Query 1: Get person info
    let person_row: (String, Option<String>) = people::table
        .filter(people::id.eq(&id))
        .select((people::id, people::name))
        .first(&mut conn)
        .map_err(|e| match e {
            diesel::result::Error::NotFound => StatusCode::NOT_FOUND,
            _ => {
                tracing::error!("Failed to query person {}: {}", id, e);
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    let person = PersonMeta {
        id: person_row.0,
        name: person_row.1,
    };

    // Query 2: Get all shots with their files in one go
    let rows: Vec<BrowseRow> = diesel::sql_query(
        "SELECT s.id AS shot_id, s.timestamp, s.review_status,
                f.id AS file_id, f.mime_type, f.is_original, f.file_size
         FROM shots s
         JOIN files f ON f.shot_id = s.id
         WHERE s.primary_person_id = ?1
         ORDER BY s.id, f.is_original DESC, f.path ASC",
    )
    .bind::<Text, _>(&id)
    .load(&mut conn)
    .map_err(|e| {
        tracing::error!("Failed to execute browse query for person {}: {}", id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Group files by shot
    let mut shots_vec: Vec<BrowseShotDetail> = Vec::new();
    let mut current_shot_id: Option<String> = None;

    for row in rows {
        let file = BrowseFileDetail {
            thumbnail_url: format!("/api/files/{}/thumbnail", row.file_id),
            id: row.file_id,
            mime_type: row.mime_type,
            is_original: row.is_original.unwrap_or(false),
            file_size: row.file_size.map(|s| s as i64),
        };

        if current_shot_id.as_deref() == Some(&row.shot_id) {
            // Same shot — append file to the last shot entry
            shots_vec.last_mut().unwrap().files.push(file);
        } else {
            // New shot
            current_shot_id = Some(row.shot_id.clone());
            shots_vec.push(BrowseShotDetail {
                id: row.shot_id,
                timestamp: row.timestamp,
                review_status: row.review_status,
                files: vec![file],
            });
        }
    }

    // Sort shots by timestamp (newest first) after grouping
    shots_vec.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(Json(PersonBrowseResponse { person, shots: shots_vec }))
}

/// Delete a person and all their face records.
/// Removes all faces belonging to this person,
/// recalculates primary_person_id for affected shots, and deletes the person.
#[utoipa::path(
    delete,
    path = "/api/people/{id}",
    tag = "people",
    summary = "Delete a person",
    description = "Delete a person and unassign all their faces. Shots will have their primary person recalculated.",
    params(
        ("id" = String, Path, description = "Person ID")
    ),
    responses(
        (status = 200, description = "Success"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn delete_person(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify person exists
    let exists: bool = people::table
        .filter(people::id.eq(&id))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .map_err(|e| {
            tracing::error!("Failed to check person existence: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Find all shot_ids affected by this person's faces (for recalculation later)
    let affected_shot_ids: Vec<String> = faces::table
        .inner_join(files::table.on(faces::file_id.eq(files::id)))
        .filter(faces::person_id.eq(&id))
        .select(files::shot_id)
        .distinct()
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query affected shots: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete all face records for this person
    diesel::delete(faces::table.filter(faces::person_id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete faces for person {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Clear primary_person_id on shots that referenced this person
    diesel::update(shots::table.filter(shots::primary_person_id.eq(&id)))
        .set(shots::primary_person_id.eq(None::<String>))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to clear primary_person_id: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete the person record
    diesel::delete(people::table.filter(people::id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete person {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Recalculate primary_person_id for all affected shots
    for shot_id in &affected_shot_ids {
        let _ = super::recalculate_primary_person(&mut conn, shot_id);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}
