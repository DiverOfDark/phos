use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::UState;

#[derive(Serialize)]
pub(crate) struct ShotBrief {
    pub id: String,
    pub thumbnail_url: String,
    pub timestamp: Option<String>,
    pub file_count: i64,
    pub primary_person_id: Option<String>,
    pub primary_person_name: Option<String>,
    pub review_status: Option<String>,
    pub folder_number: Option<i64>,
}

#[derive(Serialize)]
pub(super) struct SimilarShotItem {
    id: String,
    thumbnail_url: String,
    file_count: i64,
    primary_person_name: Option<String>,
    review_status: Option<String>,
    distance: u32,
}

#[derive(Serialize)]
pub(super) struct SimilarShotsGrouped {
    person_id: Option<String>,
    person_name: Option<String>,
    shots: Vec<SimilarShotItem>,
}

#[derive(Deserialize)]
pub(super) struct ShotsQuery {
    q: Option<String>,
    person_id: Option<String>,
    status: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

/// GET /api/shots - list shots with query params: person_id, status, q, from, to
pub(super) async fn get_shots(
    UState(state): UState,
    Query(params): Query<ShotsQuery>,
) -> Json<Vec<ShotBrief>> {
    let db = state.db.lock().await;

    let mut sql = String::from(
        "SELECT DISTINCT s.id, s.timestamp, s.primary_person_id, s.review_status, s.folder_number,
                f.id AS main_file_id, p.name AS person_name,
                (SELECT COUNT(*) FROM files WHERE shot_id = s.id) AS file_count
         FROM shots s
         LEFT JOIN files f ON s.main_file_id = f.id
         LEFT JOIN people p ON s.primary_person_id = p.id",
    );
    let mut conditions: Vec<String> = Vec::new();
    let mut bind_values: Vec<String> = Vec::new();

    if let Some(ref person_id) = params.person_id {
        conditions.push("s.primary_person_id = ?".to_string());
        bind_values.push(person_id.clone());
    }

    if let Some(ref status) = params.status {
        if status == "unsorted" {
            conditions.push("s.primary_person_id IS NULL".to_string());
        } else {
            conditions.push("s.review_status = ?".to_string());
            bind_values.push(status.clone());
        }
    }

    if let Some(ref q) = params.q {
        // Search in file paths associated with the shot via a subquery
        conditions.push(
            "EXISTS (SELECT 1 FROM files fq WHERE fq.shot_id = s.id AND fq.path LIKE ?)"
                .to_string(),
        );
        bind_values.push(format!("%{}%", q));
    }

    if let Some(ref from) = params.from {
        conditions.push("s.timestamp >= ?".to_string());
        bind_values.push(from.clone());
    }

    if let Some(ref to) = params.to {
        conditions.push("s.timestamp <= ?".to_string());
        bind_values.push(to.clone());
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(" ORDER BY s.timestamp DESC");

    let mut stmt = match db.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to prepare shots query: {}", e);
            return Json(Vec::new());
        }
    };

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = bind_values
        .iter()
        .map(|v| v as &dyn rusqlite::types::ToSql)
        .collect();

    let rows = match stmt.query_map(param_refs.as_slice(), |row| {
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
    }) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to query shots: {}", e);
            return Json(Vec::new());
        }
    };

    let shots: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(shots)
}

#[derive(Serialize)]
pub(super) struct ShotDetailResponse {
    id: String,
    timestamp: Option<String>,
    primary_person_id: Option<String>,
    primary_person_name: Option<String>,
    review_status: Option<String>,
    folder_number: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    files: Vec<FileDetail>,
    faces: Vec<FaceDetail>,
    also_contains: Vec<AlsoContainsPerson>,
    prev_shot_id: Option<String>,
    next_shot_id: Option<String>,
}

#[derive(Serialize)]
pub(super) struct FileDetail {
    id: String,
    path: String,
    mime_type: Option<String>,
    is_original: bool,
    file_size: Option<i64>,
}

