use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::UState;
use crate::schema::{enhancement_tasks, faces, files, ignored_merges, people, shots, video_keyframes};

#[derive(Serialize, ToSchema)]
pub(crate) struct ShotBrief {
    pub id: String,
    pub thumbnail_url: String,
    pub timestamp: Option<String>,
    pub file_count: i64,
    pub primary_person_id: Option<String>,
    pub primary_person_name: Option<String>,
    pub review_status: Option<String>,
    pub folder_number: Option<i64>,
    pub description: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SimilarShotItem {
    id: String,
    thumbnail_url: String,
    file_count: i64,
    primary_person_name: Option<String>,
    review_status: Option<String>,
    distance: u32,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SimilarShotsGrouped {
    person_id: Option<String>,
    person_name: Option<String>,
    shots: Vec<SimilarShotItem>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct ShotsQuery {
    q: Option<String>,
    person_id: Option<String>,
    status: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

/// GET /api/shots - list shots with query params: person_id, status, q, from, to
#[utoipa::path(
    get,
    path = "/api/shots",
    tag = "shots",
    summary = "List shots",
    description = "List shots with optional filtering by person, date range, review status, and pagination. Returns brief shot metadata suitable for gallery views.",
    params(ShotsQuery),
    responses(
        (status = 200, description = "List of shots", body = Vec<ShotBrief>)
    )
)]
pub(super) async fn get_shots(
    UState(state): UState,
    Query(params): Query<ShotsQuery>,
) -> Json<Vec<ShotBrief>> {
    let mut conn = match state.pool.get() {
        Ok(c) => c,
        Err(_) => return Json(Vec::new()),
    };

    // Build dynamic SQL query
    let mut sql = String::from(
        "SELECT DISTINCT s.id, s.timestamp, s.primary_person_id, s.review_status, s.folder_number,
                f.id AS main_file_id, p.name AS person_name,
                (SELECT COUNT(*) FROM files WHERE shot_id = s.id) AS file_count,
                s.description
         FROM shots s
         LEFT JOIN files f ON s.main_file_id = f.id
         LEFT JOIN people p ON s.primary_person_id = p.id",
    );
    let mut conditions: Vec<String> = Vec::new();
    let mut bind_values: Vec<String> = Vec::new();

    if let Some(ref person_id) = params.person_id {
        bind_values.push(person_id.clone());
        conditions.push(format!("s.primary_person_id = ?{}", bind_values.len()));
    }

    if let Some(ref status) = params.status {
        if status == "unsorted" {
            conditions.push("s.primary_person_id IS NULL".to_string());
        } else {
            bind_values.push(status.clone());
            conditions.push(format!("s.review_status = ?{}", bind_values.len()));
        }
    }

    if let Some(ref q) = params.q {
        let pattern = format!("%{}%", q);
        bind_values.push(pattern.clone());
        let idx1 = bind_values.len();
        bind_values.push(pattern);
        let idx2 = bind_values.len();
        conditions.push(format!(
            "(EXISTS (SELECT 1 FROM files fq WHERE fq.shot_id = s.id AND fq.path LIKE ?{}) OR s.description LIKE ?{})",
            idx1, idx2
        ));
    }

    if let Some(ref from) = params.from {
        bind_values.push(from.clone());
        conditions.push(format!("s.timestamp >= ?{}", bind_values.len()));
    }

    if let Some(ref to) = params.to {
        bind_values.push(to.clone());
        conditions.push(format!("s.timestamp <= ?{}", bind_values.len()));
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(" ORDER BY s.timestamp DESC");

    #[derive(diesel::QueryableByName)]
    struct ShotRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        timestamp: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        primary_person_id: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        review_status: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Integer>)]
        folder_number: Option<i32>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        main_file_id: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        person_name: Option<String>,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        file_count: i64,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        description: Option<String>,
    }

    // Build the sql_query and bind parameters dynamically
    let query = diesel::sql_query(&sql);

    // We need to bind each parameter. Since diesel::sql_query().bind() returns a
    // different type each time, we need to handle this with a macro-like approach.
    // Instead, we'll use the boxed approach with raw SQL parameter embedding.
    // Actually, diesel::sql_query uses positional params (?1, ?2, etc.) for SQLite.
    // We need to bind them in order.

