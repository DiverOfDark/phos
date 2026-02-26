use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
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
    pub scanner: Arc<crate::scanner::Scanner>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Shot CRUD
        .route("/api/shots", get(get_shots))
        .route(
            "/api/shots/:id",
            get(get_shot_detail).put(update_shot).delete(delete_shot),
        )
        // Shot operations
        .route("/api/shots/:id/similar", get(get_similar_shots))
        .route("/api/shots/:id/split", post(split_shot))
        .route("/api/shots/merge", post(merge_shots))
        .route("/api/shots/batch/confirm", post(batch_confirm))
        .route("/api/shots/batch/reassign", post(batch_reassign))
        // People
        .route("/api/people", get(get_people).post(create_person))
        .route("/api/people/merge", post(merge_people))
        .route("/api/people/:id", get(get_person_shots).put(rename_person).delete(delete_person))
        .route("/api/people/:id/faces", get(get_person_faces))
        // Faces
        .route("/api/faces/:id/thumbnail", get(get_face_thumbnail))
        .route("/api/faces/:id/person", put(reassign_face))
        .route("/api/faces/:id/suggestions", get(get_face_suggestions))
        .route("/api/faces/:id", delete(delete_face))
        // Files
        .route("/api/files/:id", get(get_file))
        .route("/api/files/:id/thumbnail", get(get_file_thumbnail))
        .route("/api/files/:id/set-original", put(set_file_original))
        .route("/api/files/:id/faces", post(add_manual_face))
        // Stats + organize
        .route("/api/stats", get(get_stats))
        .route("/api/organize/stats", get(get_organize_stats))
        .route("/api/reorganize", post(trigger_reorganize))
        // Scan + import
        .route("/api/scan", post(trigger_scan))
        .route(
            "/api/import/upload",
            put(upload_file_raw).layer(DefaultBodyLimit::max(1024 * 1024 * 1024)), // 1 GB
        )
        .route("/api/import/finalize", post(finalize_import))
        .with_state(state)
}

/// Simple raw-body upload: PUT /api/import/upload?filename=foo.jpg
/// Body is the raw file bytes. No multipart overhead.
#[derive(Deserialize)]
struct UploadQuery {
    filename: String,
}

