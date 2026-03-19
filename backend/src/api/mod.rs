mod comfyui;
mod faces;
mod files;
mod people;
pub mod settings;
mod shots;
mod stats;
mod sync;

use axum::{
    extract::DefaultBodyLimit,
    http::StatusCode,
    middleware::Next,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Phos API",
        description = "AI-powered photo/video manager",
        version = env!("PHOS_VERSION"),
        license(name = "MIT"),
    ),
    paths(
        // Auth
        crate::auth::login,
        crate::auth::callback,
        crate::auth::me,
        crate::auth::logout,
        crate::auth::token_exchange,
        crate::auth::auth_config,
        // Shots
        shots::get_shots,
        shots::get_shot_detail,
        shots::delete_shot,
        shots::update_shot,
        shots::split_shot,
        shots::get_similar_shots,
        shots::merge_shots,
        shots::ignore_merge,
        shots::batch_confirm,
        shots::batch_reassign,
        shots::get_similar_shot_groups,
        // People
        people::get_people,
        people::create_person,
        people::get_person_shots,
        people::get_person_faces,
        people::rename_person,
        people::merge_people,
        people::delete_person,
        people::get_person_browse,
        // Faces
        faces::get_face_thumbnail,
        faces::reassign_face,
        faces::get_face_suggestions,
        faces::delete_face,
        faces::add_manual_face,
        // Files
        files::get_file,
        files::delete_file,
        files::get_file_thumbnail,
        files::set_file_original,
        files::upload_file_raw,
        files::finalize_import,
        // System
        stats::get_stats,
        stats::get_organize_stats,
        stats::trigger_reorganize,
        stats::trigger_scan,
        stats::get_version,
        // ComfyUI
        comfyui::comfyui_health,
        comfyui::comfyui_list_workflows,
        comfyui::comfyui_import_workflow,
        comfyui::comfyui_delete_workflow,
        comfyui::comfyui_enhance,
        comfyui::comfyui_list_tasks,
        comfyui::comfyui_get_task,
        comfyui::comfyui_retry_task,
        comfyui::comfyui_delete_task,
        // Sync
        sync::get_sync,
        // Settings
        settings::get_webdav_settings,
        settings::set_webdav_settings,
        settings::delete_webdav_settings,
    ),
    components(
        schemas(
            // Auth
            crate::auth::SessionClaims,
            crate::auth::TokenExchangeRequest,
            crate::auth::AuthConfigResponse,
            // Shots
            shots::ShotBrief,
            shots::SimilarShotItem,
            shots::SimilarShotsGrouped,
            shots::ShotDetailResponse,
            shots::FileDetail,
            shots::FaceDetail,
            shots::AlsoContainsPerson,
            shots::UpdateShotPayload,
            shots::SplitShotPayload,
            shots::MergeShotsPayload,
            shots::BatchConfirmPayload,
            shots::BatchReassignPayload,
            shots::IgnoreMergePayload,
            shots::SimilarShotGroup,
            shots::SimilarGroupsResponse,
            // People
            people::PersonBrief,
            people::PersonFaceBrief,
            people::CreatePersonPayload,
            people::RenamePersonPayload,
            people::MergePeoplePayload,
            people::PersonMeta,
            people::BrowseFileDetail,
            people::BrowseShotDetail,
            people::PersonBrowseResponse,
            // Faces
            faces::FaceSuggestion,
            faces::ReassignFacePayload,
            faces::AddManualFacePayload,
            // Stats
            stats::StatsResponse,
            stats::OrganizeStatsResponse,
            stats::ScanParams,
            // ComfyUI
            comfyui::ImportWorkflowPayload,
            comfyui::EnhancePayload,
            // Sync
            sync::SyncResponse,
            sync::SyncPerson,
            sync::SyncShot,
            sync::SyncFile,
            sync::SyncQuery,
            // Settings
            settings::WebDavSettings,
            settings::WebDavCredentials,
        )
    ),
    modifiers(&SecurityAddon),
    security(("session_cookie" = [])),
)]
pub struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session_cookie",
            utoipa::openapi::security::SecurityScheme::Http(
                utoipa::openapi::security::HttpBuilder::new()
                    .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some(
                        "Session JWT set as HttpOnly cookie (phos_session) after OIDC login",
                    ))
                    .build(),
            ),
        );
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub scanner: Arc<crate::scanner::Scanner>,
    pub comfyui_url: Option<String>,
    pub library_root: PathBuf,
    pub multi_user: bool,
    pub user_dbs: Arc<RwLock<HashMap<String, Arc<Mutex<Connection>>>>>,
    pub shutdown_flag: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

