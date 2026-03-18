use axum::{
    extract::{Path, Query},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use image::GenericImageView;
use rusqlite::params;
use serde::Deserialize;
use std::io::Cursor;
use utoipa::IntoParams;

use super::UState;

/// Simple raw-body upload: PUT /api/import/upload?filename=foo.jpg
/// Body is the raw file bytes. No multipart overhead.
#[derive(Deserialize, IntoParams)]
pub(super) struct UploadQuery {
    filename: String,
}

#[utoipa::path(
    put,
    path = "/api/import/upload",
    tag = "import",
    summary = "Upload a file",
    description = "Upload a raw file for import. The file is written to the import staging area and indexed. Supports up to 1 GB files.",
    params(UploadQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Raw file bytes"),
    responses(
        (status = 200, description = "File uploaded and indexed successfully"),
        (status = 400, description = "Invalid filename"),
        (status = 500, description = "Internal server error"),
    )
)]
pub(super) async fn upload_file_raw(
    UState(state): UState,
    Query(query): Query<UploadQuery>,
    body: axum::body::Bytes,
) -> Result<StatusCode, StatusCode> {
    if query.filename.is_empty() || query.filename.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let base_path = state.library_root.join(&query.filename);

    if let Some(parent) = base_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            tracing::error!("Failed to create directory: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    // Deduplicate filename if it already exists on disk
    let target_path = if base_path.exists() {
        let stem = base_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = base_path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let parent = base_path.parent().unwrap();
        let mut i = 1u32;
        loop {
            let candidate = if ext.is_empty() {
                parent.join(format!("{}_{}", stem, i))
            } else {
                parent.join(format!("{}_{}.{}", stem, i, ext))
            };
            if !candidate.exists() {
                break candidate;
            }
            i += 1;
        }
    } else {
        base_path
    };

    tokio::fs::write(&target_path, &body).await.map_err(|e| {
        tracing::error!("Failed to write file {:?}: {}", target_path, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Index the file immediately (blocking -- runs face detection etc.)
    let scanner = state.scanner.clone();
    let target_path_owned = target_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let conn = match scanner.open_db() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to open DB for upload scan: {}", e);
                return;
            }
        };
        let dhash_cache = std::sync::Mutex::new(Vec::<crate::scanner::DHashCacheEntry>::new());
        if let Err(e) = scanner.process_file(&conn, &target_path_owned, &dhash_cache) {
            tracing::error!(
                "Failed to index uploaded file {:?}: {}",
                target_path_owned,
                e
            );
        }
    })
    .await
    .map_err(|e| {
        tracing::error!("Upload scan task failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}

/// POST /api/import/finalize -- run face clustering and reorganization after bulk upload.
#[utoipa::path(
    post,
    path = "/api/import/finalize",
    tag = "import",
    summary = "Finalize import",
    description = "Finalize the import process by running a full library scan and reorganization on newly imported files.",
    responses(
        (status = 200, description = "Import finalized successfully"),
        (status = 500, description = "Internal server error"),
    )
)]
pub(super) async fn finalize_import(
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let scanner = state.scanner.clone();

    let library_root = state.library_root.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        // 1. Run face clustering (drop connection before reorganize)
        tracing::info!("Finalize: running face clustering...");
        {
            let conn = scanner
                .open_db()
                .map_err(|e| format!("Failed to open DB: {}", e))?;
            scanner
                .cluster_faces(&conn)
                .map_err(|e| format!("Face clustering failed: {}", e))?;
        }

        // 2. Reorganize files to match clustering
        tracing::info!("Finalize: reorganizing files...");
        crate::import::run_reorganize(&library_root, false)
            .map_err(|e| format!("Reorganize failed: {}", e))?;

        tracing::info!("Finalize: complete");
        Ok(())
    })
    .await
    .map_err(|e| {
        tracing::error!("Finalize task panicked: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|e| {
        tracing::error!("Finalize failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Serve a file by its database ID
#[utoipa::path(
    get,
    path = "/api/files/{id}",
    tag = "files",
    summary = "Download a file",
    description = "Download the original file content by file ID. Returns the raw bytes with the appropriate content type.",
    params(("id" = String, Path, description = "File ID")),
    responses(
        (status = 200, content_type = "application/octet-stream", description = "File content"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error"),
    )
)]
pub(super) async fn get_file(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<impl IntoResponse, StatusCode> {
    let (file_path, mime_type) = {
        let db = state.db.lock().await;
        let mut stmt = db
            .prepare("SELECT path, mime_type FROM files WHERE id = ?")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        stmt.query_row(params![id], |row| {
            let path: String = row.get(0)?;
            let mime: Option<String> = row.get(1)?;
            Ok((path, mime))
        })
        .map_err(|_| StatusCode::NOT_FOUND)?
    };

    let path = crate::db::resolve_path(&state.library_root, &file_path);
    if !path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let content_type = mime_type.unwrap_or_else(|| {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg".to_string(),
            "png" => "image/png".to_string(),
            "webp" => "image/webp".to_string(),
            "mp4" => "video/mp4".to_string(),
            "mov" => "video/quicktime".to_string(),
            "mkv" => "video/x-matroska".to_string(),
            _ => "application/octet-stream".to_string(),
        }
    });

    Ok(([(header::CONTENT_TYPE, content_type)], bytes))
}

/// Serve a thumbnail for a file by its database ID.
/// For images: resize to ~320px wide JPEG.
/// For videos: extract the first frame, resize to ~320px wide JPEG.
/// Thumbnails are cached in a `.phos_thumbnails` directory next to the DB.
#[derive(Deserialize, IntoParams)]
pub(super) struct ThumbnailQuery {
    w: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/api/files/{id}/thumbnail",
    tag = "files",
    summary = "Get file thumbnail",
    description = "Retrieve a resized thumbnail or video preview for a file. Supports configurable dimensions via query parameters.",
    params(
        ("id" = String, Path, description = "File ID"),
        ThumbnailQuery,
    ),
    responses(
        (status = 200, content_type = "image/jpeg", description = "Thumbnail JPEG image"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error"),
    )
)]
pub(super) async fn get_file_thumbnail(
    Path(id): Path<String>,
    Query(query): Query<ThumbnailQuery>,
    UState(state): UState,
) -> Result<impl IntoResponse, StatusCode> {
    let (file_path, mime_type, db_path) = {
        let db = state.db.lock().await;
        let mut stmt = db
            .prepare("SELECT path, mime_type FROM files WHERE id = ?")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let (path, mime) = stmt
            .query_row(params![id], |row| {
                let p: String = row.get(0)?;
                let m: Option<String> = row.get(1)?;
                Ok((p, m))
            })
            .map_err(|_| StatusCode::NOT_FOUND)?;

        let db_path: String = db
            .query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        (path, mime, db_path)
    };

    let source_path = crate::db::resolve_path(&state.library_root, &file_path);
    if !source_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Determine thumbnail cache directory (next to the DB file)
    let db_dir = std::path::Path::new(&db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let thumb_dir = db_dir.join(".phos_thumbnails");

    // Create thumb dir if it doesn't exist
    tokio::fs::create_dir_all(&thumb_dir)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let target_width = query.w.unwrap_or(320).clamp(64, 1920);
    let cache_suffix = if target_width == 320 {
        format!("{}.jpg", id)
    } else {
        format!("{}_{}.jpg", id, target_width)
    };
    let thumb_path = thumb_dir.join(&cache_suffix);

    // Check if cached thumbnail exists
    if thumb_path.exists() {
        let bytes = tokio::fs::read(&thumb_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(([(header::CONTENT_TYPE, "image/jpeg".to_string())], bytes));
    }

    // Generate thumbnail
    let mime = mime_type.unwrap_or_default();
    let is_video = mime.starts_with("video/");

    let source_path_owned = source_path.clone();
    let thumb_path_clone = thumb_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = if is_video {
            crate::scanner::extract_first_video_frame(&source_path_owned)
                .map_err(|e| format!("Failed to extract video frame: {}", e))?
        } else {
            crate::scanner::open_image(&source_path_owned)
                .map_err(|e| format!("Failed to open image: {}", e))?
        };

        // Resize to target width, maintaining aspect ratio
        let (w, h) = img.dimensions();
        let thumbnail = if w > target_width {
            let target_height = (h as f64 * target_width as f64 / w as f64) as u32;
            img.resize(
                target_width,
                target_height,
                image::imageops::FilterType::Triangle,
            )
        } else {
            img
        };

        // Encode as JPEG
        let mut buf = Cursor::new(Vec::new());
        thumbnail
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .map_err(|e| format!("Failed to encode thumbnail: {}", e))?;

        let jpeg_bytes = buf.into_inner();

        // Cache to disk
        if let Err(e) = std::fs::write(&thumb_path_clone, &jpeg_bytes) {
            tracing::warn!("Failed to cache thumbnail to {:?}: {}", thumb_path_clone, e);
        }

        Ok(jpeg_bytes)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        tracing::error!("Thumbnail generation failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(([(header::CONTENT_TYPE, "image/jpeg".to_string())], result))
}

/// PUT /api/files/:id/set-original - set is_original=true on this file,
/// is_original=false on all other files in the same shot. Update shots.main_file_id.
#[utoipa::path(
    put,
    path = "/api/files/{id}/set-original",
    tag = "files",
    summary = "Set file as original",
    description = "Mark a file as the original (primary) file for its shot, demoting the current original.",
    params(("id" = String, Path, description = "File ID")),
    responses(
        (status = 200, description = "Success"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error"),
    )
)]
pub(super) async fn set_file_original(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    // Get the shot_id for this file
    let shot_id: String = db
        .query_row(
            "SELECT shot_id FROM files WHERE id = ?",
            params![id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Set all files in this shot to is_original = false
    db.execute(
        "UPDATE files SET is_original = 0 WHERE shot_id = ?",
        params![shot_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to clear is_original on shot files: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Set this file as original
    db.execute("UPDATE files SET is_original = 1 WHERE id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to set is_original on file: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Update shots.main_file_id
    db.execute(
        "UPDATE shots SET main_file_id = ? WHERE id = ?",
        params![id, shot_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to update shots.main_file_id: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// DELETE /api/files/:id - delete a non-original file from a shot.
/// Removes the file from disk, cleans up faces/keyframes, and updates the shot.
#[utoipa::path(
    delete,
    path = "/api/files/{id}",
    tag = "files",
    summary = "Delete a file",
    description = "Delete a non-original file from a shot. The original file cannot be deleted directly — delete the shot instead.",
    params(("id" = String, Path, description = "File ID")),
    responses(
        (status = 200, description = "Success"),
        (status = 400, description = "Cannot delete original file"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error"),
    )
)]
pub(super) async fn delete_file(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    // Get file info
    let (shot_id, file_path, is_original): (String, String, bool) = db
        .query_row(
            "SELECT shot_id, path, is_original FROM files WHERE id = ?",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Don't allow deleting the original file
    if is_original {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Collect person_ids that might become orphaned
    let affected_person_ids: Vec<String> = db
        .prepare("SELECT DISTINCT person_id FROM faces WHERE file_id = ? AND person_id IS NOT NULL")
        .and_then(|mut s| {
            s.query_map(params![id], |row| row.get(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default();

    // Delete faces
    db.execute("DELETE FROM faces WHERE file_id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete faces: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete video keyframes
    db.execute(
        "DELETE FROM video_keyframes WHERE video_file_id = ?",
        params![id],
    )
    .map_err(|e| {
        tracing::error!("Failed to delete video_keyframes: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Delete the file record
    db.execute("DELETE FROM files WHERE id = ?", params![id])
        .map_err(|e| {
            tracing::error!("Failed to delete file: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete physical file from disk (best-effort)
    let resolved_path = crate::db::resolve_path(&state.library_root, &file_path);
    if let Err(e) = std::fs::remove_file(&resolved_path) {
        tracing::warn!("Failed to delete file from disk {:?}: {}", resolved_path, e);
    }

    // Delete cached thumbnail and clean up empty directories (best-effort)
    let db_path: String = db
        .query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
        .unwrap_or_default();
    if !db_path.is_empty() {
        let db_dir = std::path::Path::new(&db_path)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let _ = std::fs::remove_file(db_dir.join(".phos_thumbnails").join(format!("{}.jpg", id)));
        let _ = crate::import::cleanup_empty_dirs(db_dir);
    }

    // Recalculate primary person for the shot
    let _ = super::recalculate_primary_person(&db, &shot_id);

    // Clean up orphaned people
    for person_id in &affected_person_ids {
        let _ = super::cleanup_orphaned_person(&db, person_id);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}
