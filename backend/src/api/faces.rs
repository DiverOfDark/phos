use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use diesel::prelude::*;
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use utoipa::ToSchema;

use super::UState;
use crate::models::NewFace;
use crate::schema::{faces, files, people};

/// Find the shot_id for a given face
fn get_shot_id_for_face(
    conn: &mut diesel::SqliteConnection,
    face_id: &str,
) -> Result<String, StatusCode> {
    faces::table
        .inner_join(files::table.on(faces::file_id.eq(files::id)))
        .filter(faces::id.eq(face_id))
        .select(files::shot_id)
        .first::<String>(conn)
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
#[derive(Serialize, ToSchema)]
pub(super) struct FaceSuggestion {
    person_id: String,
    person_name: Option<String>,
    thumbnail_url: Option<String>,
    distance: f32,
}

#[utoipa::path(
    get,
    path = "/api/faces/{id}/suggestions",
    tag = "faces",
    summary = "Get face suggestions",
    description = "Get person assignment suggestions for a face based on embedding similarity to existing people.",
    params(
        ("id" = String, Path, description = "Face ID")
    ),
    responses(
        (status = 200, body = Vec<FaceSuggestion>, description = "List of person suggestions sorted by similarity"),
        (status = 404, description = "Face not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_face_suggestions(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<Vec<FaceSuggestion>>, StatusCode> {
    use crate::ai::cosine_similarity;

    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Load the target face's embedding
    let target_blob: Option<Vec<u8>> = faces::table
        .filter(faces::id.eq(&id))
        .select(faces::embedding)
        .first::<Option<Vec<u8>>>(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let target_blob = target_blob.ok_or(StatusCode::NOT_FOUND)?;

    let target_embedding: Vec<f32> =
        crate::embedding::decode_embedding(&target_blob).ok_or_else(|| {
            tracing::error!("Failed to deserialize target face embedding");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if target_embedding.is_empty() {
        return Ok(Json(vec![]));
    }

    // Load one representative face embedding per person (the thumbnail face)
    let rows: Vec<(String, Option<String>, Option<String>, Option<Vec<u8>>)> = people::table
        .inner_join(faces::table.on(faces::id.nullable().eq(people::thumbnail_face_id)))
        .filter(faces::embedding.is_not_null())
        .select((
            people::id,
            people::name,
            people::thumbnail_face_id,
            faces::embedding,
        ))
        .load::<(String, Option<String>, Option<String>, Option<Vec<u8>>)>(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query person embeddings: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut suggestions: Vec<FaceSuggestion> = rows
        .into_iter()
        .filter_map(|(person_id, person_name, thumbnail_face_id, blob)| {
            let blob = blob?;
            let embedding: Vec<f32> = crate::embedding::decode_embedding(&blob)?;
            if embedding.len() != target_embedding.len() || embedding.is_empty() {
                return None;
            }
            let sim = cosine_similarity(&target_embedding, &embedding);
            let distance = 1.0 - sim;
            Some(FaceSuggestion {
                person_id,
                person_name,
                thumbnail_url: thumbnail_face_id
                    .map(|fid| format!("/api/faces/{}/thumbnail", fid)),
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
#[derive(Deserialize, ToSchema)]
pub(super) struct ReassignFacePayload {
    person_id: String,
}

#[utoipa::path(
    put,
    path = "/api/faces/{id}/person",
    tag = "faces",
    summary = "Reassign a face",
    description = "Reassign a face to a different person. Updates the shot's primary person if needed.",
    params(
        ("id" = String, Path, description = "Face ID to reassign")
    ),
    request_body = ReassignFacePayload,
    responses(
        (status = 200, description = "Face reassigned successfully"),
        (status = 404, description = "Face or target person not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn reassign_face(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<ReassignFacePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify the target person exists
    let person_exists: i64 = people::table
        .filter(people::id.eq(&payload.person_id))
        .count()
        .get_result(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if person_exists == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    // Find the shot_id and old person_id for this face before updating
    let shot_id = get_shot_id_for_face(&mut conn, &id)?;
    let old_person_id: Option<String> = faces::table
        .filter(faces::id.eq(&id))
        .select(faces::person_id)
        .first::<Option<String>>(&mut conn)
        .ok()
        .flatten();

    let updated = diesel::update(faces::table.filter(faces::id.eq(&id)))
        .set(faces::person_id.eq(&payload.person_id))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to reassign face: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if updated == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    // Set thumbnail_face_id if the person doesn't have one yet
    let _ = diesel::update(
        people::table
            .filter(people::id.eq(&payload.person_id))
            .filter(people::thumbnail_face_id.is_null()),
    )
    .set(people::thumbnail_face_id.eq(&id))
    .execute(&mut conn);

    // Recalculate shot's primary_person_id (unless confirmed)
    super::recalculate_primary_person(&mut conn, &shot_id)?;

    // If the old person has no remaining faces, delete them
    if let Some(pid) = &old_person_id {
        if pid != &payload.person_id {
            super::cleanup_orphaned_person(&mut conn, pid)?;
        }
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Delete a face detection (remove false positive / irrelevant face)
#[utoipa::path(
    delete,
    path = "/api/faces/{id}",
    tag = "faces",
    summary = "Delete a face",
    description = "Delete a face detection record. Cleans up orphaned people and recalculates shot assignments.",
    params(
        ("id" = String, Path, description = "Face ID to delete")
    ),
    responses(
        (status = 200, description = "Face deleted successfully"),
        (status = 404, description = "Face not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn delete_face(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Find the shot_id and person_id for this face before deleting
    let shot_id = get_shot_id_for_face(&mut conn, &id)?;
    let old_person_id: Option<String> = faces::table
        .filter(faces::id.eq(&id))
        .select(faces::person_id)
        .first::<Option<String>>(&mut conn)
        .ok()
        .flatten();

    // Delete the face record
    let deleted = diesel::delete(faces::table.filter(faces::id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete face {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    // Recalculate shot's primary_person_id from remaining faces
    super::recalculate_primary_person(&mut conn, &shot_id)?;

    // If the person has no remaining faces, delete them
    if let Some(pid) = &old_person_id {
        super::cleanup_orphaned_person(&mut conn, pid)?;
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Serve a face thumbnail: crop face from source image, resize, cache as JPEG
#[utoipa::path(
    get,
    path = "/api/faces/{id}/thumbnail",
    tag = "faces",
    summary = "Get face thumbnail",
    description = "Retrieve the cropped JPEG thumbnail image for a detected face.",
    params(
        ("id" = String, Path, description = "Face ID")
    ),
    responses(
        (status = 200, content_type = "image/jpeg", description = "JPEG face thumbnail"),
        (status = 404, description = "Face not found or source image missing"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_face_thumbnail(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<impl IntoResponse, StatusCode> {
    let (file_path, box_x1, box_y1, box_x2, box_y2) = {
        let mut conn = state
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        faces::table
            .inner_join(files::table.on(faces::file_id.eq(files::id)))
            .filter(faces::id.eq(&id))
            .select((
                files::path,
                faces::box_x1,
                faces::box_y1,
                faces::box_x2,
                faces::box_y2,
            ))
            .first::<(String, Option<f32>, Option<f32>, Option<f32>, Option<f32>)>(&mut conn)
            .map_err(|_| StatusCode::NOT_FOUND)?
    };

    let box_x1 = box_x1.unwrap_or(0.0);
    let box_y1 = box_y1.unwrap_or(0.0);
    let box_x2 = box_x2.unwrap_or(0.0);
    let box_y2 = box_y2.unwrap_or(0.0);

    let source_path = crate::db::resolve_path(&state.library_root, &file_path);
    if !source_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Cache directory — use library_root (parent of .phos.db)
    let thumb_dir = state.library_root.join(".phos_thumbnails").join("faces");

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
    let source_path_owned = source_path.clone();
    let thumb_path_clone = thumb_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = crate::scanner::open_image(&source_path_owned)
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
#[derive(Deserialize, ToSchema)]
pub(super) struct AddManualFacePayload {
    box_x1: f32,
    box_y1: f32,
    box_x2: f32,
    box_y2: f32,
}

#[utoipa::path(
    post,
    path = "/api/files/{id}/faces",
    tag = "faces",
    summary = "Add manual face",
    description = "Manually add a face region to a file with a specified bounding box and person assignment.",
    params(
        ("id" = String, Path, description = "File ID to add a manual face to")
    ),
    request_body = AddManualFacePayload,
    responses(
        (status = 200, description = "Face added successfully"),
        (status = 400, description = "Invalid bounding box coordinates"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error")
    )
)]
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
        let mut conn = state
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        files::table
            .filter(files::id.eq(&file_id))
            .select((files::path, files::shot_id))
            .first::<(String, String)>(&mut conn)
            .map_err(|_| StatusCode::NOT_FOUND)?
    };

    let bbox = (
        payload.box_x1,
        payload.box_y1,
        payload.box_x2,
        payload.box_y2,
    );
    let scanner = state.scanner.clone();
    let resolved_path = crate::db::resolve_path(&state.library_root, &file_path);

    // Compute embedding on a blocking thread
    let (face_id, embedding_blob) =
        tokio::task::spawn_blocking(move || -> Result<(String, Vec<u8>), String> {
            let img = crate::scanner::open_image(&resolved_path)
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

            let blob = crate::embedding::encode_embedding(&embedding);

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
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    diesel::insert_into(faces::table)
        .values(NewFace {
            id: &face_id,
            file_id: &file_id,
            person_id: None,
            box_x1: Some(payload.box_x1),
            box_y1: Some(payload.box_y1),
            box_x2: Some(payload.box_x2),
            box_y2: Some(payload.box_y2),
            embedding: Some(&embedding_blob),
            score: Some(1.0),
        })
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to insert manual face: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    super::recalculate_primary_person(&mut conn, &shot_id)?;

    Ok(Json(serde_json::json!({"id": face_id})))
}