async fn upload_file_raw(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    body: axum::body::Bytes,
) -> Result<StatusCode, StatusCode> {
    if query.filename.is_empty() || query.filename.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let library_path = std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string());
    let base_path = std::path::Path::new(&library_path).join(&query.filename);

    if let Some(parent) = base_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            tracing::error!("Failed to create directory: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    // Deduplicate filename if it already exists on disk
    let target_path = if base_path.exists() {
        let stem = base_path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
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
            tracing::error!("Failed to index uploaded file {:?}: {}", target_path_owned, e);
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
async fn finalize_import(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let scanner = state.scanner.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let library_path =
            std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string());
        let library = std::path::Path::new(&library_path);

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
        crate::import::run_reorganize(library, false)
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

#[derive(Serialize)]
struct ShotBrief {
    id: String,
    thumbnail_url: String,
    timestamp: Option<String>,
    file_count: i64,
    primary_person_id: Option<String>,
    primary_person_name: Option<String>,
    review_status: Option<String>,
    folder_number: Option<i64>,
}

#[derive(Serialize)]
struct SimilarShotItem {
    id: String,
    thumbnail_url: String,
    file_count: i64,
    primary_person_name: Option<String>,
    review_status: Option<String>,
    distance: u32,
}

#[derive(Deserialize)]
struct ShotsQuery {
    q: Option<String>,
    person_id: Option<String>,
    status: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

/// GET /api/shots - list shots with query params: person_id, status, q, from, to
async fn get_shots(
    State(state): State<AppState>,
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
        conditions.push("s.review_status = ?".to_string());
        bind_values.push(status.clone());
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
struct ShotDetailResponse {
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
}

#[derive(Serialize)]
struct FileDetail {
    id: String,
    path: String,
    mime_type: Option<String>,
    is_original: bool,
    file_size: Option<i64>,
}

#[derive(Serialize)]
struct FaceDetail {
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
struct AlsoContainsPerson {
    id: String,
    name: Option<String>,
}

/// GET /api/shots/:id - detail with files, faces, primary person, also_contains
async fn get_shot_detail(
    Path(id): Path<String>,
    State(state): State<AppState>,
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
        .prepare("SELECT id, path, mime_type, is_original, file_size FROM files WHERE shot_id = ?")
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
    }))
}

async fn delete_shot(
    Path(id): Path<String>,
    State(state): State<AppState>,
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

    // Clean up cached thumbnails (best-effort)
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
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

async fn get_people(State(state): State<AppState>) -> Json<Vec<PersonBrief>> {
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
                shot_count: row.get(4)?,
                pending_count: row.get(5)?,
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
    shot_count: i64,
    pending_count: i64,
}

/// Create a new person with a name
#[derive(Deserialize)]
struct CreatePersonPayload {
    name: String,
}

async fn create_person(
    State(state): State<AppState>,
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

async fn get_person_shots(
    Path(id): Path<String>,
    State(state): State<AppState>,
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

    // Determine the library path so we can rename the folder on disk
    let library_path =
        std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string());
    let library = std::path::Path::new(&library_path);

    crate::import::rename_person_folder(&db, library, &id, &payload.name).map_err(|e| {
        tracing::error!("Failed to rename person folder: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

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

    // Update shots that reference the source person as their primary
    db.execute(
        "UPDATE shots SET primary_person_id = ? WHERE primary_person_id = ?",
        params![payload.target_id, payload.source_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to reassign shots during merge: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    db.execute("DELETE FROM people WHERE id = ?", params![payload.source_id])
        .map_err(|e| {
            tracing::error!("Failed to delete merged person: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Delete a person and all their face records.
/// Removes all faces belonging to this person, cleans up face_neighbors,
/// recalculates primary_person_id for affected shots, and deletes the person.
async fn delete_person(
    Path(id): Path<String>,
    State(state): State<AppState>,
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
        let _ = recalculate_primary_person(&db, shot_id);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Recalculate `shots.primary_person_id` for a given shot.
/// Finds the face with the largest bounding box area that has a person_id,
/// and sets that as the shot's primary_person_id.
/// Skips if `review_status == 'confirmed'`.
/// If no faces have a person_id, sets primary_person_id = NULL.
fn recalculate_primary_person(db: &Connection, shot_id: &str) -> Result<(), StatusCode> {
    // Check review_status — skip confirmed shots
    let review_status: Option<String> = db
        .query_row(
            "SELECT review_status FROM shots WHERE id = ?",
            params![shot_id],
            |row| row.get(0),
        )
        .map_err(|e| {
            tracing::error!("Failed to get shot review_status: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if review_status.as_deref() == Some("confirmed") {
        return Ok(());
    }

    // Find the face with the largest bounding box area that has a person_id
    let primary_person: Option<String> = db
        .query_row(
            "SELECT fa.person_id FROM faces fa
             JOIN files f ON fa.file_id = f.id
             WHERE f.shot_id = ? AND fa.person_id IS NOT NULL
             ORDER BY (fa.box_x2 - fa.box_x1) * (fa.box_y2 - fa.box_y1) DESC
             LIMIT 1",
            params![shot_id],
            |row| row.get(0),
        )
        .ok();

    db.execute(
        "UPDATE shots SET primary_person_id = ? WHERE id = ?",
        params![primary_person, shot_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to update shot primary_person_id: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(())
}

/// Find the shot_id for a given face
fn get_shot_id_for_face(db: &Connection, face_id: &str) -> Result<String, StatusCode> {
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
struct FaceSuggestion {
    person_id: String,
    person_name: Option<String>,
    thumbnail_url: Option<String>,
    distance: f32,
}

async fn get_face_suggestions(
    Path(id): Path<String>,
    State(state): State<AppState>,
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
                thumbnail_url: thumbnail_face_id
                    .map(|fid| format!("/api/faces/{}/thumbnail", fid)),
                distance,
            })
        })
        .collect();

    suggestions.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
    suggestions.truncate(10);

    Ok(Json(suggestions))
}

/// Reassign a face to a different person
#[derive(Deserialize)]
struct ReassignFacePayload {
    person_id: String,
}

async fn reassign_face(
    Path(id): Path<String>,
    State(state): State<AppState>,
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

    // Find the shot_id for this face before updating
    let shot_id = get_shot_id_for_face(&db, &id)?;

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
    recalculate_primary_person(&db, &shot_id)?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// Delete a face detection (remove false positive / irrelevant face)
async fn delete_face(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;

    // Find the shot_id for this face before deleting
    let shot_id = get_shot_id_for_face(&db, &id)?;

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
    recalculate_primary_person(&db, &shot_id)?;

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
#[derive(Deserialize)]
struct ThumbnailQuery {
    w: Option<u32>,
}

async fn get_file_thumbnail(
    Path(id): Path<String>,
    Query(query): Query<ThumbnailQuery>,
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

    let source_path_owned = file_path.clone();
    let thumb_path_clone = thumb_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = if is_video {
            crate::scanner::extract_first_video_frame(std::path::Path::new(&source_path_owned))
                .map_err(|e| format!("Failed to extract video frame: {}", e))?
        } else {
            crate::scanner::open_image(std::path::Path::new(&source_path_owned))
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

/// Return aggregate stats about the library
#[derive(Serialize)]
struct StatsResponse {
    total_shots: i64,
    total_people: i64,
    total_files: i64,
}

async fn get_stats(State(state): State<AppState>) -> Json<StatsResponse> {
    let db = state.db.lock().await;

    let total_shots: i64 = db
        .query_row("SELECT COUNT(*) FROM shots", [], |r| r.get(0))
        .unwrap_or(0);
    let total_people: i64 = db
        .query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
        .unwrap_or(0);
    let total_files: i64 = db
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap_or(0);

    Json(StatsResponse {
        total_shots,
        total_people,
        total_files,
    })
}

/// Return detailed organize stats about the library
#[derive(Serialize)]
struct OrganizeStatsResponse {
    total_shots: i64,
    total_files: i64,
    total_people: i64,
    pending_review: i64,
    confirmed: i64,
    unsorted: i64,
    unnamed_people: i64,
}

async fn get_organize_stats(State(state): State<AppState>) -> Json<OrganizeStatsResponse> {
    let db = state.db.lock().await;

    let total_shots: i64 = db
        .query_row("SELECT COUNT(*) FROM shots", [], |r| r.get(0))
        .unwrap_or(0);
    let total_files: i64 = db
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap_or(0);
    let total_people: i64 = db
        .query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
        .unwrap_or(0);
    let pending_review: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE review_status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let confirmed: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE review_status = 'confirmed'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let unsorted: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE primary_person_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let unnamed_people: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM people WHERE name IS NULL OR name = ''",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Json(OrganizeStatsResponse {
        total_shots,
        total_files,
        total_people,
        pending_review,
        confirmed,
        unsorted,
        unnamed_people,
    })
}

/// Trigger filesystem reorganization in a background thread
async fn trigger_reorganize() -> Json<serde_json::Value> {
    std::thread::spawn(move || {
        let library_path =
            std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string());
        let library = std::path::Path::new(&library_path);

        if let Err(e) = crate::import::run_reorganize(library, false) {
            tracing::error!("Background reorganize failed: {}", e);
        } else {
            tracing::info!("Background reorganize completed successfully");
        }
    });

    Json(serde_json::json!({"status": "started"}))
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

// ---------------------------------------------------------------------------
// Shot Operations (Task 3A)
// ---------------------------------------------------------------------------

/// PUT /api/shots/:id - update primary_person_id and/or review_status.
/// When primary_person changes, assign new folder_number (MAX+1 for that person).
#[derive(Deserialize)]
struct UpdateShotPayload {
    primary_person_id: Option<String>,
    review_status: Option<String>,
}

async fn update_shot(
    Path(id): Path<String>,
    State(state): State<AppState>,
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
struct SplitShotPayload {
    file_ids: Vec<String>,
}

async fn split_shot(
    Path(id): Path<String>,
    State(state): State<AppState>,
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

/// GET /api/shots/:id/similar - find visually similar shots by dHash hamming distance
async fn get_similar_shots(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SimilarShotItem>>, StatusCode> {
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

    // Load candidate shots (same person, only main files)
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

    let candidates: Vec<(String, String, Option<String>, Option<String>, i64, Vec<u8>)> = stmt
        .query_map(params![id, primary_person_id], |row| {
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
        .filter_map(|(shot_id, main_fid, review_status, person_name, file_count, blob)| {
            if blob.len() != 8 {
                return None;
            }
            let mut candidate_dhash = [0u8; 8];
            candidate_dhash.copy_from_slice(&blob);
            let distance = crate::scanner::hamming_distance(&current_dhash, &candidate_dhash);
            Some(SimilarShotItem {
                id: shot_id,
                thumbnail_url: format!("/api/files/{}/thumbnail", main_fid),
                file_count,
                primary_person_name: person_name,
                review_status,
                distance,
            })
        })
        .collect();

    results.sort_by_key(|r| r.distance);
    results.truncate(10);

    Ok(Json(results))
}

/// POST /api/shots/merge - move all files from source to target shot. Delete source.
#[derive(Deserialize)]
struct MergeShotsPayload {
    source_id: String,
    target_id: String,
}

async fn merge_shots(
    State(state): State<AppState>,
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
    db.execute(
        "DELETE FROM shots WHERE id = ?",
        params![payload.source_id],
    )
    .map_err(|e| {
        tracing::error!("Failed to delete source shot during merge: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// POST /api/shots/batch/confirm - batch set review_status = 'confirmed'
#[derive(Deserialize)]
struct BatchConfirmPayload {
    shot_ids: Vec<String>,
}

async fn batch_confirm(
    State(state): State<AppState>,
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
struct BatchReassignPayload {
    shot_ids: Vec<String>,
    person_id: String,
}

async fn batch_reassign(
    State(state): State<AppState>,
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

/// PUT /api/files/:id/set-original - set is_original=true on this file,
/// is_original=false on all other files in the same shot. Update shots.main_file_id.
async fn set_file_original(
    Path(id): Path<String>,
    State(state): State<AppState>,
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
    db.execute(
        "UPDATE files SET is_original = 1 WHERE id = ?",
        params![id],
    )
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

/// POST /api/files/:id/faces - manually add a face bounding box.
/// The server computes the embedding from the given bbox coordinates.
#[derive(Deserialize)]
struct AddManualFacePayload {
    box_x1: f32,
    box_y1: f32,
    box_x2: f32,
    box_y2: f32,
}

async fn add_manual_face(
    Path(file_id): Path<String>,
    State(state): State<AppState>,
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

    let bbox = (payload.box_x1, payload.box_y1, payload.box_x2, payload.box_y2);
    let scanner = state.scanner.clone();
    let file_path_owned = file_path.clone();

    // Compute embedding on a blocking thread
    let (face_id, embedding_blob) = tokio::task::spawn_blocking(move || -> Result<(String, Vec<u8>), String> {
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

    recalculate_primary_person(&db, &shot_id)?;

    Ok(Json(serde_json::json!({"id": face_id})))
}
