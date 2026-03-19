use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use super::UState;
use crate::db;

/// Fixed username used in single-user mode (no OIDC).
const SINGLE_USER_WEBDAV_USERNAME: &str = "phos";

#[derive(Serialize, ToSchema)]
pub struct WebDavSettings {
    pub enabled: bool,
    /// The WebDAV username (OIDC sub in multi-user mode, "phos" in single-user).
    pub username: String,
}

#[derive(Deserialize, ToSchema)]
pub struct WebDavCredentials {
    pub password: String,
}

/// Derive the WebDAV username: in multi-user mode, use the OIDC `sub` claim;
/// in single-user mode, use a fixed value.
fn webdav_username(state: &super::AppState, parts: &axum::http::request::Parts) -> String {
    if state.multi_user {
        parts
            .extensions
            .get::<crate::auth::SessionClaims>()
            .map(|c| c.sub.clone())
            .unwrap_or_else(|| SINGLE_USER_WEBDAV_USERNAME.to_string())
    } else {
        SINGLE_USER_WEBDAV_USERNAME.to_string()
    }
}

#[utoipa::path(
    get,
    path = "/api/settings/webdav",
    responses(
        (status = 200, body = WebDavSettings)
    ),
    tag = "Settings"
)]
pub async fn get_webdav_settings(
    UState(state): UState,
    request: axum::extract::Request,
) -> Result<Json<WebDavSettings>, StatusCode> {
    let (parts, _) = request.into_parts();
    let username = webdav_username(&state, &parts);
    let db = state.db.lock().await;
    let has_password = db::get_setting(&db, "webdav_password").is_some();

    Ok(Json(WebDavSettings {
        enabled: has_password,
        username,
    }))
}

#[utoipa::path(
    put,
    path = "/api/settings/webdav",
    request_body = WebDavCredentials,
    responses(
        (status = 200, description = "Credentials saved")
    ),
    tag = "Settings"
)]
pub async fn set_webdav_settings(
    UState(state): UState,
    Json(creds): Json<WebDavCredentials>,
) -> Result<StatusCode, StatusCode> {
    if creds.password.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let password_hash = format!("{:x}", Sha256::digest(creds.password.as_bytes()));

    let db = state.db.lock().await;
    db::set_setting(&db, "webdav_password", &password_hash)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/api/settings/webdav",
    responses(
        (status = 200, description = "WebDAV disabled")
    ),
    tag = "Settings"
)]
pub async fn delete_webdav_settings(
    UState(state): UState,
) -> Result<StatusCode, StatusCode> {
    let db = state.db.lock().await;
    db::delete_setting(&db, "webdav_password")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}
