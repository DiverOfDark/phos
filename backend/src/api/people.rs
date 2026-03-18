use axum::{
    extract::Path,
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::UState;
use super::shots::ShotBrief;

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
pub(super) async fn get_people(UState(state): UState) -> Json<Vec<PersonBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare(
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
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            let thumbnail_face_id: Option<String> = row.get(3)?;
            let cover_file_id: Option<String> = row.get(7)?;
            Ok(PersonBrief {
                id: row.get(0)?,
                name: row.get(1)?,
                face_count: row.get(2)?,
                thumbnail_url: thumbnail_face_id.map(|fid| format!("/api/faces/{}/thumbnail", fid)),
                shot_count: row.get(4)?,
                pending_count: row.get(5)?,
                updated_at: row.get(6)?,
                cover_shot_thumbnail_url: cover_file_id.map(|fid| format!("/api/files/{}/thumbnail", fid)),
            })
        })
        .unwrap();

    let people: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(people)
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

    let db = state.db.lock().await;
    let id = uuid::Uuid::new_v4().to_string();

    db.execute(
        "INSERT INTO people (id, name) VALUES (?, ?)",
        params![id, name],
    )
    .map_err(|e| {
        tracing::error!("Failed to create person: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({"id": id, "name": name})))
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
) -> Json<Vec<ShotBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare(
            "SELECT DISTINCT s.id, s.timestamp, s.primary_person_id, s.review_status, s.folder_number,
                    f.id AS main_file_id, p.name AS person_name,
                    (SELECT COUNT(*) FROM files WHERE shot_id = s.id) AS file_count
             FROM shots s
             LEFT JOIN files f ON s.main_file_id = f.id
             LEFT JOIN people p ON s.primary_person_id = p.id
             WHERE s.primary_person_id = ?
             ORDER BY s.timestamp DESC",
        )
        .unwrap();

    let rows = stmt
        .query_map(params![id], |row| {
            let main_file_id: Option<String> = row.get(5)?;
            Ok(ShotBrief {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                primary_person_id: row.get(2)?,
                review_status: row.get(3)?,
                folder_number: row.get(4)?,
                thumbnail_url: main_file_id
                    .map(|fid| format!("/api/files/{}/thumbnail", fid))
                    .unwrap_or_default(),
                primary_person_name: row.get(6)?,
                file_count: row.get(7)?,
            })
        })
        .unwrap();

    let shots: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(shots)
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
) -> Json<Vec<PersonFaceBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare("SELECT id FROM faces WHERE person_id = ? LIMIT 12")
        .unwrap();
    let rows = stmt
        .query_map(params![id], |row| {
            let face_id: String = row.get(0)?;
            Ok(PersonFaceBrief {
                thumbnail_url: format!("/api/faces/{}/thumbnail", face_id),
                id: face_id,
            })
        })
        .unwrap();

    let faces: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(faces)
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

    let db = state.db.lock().await;

    db.execute(
        "UPDATE faces SET person_id = ? WHERE person_id = ?",
        params![payload.target_id, payload.source_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to reassign faces during merge: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Update shots that reference the source person as their primary
    db.execute(
        "UPDATE shots SET primary_person_id = ? WHERE primary_person_id = ?",
        params![payload.target_id, payload.source_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to reassign shots during merge: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    db.execute(
        "DELETE FROM people WHERE id = ?",
        params![payload.source_id],
    )
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
    let db = state.db.lock().await;

    // Query 1: Get person info
    let person = db
        .query_row(
            "SELECT id, name FROM people WHERE id = ?",
            params![id],
            |row| {
                Ok(PersonMeta {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StatusCode::NOT_FOUND,
            _ => {
                tracing::error!("Failed to query person {}: {}", id, e);
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    // Query 2: Get all shots with their files in one go
    let mut stmt = db
        .prepare(
            "SELECT s.id, s.timestamp, s.review_status,
                    f.id, f.mime_type, f.is_original, f.file_size
             FROM shots s
             JOIN files f ON f.shot_id = s.id
             WHERE s.primary_person_id = ?
             ORDER BY s.id, f.is_original DESC, f.path ASC",
        )
        .map_err(|e| {
            tracing::error!("Failed to prepare browse query for person {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let rows = stmt
        .query_map(params![id], |row| {
            let shot_id: String = row.get(0)?;
            let timestamp: Option<String> = row.get(1)?;
            let review_status: Option<String> = row.get(2)?;
            let file_id: String = row.get(3)?;
            let mime_type: Option<String> = row.get(4)?;
            let is_original: bool = row.get(5)?;
            let file_size: Option<i64> = row.get(6)?;
            Ok((shot_id, timestamp, review_status, file_id, mime_type, is_original, file_size))
        })
        .map_err(|e| {
            tracing::error!("Failed to execute browse query for person {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Group files by shot
    let mut shots: Vec<BrowseShotDetail> = Vec::new();
    let mut current_shot_id: Option<String> = None;

    for row in rows {
        let (shot_id, timestamp, review_status, file_id, mime_type, is_original, file_size) =
            row.map_err(|e| {
                tracing::error!("Failed to read browse row: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        let file = BrowseFileDetail {
            thumbnail_url: format!("/api/files/{}/thumbnail", file_id),
            id: file_id,
            mime_type,
            is_original,
            file_size,
        };

        if current_shot_id.as_deref() == Some(&shot_id) {
            // Same shot — append file to the last shot entry
            shots.last_mut().unwrap().files.push(file);
        } else {
            // New shot
            current_shot_id = Some(shot_id.clone());
            shots.push(BrowseShotDetail {
                id: shot_id,
                timestamp,
                review_status,
                files: vec![file],
            });
        }
    }

    // Sort shots by timestamp (newest first) after grouping
    shots.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(Json(PersonBrowseResponse { person, shots }))
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
    let db = state.db.lock().await;

    // Verify person exists
    let exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM people WHERE id = ?",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .map_err(|e| {
            tracing::error!("Failed to check person existence: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Find all shot_ids affected by this person's faces (for recalculation later)
    let mut stmt = db
        .prepare(
            "SELECT DISTINCT f.shot_id FROM faces fa
             JOIN files f ON fa.file_id = f.id
             WHERE fa.person_id = ?",
        )
        .map_err(|e| {
            tracing::error!("Failed to prepare affected shots query: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let affected_shot_ids: Vec<String> = stmt
        .query_map(params![id], |row| row.get(0))
        .map_err(|e| {
            tracing::error!("Failed to query affected shots: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Delete all face records for this person
    db.execute("DELETE FROM faces WHERE person_id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete faces for person {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Clear primary_person_id on shots that referenced this person
    db.execute(
        "UPDATE shots SET primary_person_id = NULL WHERE primary_person_id = ?",
        params![id],
    )
    .map_err(|e| {
        tracing::error!("Failed to clear primary_person_id: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Delete the person record
    db.execute("DELETE FROM people WHERE id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete person {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Recalculate primary_person_id for all affected shots
    for shot_id in &affected_shot_ids {
        let _ = super::recalculate_primary_person(&db, shot_id);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}
