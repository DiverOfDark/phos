use axum::{
    extract::Path,
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::UState;
use super::shots::ShotBrief;

#[derive(Serialize)]
pub(super) struct PersonBrief {
    id: String,
    name: Option<String>,
    face_count: i64,
    thumbnail_url: Option<String>,
    shot_count: i64,
    pending_count: i64,
}

pub(super) async fn get_people(UState(state): UState) -> Json<Vec<PersonBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare(
            "SELECT p.id, p.name, COUNT(DISTINCT fa.id) as face_count, p.thumbnail_face_id,
                    COUNT(DISTINCT CASE WHEN s_primary.id IS NOT NULL THEN s_primary.id END) as shot_count,
                    COUNT(DISTINCT CASE WHEN s_primary.id IS NOT NULL AND s_primary.review_status = 'pending' THEN s_primary.id END) as pending_count
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
            Ok(PersonBrief {
                id: row.get(0)?,
                name: row.get(1)?,
                face_count: row.get(2)?,
                thumbnail_url: thumbnail_face_id.map(|fid| format!("/api/faces/{}/thumbnail", fid)),
                shot_count: row.get(4)?,
                pending_count: row.get(5)?,
            })
        })
        .unwrap();

    let people: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(people)
}

/// Create a new person with a name
#[derive(Deserialize)]
pub(super) struct CreatePersonPayload {
    name: String,
}

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
             WHERE EXISTS (
                 SELECT 1 FROM faces fa
                 JOIN files ff ON fa.file_id = ff.id
                 WHERE ff.shot_id = s.id AND fa.person_id = ?
             )
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
#[derive(Serialize)]
pub(super) struct PersonFaceBrief {
    id: String,
    thumbnail_url: String,
}

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
#[derive(Deserialize)]
pub(super) struct RenamePersonPayload {
    name: String,
}

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
#[derive(Deserialize)]
pub(super) struct MergePeoplePayload {
    source_id: String,
    target_id: String,
}

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

/// Delete a person and all their face records.
/// Removes all faces belonging to this person, cleans up face_neighbors,
/// recalculates primary_person_id for affected shots, and deletes the person.
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

    // Delete face_neighbors for all faces of this person
    db.execute(
        "DELETE FROM face_neighbors WHERE face_id_a IN (SELECT id FROM faces WHERE person_id = ?)
         OR face_id_b IN (SELECT id FROM faces WHERE person_id = ?)",
        params![id, id],
    )
    .map_err(|e| {
        tracing::error!("Failed to delete face_neighbors for person {}: {}", id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

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
