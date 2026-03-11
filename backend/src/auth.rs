use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{AppendHeaders, IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use openidconnect::{
    core::{CoreClient, CoreProviderMetadata, CoreResponseType},
    reqwest::async_http_client,
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};

const SESSION_COOKIE: &str = "phos_session";
const OIDC_STATE_COOKIE: &str = "phos_oidc_state";

/// JWT claims for the user session cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    pub sub: String,
    pub name: String,
    pub email: String,
    pub exp: usize,
    pub iat: usize,
}

/// JWT claims stored in the short-lived OIDC state cookie (login → callback).
#[derive(Serialize, Deserialize)]
struct OidcStateClaims {
    csrf_token: String,
    nonce: String,
    pkce_verifier: String,
    exp: usize,
}

/// Shared state for auth route handlers and the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    oidc_client: CoreClient,
    jwt_encoding_key: EncodingKey,
    jwt_decoding_key: DecodingKey,
    jwt_ttl_secs: u64,
    scopes: Vec<String>,
}

/// Discover the OIDC provider and build the auth state.
pub async fn init_oidc(
    issuer_url: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    jwt_secret: &str,
    jwt_ttl_secs: u64,
    scopes: Vec<String>,
) -> anyhow::Result<AuthState> {
    let provider_metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new(issuer_url.to_string())?,
        async_http_client,
    )
    .await
    .map_err(|e| anyhow::anyhow!("OIDC discovery failed: {}", e))?;

    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.to_string()),
        Some(ClientSecret::new(client_secret.to_string())),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string())?);

    Ok(AuthState {
        oidc_client: client,
        jwt_encoding_key: EncodingKey::from_secret(jwt_secret.as_bytes()),
        jwt_decoding_key: DecodingKey::from_secret(jwt_secret.as_bytes()),
        jwt_ttl_secs,
        scopes,
    })
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// GET /api/auth/login — redirect to the OIDC provider.
async fn login(State(auth): State<AuthState>) -> impl IntoResponse {
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let mut req = auth.oidc_client.authorize_url(
        AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );
    req = req.set_pkce_challenge(pkce_challenge);
    for scope in &auth.scopes {
        req = req.add_scope(Scope::new(scope.clone()));
    }
    let (auth_url, csrf_token, nonce) = req.url();

    // Store flow state in a signed, short-lived JWT cookie.
    let now = chrono::Utc::now().timestamp() as usize;
    let state_claims = OidcStateClaims {
        csrf_token: csrf_token.secret().clone(),
        nonce: nonce.secret().clone(),
        pkce_verifier: pkce_verifier.secret().clone(),
        exp: now + 600, // 10 minutes
    };
    let state_jwt = encode(&Header::default(), &state_claims, &auth.jwt_encoding_key)
        .expect("JWT encode should not fail");

    let cookie =
        format!("{OIDC_STATE_COOKIE}={state_jwt}; HttpOnly; SameSite=Lax; Path=/; Max-Age=600");

    (
        AppendHeaders([(header::SET_COOKIE, cookie)]),
        Redirect::to(auth_url.as_str()),
    )
}

/// Query parameters returned by the OIDC provider on callback.
#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: String,
    error: Option<String>,
    error_description: Option<String>,
}

/// GET /api/auth/callback — exchange authorization code for tokens.
async fn callback(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Result<impl IntoResponse, StatusCode> {
    // Handle provider-side errors.
    if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or_default();
        tracing::warn!("OIDC provider returned error: {} — {}", err, desc);
        let location = format!("/login?error={}", urlencoding::encode(&desc));
        return Ok((
            AppendHeaders(vec![(header::SET_COOKIE, clear_cookie(OIDC_STATE_COOKIE))]),
            Redirect::to(&location),
        ));
    }

    let code = params.code.ok_or(StatusCode::BAD_REQUEST)?;

    // Recover flow state from the signed cookie.
    let state_jwt = get_cookie_value(&headers, OIDC_STATE_COOKIE).ok_or(StatusCode::BAD_REQUEST)?;
    let state_claims: OidcStateClaims = decode(
        &state_jwt,
        &auth.jwt_decoding_key,
        &Validation::new(Algorithm::HS256),
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?
    .claims;

    // Verify CSRF token.
    if params.state != state_claims.csrf_token {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Exchange the authorization code for tokens.
    let token_response = auth
        .oidc_client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(PkceCodeVerifier::new(state_claims.pkce_verifier))
        .request_async(async_http_client)
        .await
        .map_err(|e| {
            tracing::error!("Token exchange failed: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    // Validate the ID token.
    let id_token = token_response.id_token().ok_or(StatusCode::BAD_GATEWAY)?;
    let nonce = Nonce::new(state_claims.nonce);
    let claims = id_token
        .claims(&auth.oidc_client.id_token_verifier(), &nonce)
        .map_err(|e| {
            tracing::error!("ID token verification failed: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    // Build our session JWT.
    let now = chrono::Utc::now().timestamp() as usize;
    let session = SessionClaims {
        sub: claims.subject().to_string(),
        name: claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.to_string())
            .unwrap_or_default(),
        email: claims.email().map(|e| e.to_string()).unwrap_or_default(),
        exp: now + auth.jwt_ttl_secs as usize,
        iat: now,
    };
    let session_jwt = encode(&Header::default(), &session, &auth.jwt_encoding_key)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let session_cookie = format!(
        "{SESSION_COOKIE}={session_jwt}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        auth.jwt_ttl_secs
    );

    Ok((
        AppendHeaders(vec![
            (header::SET_COOKIE, session_cookie),
            (header::SET_COOKIE, clear_cookie(OIDC_STATE_COOKIE)),
        ]),
        Redirect::to("/"),
    ))
}

/// GET /api/auth/me — return the current user's claims.
async fn me(
    State(auth): State<AuthState>,
    headers: HeaderMap,
) -> Result<Json<SessionClaims>, StatusCode> {
    let claims = parse_session_cookie(&headers, &auth.jwt_decoding_key)?;
    Ok(Json(claims))
}

/// GET /api/auth/logout — clear the session cookie.
async fn logout() -> impl IntoResponse {
    (
        AppendHeaders([(header::SET_COOKIE, clear_cookie(SESSION_COOKIE))]),
        Redirect::to("/login"),
    )
}

// ---------------------------------------------------------------------------
// Auth middleware — applied to all /api/* routes except /api/auth/*
// ---------------------------------------------------------------------------

pub async fn require_auth(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    mut request: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let claims = parse_session_cookie(&headers, &auth.jwt_decoding_key)?;
    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_auth_router(auth: AuthState) -> Router {
    Router::new()
        .route("/api/auth/login", get(login))
        .route("/api/auth/callback", get(callback))
        .route("/api/auth/me", get(me))
        .route("/api/auth/logout", get(logout))
        .with_state(auth)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_session_cookie(
    headers: &HeaderMap,
    key: &DecodingKey,
) -> Result<SessionClaims, StatusCode> {
    let token = get_cookie_value(headers, SESSION_COOKIE).ok_or(StatusCode::UNAUTHORIZED)?;
    let data = decode::<SessionClaims>(&token, key, &Validation::new(Algorithm::HS256))
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    Ok(data.claims)
}

fn get_cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{name}=");
    header
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with(&prefix))?
        .strip_prefix(&prefix)
        .map(|s| s.to_string())
}

fn clear_cookie(name: &str) -> String {
    format!("{name}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}
