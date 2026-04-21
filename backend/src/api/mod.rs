mod comfyui;
mod faces;
mod files;
mod people;
pub mod settings;
mod shots;
mod stats;

use axum::{
    extract::DefaultBodyLimit,
    http::StatusCode,
    middleware::Next,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use crate::db::DbPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
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
        comfyui::comfyui_shot_generations,
        comfyui::comfyui_list_presets,
        comfyui::comfyui_create_preset,
        comfyui::comfyui_update_preset,
        comfyui::comfyui_delete_preset,
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
            comfyui::PresetPayload,
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
    pub pool: DbPool,
    pub scanner: Arc<crate::scanner::Scanner>,
    pub comfyui_url: Option<String>,
    pub library_root: PathBuf,
    pub multi_user: bool,
    pub user_pools: Arc<RwLock<HashMap<String, DbPool>>>,
    pub shutdown_flag: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

/// Per-request state extractor. In multi-user mode, resolves to the per-user
/// AppState (set by the `resolve_user_db` middleware). Falls back to the
/// router-level state in single-user mode.
pub struct UState(pub AppState);

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
            let user_pool = match get_or_create_user_pool(&state, user_sub).await {
                Ok(p) => p,
                Err(status) => return status.into_response(),
            };
            let user_library = state.library_root.join(user_sub);
            let user_scanner =
                Arc::new(state.scanner.with_db_path(user_library.join(".phos.db")));
            AppState {
                pool: user_pool,
                scanner: user_scanner,
                comfyui_url: state.comfyui_url.clone(),
                library_root: user_library,
                multi_user: state.multi_user,
                user_pools: state.user_pools.clone(),
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

async fn get_or_create_user_pool(
    state: &AppState,
    user_sub: &str,
) -> Result<DbPool, StatusCode> {
    // Fast path: read lock
    {
        let pools = state.user_pools.read().await;
        if let Some(pool) = pools.get(user_sub) {
            return Ok(pool.clone());
        }
    }
    // Slow path: write lock
    let mut pools = state.user_pools.write().await;
    if let Some(pool) = pools.get(user_sub) {
        return Ok(pool.clone());
    }
    let user_dir = state.library_root.join(user_sub);
    let db_path = user_dir.join(".phos.db");
    let pool = crate::db::establish_pool(&db_path).map_err(|e| {
        tracing::error!("Failed to create pool for user {}: {}", user_sub, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    crate::db::run_migrations(&pool).map_err(|e| {
        tracing::error!("Failed to run migrations for user {}: {}", user_sub, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    pools.insert(user_sub.to_string(), pool.clone());
    drop(pools);

    // Spawn a ComfyUI enhancement worker for the new user
    if let Some(ref url) = state.comfyui_url {
        tracing::info!("Spawning ComfyUI worker for user {}", user_sub);
        crate::comfyui::spawn_enhancement_worker(
            db_path,
            url.clone(),
            state.shutdown_flag.clone(),
        );
    }
    Ok(pool)
}

/// Recalculate `shots.primary_person_id` for a given shot.
/// Finds the face with the largest bounding box area that has a person_id,
/// and sets that as the shot's primary_person_id.
/// Skips if `review_status == 'confirmed'`.
/// If no faces have a person_id, sets primary_person_id = NULL.
pub(crate) fn recalculate_primary_person(conn: &mut diesel::SqliteConnection, shot_id: &str) -> Result<(), StatusCode> {
    use crate::schema::{faces, files, shots};
    use diesel::prelude::*;

    // Check review_status — skip confirmed shots
    let review_status: Option<String> = shots::table
        .filter(shots::id.eq(shot_id))
        .select(shots::review_status)
        .first(conn)
        .map_err(|e| {
            tracing::error!("Failed to get shot review_status: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if review_status.as_deref() == Some("confirmed") {
        return Ok(());
    }

    // Find the face with the largest bounding box area that has a person_id
    let primary_person: Option<String> = faces::table
        .inner_join(files::table.on(faces::file_id.eq(files::id)))
        .select(faces::person_id.assume_not_null())
        .filter(files::shot_id.eq(shot_id))
        .filter(faces::person_id.is_not_null())
        .order(
            diesel::dsl::sql::<diesel::sql_types::Nullable<diesel::sql_types::Float>>(
                "(faces.box_x2 - faces.box_x1) * (faces.box_y2 - faces.box_y1)",
            )
            .desc(),
        )
        .first::<String>(conn)
        .optional()
        .ok()
        .flatten();

    diesel::update(shots::table.filter(shots::id.eq(shot_id)))
        .set(shots::primary_person_id.eq(primary_person))
        .execute(conn)
        .map_err(|e| {
            tracing::error!("Failed to update shot primary_person_id: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(())
}

/// Delete a person record if they have no remaining faces.
/// Clears primary_person_id on any shots referencing this person.
pub(crate) fn cleanup_orphaned_person(conn: &mut diesel::SqliteConnection, person_id: &str) -> Result<(), StatusCode> {
    use crate::schema::{faces, people, shots};
    use diesel::prelude::*;

    let face_count: i64 = faces::table
        .filter(faces::person_id.eq(person_id))
        .count()
        .get_result(conn)
        .map_err(|e| {
            tracing::error!("Failed to count faces for person {}: {}", person_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if face_count == 0 {
        diesel::update(shots::table.filter(shots::primary_person_id.eq(person_id)))
            .set(shots::primary_person_id.eq(None::<String>))
            .execute(conn)
            .map_err(|e| {
                tracing::error!(
                    "Failed to clear primary_person_id for person {}: {}",
                    person_id,
                    e
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        diesel::delete(people::table.filter(people::id.eq(person_id)))
            .execute(conn)
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
            "/api/shots/{id}",
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
        .route("/api/shots/{id}/similar", get(shots::get_similar_shots))
        .route("/api/shots/{id}/split", post(shots::split_shot))
        .route("/api/shots/batch/confirm", post(shots::batch_confirm))
        .route("/api/shots/batch/reassign", post(shots::batch_reassign))
        // People
        .route(
            "/api/people",
            get(people::get_people).post(people::create_person),
        )
        .route("/api/people/merge", post(people::merge_people))
        .route(
            "/api/people/{id}",
            get(people::get_person_shots)
                .put(people::rename_person)
                .delete(people::delete_person),
        )
        .route("/api/people/{id}/browse", get(people::get_person_browse))
        .route("/api/people/{id}/faces", get(people::get_person_faces))
        // Faces
        .route("/api/faces/{id}/thumbnail", get(faces::get_face_thumbnail))
        .route("/api/faces/{id}/person", put(faces::reassign_face))
        .route(
            "/api/faces/{id}/suggestions",
            get(faces::get_face_suggestions),
        )
        .route("/api/faces/{id}", delete(faces::delete_face))
        // Files
        .route(
            "/api/files/{id}",
            get(files::get_file).delete(files::delete_file),
        )
        .route("/api/files/{id}/thumbnail", get(files::get_file_thumbnail))
        .route("/api/files/{id}/set-original", put(files::set_file_original))
        .route("/api/files/{id}/faces", post(faces::add_manual_face))
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
            "/api/comfyui/workflows/{id}",
            delete(comfyui::comfyui_delete_workflow),
        )
        .route(
            "/api/comfyui/workflows/{id}/presets",
            get(comfyui::comfyui_list_presets).post(comfyui::comfyui_create_preset),
        )
        .route(
            "/api/comfyui/workflows/{workflow_id}/presets/{preset_id}",
            put(comfyui::comfyui_update_preset).delete(comfyui::comfyui_delete_preset),
        )
        .route(
            "/api/comfyui/generations/{shot_id}",
            get(comfyui::comfyui_shot_generations),
        )
        .route("/api/comfyui/enhance", post(comfyui::comfyui_enhance))
        .route("/api/comfyui/tasks", get(comfyui::comfyui_list_tasks))
        .route(
            "/api/comfyui/tasks/{id}",
            get(comfyui::comfyui_get_task).delete(comfyui::comfyui_delete_task),
        )
        .route(
            "/api/comfyui/tasks/{id}/retry",
            post(comfyui::comfyui_retry_task),
        )
        .route("/api/version", get(stats::get_version))
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
