use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use image::GenericImageView;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/api/photos", get(get_photos))
        .route("/api/photos/:id", get(get_photo_detail))
        .route("/api/people", get(get_people))
        .route("/api/people/merge", post(merge_people))
        .route("/api/people/:id", get(get_person_photos).put(rename_person))
        .route("/api/people/:id/faces", get(get_person_faces))
        .route("/api/faces/:id/thumbnail", get(get_face_thumbnail))
        .route("/api/files/:id", get(get_file))
        .route("/api/files/:id/thumbnail", get(get_file_thumbnail))
        .route("/api/stats", get(get_stats))
        .route("/api/scan", post(trigger_scan))
        .route("/api/import/upload", post(upload_file))
        .with_state(state)
}

async fn upload_file(
    State(_state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> Result<StatusCode, StatusCode> {
    let mut file_path: Option<String> = None;
    let mut file_content: Option<axum::body::Bytes> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::error!("Failed to get next field: {}", e);
        StatusCode::BAD_REQUEST
    })? {
        let name = field.name().unwrap_or_default().to_string();
        if name == "path" {
            file_path = Some(field.text().await.map_err(|e| {
                tracing::error!("Failed to get field text: {}", e);
                StatusCode::BAD_REQUEST
            })?);
        } else if name == "file" {
            file_content = Some(field.bytes().await.map_err(|e| {
                tracing::error!("Failed to get field bytes: {}", e);
                StatusCode::BAD_REQUEST
            })?);
        }
    }

    if let (Some(rel_path), Some(content)) = (file_path, file_content) {
        let library_path = std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string());
        let target_path = std::path::Path::new(&library_path).join(&rel_path);

        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }

        tokio::fs::write(target_path, content).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

#[derive(Serialize)]
struct PhotoBrief {
    id: String,
    thumbnail_url: String,
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct PhotosQuery {
    q: Option<String>,
    person_id: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

async fn get_photos(
    State(state): State<AppState>,
    Query(params): Query<PhotosQuery>,
) -> Json<Vec<PhotoBrief>> {
    let db = state.db.lock().await;

    let mut sql = String::from(
        "SELECT DISTINCT p.id, f.path, p.timestamp FROM photos p JOIN files f ON p.main_file_id = f.id",
    );
    let mut conditions: Vec<String> = Vec::new();
    let mut bind_values: Vec<String> = Vec::new();

    if let Some(ref person_id) = params.person_id {
        sql.push_str(" JOIN faces fa ON f.id = fa.file_id");
        conditions.push("fa.person_id = ?".to_string());
        bind_values.push(person_id.clone());
    }

    if let Some(ref q) = params.q {
        conditions.push("f.path LIKE ?".to_string());
        bind_values.push(format!("%{}%", q));
    }

    if let Some(ref from) = params.from {
        conditions.push("p.timestamp >= ?".to_string());
        bind_values.push(from.clone());
    }

    if let Some(ref to) = params.to {
        conditions.push("p.timestamp <= ?".to_string());
        bind_values.push(to.clone());
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(" ORDER BY p.timestamp DESC");

    let mut stmt = match db.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to prepare photos query: {}", e);
            return Json(Vec::new());
        }
    };

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = bind_values
        .iter()
        .map(|v| v as &dyn rusqlite::types::ToSql)
        .collect();

    let rows = match stmt.query_map(param_refs.as_slice(), |row| {
        Ok(PhotoBrief {
            id: row.get(0)?,
            thumbnail_url: format!("/api/files/{}", row.get::<_, String>(0)?),
            timestamp: row.get(2)?,
        })
    }) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to query photos: {}", e);
            return Json(Vec::new());
        }
    };

    let photos: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(photos)
}

#[derive(Serialize)]
struct PhotoDetail {
    id: String,
    files: Vec<FileDetail>,
    faces: Vec<FaceDetail>,
}

#[derive(Serialize)]
struct FileDetail {
    id: String,
    path: String,
    mime_type: String,
}

#[derive(Serialize)]
struct FaceDetail {
    id: String,
    person_id: Option<String>,
    box_x1: f32,
    box_y1: f32,
    box_x2: f32,
    box_y2: f32,
}

