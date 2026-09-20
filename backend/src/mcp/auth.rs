//! Who is allowed to talk to `/mcp`.
//!
//! A static bearer token, generated in Settings and rotatable there, the way
//! the S3 and WebDAV credentials already work. The token goes in the
//! `Authorization` header, never in the URL: a secret in a path is written to
//! every access log, proxy log and `RUST_LOG=debug` line the request passes
//! through, and cannot be rotated without reconfiguring every client that holds
//! the endpoint. A header costs the same and loses none of the convenience,
//! because the Settings page hands over the whole `claude mcp add …` command.
//!
//! Only the SHA-256 of the token is stored, so a stolen database is not a
//! stolen token, and the comparison is constant-time.
//!
//! **Multi-user.** The token has to name its owner, because this middleware
//! runs before [`crate::api::resolve_user_db`] and there is no session to read
//! the user from — and scanning every user's database for a matching hash would
//! be both slow and a way to probe for users. So a multi-user token is
//! `phos_mcp_<base64url(sub)>.<secret>`: the prefix says whose library to open,
//! the secret proves the right to open it. An unknown `sub` is rejected without
//! creating anything, so a made-up prefix cannot conjure a library.
//!
//! This is the pragmatic half of the story. The spec's answer for a hosted
//! server is OAuth: 401 with `WWW-Authenticate: Bearer resource_metadata=…`,
//! `/.well-known/oauth-protected-resource`, and the client running PKCE against
//! the OIDC issuer Phos already knows. That is worth doing for a multi-user
//! deployment and is not done here; the `WWW-Authenticate` header below is the
//! hook it will hang from.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use diesel::prelude::*;
use sha2::{Digest, Sha256};

use crate::api::AppState;
use crate::db::DbPool;
use crate::schema::settings;

/// Settings key holding the SHA-256 (hex) of this library's MCP token.
pub const TOKEN_HASH_SETTING: &str = "mcp_token_hash";

/// Marks a token as ours, so a session JWT and an MCP token are never confused.
const TOKEN_PREFIX: &str = "phos_mcp_";

/// Mint a token for `sub` (`None` in single-user mode).
///
/// 30 random bytes → 40 base64url characters, the same strength and shape as
/// the S3 secret key next to it.
pub fn generate_token(sub: Option<&str>) -> String {
    let mut bytes = Vec::with_capacity(32);
    while bytes.len() < 30 {
        // UUID v4 draws 122 bits from the platform CSRNG per call.
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes.truncate(30);
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);

    match sub {
        Some(sub) => {
            let owner = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sub.as_bytes());
            format!("{TOKEN_PREFIX}{owner}.{secret}")
        }
        None => format!("{TOKEN_PREFIX}{secret}"),
    }
}

/// The hex SHA-256 stored for a token. The whole token is hashed, owner prefix
/// included, so a prefix swapped onto a valid secret does not verify.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Whose library a token claims to be for, if it says.
fn token_owner(token: &str) -> Option<String> {
    let body = token.strip_prefix(TOKEN_PREFIX)?;
    let (owner_b64, _) = body.split_once('.')?;
    let owner = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(owner_b64)
        .ok()?;
    String::from_utf8(owner).ok()
}

/// Compare two hex digests without leaking where they first differ.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Read the token a request presents, if any.
///
/// RFC 7235 makes the auth scheme case-insensitive, and clients do send
/// `bearer`, so the scheme is split off and compared without regard to case
/// rather than matched as a literal prefix.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim_start())
        .filter(|token| !token.is_empty())
}

fn unauthorized(detail: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"phos-mcp\"")],
        detail.to_string(),
    )
        .into_response()
}

/// Does `pool`'s library accept `token`?
fn token_matches(pool: &DbPool, token: &str) -> bool {
    let Ok(mut conn) = pool.get() else {
        return false;
    };
    let stored: Option<String> = settings::table
        .filter(settings::key.eq(TOKEN_HASH_SETTING))
        .select(settings::value)
        .first::<String>(&mut conn)
        .ok();

    stored.is_some_and(|stored| constant_time_eq(&stored, &hash_token(token)))
}