/// Per-request state extractor. In multi-user mode, resolves to the per-user
/// AppState (set by the `resolve_user_db` middleware). Falls back to the
/// router-level state in single-user mode.
pub struct UState(pub AppState);

#[axum::async_trait]
impl axum::extract::FromRequestParts<AppState> for UState {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(user_state) = parts.extensions.get::<AppState>() {
            Ok(UState(user_state.clone()))
        } else {
            Ok(UState(state.clone()))
        }
    }
}

/// Middleware that resolves the per-user database and library path.
/// In multi-user mode (OIDC enabled), creates a per-user AppState with
/// the user's own DB connection and library subfolder.
pub async fn resolve_user_db(
    UState(state): UState,
    mut request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let user_state = if state.multi_user {
        if let Some(claims) = request
            .extensions()
            .get::<crate::auth::SessionClaims>()
            .cloned()
        {
            let user_sub = &claims.sub;
            let db = match get_or_create_user_db(&state, user_sub).await {
                Ok(db) => db,
                Err(status) => return status.into_response(),
            };
            let user_library = state.library_root.join(user_sub);
            let user_scanner =
                Arc::new(state.scanner.with_db_path(user_library.join(".phos.db")));
            AppState {
                db,
                scanner: user_scanner,
                comfyui_url: state.comfyui_url.clone(),
                library_root: user_library,
                multi_user: state.multi_user,
                user_dbs: state.user_dbs.clone(),
                shutdown_flag: state.shutdown_flag.clone(),
            }
        } else {
            state
        }
    } else {
        state
    };
    request.extensions_mut().insert(user_state);
    next.run(request).await
}