async fn get_photo_detail(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Json<PhotoDetail> {
    let db = state.db.lock().await;

    // Get files for this photo
    let mut stmt = db
        .prepare("SELECT id, path, mime_type FROM files WHERE photo_id = ?")
        .unwrap();
    let files = stmt
        .query_map(params![id], |row| {
            Ok(FileDetail {
                id: row.get(0)?,
                path: row.get(1)?,
                mime_type: row.get(2)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // Get faces for these files
    let mut stmt = db.prepare("SELECT fa.id, fa.person_id, fa.box_x1, fa.box_y1, fa.box_x2, fa.box_y2 FROM faces fa JOIN files f ON fa.file_id = f.id WHERE f.photo_id = ?").unwrap();
    let faces = stmt
        .query_map(params![id], |row| {
            Ok(FaceDetail {
                id: row.get(0)?,
                person_id: row.get(1)?,
                box_x1: row.get(2)?,
                box_y1: row.get(3)?,
                box_x2: row.get(4)?,
                box_y2: row.get(5)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    Json(PhotoDetail { id, files, faces })
}

async fn get_people(State(state): State<AppState>) -> Json<Vec<PersonBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare(
            "SELECT p.id, p.name, COUNT(f.id) as face_count, p.thumbnail_face_id
             FROM people p
             LEFT JOIN faces f ON f.person_id = p.id
             GROUP BY p.id
             ORDER BY face_count DESC",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            let thumbnail_face_id: Option<String> = row.get(3)?;
            Ok(PersonBrief {
                id: row.get(0)?,
                name: row.get(1)?,
                face_count: row.get(2)?,
                thumbnail_url: thumbnail_face_id
                    .map(|fid| format!("/api/faces/{}/thumbnail", fid)),
            })
        })
        .unwrap();

    let people: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(people)
}

#[derive(Serialize)]
struct PersonBrief {
    id: String,
    name: Option<String>,
    face_count: i64,
    thumbnail_url: Option<String>,
}

async fn get_person_photos(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Json<Vec<PhotoBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare(
            "
        SELECT DISTINCT p.id, f.path, p.timestamp
        FROM photos p
        JOIN files f ON p.main_file_id = f.id
        JOIN faces fa ON f.id = fa.file_id
        WHERE fa.person_id = ?
        ORDER BY p.timestamp DESC
    ",
        )
        .unwrap();

    let rows = stmt
        .query_map(params![id], |row| {
            Ok(PhotoBrief {
                id: row.get(0)?,
                thumbnail_url: format!("/{}", row.get::<_, String>(1)?),
                timestamp: row.get(2)?,
            })
        })
        .unwrap();

    let photos: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(photos)
}

/// Get up to 12 face IDs with thumbnail URLs for a person
#[derive(Serialize)]
struct PersonFaceBrief {
    id: String,
    thumbnail_url: String,
}

async fn get_person_faces(
    Path(id): Path<String>,
    State(state): State<AppState>,
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
struct RenamePersonPayload {
    name: String,
}

async fn rename_person(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<RenamePersonPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;
    let updated = db
        .execute("UPDATE people SET name = ? WHERE id = ?", params![payload.name, id])
        .map_err(|e| {
            tracing::error!("Failed to rename person: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if updated == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Merge two people: move all faces from source to target, then delete source
#[derive(Deserialize)]
struct MergePeoplePayload {
    source_id: String,
    target_id: String,
}

async fn merge_people(
    State(state): State<AppState>,
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

    db.execute("DELETE FROM people WHERE id = ?", params![payload.source_id])
        .map_err(|e| {
            tracing::error!("Failed to delete merged person: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Serve a face thumbnail: crop face from source image, resize, cache as JPEG
async fn get_face_thumbnail(
    Path(id): Path<String>,
    State(state): State<AppState>,
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
        let img =
            image::open(&file_path_owned).map_err(|e| format!("Failed to open image: {}", e))?;

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

/// Serve a file by its database ID
async fn get_file(
    Path(id): Path<String>,
    State(state): State<AppState>,
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

    let path = std::path::Path::new(&file_path);
    if !path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let bytes = tokio::fs::read(path)
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
async fn get_file_thumbnail(
    Path(id): Path<String>,
    State(state): State<AppState>,
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

    let source_path = std::path::Path::new(&file_path);
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

    let thumb_path = thumb_dir.join(format!("{}.jpg", id));

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

    let source_path_owned = file_path.clone();
    let thumb_path_clone = thumb_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = if is_video {
            crate::scanner::extract_first_video_frame(std::path::Path::new(&source_path_owned))
                .map_err(|e| format!("Failed to extract video frame: {}", e))?
        } else {
            image::open(&source_path_owned).map_err(|e| format!("Failed to open image: {}", e))?
        };

        // Resize to ~320px wide, maintaining aspect ratio
        let (w, h) = img.dimensions();
        let target_width = 320u32;
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

/// Return aggregate stats about the library
#[derive(Serialize)]
struct StatsResponse {
    total_photos: i64,
    total_people: i64,
    total_files: i64,
}

async fn get_stats(State(state): State<AppState>) -> Json<StatsResponse> {
    let db = state.db.lock().await;

    let total_photos: i64 = db
        .query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))
        .unwrap_or(0);
    let total_people: i64 = db
        .query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
        .unwrap_or(0);
    let total_files: i64 = db
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap_or(0);

    Json(StatsResponse {
        total_photos,
        total_people,
        total_files,
    })
}

#[derive(Deserialize)]
pub struct ScanParams {
    pub path: String,
}

async fn trigger_scan(
    State(state): State<AppState>,
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