    // Unfortunately, diesel's sql_query chaining changes the type with each bind,
    // making dynamic binding difficult. We'll handle up to the maximum number of
    // parameters we can have (max 5: person_id, status, q_pattern1, q_pattern2, from, to = 6 max).
    let rows: Result<Vec<ShotRow>, _> = match bind_values.len() {
        0 => query.load::<ShotRow>(&mut conn),
        1 => query
            .bind::<diesel::sql_types::Text, _>(&bind_values[0])
            .load::<ShotRow>(&mut conn),
        2 => query
            .bind::<diesel::sql_types::Text, _>(&bind_values[0])
            .bind::<diesel::sql_types::Text, _>(&bind_values[1])
            .load::<ShotRow>(&mut conn),
        3 => query
            .bind::<diesel::sql_types::Text, _>(&bind_values[0])
            .bind::<diesel::sql_types::Text, _>(&bind_values[1])
            .bind::<diesel::sql_types::Text, _>(&bind_values[2])
            .load::<ShotRow>(&mut conn),
        4 => query
            .bind::<diesel::sql_types::Text, _>(&bind_values[0])
            .bind::<diesel::sql_types::Text, _>(&bind_values[1])
            .bind::<diesel::sql_types::Text, _>(&bind_values[2])
            .bind::<diesel::sql_types::Text, _>(&bind_values[3])
            .load::<ShotRow>(&mut conn),
        5 => query
            .bind::<diesel::sql_types::Text, _>(&bind_values[0])
            .bind::<diesel::sql_types::Text, _>(&bind_values[1])
            .bind::<diesel::sql_types::Text, _>(&bind_values[2])
            .bind::<diesel::sql_types::Text, _>(&bind_values[3])
            .bind::<diesel::sql_types::Text, _>(&bind_values[4])
            .load::<ShotRow>(&mut conn),
        _ => query
            .bind::<diesel::sql_types::Text, _>(&bind_values[0])
            .bind::<diesel::sql_types::Text, _>(&bind_values[1])
            .bind::<diesel::sql_types::Text, _>(&bind_values[2])
            .bind::<diesel::sql_types::Text, _>(&bind_values[3])
            .bind::<diesel::sql_types::Text, _>(&bind_values[4])
            .bind::<diesel::sql_types::Text, _>(&bind_values[5])
            .load::<ShotRow>(&mut conn),
    };

    let shots = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| ShotBrief {
                id: row.id,
                thumbnail_url: row
                    .main_file_id
                    .map(|fid| format!("/api/files/{}/thumbnail", fid))
                    .unwrap_or_default(),
                timestamp: row.timestamp,
                file_count: row.file_count,
                primary_person_id: row.primary_person_id,
                primary_person_name: row.person_name,
                review_status: row.review_status,
                folder_number: row.folder_number.map(|v| v as i64),
                description: row.description,
            })
            .collect(),
        Err(e) => {
            tracing::error!("Failed to query shots: {}", e);
            Vec::new()
        }
    };

    Json(shots)
}

