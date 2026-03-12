use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use image::GenericImageView;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

use super::UState;

/// Find the shot_id for a given face
fn get_shot_id_for_face(db: &rusqlite::Connection, face_id: &str) -> Result<String, StatusCode> {
    db.query_row(
        "SELECT f.shot_id FROM faces fa JOIN files f ON fa.file_id = f.id WHERE fa.id = ?",
        params![face_id],
        |row| row.get(0),
    )
    .map_err(|e| {
        tracing::error!("Failed to find shot for face {}: {}", face_id, e);
        StatusCode::NOT_FOUND
    })
}

/// Get suggested persons for a face based on embedding similarity.
///
/// Computes cosine similarity on-the-fly between the target face and the
/// thumbnail face of every person, so results are always up-to-date even
/// for newly created persons.
#[derive(Serialize)]
pub(super) struct FaceSuggestion {
    person_id: String,
    person_name: Option<String>,
    thumbnail_url: Option<String>,
    distance: f32,
}

pub(super) async fn get_face_suggestions(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<Vec<FaceSuggestion>>, StatusCode> {
    use crate::ai::cosine_similarity;

    let db = state.db.lock().await;

    // Load the target face's embedding
    let target_blob: Vec<u8> = db
        .query_row(
            "SELECT embedding FROM faces WHERE id = ?",
            params![id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let target_embedding: Vec<f32> = bincode::deserialize(&target_blob).map_err(|e| {
        tracing::error!("Failed to deserialize target face embedding: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if target_embedding.is_empty() {
        return Ok(Json(vec![]));
    }

    // Load one representative face embedding per person (the thumbnail face)
    let mut stmt = db
        .prepare(
            "SELECT p.id, p.name, p.thumbnail_face_id, f.embedding
             FROM people p
             JOIN faces f ON f.id = p.thumbnail_face_id
             WHERE f.embedding IS NOT NULL",
        )
        .map_err(|e| {
            tracing::error!("Failed to prepare person embeddings query: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut suggestions: Vec<FaceSuggestion> = stmt
        .query_map([], |row| {
            let person_id: String = row.get(0)?;
            let person_name: Option<String> = row.get(1)?;
            let thumbnail_face_id: Option<String> = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            Ok((person_id, person_name, thumbnail_face_id, blob))
        })
        .map_err(|e| {
            tracing::error!("Failed to query person embeddings: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
        .filter_map(|(person_id, person_name, thumbnail_face_id, blob)| {
            let embedding: Vec<f32> = bincode::deserialize(&blob).ok()?;
            if embedding.len() != target_embedding.len() || embedding.is_empty() {
                return None;
            }
            let sim = cosine_similarity(&target_embedding, &embedding);
            let distance = 1.0 - sim;
            Some(FaceSuggestion {
                person_id,
                person_name,
                thumbnail_url: thumbnail_face_id.map(|fid| format!("/api/faces/{}/thumbnail", fid)),
                distance,
            })
        })
        .collect();

    suggestions.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    suggestions.truncate(10);

    Ok(Json(suggestions))
}

/// Reassign a face to a different person
#[derive(Deserialize)]
pub(super) struct ReassignFacePayload {
    person_id: String,
}

pub(super) async fn reassign_face(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<ReassignFacePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    // Verify the target person exists
    let person_exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM people WHERE id = ?",
            params![payload.person_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !person_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Find the shot_id and old person_id for this face before updating
    let shot_id = get_shot_id_for_face(&db, &id)?;
    let old_person_id: Option<String> = db
        .query_row(
            "SELECT person_id FROM faces WHERE id = ?",
            params![id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    let updated = db
        .execute(
            "UPDATE faces SET person_id = ? WHERE id = ?",
            params![payload.person_id, id],
        )
        .map_err(|e| {
            tracing::error!("Failed to reassign face: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if updated == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    // Set thumbnail_face_id if the person doesn't have one yet
    let _ = db.execute(
        "UPDATE people SET thumbnail_face_id = ? WHERE id = ? AND thumbnail_face_id IS NULL",
        params![id, payload.person_id],
    );

    // Recalculate shot's primary_person_id (unless confirmed)
    super::recalculate_primary_person(&db, &shot_id)?;

    // If the old person has no remaining faces, delete them
    if let Some(pid) = &old_person_id {
        if pid != &payload.person_id {
            super::cleanup_orphaned_person(&db, pid)?;
        }
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Delete a face detection (remove false positive / irrelevant face)
pub(super) async fn delete_face(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    // Find the shot_id and person_id for this face before deleting
    let shot_id = get_shot_id_for_face(&db, &id)?;
    let old_person_id: Option<String> = db
        .query_row(
            "SELECT person_id FROM faces WHERE id = ?",
            params![id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    // Delete face_neighbors entries where this face appears
    db.execute(
        "DELETE FROM face_neighbors WHERE face_id_a = ? OR face_id_b = ?",
        params![id, id],
    )
    .map_err(|e| {
        tracing::error!("Failed to delete face_neighbors for face {}: {}", id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Delete the face record
    let deleted = db
        .execute("DELETE FROM faces WHERE id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete face {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    // Recalculate shot's primary_person_id from remaining faces
    super::recalculate_primary_person(&db, &shot_id)?;

    // If the person has no remaining faces, delete them
    if let Some(pid) = &old_person_id {
        super::cleanup_orphaned_person(&db, pid)?;
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Serve a face thumbnail: crop face from source image, resize, cache as JPEG
pub(super) async fn get_face_thumbnail(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<impl IntoResponse, StatusCode> {
    let (file_path, box_x1, box_y1, box_x2, box_y2, db_path) = {
        let db = state.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT fi.path, fa.box_x1, fa.box_y1, fa.box_x2, fa.box_y2
                 FROM faces fa
                 JOIN files fi ON fa.file_id = fi.id
                 WHERE fa.id = ?",
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let result = stmt
            .query_row(params![id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f32>(1)?,
                    row.get::<_, f32>(2)?,
                    row.get::<_, f32>(3)?,
                    row.get::<_, f32>(4)?,
                ))
            })
            .map_err(|_| StatusCode::NOT_FOUND)?;

        let db_path: String = db
            .query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        (result.0, result.1, result.2, result.3, result.4, db_path)
    };

    let source_path = std::path::Path::new(&file_path);
    if !source_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Cache directory
    let db_dir = std::path::Path::new(&db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let thumb_dir = db_dir.join(".phos_thumbnails").join("faces");

    tokio::fs::create_dir_all(&thumb_dir)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let thumb_path = thumb_dir.join(format!("{}.jpg", id));

    // Return cached thumbnail if it exists
    if thumb_path.exists() {
        let bytes = tokio::fs::read(&thumb_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(([(header::CONTENT_TYPE, "image/jpeg".to_string())], bytes));
    }

    // Generate face thumbnail
    let file_path_owned = file_path.clone();
    let thumb_path_clone = thumb_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = crate::scanner::open_image(std::path::Path::new(&file_path_owned))
            .map_err(|e| format!("Failed to open image: {}", e))?;

        let (img_w, img_h) = img.dimensions();

        // Box coords are in pixels; add some padding around the face
        let pad = 0.15;
        let face_w = box_x2 - box_x1;
        let face_h = box_y2 - box_y1;
        let cx1 = (box_x1 - face_w * pad).max(0.0) as u32;
        let cy1 = (box_y1 - face_h * pad).max(0.0) as u32;
        let cx2 = ((box_x2 + face_w * pad) as u32).min(img_w);
        let cy2 = ((box_y2 + face_h * pad) as u32).min(img_h);

        let crop_w = cx2.saturating_sub(cx1).max(1);
        let crop_h = cy2.saturating_sub(cy1).max(1);

        let cropped = img.crop_imm(cx1, cy1, crop_w, crop_h);

        // Resize to 150px wide
        let target_width = 150u32;
        let thumbnail = if crop_w > target_width {
            let target_height = (crop_h as f64 * target_width as f64 / crop_w as f64) as u32;
            cropped.resize(
                target_width,
                target_height,
                image::imageops::FilterType::Triangle,
            )
        } else {
            cropped
        };

        let mut buf = Cursor::new(Vec::new());
        thumbnail
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .map_err(|e| format!("Failed to encode face thumbnail: {}", e))?;

        let jpeg_bytes = buf.into_inner();

        if let Err(e) = std::fs::write(&thumb_path_clone, &jpeg_bytes) {
            tracing::warn!(
                "Failed to cache face thumbnail to {:?}: {}",
                thumb_path_clone,
                e
            );
        }

        Ok(jpeg_bytes)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        tracing::error!("Face thumbnail generation failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(([(header::CONTENT_TYPE, "image/jpeg".to_string())], result))
}

/// POST /api/files/:id/faces - manually add a face bounding box.
/// The server computes the embedding from the given bbox coordinates.
#[derive(Deserialize)]
pub(super) struct AddManualFacePayload {
    box_x1: f32,
    box_y1: f32,
    box_x2: f32,
    box_y2: f32,
}

pub(super) async fn add_manual_face(
    Path(file_id): Path<String>,
    UState(state): UState,
    Json(payload): Json<AddManualFacePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Validate bbox
    if payload.box_x1 >= payload.box_x2 || payload.box_y1 >= payload.box_y2 {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Look up the file path and shot_id
    let (file_path, shot_id) = {
        let db = state.db.lock().await;
        db.query_row(
            "SELECT path, shot_id FROM files WHERE id = ?",
            params![file_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?
    };

    let bbox = (
        payload.box_x1,
        payload.box_y1,
        payload.box_x2,
        payload.box_y2,
    );
    let scanner = state.scanner.clone();
    let file_path_owned = file_path.clone();

    // Compute embedding on a blocking thread
    let (face_id, embedding_blob) =
        tokio::task::spawn_blocking(move || -> Result<(String, Vec<u8>), String> {
            let img = crate::scanner::open_image(std::path::Path::new(&file_path_owned))
                .map_err(|e| format!("Failed to open image: {}", e))?;

            let embedding = if let Some(ai) = scanner.ai() {
                ai.extract_embedding(&img, None, bbox)
                    .map_err(|e| format!("Failed to extract embedding: {}", e))?
            } else {
                // Dummy mode
                let mut emb = vec![0.1f32; 512];
                emb[0] = bbox.0 / 1000.0;
                emb[1] = bbox.1 / 1000.0;
                emb
            };

            let blob = bincode::serialize(&embedding)
                .map_err(|e| format!("Failed to serialize embedding: {}", e))?;

            let id = uuid::Uuid::new_v4().to_string();
            Ok((id, blob))
        })
        .await
        .map_err(|e| {
            tracing::error!("Manual face task panicked: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|e| {
            tracing::error!("Manual face extraction failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Insert the face and recalculate primary person
    let db = state.db.lock().await;

    db.execute(
        "INSERT INTO faces (id, file_id, person_id, box_x1, box_y1, box_x2, box_y2, embedding, score) VALUES (?, ?, NULL, ?, ?, ?, ?, ?, 1.0)",
        params![face_id, file_id, payload.box_x1, payload.box_y1, payload.box_x2, payload.box_y2, embedding_blob],
    )
    .map_err(|e| {
        tracing::error!("Failed to insert manual face: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    super::recalculate_primary_person(&db, &shot_id)?;

    Ok(Json(serde_json::json!({"id": face_id})))
}
