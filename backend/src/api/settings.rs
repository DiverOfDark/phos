use axum::http::StatusCode;
use axum::Json;
use base64::Engine;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use super::UState;
use crate::schema::settings;

/// Fixed username / S3 access key used in single-user mode (no OIDC).
const SINGLE_USER_USERNAME: &str = "phos";

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

/// Derive the WebDAV username / S3 access key: in multi-user mode, use the
/// OIDC `sub` claim; in single-user mode, use a fixed value.
fn share_username(state: &super::AppState, parts: &axum::http::request::Parts) -> String {
    if state.multi_user {
        parts
            .extensions
            .get::<crate::auth::SessionClaims>()
            .map(|c| c.sub.clone())
            .unwrap_or_else(|| SINGLE_USER_USERNAME.to_string())
    } else {
        SINGLE_USER_USERNAME.to_string()
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
    let username = share_username(&state, &parts);
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let has_password: bool = settings::table
        .filter(settings::key.eq("webdav_password"))
        .select(settings::value)
        .first::<String>(&mut conn)
        .is_ok();

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

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    diesel::insert_into(settings::table)
        .values((settings::key.eq("webdav_password"), settings::value.eq(&password_hash)))
        .on_conflict(settings::key)
        .do_update()
        .set(settings::value.eq(&password_hash))
        .execute(&mut conn)
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
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    diesel::delete(settings::table.filter(settings::key.eq("webdav_password")))
        .execute(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

#[derive(Serialize, ToSchema)]
pub struct S3Settings {
    pub enabled: bool,
    /// The S3 access key id (OIDC sub in multi-user mode, "phos" in single-user).
    pub access_key: String,
    /// The fixed bucket name the library is exposed as.
    pub bucket: String,
    /// External endpoint URL override (PHOS_S3_PUBLIC_URL), if configured.
    pub endpoint: Option<String>,
    /// The secret key. Stored in plaintext server-side because SigV4 signing
    /// requires the real secret; only returned to the authenticated owner.
    pub secret_key: Option<String>,
}

fn s3_settings_response(
    state: &super::AppState,
    parts: &axum::http::request::Parts,
    secret_key: Option<String>,
) -> S3Settings {
    S3Settings {
        enabled: secret_key.is_some(),
        access_key: share_username(state, parts),
        bucket: crate::s3::BUCKET_NAME.to_string(),
        endpoint: std::env::var("PHOS_S3_PUBLIC_URL").ok(),
        secret_key,
    }
}

#[utoipa::path(
    get,
    path = "/api/settings/s3",
    responses(
        (status = 200, body = S3Settings)
    ),
    tag = "Settings"
)]
pub async fn get_s3_settings(
    UState(state): UState,
    request: axum::extract::Request,
) -> Result<Json<S3Settings>, StatusCode> {
    let (parts, _) = request.into_parts();
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let secret_key: Option<String> = settings::table
        .filter(settings::key.eq(crate::s3::SECRET_KEY_SETTING))
        .select(settings::value)
        .first::<String>(&mut conn)
        .ok();

    Ok(Json(s3_settings_response(&state, &parts, secret_key)))
}

/// Generate a fresh random secret key. 30 random bytes → 40 chars base64url.
fn generate_s3_secret() -> String {
    let mut bytes = Vec::with_capacity(32);
    while bytes.len() < 30 {
        // UUID v4 uses the platform's CSRNG for 122 bits of randomness per call.
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes.truncate(30);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[utoipa::path(
    post,
    path = "/api/settings/s3",
    responses(
        (status = 200, description = "Credentials generated", body = S3Settings)
    ),
    tag = "Settings"
)]
pub async fn generate_s3_settings(
    UState(state): UState,
    request: axum::extract::Request,
) -> Result<Json<S3Settings>, StatusCode> {
    let (parts, _) = request.into_parts();
    let secret = generate_s3_secret();

    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    diesel::insert_into(settings::table)
        .values((settings::key.eq(crate::s3::SECRET_KEY_SETTING), settings::value.eq(&secret)))
        .on_conflict(settings::key)
        .do_update()
        .set(settings::value.eq(&secret))
        .execute(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(s3_settings_response(&state, &parts, Some(secret))))
}

#[utoipa::path(
    delete,
    path = "/api/settings/s3",
    responses(
        (status = 200, description = "S3 access disabled")
    ),
    tag = "Settings"
)]
pub async fn delete_s3_settings(UState(state): UState) -> Result<StatusCode, StatusCode> {
    let mut conn = state.pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    diesel::delete(settings::table.filter(settings::key.eq(crate::s3::SECRET_KEY_SETTING)))
        .execute(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}