#[derive(Serialize, ToSchema)]
pub(super) struct ShotDetailResponse {
    id: String,
    timestamp: Option<String>,
    primary_person_id: Option<String>,
    primary_person_name: Option<String>,
    review_status: Option<String>,
    folder_number: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    description: Option<String>,
    files: Vec<FileDetail>,
    faces: Vec<FaceDetail>,
    also_contains: Vec<AlsoContainsPerson>,
    prev_shot_id: Option<String>,
    next_shot_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct FileDetail {
    id: String,
    path: String,
    mime_type: Option<String>,
    is_original: bool,
    file_size: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    duration_ms: Option<i64>,
    thumbnail_url: String,
}

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
pub(super) struct AlsoContainsPerson {
    id: String,
    name: Option<String>,
}

/// GET /api/shots/:id - detail with files, faces, primary person, also_contains
#[utoipa::path(
    get,
    path = "/api/shots/{id}",
    tag = "shots",
    summary = "Get shot details",
    description = "Retrieve full details for a single shot including all associated files, detected faces, and person assignments.",
    params(("id" = String, Path, description = "Shot ID")),
    responses(
        (status = 200, description = "Shot detail", body = ShotDetailResponse),
        (status = 404, description = "Shot not found")
    )
)]
pub(super) async fn get_shot_detail(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<ShotDetailResponse>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get shot metadata with person name via sql_query (LEFT JOIN)
    #[derive(diesel::QueryableByName)]
    struct ShotMetaRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        timestamp: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        primary_person_id: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        review_status: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Integer>)]
        folder_number: Option<i32>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        person_name: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Integer>)]
        width: Option<i32>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Integer>)]
        height: Option<i32>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        description: Option<String>,
    }

    let shot_row: ShotMetaRow = diesel::sql_query(
        "SELECT s.id, s.timestamp, s.primary_person_id, s.review_status, s.folder_number, p.name AS person_name, s.width, s.height, s.description
         FROM shots s
         LEFT JOIN people p ON s.primary_person_id = p.id
         WHERE s.id = ?1",
    )
    .bind::<diesel::sql_types::Text, _>(&id)
    .get_result::<ShotMetaRow>(&mut conn)
    .map_err(|_| StatusCode::NOT_FOUND)?;

    let shot_width = shot_row.width.map(|v| v as i64);
    let shot_height = shot_row.height.map(|v| v as i64);

    // Get files for this shot
    let file_rows: Vec<(String, String, Option<String>, Option<bool>, Option<i32>)> = files::table
        .filter(files::shot_id.eq(&id))
        .select((files::id, files::path, files::mime_type, files::is_original, files::file_size))
        .order((files::is_original.desc(), files::path.asc()))
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query files: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let detail_files: Vec<FileDetail> = file_rows
        .into_iter()
        .map(|(file_id, path, mime_type, is_original, file_size)| FileDetail {
            thumbnail_url: format!("/api/files/{}/thumbnail", file_id),
            id: file_id,
            path,
            mime_type,
            is_original: is_original.unwrap_or(false),
            file_size: file_size.map(|v| v as i64),
            width: shot_width,
            height: shot_height,
            duration_ms: None,
        })
        .collect();

    // We already set width/height from shot metadata above in the map

    // Get faces for files in this shot, with person names
    #[derive(diesel::QueryableByName)]
    struct FaceRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
        #[diesel(sql_type = diesel::sql_types::Text)]
        file_id: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        person_id: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        person_name: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Float>)]
        box_x1: Option<f32>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Float>)]
        box_y1: Option<f32>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Float>)]
        box_x2: Option<f32>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Float>)]
        box_y2: Option<f32>,
    }

    let face_rows: Vec<FaceRow> = diesel::sql_query(
        "SELECT fa.id, fa.file_id, fa.person_id, p.name AS person_name, fa.box_x1, fa.box_y1, fa.box_x2, fa.box_y2
         FROM faces fa
         JOIN files f ON fa.file_id = f.id
         LEFT JOIN people p ON fa.person_id = p.id
         WHERE f.shot_id = ?1",
    )
    .bind::<diesel::sql_types::Text, _>(&id)
    .load::<FaceRow>(&mut conn)
    .map_err(|e| {
        tracing::error!("Failed to query faces: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let detail_faces: Vec<FaceDetail> = face_rows
        .into_iter()
        .map(|row| FaceDetail {
            id: row.id,
            file_id: row.file_id,
            person_id: row.person_id,
            person_name: row.person_name,
            box_x1: row.box_x1.unwrap_or(0.0),
            box_y1: row.box_y1.unwrap_or(0.0),
            box_x2: row.box_x2.unwrap_or(0.0),
            box_y2: row.box_y2.unwrap_or(0.0),
        })
        .collect();

    // Compute also_contains: people who have faces in this shot OTHER than the primary person
    let mut also_contains: Vec<AlsoContainsPerson> = Vec::new();
    let mut seen_person_ids = std::collections::HashSet::new();
    for face in &detail_faces {
        if let Some(ref pid) = face.person_id {
            // Skip the primary person and duplicates
            if Some(pid.as_str()) != shot_row.primary_person_id.as_deref() && seen_person_ids.insert(pid.clone()) {
                also_contains.push(AlsoContainsPerson {
                    id: pid.clone(),
                    name: face.person_name.clone(),
                });
            }
        }
    }

    // Previous shot (newer timestamp, or same timestamp with id > current for stable ordering)
    #[derive(diesel::QueryableByName)]
    struct NavRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
    }

    let prev_shot_id: Option<String> = diesel::sql_query(
        "SELECT id FROM shots
         WHERE (timestamp > ?1 OR (timestamp = ?1 AND id > ?2) OR (timestamp IS NULL AND ?1 IS NOT NULL))
         ORDER BY timestamp ASC, id ASC
         LIMIT 1",
    )
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&shot_row.timestamp)
    .bind::<diesel::sql_types::Text, _>(&id)
    .get_result::<NavRow>(&mut conn)
    .ok()
    .map(|r| r.id);

    // Next shot (older timestamp, or same timestamp with id < current for stable ordering)
    let next_shot_id: Option<String> = diesel::sql_query(
        "SELECT id FROM shots
         WHERE (timestamp < ?1 OR (timestamp = ?1 AND id < ?2) OR (?1 IS NULL AND timestamp IS NOT NULL))
         ORDER BY timestamp DESC, id DESC
         LIMIT 1",
    )
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&shot_row.timestamp)
    .bind::<diesel::sql_types::Text, _>(&id)
    .get_result::<NavRow>(&mut conn)
    .ok()
    .map(|r| r.id);

    Ok(Json(ShotDetailResponse {
        id: shot_row.id,
        timestamp: shot_row.timestamp,
        primary_person_id: shot_row.primary_person_id,
        primary_person_name: shot_row.person_name,
        review_status: shot_row.review_status,
        folder_number: shot_row.folder_number.map(|v| v as i64),
        width: shot_width,
        height: shot_height,
        description: shot_row.description,
        files: detail_files,
        faces: detail_faces,
        also_contains,
        prev_shot_id,
        next_shot_id,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/shots/{id}",
    tag = "shots",
    summary = "Delete a shot",
    description = "Delete a shot and all its associated files from disk and database. This action is irreversible.",
    params(("id" = String, Path, description = "Shot ID")),
    responses(
        (status = 200, description = "Shot deleted successfully"),
        (status = 404, description = "Shot not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn delete_shot(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Collect file paths and IDs to clean up
    let file_rows: Vec<(String, String)> = files::table
        .filter(files::shot_id.eq(&id))
        .select((files::id, files::path))
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query files for delete: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if file_rows.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    let file_ids: Vec<&str> = file_rows.iter().map(|(id, _)| id.as_str()).collect();

    // Delete faces for all files in this shot
    diesel::delete(faces::table.filter(faces::file_id.eq_any(&file_ids)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete faces: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete video_keyframes
    diesel::delete(video_keyframes::table.filter(video_keyframes::video_file_id.eq_any(&file_ids)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete video_keyframes: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Clear enhancement_tasks referencing these files
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::output_file_id.eq_any(&file_ids)))
        .set(enhancement_tasks::output_file_id.eq(None::<String>))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to clear enhancement_tasks: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete enhancement_tasks for this shot
    diesel::delete(enhancement_tasks::table.filter(enhancement_tasks::shot_id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete enhancement_tasks: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete files from DB
    diesel::delete(files::table.filter(files::shot_id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete files: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete the shot record
    let deleted = diesel::delete(shots::table.filter(shots::id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete shot: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    // Delete physical files from disk (best-effort)
    for (_, path) in &file_rows {
        let resolved = crate::db::resolve_path(&state.library_root, path);
        if let Err(e) = std::fs::remove_file(&resolved) {
            tracing::warn!("Failed to delete file from disk {:?}: {}", resolved, e);
        }
    }

    // Clean up cached thumbnails and empty directories (best-effort)
    let thumb_dir = state.library_root.join(".phos_thumbnails");
    for (fid, _) in &file_rows {
        let _ = std::fs::remove_file(thumb_dir.join(format!("{}.jpg", fid)));
    }
    let _ = crate::import::cleanup_empty_dirs(&state.library_root);

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// PUT /api/shots/:id - update primary_person_id and/or review_status.
/// When primary_person changes, assign new folder_number (MAX+1 for that person).
#[derive(Deserialize, ToSchema)]
pub(super) struct UpdateShotPayload {
    primary_person_id: Option<String>,
    review_status: Option<String>,
}

#[utoipa::path(
    put,
    path = "/api/shots/{id}",
    tag = "shots",
    summary = "Update a shot",
    description = "Update shot metadata such as the primary person assignment and review status.",
    params(("id" = String, Path, description = "Shot ID")),
    request_body = UpdateShotPayload,
    responses(
        (status = 200, description = "Shot updated successfully"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Shot not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn update_shot(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<UpdateShotPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify the shot exists
    let exists: bool = shots::table
        .filter(shots::id.eq(&id))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    if let Some(ref person_id) = payload.primary_person_id {
        if person_id.is_empty() {
            // Set to unsorted (NULL primary_person_id)
            let max_folder: i64 = shots::table
                .filter(shots::primary_person_id.is_null())
                .select(diesel::dsl::max(shots::folder_number))
                .first::<Option<i32>>(&mut conn)
                .unwrap_or(None)
                .map(|v| v as i64)
                .unwrap_or(0);

            diesel::update(shots::table.filter(shots::id.eq(&id)))
                .set((
                    shots::primary_person_id.eq(None::<String>),
                    shots::folder_number.eq((max_folder + 1) as i32),
                ))
                .execute(&mut conn)
                .map_err(|e| {
                    tracing::error!("Failed to update shot to unsorted: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        } else {
            // Verify person exists
            let person_exists: bool = people::table
                .filter(people::id.eq(person_id))
                .count()
                .get_result::<i64>(&mut conn)
                .map(|c| c > 0)
                .unwrap_or(false);

            if !person_exists {
                return Err(StatusCode::BAD_REQUEST);
            }

            // Assign new folder_number for this person (MAX+1)
            let max_folder: i64 = shots::table
                .filter(shots::primary_person_id.eq(person_id))
                .select(diesel::dsl::max(shots::folder_number))
                .first::<Option<i32>>(&mut conn)
                .unwrap_or(None)
                .map(|v| v as i64)
                .unwrap_or(0);

            diesel::update(shots::table.filter(shots::id.eq(&id)))
                .set((
                    shots::primary_person_id.eq(person_id),
                    shots::folder_number.eq((max_folder + 1) as i32),
                ))
                .execute(&mut conn)
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
        diesel::update(shots::table.filter(shots::id.eq(&id)))
            .set(shots::review_status.eq(status))
            .execute(&mut conn)
            .map_err(|e| {
                tracing::error!("Failed to update shot review_status: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// POST /api/shots/:id/split - create new shot from specified files.
/// New shot inherits primary person from faces. Both shots get review_status = 'pending'.
#[derive(Deserialize, ToSchema)]
pub(super) struct SplitShotPayload {
    file_ids: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/api/shots/{id}/split",
    tag = "shots",
    summary = "Split a shot",
    description = "Split selected files out of a shot into a new separate shot. Useful when the scanner incorrectly groups unrelated files together.",
    params(("id" = String, Path, description = "Shot ID")),
    request_body = SplitShotPayload,
    responses(
        (status = 200, description = "Shot split successfully"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Shot not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn split_shot(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<SplitShotPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.file_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify the source shot exists
    let exists: bool = shots::table
        .filter(shots::id.eq(&id))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Verify all file_ids belong to this shot
    for fid in &payload.file_ids {
        let belongs: bool = files::table
            .filter(files::id.eq(fid))
            .filter(files::shot_id.eq(&id))
            .count()
            .get_result::<i64>(&mut conn)
            .map(|c| c > 0)
            .unwrap_or(false);

        if !belongs {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Verify we're not splitting ALL files (must leave at least one in the source shot)
    let total_files: i64 = files::table
        .filter(files::shot_id.eq(&id))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    if payload.file_ids.len() as i64 >= total_files {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Get the source shot's metadata
    let (timestamp, width, height, latitude, longitude): (
        Option<String>,
        Option<i32>,
        Option<i32>,
        Option<f32>,
        Option<f32>,
    ) = shots::table
        .filter(shots::id.eq(&id))
        .select((shots::timestamp, shots::width, shots::height, shots::latitude, shots::longitude))
        .first(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to get source shot metadata: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Create new shot
    let new_shot_id = uuid::Uuid::new_v4().to_string();

    diesel::insert_into(shots::table)
        .values((
            shots::id.eq(&new_shot_id),
            shots::timestamp.eq(&timestamp),
            shots::width.eq(width),
            shots::height.eq(height),
            shots::latitude.eq(latitude),
            shots::longitude.eq(longitude),
            shots::review_status.eq("pending"),
        ))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to create new shot for split: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Move the specified files to the new shot
    diesel::update(files::table.filter(files::id.eq_any(&payload.file_ids)))
        .set(files::shot_id.eq(&new_shot_id))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to move files to new shot: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Ensure the new shot has at least one is_original file
    let new_has_original: bool = files::table
        .filter(files::shot_id.eq(&new_shot_id))
        .filter(files::is_original.eq(true))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !new_has_original {
        // Get the first file in the new shot and mark it as original
        if let Ok(first_file_id) = files::table
            .filter(files::shot_id.eq(&new_shot_id))
            .select(files::id)
            .first::<String>(&mut conn)
        {
            let _ = diesel::update(files::table.filter(files::id.eq(&first_file_id)))
                .set(files::is_original.eq(true))
                .execute(&mut conn);
        }
    }

    // Set the new shot's main_file_id to its original file
    if let Ok(new_main_file) = files::table
        .filter(files::shot_id.eq(&new_shot_id))
        .filter(files::is_original.eq(true))
        .select(files::id)
        .first::<String>(&mut conn)
    {
        let _ = diesel::update(shots::table.filter(shots::id.eq(&new_shot_id)))
            .set(shots::main_file_id.eq(&new_main_file))
            .execute(&mut conn);
    }

    // Ensure the source shot still has an original
    let source_has_original: bool = files::table
        .filter(files::shot_id.eq(&id))
        .filter(files::is_original.eq(true))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !source_has_original {
        if let Ok(first_file_id) = files::table
            .filter(files::shot_id.eq(&id))
            .select(files::id)
            .first::<String>(&mut conn)
        {
            let _ = diesel::update(files::table.filter(files::id.eq(&first_file_id)))
                .set(files::is_original.eq(true))
                .execute(&mut conn);
        }
    }

    // Update the source shot's main_file_id
    if let Ok(source_main_file) = files::table
        .filter(files::shot_id.eq(&id))
        .filter(files::is_original.eq(true))
        .select(files::id)
        .first::<String>(&mut conn)
    {
        let _ = diesel::update(shots::table.filter(shots::id.eq(&id)))
            .set(shots::main_file_id.eq(&source_main_file))
            .execute(&mut conn);
    }

    // Determine primary person for the new shot based on its faces
    // (face with the largest bounding box area that has a person_id)
    #[derive(diesel::QueryableByName)]
    struct PersonIdRow {
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        person_id: Option<String>,
    }

    let new_primary_person: Option<String> = diesel::sql_query(
        "SELECT fa.person_id
         FROM faces fa
         JOIN files f ON fa.file_id = f.id
         WHERE f.shot_id = ?1 AND fa.person_id IS NOT NULL
         ORDER BY (fa.box_x2 - fa.box_x1) * (fa.box_y2 - fa.box_y1) DESC
         LIMIT 1",
    )
    .bind::<diesel::sql_types::Text, _>(&new_shot_id)
    .get_result::<PersonIdRow>(&mut conn)
    .ok()
    .and_then(|r| r.person_id);

    if let Some(ref ppid) = new_primary_person {
        let max_folder: i64 = shots::table
            .filter(shots::primary_person_id.eq(ppid))
            .select(diesel::dsl::max(shots::folder_number))
            .first::<Option<i32>>(&mut conn)
            .unwrap_or(None)
            .map(|v| v as i64)
            .unwrap_or(0);

        let _ = diesel::update(shots::table.filter(shots::id.eq(&new_shot_id)))
            .set((
                shots::primary_person_id.eq(ppid),
                shots::folder_number.eq((max_folder + 1) as i32),
            ))
            .execute(&mut conn);
    }

    // Set both shots to pending
    let _ = diesel::update(
        shots::table.filter(shots::id.eq(&id).or(shots::id.eq(&new_shot_id))),
    )
    .set(shots::review_status.eq("pending"))
    .execute(&mut conn);

    Ok(Json(
        serde_json::json!({"status": "ok", "new_shot_id": new_shot_id}),
    ))
}

/// GET /api/shots/:id/similar - find visually similar shots by dHash hamming distance, grouped by person
#[utoipa::path(
    get,
    path = "/api/shots/{id}/similar",
    tag = "shots",
    summary = "Find similar shots",
    description = "Find shots visually similar to the given shot, grouped by the person they are assigned to. Uses perceptual hash comparison.",
    params(("id" = String, Path, description = "Shot ID")),
    responses(
        (status = 200, description = "Similar shots grouped by person", body = Vec<SimilarShotsGrouped>),
        (status = 404, description = "Shot not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_similar_shots(
    UState(state): UState,
    Path(id): Path<String>,
) -> Result<Json<Vec<SimilarShotsGrouped>>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get current shot's primary_person_id and main_file_id
    let (main_file_id, primary_person_id): (Option<String>, Option<String>) = shots::table
        .filter(shots::id.eq(&id))
        .select((shots::main_file_id, shots::primary_person_id))
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let main_file_id = match main_file_id {
        Some(fid) => fid,
        None => return Ok(Json(vec![])),
    };

    // Load the main file's dHash
    let current_dhash_blob: Option<Vec<u8>> = files::table
        .filter(files::id.eq(&main_file_id))
        .filter(files::visual_embedding.is_not_null())
        .select(files::visual_embedding)
        .first::<Option<Vec<u8>>>(&mut conn)
        .ok()
        .flatten();

    let current_dhash = match current_dhash_blob {
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
            people::table
                .filter(people::id.eq(pid))
                .select(people::name)
                .first::<Option<String>>(&mut conn)
                .ok()
                .flatten()
        });
        person_ids.push((primary_person_id.clone(), primary_name));
    }

    // Add secondary people from faces on this shot's files
    {
        #[derive(diesel::QueryableByName)]
        struct SecondaryPersonRow {
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            person_id: Option<String>,
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            name: Option<String>,
        }

        let secondary: Vec<SecondaryPersonRow> = diesel::sql_query(
            "SELECT DISTINCT fa.person_id, p.name
             FROM faces fa
             JOIN files fi ON fi.id = fa.file_id
             JOIN people p ON p.id = fa.person_id
             WHERE fi.shot_id = ?1
               AND fa.person_id IS NOT NULL
               AND (?2 IS NULL OR fa.person_id != ?2)",
        )
        .bind::<diesel::sql_types::Text, _>(&id)
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(&primary_person_id)
        .load::<SecondaryPersonRow>(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query faces: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        for row in secondary {
            person_ids.push((row.person_id, row.name));
        }
    }

    // If no people at all, query shots with NULL primary_person_id
    if person_ids.is_empty() {
        person_ids.push((None, None));
    }

    // For each person, find similar shots
    let mut groups: Vec<SimilarShotsGrouped> = Vec::new();

    #[derive(diesel::QueryableByName)]
    struct CandidateRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        main_file_id: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        review_status: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        person_name: Option<String>,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        file_count: i64,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
        visual_embedding: Option<Vec<u8>>,
    }

    for (person_id, person_name) in &person_ids {
        let candidates: Vec<CandidateRow> = diesel::sql_query(
            "SELECT s.id, s.main_file_id, s.review_status, p.name AS person_name,
                    (SELECT COUNT(*) FROM files WHERE shot_id = s.id) as file_count,
                    f.visual_embedding
             FROM shots s
             LEFT JOIN people p ON s.primary_person_id = p.id
             JOIN files f ON f.id = s.main_file_id AND f.visual_embedding IS NOT NULL
             WHERE s.id != ?1
               AND (s.primary_person_id = ?2 OR (?2 IS NULL AND s.primary_person_id IS NULL))",
        )
        .bind::<diesel::sql_types::Text, _>(&id)
        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(person_id)
        .load::<CandidateRow>(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query similar shots: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        let mut results: Vec<SimilarShotItem> = candidates
            .into_iter()
            .filter_map(|row| {
                let blob = row.visual_embedding?;
                if blob.len() != 8 {
                    return None;
                }
                let mut candidate_dhash = [0u8; 8];
                candidate_dhash.copy_from_slice(&blob);
                let distance =
                    crate::scanner::hamming_distance(&current_dhash, &candidate_dhash);
                Some(SimilarShotItem {
                    id: row.id,
                    thumbnail_url: format!(
                        "/api/files/{}/thumbnail",
                        row.main_file_id.unwrap_or_default()
                    ),
                    file_count: row.file_count,
                    primary_person_name: row.person_name,
                    review_status: row.review_status,
                    distance,
                })
            })
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
#[derive(Deserialize, ToSchema)]
pub(super) struct MergeShotsPayload {
    source_id: String,
    target_id: String,
    person_id: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/shots/merge",
    tag = "shots",
    summary = "Merge shots",
    description = "Merge two or more shots into a single shot. All files from the source shots are moved to the target shot.",
    request_body = MergeShotsPayload,
    responses(
        (status = 200, description = "Shots merged successfully"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Shot not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn merge_shots(
    UState(state): UState,
    Json(payload): Json<MergeShotsPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.source_id == payload.target_id {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify both shots exist
    for sid in [&payload.source_id, &payload.target_id] {
        let exists: bool = shots::table
            .filter(shots::id.eq(sid))
            .count()
            .get_result::<i64>(&mut conn)
            .map(|c| c > 0)
            .unwrap_or(false);

        if !exists {
            return Err(StatusCode::NOT_FOUND);
        }
    }

    // Move all files from source to target
    // Set is_original = false on moved files (target keeps its original)
    diesel::update(files::table.filter(files::shot_id.eq(&payload.source_id)))
        .set((
            files::shot_id.eq(&payload.target_id),
            files::is_original.eq(false),
        ))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to move files during shot merge: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete the source shot
    diesel::delete(shots::table.filter(shots::id.eq(&payload.source_id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete source shot during merge: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // If person_id is provided, update the target shot's primary person
    if let Some(ref person_id) = payload.person_id {
        diesel::update(shots::table.filter(shots::id.eq(&payload.target_id)))
            .set((
                shots::primary_person_id.eq(person_id),
                shots::folder_number.eq(None::<i32>),
            ))
            .execute(&mut conn)
            .map_err(|e| {
                tracing::error!("Failed to update primary_person_id during merge: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    // Clean up people who lost all their shots after this merge
    if let Err(e) = crate::db::cleanup_orphaned_people(&mut conn) {
        tracing::error!("Failed to cleanup orphaned people after merge: {}", e);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// POST /api/shots/batch/confirm - batch set review_status = 'confirmed'
#[derive(Deserialize, ToSchema)]
pub(super) struct BatchConfirmPayload {
    shot_ids: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/api/shots/batch/confirm",
    tag = "shots",
    summary = "Batch confirm shots",
    description = "Confirm the person assignment for multiple shots at once, setting their review status to confirmed.",
    request_body = BatchConfirmPayload,
    responses(
        (status = 200, description = "Shots confirmed successfully"),
        (status = 400, description = "Invalid request"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn batch_confirm(
    UState(state): UState,
    Json(payload): Json<BatchConfirmPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.shot_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut updated_count = 0usize;
    for sid in &payload.shot_ids {
        let updated = diesel::update(shots::table.filter(shots::id.eq(sid)))
            .set(shots::review_status.eq("confirmed"))
            .execute(&mut conn)
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
#[derive(Deserialize, ToSchema)]
pub(super) struct BatchReassignPayload {
    shot_ids: Vec<String>,
    person_id: String,
}

#[utoipa::path(
    post,
    path = "/api/shots/batch/reassign",
    tag = "shots",
    summary = "Batch reassign shots",
    description = "Reassign multiple shots to a different person in a single operation.",
    request_body = BatchReassignPayload,
    responses(
        (status = 200, description = "Shots reassigned successfully"),
        (status = 400, description = "Invalid request"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn batch_reassign(
    UState(state): UState,
    Json(payload): Json<BatchReassignPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if payload.shot_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify person exists (unless empty = unsorted)
    let person_id_value: Option<&str> = if payload.person_id.is_empty() {
        None
    } else {
        let exists: bool = people::table
            .filter(people::id.eq(&payload.person_id))
            .count()
            .get_result::<i64>(&mut conn)
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
            Some(pid) => shots::table
                .filter(shots::primary_person_id.eq(pid))
                .select(diesel::dsl::max(shots::folder_number))
                .first::<Option<i32>>(&mut conn)
                .unwrap_or(None)
                .map(|v| v as i64)
                .unwrap_or(0),
            None => shots::table
                .filter(shots::primary_person_id.is_null())
                .select(diesel::dsl::max(shots::folder_number))
                .first::<Option<i32>>(&mut conn)
                .unwrap_or(None)
                .map(|v| v as i64)
                .unwrap_or(0),
        };

        let updated = match person_id_value {
            Some(pid) => diesel::update(shots::table.filter(shots::id.eq(sid)))
                .set((
                    shots::primary_person_id.eq(pid),
                    shots::folder_number.eq((max_folder + 1) as i32),
                    shots::review_status.eq("confirmed"),
                ))
                .execute(&mut conn),
            None => diesel::update(shots::table.filter(shots::id.eq(sid)))
                .set((
                    shots::primary_person_id.eq(None::<String>),
                    shots::folder_number.eq((max_folder + 1) as i32),
                    shots::review_status.eq("confirmed"),
                ))
                .execute(&mut conn),
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

#[derive(Deserialize, ToSchema)]
pub(super) struct IgnoreMergePayload {
    shot_id_1: String,
    shot_id_2: String,
}

#[utoipa::path(
    post,
    path = "/api/shots/merge/ignore",
    tag = "shots",
    summary = "Ignore merge suggestion",
    description = "Mark a pair of similar shots as intentionally separate so they are no longer suggested for merging.",
    request_body = IgnoreMergePayload,
    responses(
        (status = 200, description = "Merge pair ignored successfully"),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn ignore_merge(
    UState(state): UState,
    Json(payload): Json<IgnoreMergePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (s1, s2) = if payload.shot_id_1 < payload.shot_id_2 {
        (&payload.shot_id_1, &payload.shot_id_2)
    } else {
        (&payload.shot_id_2, &payload.shot_id_1)
    };

    diesel::insert_or_ignore_into(ignored_merges::table)
        .values((
            ignored_merges::shot_id_1.eq(s1),
            ignored_merges::shot_id_2.eq(s2),
        ))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to insert ignored_merge: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

#[derive(Serialize, ToSchema)]
pub(super) struct SimilarShotGroup {
    primary: SimilarShotItem,
    candidates: Vec<SimilarShotItem>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct SimilarGroupsQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SimilarGroupsResponse {
    groups: Vec<SimilarShotGroup>,
    total: usize,
    offset: usize,
    limit: usize,
}

#[utoipa::path(
    get,
    path = "/api/shots/similar-groups",
    tag = "shots",
    summary = "List similar shot groups",
    description = "Retrieve paginated groups of similar shots across the entire library for bulk deduplication review.",
    params(SimilarGroupsQuery),
    responses(
        (status = 200, description = "Paginated similar shot groups", body = SimilarGroupsResponse),
        (status = 500, description = "Internal server error")
    )
)]
pub(super) async fn get_similar_shot_groups(
    UState(state): UState,
    Query(query): Query<SimilarGroupsQuery>,
) -> Result<Json<SimilarGroupsResponse>, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    #[derive(diesel::QueryableByName)]
    struct ShotDataRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        main_file_id: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        review_status: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        person_name: Option<String>,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        file_count: i64,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
        visual_embedding: Option<Vec<u8>>,
    }

    let all_shots: Vec<ShotDataRow> = diesel::sql_query(
        "SELECT s.id, s.main_file_id, s.review_status, p.name AS person_name,
                (SELECT COUNT(*) FROM files WHERE shot_id = s.id) as file_count,
                f.visual_embedding
         FROM shots s
         LEFT JOIN people p ON s.primary_person_id = p.id
         JOIN files f ON f.id = s.main_file_id AND f.visual_embedding IS NOT NULL
         ORDER BY s.timestamp DESC",
    )
    .load::<ShotDataRow>(&mut conn)
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    struct ShotData {
        id: String,
        main_fid: String,
        review_status: Option<String>,
        person_name: Option<String>,
        file_count: i64,
        dhash: [u8; 8],
    }

    let candidates: Vec<ShotData> = all_shots
        .into_iter()
        .filter_map(|row| {
            let blob = row.visual_embedding?;
            if blob.len() != 8 {
                return None;
            }
            let mut dhash = [0u8; 8];
            dhash.copy_from_slice(&blob);
            Some(ShotData {
                id: row.id,
                main_fid: row.main_file_id.unwrap_or_default(),
                review_status: row.review_status,
                person_name: row.person_name,
                file_count: row.file_count,
                dhash,
            })
        })
        .collect();

    let mut ignored = std::collections::HashSet::new();
    let ignored_rows: Vec<(String, String)> = ignored_merges::table
        .select((ignored_merges::shot_id_1, ignored_merges::shot_id_2))
        .load(&mut conn)
        .unwrap_or_default();
    for pair in ignored_rows {
        ignored.insert(pair);
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