#[derive(Serialize)]
pub(super) struct FaceDetail {
    id: String,
    file_id: String,
    person_id: Option<String>,
    person_name: Option<String>,
    box_x1: f32,
    box_y1: f32,
    box_x2: f32,
    box_y2: f32,
}

#[derive(Serialize)]
pub(super) struct AlsoContainsPerson {
    id: String,
    name: Option<String>,
}

/// GET /api/shots/:id - detail with files, faces, primary person, also_contains
pub(super) async fn get_shot_detail(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<ShotDetailResponse>, StatusCode> {
    let db = state.db.lock().await;

    // Get shot metadata
    let shot_row = db
        .query_row(
            "SELECT s.id, s.timestamp, s.primary_person_id, s.review_status, s.folder_number, p.name, s.width, s.height
             FROM shots s
             LEFT JOIN people p ON s.primary_person_id = p.id
             WHERE s.id = ?",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Get files for this shot
    let mut stmt = db
        .prepare("SELECT id, path, mime_type, is_original, file_size FROM files WHERE shot_id = ? ORDER BY is_original DESC, path ASC")
        .map_err(|e| {
            tracing::error!("Failed to prepare files query: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let files: Vec<FileDetail> = stmt
        .query_map(params![id], |row| {
            Ok(FileDetail {
                id: row.get(0)?,
                path: row.get(1)?,
                mime_type: row.get(2)?,
                is_original: row.get::<_, bool>(3).unwrap_or(false),
                file_size: row.get(4)?,
            })
        })
        .map_err(|e| {
            tracing::error!("Failed to query files: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Get faces for files in this shot, with person names
    let mut stmt = db
        .prepare(
            "SELECT fa.id, fa.file_id, fa.person_id, p.name, fa.box_x1, fa.box_y1, fa.box_x2, fa.box_y2
             FROM faces fa
             JOIN files f ON fa.file_id = f.id
             LEFT JOIN people p ON fa.person_id = p.id
             WHERE f.shot_id = ?",
        )
        .map_err(|e| {
            tracing::error!("Failed to prepare faces query: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let faces: Vec<FaceDetail> = stmt
        .query_map(params![id], |row| {
            Ok(FaceDetail {
                id: row.get(0)?,
                file_id: row.get(1)?,
                person_id: row.get(2)?,
                person_name: row.get(3)?,
                box_x1: row.get(4)?,
                box_y1: row.get(5)?,
                box_x2: row.get(6)?,
                box_y2: row.get(7)?,
            })
        })
        .map_err(|e| {
            tracing::error!("Failed to query faces: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Compute also_contains: people who have faces in this shot OTHER than the primary person
    let mut also_contains: Vec<AlsoContainsPerson> = Vec::new();
    let mut seen_person_ids = std::collections::HashSet::new();
    for face in &faces {
        if let Some(ref pid) = face.person_id {
            // Skip the primary person and duplicates
            if Some(pid.as_str()) != shot_row.2.as_deref() && seen_person_ids.insert(pid.clone()) {
                also_contains.push(AlsoContainsPerson {
                    id: pid.clone(),
                    name: face.person_name.clone(),
                });
            }
        }
    }

    // Previous shot (newer timestamp, or same timestamp with id > current for stable ordering)
    let prev_shot_id: Option<String> = db
        .query_row(
            "SELECT id FROM shots
             WHERE (timestamp > ?1 OR (timestamp = ?1 AND id > ?2) OR (timestamp IS NULL AND ?1 IS NOT NULL))
             ORDER BY timestamp ASC, id ASC
             LIMIT 1",
            params![shot_row.1, id],
            |row| row.get(0),
        )
        .ok();

    // Next shot (older timestamp, or same timestamp with id < current for stable ordering)
    let next_shot_id: Option<String> = db
        .query_row(
            "SELECT id FROM shots
             WHERE (timestamp < ?1 OR (timestamp = ?1 AND id < ?2) OR (?1 IS NULL AND timestamp IS NOT NULL))
             ORDER BY timestamp DESC, id DESC
             LIMIT 1",
            params![shot_row.1, id],
            |row| row.get(0),
        )
        .ok();

    Ok(Json(ShotDetailResponse {
        id: shot_row.0,
        timestamp: shot_row.1,
        primary_person_id: shot_row.2,
        primary_person_name: shot_row.5,
        review_status: shot_row.3,
        folder_number: shot_row.4,
        width: shot_row.6,
        height: shot_row.7,
        files,
        faces,
        also_contains,
        prev_shot_id,
        next_shot_id,
    }))
}

pub(super) async fn delete_shot(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    // Collect file paths and IDs to clean up
    let mut stmt = db
        .prepare("SELECT id, path FROM files WHERE shot_id = ?")
        .map_err(|e| {
            tracing::error!("Failed to prepare file query for delete: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let files: Vec<(String, String)> = stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| {
            tracing::error!("Failed to query files for delete: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .collect();

    if files.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    let file_ids: Vec<&str> = files.iter().map(|(id, _)| id.as_str()).collect();

    // Delete face_neighbors referencing faces of these files
    for fid in &file_ids {
        db.execute(
            "DELETE FROM face_neighbors WHERE face_id_a IN (SELECT id FROM faces WHERE file_id = ?) OR face_id_b IN (SELECT id FROM faces WHERE file_id = ?)",
            params![fid, fid],
        ).map_err(|e| {
            tracing::error!("Failed to delete face_neighbors: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    // Delete faces
    for fid in &file_ids {
        db.execute("DELETE FROM faces WHERE file_id = ?", params![fid])
            .map_err(|e| {
                tracing::error!("Failed to delete faces: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    // Delete video_keyframes
    for fid in &file_ids {
        db.execute(
            "DELETE FROM video_keyframes WHERE video_file_id = ?",
            params![fid],
        )
        .map_err(|e| {
            tracing::error!("Failed to delete video_keyframes: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    // Delete files from DB
    db.execute("DELETE FROM files WHERE shot_id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete files: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete the shot record
    let deleted = db
        .execute("DELETE FROM shots WHERE id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete shot: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    // Delete physical files from disk (best-effort)
    for (_, path) in &files {
        if let Err(e) = std::fs::remove_file(path) {
            tracing::warn!("Failed to delete file from disk {:?}: {}", path, e);
        }
    }

    // Clean up cached thumbnails and empty directories (best-effort)
    let db_path: String = db
        .query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
        .unwrap_or_default();
    if !db_path.is_empty() {
        let db_dir = std::path::Path::new(&db_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let thumb_dir = db_dir.join(".phos_thumbnails");
        for (fid, _) in &files {
            let _ = std::fs::remove_file(thumb_dir.join(format!("{}.jpg", fid)));
        }
        let _ = crate::import::cleanup_empty_dirs(db_dir);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// PUT /api/shots/:id - update primary_person_id and/or review_status.
/// When primary_person changes, assign new folder_number (MAX+1 for that person).
#[derive(Deserialize)]
pub(super) struct UpdateShotPayload {
    primary_person_id: Option<String>,
    review_status: Option<String>,
}

pub(super) async fn update_shot(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<UpdateShotPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    // Verify the shot exists
    let exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE id = ?",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    if let Some(ref person_id) = payload.primary_person_id {
        if person_id.is_empty() {
            // Set to unsorted (NULL primary_person_id)
            let max_folder: i64 = db
                .query_row(
                    "SELECT COALESCE(MAX(folder_number), 0) FROM shots WHERE primary_person_id IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            db.execute(
                "UPDATE shots SET primary_person_id = NULL, folder_number = ? WHERE id = ?",
                params![max_folder + 1, id],
            )
            .map_err(|e| {
                tracing::error!("Failed to update shot to unsorted: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        } else {
            // Verify person exists
            let person_exists: bool = db
                .query_row(
                    "SELECT COUNT(*) FROM people WHERE id = ?",
                    params![person_id],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            if !person_exists {
                return Err(StatusCode::BAD_REQUEST);
            }

            // Assign new folder_number for this person (MAX+1)
            let max_folder: i64 = db
                .query_row(
                    "SELECT COALESCE(MAX(folder_number), 0) FROM shots WHERE primary_person_id = ?",
                    params![person_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            db.execute(
                "UPDATE shots SET primary_person_id = ?, folder_number = ? WHERE id = ?",
                params![person_id, max_folder + 1, id],
            )
            .map_err(|e| {
                tracing::error!("Failed to update shot primary_person_id: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }
    }

    if let Some(ref status) = payload.review_status {
        if status != "pending" && status != "confirmed" {
            return Err(StatusCode::BAD_REQUEST);
        }
        db.execute(
            "UPDATE shots SET review_status = ? WHERE id = ?",
            params![status, id],
        )
        .map_err(|e| {
            tracing::error!("Failed to update shot review_status: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// POST /api/shots/:id/split - create new shot from specified files.
/// New shot inherits primary person from faces. Both shots get review_status = 'pending'.
#[derive(Deserialize)]
pub(super) struct SplitShotPayload {
    file_ids: Vec<String>,
}

pub(super) async fn split_shot(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<SplitShotPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.file_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = state.db.lock().await;

    // Verify the source shot exists
    let exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE id = ?",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Verify all file_ids belong to this shot
    for fid in &payload.file_ids {
        let belongs: bool = db
            .query_row(
                "SELECT COUNT(*) FROM files WHERE id = ? AND shot_id = ?",
                params![fid, id],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !belongs {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Verify we're not splitting ALL files (must leave at least one in the source shot)
    let total_files: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM files WHERE shot_id = ?",
            params![id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if payload.file_ids.len() as i64 >= total_files {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Get the source shot's metadata
    #[allow(clippy::type_complexity)]
    let (timestamp, width, height, latitude, longitude): (
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        Option<f64>,
    ) = db
        .query_row(
            "SELECT timestamp, width, height, latitude, longitude FROM shots WHERE id = ?",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|e| {
            tracing::error!("Failed to get source shot metadata: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Create new shot
    let new_shot_id = uuid::Uuid::new_v4().to_string();

    db.execute(
        "INSERT INTO shots (id, timestamp, width, height, latitude, longitude, review_status) VALUES (?, ?, ?, ?, ?, ?, 'pending')",
        params![new_shot_id, timestamp, width, height, latitude, longitude],
    )
    .map_err(|e| {
        tracing::error!("Failed to create new shot for split: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Move the specified files to the new shot
    for fid in &payload.file_ids {
        db.execute(
            "UPDATE files SET shot_id = ? WHERE id = ?",
            params![new_shot_id, fid],
        )
        .map_err(|e| {
            tracing::error!("Failed to move file to new shot: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    // Ensure the new shot has at least one is_original file
    let new_has_original: bool = db
        .query_row(
            "SELECT COUNT(*) FROM files WHERE shot_id = ? AND is_original = 1",
            params![new_shot_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !new_has_original {
        let _ = db.execute(
            "UPDATE files SET is_original = 1 WHERE id = (SELECT id FROM files WHERE shot_id = ? LIMIT 1)",
            params![new_shot_id],
        );
    }

    // Set the new shot's main_file_id to its original file
    if let Ok(new_main_file) = db.query_row::<String, _, _>(
        "SELECT id FROM files WHERE shot_id = ? AND is_original = 1 LIMIT 1",
        params![new_shot_id],
        |row| row.get(0),
    ) {
        let _ = db.execute(
            "UPDATE shots SET main_file_id = ? WHERE id = ?",
            params![new_main_file, new_shot_id],
        );
    }

    // Ensure the source shot still has an original
    let source_has_original: bool = db
        .query_row(
            "SELECT COUNT(*) FROM files WHERE shot_id = ? AND is_original = 1",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !source_has_original {
        let _ = db.execute(
            "UPDATE files SET is_original = 1 WHERE id = (SELECT id FROM files WHERE shot_id = ? LIMIT 1)",
            params![id],
        );
    }

    // Update the source shot's main_file_id
    if let Ok(source_main_file) = db.query_row::<String, _, _>(
        "SELECT id FROM files WHERE shot_id = ? AND is_original = 1 LIMIT 1",
        params![id],
        |row| row.get(0),
    ) {
        let _ = db.execute(
            "UPDATE shots SET main_file_id = ? WHERE id = ?",
            params![source_main_file, id],
        );
    }

    // Determine primary person for the new shot based on its faces
    // (face with the largest bounding box area that has a person_id)
    let new_primary_person: Option<String> = db
        .query_row(
            "SELECT fa.person_id
             FROM faces fa
             JOIN files f ON fa.file_id = f.id
             WHERE f.shot_id = ? AND fa.person_id IS NOT NULL
             ORDER BY (fa.box_x2 - fa.box_x1) * (fa.box_y2 - fa.box_y1) DESC
             LIMIT 1",
            params![new_shot_id],
            |row| row.get(0),
        )
        .ok();

    if let Some(ref ppid) = new_primary_person {
        let max_folder: i64 = db
            .query_row(
                "SELECT COALESCE(MAX(folder_number), 0) FROM shots WHERE primary_person_id = ?",
                params![ppid],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let _ = db.execute(
            "UPDATE shots SET primary_person_id = ?, folder_number = ? WHERE id = ?",
            params![ppid, max_folder + 1, new_shot_id],
        );
    }

    // Set both shots to pending
    let _ = db.execute(
        "UPDATE shots SET review_status = 'pending' WHERE id = ? OR id = ?",
        params![id, new_shot_id],
    );

    Ok(Json(
        serde_json::json!({"status": "ok", "new_shot_id": new_shot_id}),
    ))
}

/// GET /api/shots/:id/similar - find visually similar shots by dHash hamming distance, grouped by person
pub(super) async fn get_similar_shots(
    UState(state): UState,
    Path(id): Path<String>,
) -> Result<Json<Vec<SimilarShotsGrouped>>, StatusCode> {
    let db = state.db.lock().await;

    // Get current shot's primary_person_id and main_file_id
    let (main_file_id, primary_person_id): (Option<String>, Option<String>) = db
        .query_row(
            "SELECT main_file_id, primary_person_id FROM shots WHERE id = ?",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let main_file_id = match main_file_id {
        Some(fid) => fid,
        None => return Ok(Json(vec![])),
    };

    // Load the main file's dHash
    let current_dhash: Option<Vec<u8>> = db
        .query_row(
            "SELECT visual_embedding FROM files WHERE id = ? AND visual_embedding IS NOT NULL",
            params![main_file_id],
            |row| row.get(0),
        )
        .ok();

    let current_dhash = match current_dhash {
        Some(blob) if blob.len() == 8 => {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&blob);
            arr
        }
        _ => return Ok(Json(vec![])),
    };

    // Collect all person IDs from this shot's faces (primary + also_contains)
    let mut person_ids: Vec<(Option<String>, Option<String>)> = Vec::new();

    // Add primary person
    if primary_person_id.is_some() {
        let primary_name: Option<String> = primary_person_id.as_ref().and_then(|pid| {
            db.query_row(
                "SELECT name FROM people WHERE id = ?",
                params![pid],
                |row| row.get(0),
            )
            .ok()
        });
        person_ids.push((primary_person_id.clone(), primary_name));
    }

    // Add secondary people from faces on this shot's files
    {
        let mut stmt = db
            .prepare(
                "SELECT DISTINCT fa.person_id, p.name
                 FROM faces fa
                 JOIN files fi ON fi.id = fa.file_id
                 JOIN people p ON p.id = fa.person_id
                 WHERE fi.shot_id = ?1
                   AND fa.person_id IS NOT NULL
                   AND (?2 IS NULL OR fa.person_id != ?2)",
            )
            .map_err(|e| {
                tracing::error!("Failed to prepare faces query: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        let secondary: Vec<(Option<String>, Option<String>)> = stmt
            .query_map(params![id, primary_person_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| {
                tracing::error!("Failed to query faces: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .filter_map(|r| r.ok())
            .collect();
        person_ids.extend(secondary);
    }

    // If no people at all, query shots with NULL primary_person_id
    if person_ids.is_empty() {
        person_ids.push((None, None));
    }

    // For each person, find similar shots
    let mut groups: Vec<SimilarShotsGrouped> = Vec::new();

    for (person_id, person_name) in &person_ids {
        let mut stmt = db
            .prepare(
                "SELECT s.id, s.main_file_id, s.review_status, p.name,
                        (SELECT COUNT(*) FROM files WHERE shot_id = s.id) as file_count,
                        f.visual_embedding
                 FROM shots s
                 LEFT JOIN people p ON s.primary_person_id = p.id
                 JOIN files f ON f.id = s.main_file_id AND f.visual_embedding IS NOT NULL
                 WHERE s.id != ?1
                   AND (s.primary_person_id = ?2 OR (?2 IS NULL AND s.primary_person_id IS NULL))",
            )
            .map_err(|e| {
                tracing::error!("Failed to prepare similar shots query: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        #[allow(clippy::type_complexity)]
        let candidates: Vec<(
            String,
            String,
            Option<String>,
            Option<String>,
            i64,
            Vec<u8>,
        )> = stmt
            .query_map(params![id, person_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|e| {
                tracing::error!("Failed to query similar shots: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut results: Vec<SimilarShotItem> = candidates
            .into_iter()
            .filter_map(
                |(shot_id, main_fid, review_status, pname, file_count, blob)| {
                    if blob.len() != 8 {
                        return None;
                    }
                    let mut candidate_dhash = [0u8; 8];
                    candidate_dhash.copy_from_slice(&blob);
                    let distance =
                        crate::scanner::hamming_distance(&current_dhash, &candidate_dhash);
                    Some(SimilarShotItem {
                        id: shot_id,
                        thumbnail_url: format!("/api/files/{}/thumbnail", main_fid),
                        file_count,
                        primary_person_name: pname,
                        review_status,
                        distance,
                    })
                },
            )
            .collect();

        results.sort_by_key(|r| r.distance);

        if !results.is_empty() {
            groups.push(SimilarShotsGrouped {
                person_id: person_id.clone(),
                person_name: person_name.clone(),
                shots: results,
            });
        }
    }

    Ok(Json(groups))
}

/// POST /api/shots/merge - move all files from source to target shot. Delete source.
#[derive(Deserialize)]
pub(super) struct MergeShotsPayload {
    source_id: String,
    target_id: String,
    person_id: Option<String>,
}

pub(super) async fn merge_shots(
    UState(state): UState,
    Json(payload): Json<MergeShotsPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.source_id == payload.target_id {
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = state.db.lock().await;

    // Verify both shots exist
    for sid in [&payload.source_id, &payload.target_id] {
        let exists: bool = db
            .query_row(
                "SELECT COUNT(*) FROM shots WHERE id = ?",
                params![sid],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !exists {
            return Err(StatusCode::NOT_FOUND);
        }
    }

    // Move all files from source to target
    // Set is_original = false on moved files (target keeps its original)
    db.execute(
        "UPDATE files SET shot_id = ?, is_original = 0 WHERE shot_id = ?",
        params![payload.target_id, payload.source_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to move files during shot merge: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Delete the source shot
    db.execute("DELETE FROM shots WHERE id = ?", params![payload.source_id])
        .map_err(|e| {
            tracing::error!("Failed to delete source shot during merge: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // If person_id is provided, update the target shot's primary person
    if let Some(ref person_id) = payload.person_id {
        db.execute(
            "UPDATE shots SET primary_person_id = ?, folder_number = NULL WHERE id = ?",
            params![person_id, payload.target_id],
        )
        .map_err(|e| {
            tracing::error!("Failed to update primary_person_id during merge: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// POST /api/shots/batch/confirm - batch set review_status = 'confirmed'
#[derive(Deserialize)]
pub(super) struct BatchConfirmPayload {
    shot_ids: Vec<String>,
}

pub(super) async fn batch_confirm(
    UState(state): UState,
    Json(payload): Json<BatchConfirmPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.shot_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = state.db.lock().await;

    let mut updated_count = 0usize;
    for sid in &payload.shot_ids {
        let updated = db
            .execute(
                "UPDATE shots SET review_status = 'confirmed' WHERE id = ?",
                params![sid],
            )
            .map_err(|e| {
                tracing::error!("Failed to confirm shot {}: {}", sid, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        updated_count += updated;
    }

    Ok(Json(
        serde_json::json!({"status": "ok", "updated": updated_count}),
    ))
}

/// POST /api/shots/batch/reassign - batch set primary_person_id, assign new folder numbers, set confirmed.
#[derive(Deserialize)]
pub(super) struct BatchReassignPayload {
    shot_ids: Vec<String>,
    person_id: String,
}

pub(super) async fn batch_reassign(
    UState(state): UState,
    Json(payload): Json<BatchReassignPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.shot_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = state.db.lock().await;

    // Verify person exists (unless empty = unsorted)
    let person_id_value: Option<&str> = if payload.person_id.is_empty() {
        None
    } else {
        let exists: bool = db
            .query_row(
                "SELECT COUNT(*) FROM people WHERE id = ?",
                params![payload.person_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !exists {
            return Err(StatusCode::BAD_REQUEST);
        }
        Some(payload.person_id.as_str())
    };

    let mut updated_count = 0usize;
    for sid in &payload.shot_ids {
        // Assign new folder_number for this person/unsorted namespace
        let max_folder: i64 = match person_id_value {
            Some(pid) => db
                .query_row(
                    "SELECT COALESCE(MAX(folder_number), 0) FROM shots WHERE primary_person_id = ?",
                    params![pid],
                    |row| row.get(0),
                )
                .unwrap_or(0),
            None => db
                .query_row(
                    "SELECT COALESCE(MAX(folder_number), 0) FROM shots WHERE primary_person_id IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0),
        };

        let updated = match person_id_value {
            Some(pid) => db.execute(
                "UPDATE shots SET primary_person_id = ?, folder_number = ?, review_status = 'confirmed' WHERE id = ?",
                params![pid, max_folder + 1, sid],
            ),
            None => db.execute(
                "UPDATE shots SET primary_person_id = NULL, folder_number = ?, review_status = 'confirmed' WHERE id = ?",
                params![max_folder + 1, sid],
            ),
        }
        .map_err(|e| {
            tracing::error!("Failed to reassign shot {}: {}", sid, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        updated_count += updated;
    }

    Ok(Json(
        serde_json::json!({"status": "ok", "updated": updated_count}),
    ))
}

#[derive(Deserialize)]
pub(super) struct IgnoreMergePayload {
    shot_id_1: String,
    shot_id_2: String,
}

pub(super) async fn ignore_merge(
    UState(state): UState,
    Json(payload): Json<IgnoreMergePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;
    let (s1, s2) = if payload.shot_id_1 < payload.shot_id_2 {
        (&payload.shot_id_1, &payload.shot_id_2)
    } else {
        (&payload.shot_id_2, &payload.shot_id_1)
    };
    db.execute(
        "INSERT OR IGNORE INTO ignored_merges (shot_id_1, shot_id_2) VALUES (?, ?)",
        params![s1, s2],
    )
    .map_err(|e| {
        tracing::error!("Failed to insert ignored_merge: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(serde_json::json!({"status": "ok"})))
}

#[derive(Serialize)]
pub(super) struct SimilarShotGroup {
    primary: SimilarShotItem,
    candidates: Vec<SimilarShotItem>,
}

#[derive(Deserialize)]
pub(super) struct SimilarGroupsQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Serialize)]
pub(super) struct SimilarGroupsResponse {
    groups: Vec<SimilarShotGroup>,
    total: usize,
    offset: usize,
    limit: usize,
}

pub(super) async fn get_similar_shot_groups(
    UState(state): UState,
    Query(query): Query<SimilarGroupsQuery>,
) -> Result<Json<SimilarGroupsResponse>, StatusCode> {
    let db = state.db.lock().await;

    let mut stmt = db
        .prepare(
            "SELECT s.id, s.main_file_id, s.review_status, p.name,
                (SELECT COUNT(*) FROM files WHERE shot_id = s.id) as file_count,
                f.visual_embedding
         FROM shots s
         LEFT JOIN people p ON s.primary_person_id = p.id
         JOIN files f ON f.id = s.main_file_id AND f.visual_embedding IS NOT NULL
         ORDER BY s.timestamp DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    struct ShotData {
        id: String,
        main_fid: String,
        review_status: Option<String>,
        person_name: Option<String>,
        file_count: i64,
        dhash: [u8; 8],
    }

    let candidates: Vec<ShotData> = stmt
        .query_map([], |row| {
            let blob: Vec<u8> = row.get(5)?;
            let mut dhash = [0u8; 8];
            if blob.len() == 8 {
                dhash.copy_from_slice(&blob);
            }
            Ok(ShotData {
                id: row.get(0)?,
                main_fid: row.get(1)?,
                review_status: row.get(2)?,
                person_name: row.get(3)?,
                file_count: row.get(4)?,
                dhash,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter_map(|r| r.ok())
        .collect();

    let mut ignored = std::collections::HashSet::new();
    if let Ok(mut ignore_stmt) = db.prepare("SELECT shot_id_1, shot_id_2 FROM ignored_merges") {
        if let Ok(rows) = ignore_stmt.query_map([], |row| {
            let s1: String = row.get(0)?;
            let s2: String = row.get(1)?;
            Ok((s1, s2))
        }) {
            for pair in rows.flatten() {
                ignored.insert(pair);
            }
        }
    }

    let mut groups: Vec<SimilarShotGroup> = Vec::new();
    let mut used = std::collections::HashSet::new();

    for i in 0..candidates.len() {
        if used.contains(&candidates[i].id) {
            continue;
        }

        let mut current_group = Vec::new();
        for j in (i + 1)..candidates.len() {
            if used.contains(&candidates[j].id) {
                continue;
            }

            let (s1, s2) = if candidates[i].id < candidates[j].id {
                (&candidates[i].id, &candidates[j].id)
            } else {
                (&candidates[j].id, &candidates[i].id)
            };

            if ignored.contains(&(s1.clone(), s2.clone())) {
                continue;
            }

            let dist = crate::scanner::hamming_distance(&candidates[i].dhash, &candidates[j].dhash);
            if dist <= 10 {
                // Threshold for similarity
                current_group.push(j);
            }
        }

        if !current_group.is_empty() {
            let mut candidates_items = Vec::new();
            for idx in current_group {
                used.insert(candidates[idx].id.clone());
                candidates_items.push(SimilarShotItem {
                    id: candidates[idx].id.clone(),
                    thumbnail_url: format!("/api/files/{}/thumbnail", candidates[idx].main_fid),
                    file_count: candidates[idx].file_count,
                    primary_person_name: candidates[idx].person_name.clone(),
                    review_status: candidates[idx].review_status.clone(),
                    distance: crate::scanner::hamming_distance(
                        &candidates[i].dhash,
                        &candidates[idx].dhash,
                    ),
                });
            }

            used.insert(candidates[i].id.clone());

            let primary_item = SimilarShotItem {
                id: candidates[i].id.clone(),
                thumbnail_url: format!("/api/files/{}/thumbnail", candidates[i].main_fid),
                file_count: candidates[i].file_count,
                primary_person_name: candidates[i].person_name.clone(),
                review_status: candidates[i].review_status.clone(),
                distance: 0,
            };

            let mut all_items = vec![primary_item];
            all_items.extend(candidates_items);
            // Sort by file_count descending, so the fattest shot is primary
            all_items.sort_by(|a, b| b.file_count.cmp(&a.file_count));

            let actual_primary = all_items.remove(0);

            groups.push(SimilarShotGroup {
                primary: actual_primary,
                candidates: all_items,
            });
        }
    }

    let total = groups.len();
    let offset = query.offset.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(50).min(200);
    let page = groups.into_iter().skip(offset).take(limit).collect();

    Ok(Json(SimilarGroupsResponse {
        groups: page,
        total,
        offset,
        limit,
    }))
}
