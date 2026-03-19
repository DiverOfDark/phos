use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use super::UState;
use crate::db;

#[derive(Serialize, ToSchema)]
pub struct WebDavSettings {
    pub enabled: bool,
    pub username: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct WebDavCredentials {
    pub username: String,
    pub password: String,
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
) -> Result<Json<WebDavSettings>, StatusCode> {
    let db = state.db.lock().await;
    let username = db::get_setting(&db, "webdav_username");
    let has_password = db::get_setting(&db, "webdav_password").is_some();

    Ok(Json(WebDavSettings {
        enabled: username.is_some() && has_password,
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
    if creds.username.is_empty() || creds.password.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let password_hash = format!("{:x}", Sha256::digest(creds.password.as_bytes()));

    let db = state.db.lock().await;
    db::set_setting(&db, "webdav_username", &creds.username)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    db::delete_setting(&db, "webdav_username")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    db::delete_setting(&db, "webdav_password")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}
