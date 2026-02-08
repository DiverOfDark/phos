use axum::{
    routing::{get, post},
    extract::{Path, State, Query},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use rusqlite::{params, Connection};
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
        .route("/api/people/:id", get(get_person_photos))
        .route("/api/scan", post(trigger_scan))
        .with_state(state)
}

#[derive(Serialize)]
struct PhotoBrief {
    id: String,
    thumbnail_url: String,
    timestamp: Option<String>,
}

async fn get_photos(State(state): State<AppState>) -> Json<Vec<PhotoBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db.prepare("SELECT p.id, f.path FROM photos p JOIN files f ON p.main_file_id = f.id ORDER BY p.timestamp DESC").unwrap();
    let rows = stmt.query_map([], |row| {
        Ok(PhotoBrief {
            id: row.get(0)?,
            thumbnail_url: format!("/api/files/{}", row.get::<_, String>(0)?),
            timestamp: None,
        })
    }).unwrap();

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

async fn get_photo_detail(Path(id): Path<String>, State(state): State<AppState>) -> Json<PhotoDetail> {
    let db = state.db.lock().await;
    
    // Get files for this photo
    let mut stmt = db.prepare("SELECT id, path, mime_type FROM files WHERE photo_id = ?").unwrap();
    let files = stmt.query_map(params![id], |row| {
        Ok(FileDetail {
            id: row.get(0)?,
            path: row.get(1)?,
            mime_type: row.get(2)?,
        })
    }).unwrap().filter_map(|r| r.ok()).collect();

    // Get faces for these files
    let mut stmt = db.prepare("SELECT fa.id, fa.person_id, fa.box_x1, fa.box_y1, fa.box_x2, fa.box_y2 FROM faces fa JOIN files f ON fa.file_id = f.id WHERE f.photo_id = ?").unwrap();
    let faces = stmt.query_map(params![id], |row| {
        Ok(FaceDetail {
            id: row.get(0)?,
            person_id: row.get(1)?,
            box_x1: row.get(2)?,
            box_y1: row.get(3)?,
            box_x2: row.get(4)?,
            box_y2: row.get(5)?,
        })
    }).unwrap().filter_map(|r| r.ok()).collect();

    Json(PhotoDetail { id, files, faces })
}

async fn get_people(State(state): State<AppState>) -> Json<Vec<PersonBrief>> {
    let db = state.db.lock().await;
    let mut stmt = db.prepare("SELECT id, name FROM people").unwrap();
    let rows = stmt.query_map([], |row| {
        Ok(PersonBrief {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    }).unwrap();

    let people: Vec<_> = rows.filter_map(|r| r.ok()).collect();
    Json(people)
}

#[derive(Serialize)]
struct PersonBrief {
    id: String,
    name: Option<String>,
}

async fn get_person_photos(Path(id): Path<String>, State(state): State<AppState>) -> Json<Vec<PhotoBrief>> {
     // Implementation for getting photos where a specific person appears
     Json(vec![])
}

async fn trigger_scan() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "started"}))
}