async fn get_or_create_user_db(
    state: &AppState,
    user_sub: &str,
) -> Result<Arc<Mutex<Connection>>, StatusCode> {
    // Fast path: read lock
    {
        let dbs = state.user_dbs.read().await;
        if let Some(db) = dbs.get(user_sub) {
            return Ok(db.clone());
        }
    }
    // Slow path: write lock (re-check to avoid TOCTOU race)
    let mut dbs = state.user_dbs.write().await;
    if let Some(db) = dbs.get(user_sub) {
        return Ok(db.clone());
    }
    // Create under the write lock so no two requests init the same user
    let user_dir = state.library_root.join(user_sub);
    std::fs::create_dir_all(&user_dir).map_err(|e| {
        tracing::error!("Failed to create user directory {:?}: {}", user_dir, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let db_path = user_dir.join(".phos.db");
    let conn = crate::db::init_db(&db_path).map_err(|e| {
        tracing::error!("Failed to initialize database for user {}: {}", user_sub, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let shared = Arc::new(Mutex::new(conn));
    dbs.insert(user_sub.to_string(), shared.clone());
    drop(dbs);

    // Spawn a ComfyUI enhancement worker for the new user
    if let Some(ref url) = state.comfyui_url {
        tracing::info!("Spawning ComfyUI worker for user {}", user_sub);
        crate::comfyui::spawn_enhancement_worker(
            db_path,
            url.clone(),
            state.shutdown_flag.clone(),
        );
    }
    Ok(shared)
}

/// Recalculate `shots.primary_person_id` for a given shot.
/// Finds the face with the largest bounding box area that has a person_id,
/// and sets that as the shot's primary_person_id.
/// Skips if `review_status == 'confirmed'`.
/// If no faces have a person_id, sets primary_person_id = NULL.
pub(crate) fn recalculate_primary_person(db: &Connection, shot_id: &str) -> Result<(), StatusCode> {
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

/// Delete a person record if they have no remaining faces.
/// Clears primary_person_id on any shots referencing this person.
pub(crate) fn cleanup_orphaned_person(db: &Connection, person_id: &str) -> Result<(), StatusCode> {
    let face_count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM faces WHERE person_id = ?",
            params![person_id],
            |row| row.get(0),
        )
        .map_err(|e| {
            tracing::error!("Failed to count faces for person {}: {}", person_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if face_count == 0 {
        db.execute(
            "UPDATE shots SET primary_person_id = NULL WHERE primary_person_id = ?",
            params![person_id],
        )
        .map_err(|e| {
            tracing::error!(
                "Failed to clear primary_person_id for person {}: {}",
                person_id,
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        db.execute("DELETE FROM people WHERE id = ?", params![person_id])
            .map_err(|e| {
                tracing::error!("Failed to delete orphaned person {}: {}", person_id, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        tracing::info!("Deleted orphaned person {}", person_id);
    }

    Ok(())
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Shot CRUD
        .route("/api/shots", get(shots::get_shots))
        .route(
            "/api/shots/:id",
            get(shots::get_shot_detail)
                .put(shots::update_shot)
                .delete(shots::delete_shot),
        )
        // Shot operations
        .route(
            "/api/shots/similar-groups",
            get(shots::get_similar_shot_groups),
        )
        .route("/api/shots/merge", post(shots::merge_shots))
        .route("/api/shots/merge/ignore", post(shots::ignore_merge))
        .route("/api/shots/:id/similar", get(shots::get_similar_shots))
        .route("/api/shots/:id/split", post(shots::split_shot))
        .route("/api/shots/batch/confirm", post(shots::batch_confirm))
        .route("/api/shots/batch/reassign", post(shots::batch_reassign))
        // People
        .route(
            "/api/people",
            get(people::get_people).post(people::create_person),
        )
        .route("/api/people/merge", post(people::merge_people))
        .route(
            "/api/people/:id",
            get(people::get_person_shots)
                .put(people::rename_person)
                .delete(people::delete_person),
        )
        .route("/api/people/:id/browse", get(people::get_person_browse))
        .route("/api/people/:id/faces", get(people::get_person_faces))
        // Faces
        .route("/api/faces/:id/thumbnail", get(faces::get_face_thumbnail))
        .route("/api/faces/:id/person", put(faces::reassign_face))
        .route(
            "/api/faces/:id/suggestions",
            get(faces::get_face_suggestions),
        )
        .route("/api/faces/:id", delete(faces::delete_face))
        // Files
        .route(
            "/api/files/:id",
            get(files::get_file).delete(files::delete_file),
        )
        .route("/api/files/:id/thumbnail", get(files::get_file_thumbnail))
        .route("/api/files/:id/set-original", put(files::set_file_original))
        .route("/api/files/:id/faces", post(faces::add_manual_face))
        // Stats + organize
        .route("/api/stats", get(stats::get_stats))
        .route("/api/organize/stats", get(stats::get_organize_stats))
        .route("/api/reorganize", post(stats::trigger_reorganize))
        // Scan + import
        .route("/api/scan", post(stats::trigger_scan))
        .route(
            "/api/import/upload",
            put(files::upload_file_raw).layer(DefaultBodyLimit::max(1024 * 1024 * 1024)), // 1 GB
        )
        .route("/api/import/finalize", post(files::finalize_import))
        // ComfyUI integration
        .route("/api/comfyui/health", get(comfyui::comfyui_health))
        .route(
            "/api/comfyui/workflows",
            get(comfyui::comfyui_list_workflows).post(comfyui::comfyui_import_workflow),
        )
        .route(
            "/api/comfyui/workflows/:id",
            delete(comfyui::comfyui_delete_workflow),
        )
        .route("/api/comfyui/enhance", post(comfyui::comfyui_enhance))
        .route("/api/comfyui/tasks", get(comfyui::comfyui_list_tasks))
        .route(
            "/api/comfyui/tasks/:id",
            get(comfyui::comfyui_get_task).delete(comfyui::comfyui_delete_task),
        )
        .route(
            "/api/comfyui/tasks/:id/retry",
            post(comfyui::comfyui_retry_task),
        )
        .route("/api/version", get(stats::get_version))
        // Sync
        .route("/api/sync", get(sync::get_sync))
        // Settings
        .route(
            "/api/settings/webdav",
            get(settings::get_webdav_settings)
                .put(settings::set_webdav_settings)
                .delete(settings::delete_webdav_settings),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            resolve_user_db,
        ))
        .with_state(state)
}