/// Gate `/mcp` on a valid token, and say which library the rest of the stack
/// should serve.
///
/// On success in multi-user mode this inserts the [`crate::auth::SessionClaims`]
/// that [`crate::api::resolve_user_db`] looks for, so everything downstream —
/// the pool, the library root, the per-user settings — behaves exactly as it
/// does for a logged-in browser.
pub async fn require_mcp_token(
    State(state): State<AppState>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(token) = bearer(request.headers()).map(str::to_owned) else {
        return unauthorized(
            "MCP access needs an Authorization: Bearer header. Generate a token in Phos \
             Settings → MCP.",
        );
    };
    if !token.starts_with(TOKEN_PREFIX) {
        return unauthorized("That is not a Phos MCP token.");
    }

    let pool = if state.multi_user {
        let Some(sub) = token_owner(&token) else {
            return unauthorized("This server is multi-user; its MCP tokens name their owner.");
        };
        // Only an existing library, never a created one: `get_or_create_user_pool`
        // would happily make a database for an invented `sub`, which turns a
        // bad token into a way to litter the library root.
        if !state.library_root.join(&sub).join(".phos.db").exists() {
            return unauthorized("Unknown token.");
        }
        match crate::api::get_or_create_user_pool(&state, &sub).await {
            Ok(pool) => {
                request.extensions_mut().insert(crate::auth::SessionClaims {
                    sub: sub.clone(),
                    name: sub.clone(),
                    email: String::new(),
                    exp: 0,
                    iat: 0,
                });
                pool
            }
            Err(status) => return status.into_response(),
        }
    } else {
        state.pool.clone()
    };

    let token_for_check = token.clone();
    let matched = tokio::task::spawn_blocking(move || token_matches(&pool, &token_for_check)).await;

    match matched {
        Ok(true) => next.run(request).await,
        Ok(false) => unauthorized("Unknown token."),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_user_token_has_no_owner() {
        let token = generate_token(None);
        assert!(token.starts_with(TOKEN_PREFIX));
        assert_eq!(token_owner(&token), None);
    }

    #[test]
    fn multi_user_token_round_trips_its_owner() {
        // A `sub` is whatever the identity provider says it is, so it must
        // survive characters that are not URL-safe on their own.
        let sub = "auth0|abc/def+ghi";
        let token = generate_token(Some(sub));
        assert_eq!(token_owner(&token).as_deref(), Some(sub));
    }

    #[test]
    fn two_tokens_are_never_the_same() {
        assert_ne!(generate_token(None), generate_token(None));
    }

    #[test]
    fn the_whole_token_is_hashed_not_just_the_secret() {
        // Otherwise a valid secret with someone else's owner prefix would verify
        // against that someone else's library.
        let mine = generate_token(Some("alice"));
        let secret = mine.rsplit_once('.').unwrap().1;
        let forged = format!("{TOKEN_PREFIX}{}.{secret}", {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("bob")
        });
        assert_ne!(hash_token(&mine), hash_token(&forged));
    }

    #[test]
    fn constant_time_eq_still_compares() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn bearer_is_read_from_the_header_only() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);
        headers.insert(header::AUTHORIZATION, "Bearer phos_mcp_x".parse().unwrap());
        assert_eq!(bearer(&headers), Some("phos_mcp_x"));
        headers.insert(header::AUTHORIZATION, "Basic phos_mcp_x".parse().unwrap());
        assert_eq!(bearer(&headers), None);
    }

    #[test]
    fn the_bearer_scheme_is_case_insensitive() {
        // RFC 7235: the scheme is a case-insensitive token, and clients do send
        // `bearer`. Rejecting those is a login failure with no explanation.
        for header_value in [
            "bearer phos_mcp_x",
            "BEARER phos_mcp_x",
            "BeArEr phos_mcp_x",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, header_value.parse().unwrap());
            assert_eq!(bearer(&headers), Some("phos_mcp_x"), "{header_value}");
        }

        // A scheme with nothing after it is not a token.
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer  ".parse().unwrap());
        assert_eq!(bearer(&headers), None);
    }
}
