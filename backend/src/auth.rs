use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{AppendHeaders, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use openidconnect::{
    core::{CoreClient, CoreIdToken, CoreProviderMetadata, CoreResponseType},
    reqwest::async_http_client,
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const SESSION_COOKIE: &str = "phos_session";
const OIDC_STATE_COOKIE: &str = "phos_oidc_state";

/// JWT claims for the user session cookie.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
    issuer_url: String,
    client_id: String,
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
        issuer_url: issuer_url.to_string(),
        client_id: client_id.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// GET /api/auth/login — redirect to the OIDC provider.
#[utoipa::path(
    get,
    path = "/api/auth/login",
    tag = "auth",
    summary = "Initiate OIDC login",
    description = "Initiates the OIDC login flow by redirecting the user to the configured identity provider. Stores PKCE and CSRF state in a signed cookie for the callback.",
    responses(
        (status = 302, description = "Redirect to OIDC provider"),
    )
)]
pub(crate) async fn login(State(auth): State<AuthState>) -> impl IntoResponse {
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
pub(crate) struct CallbackParams {
    code: Option<String>,
    state: String,
    error: Option<String>,
    error_description: Option<String>,
}

/// GET /api/auth/callback — exchange authorization code for tokens.
#[utoipa::path(
    get,
    path = "/api/auth/callback",
    tag = "auth",
    summary = "Handle OIDC callback",
    description = "Handles the OIDC provider callback after user authentication. Validates the CSRF token, exchanges the authorization code for tokens, verifies the ID token, and issues a session JWT cookie.",
    params(
        ("code" = Option<String>, Query, description = "Authorization code from OIDC provider"),
        ("state" = String, Query, description = "CSRF state token"),
        ("error" = Option<String>, Query, description = "Error code from OIDC provider"),
        ("error_description" = Option<String>, Query, description = "Error description from OIDC provider"),
    ),
    responses(
        (status = 302, description = "Redirect to app after successful auth"),
        (status = 400, description = "Invalid callback parameters"),
        (status = 502, description = "Token exchange or ID token verification failed"),
    )
)]
pub(crate) async fn callback(
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
        .claims(&auth.oidc_client.id_token_verifier().set_other_audience_verifier_fn(|_| true), &nonce)
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
#[utoipa::path(
    get,
    path = "/api/auth/me",
    tag = "auth",
    summary = "Get current user",
    description = "Returns the current authenticated user's session claims (subject, name, email) from the session JWT cookie.",
    responses(
        (status = 200, description = "Current user session", body = SessionClaims),
        (status = 401, description = "Not authenticated"),
    ),
    security(("session_cookie" = []))
)]
pub(crate) async fn me(
    State(auth): State<AuthState>,
    headers: HeaderMap,
) -> Result<Json<SessionClaims>, StatusCode> {
    let claims = parse_session_token(&headers, &auth.jwt_decoding_key)?;
    Ok(Json(claims))
}

/// GET /api/auth/logout — clear the session cookie.
#[utoipa::path(
    get,
    path = "/api/auth/logout",
    tag = "auth",
    summary = "Log out",
    description = "Clears the session cookie and redirects the user to the login page.",
    responses(
        (status = 302, description = "Redirect to login page after clearing session"),
    )
)]
pub(crate) async fn logout() -> impl IntoResponse {
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
    let claims = parse_session_token(&headers, &auth.jwt_decoding_key)?;
    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

// ---------------------------------------------------------------------------
// Auth config — returns OIDC configuration so mobile clients only need the server URL
// ---------------------------------------------------------------------------

/// Response for the auth config endpoint.
#[derive(Serialize, ToSchema)]
pub(crate) struct AuthConfigResponse {
    issuer: String,
    client_id: String,
    scopes: Vec<String>,
}

/// GET /api/auth/config — return OIDC configuration for mobile clients.
#[utoipa::path(
    get,
    path = "/api/auth/config",
    tag = "auth",
    summary = "Get OIDC configuration",
    description = "Returns the OIDC issuer URL, client ID, and scopes so mobile clients only need the Phos server URL to self-configure authentication.",
    responses(
        (status = 200, description = "OIDC configuration", body = AuthConfigResponse),
    )
)]
pub(crate) async fn auth_config(
    State(auth): State<AuthState>,
) -> Json<AuthConfigResponse> {
    Json(AuthConfigResponse {
        issuer: auth.issuer_url.clone(),
        client_id: auth.client_id.clone(),
        scopes: auth.scopes.clone(),
    })
}

// ---------------------------------------------------------------------------
// Token exchange — mobile clients exchange an OIDC ID token for a session JWT
// ---------------------------------------------------------------------------

#[derive(Deserialize, ToSchema)]
pub(crate) struct TokenExchangeRequest {
    id_token: String,
}

#[utoipa::path(
    post,
    path = "/api/auth/token",
    tag = "auth",
    summary = "Exchange OIDC ID token for session JWT",
    description = "Mobile clients perform the OIDC Authorization Code + PKCE flow directly with the identity provider, then send the resulting ID token here. Phos validates it against the provider's JWKS and returns a session JWT that can be used in the `Authorization: Bearer` header.",
    request_body(content = TokenExchangeRequest, description = "OIDC ID token to exchange"),
    responses(
        (status = 200, description = "Session JWT token", body = serde_json::Value),
        (status = 400, description = "Invalid ID token format"),
        (status = 401, description = "ID token verification failed"),
    )
)]
pub(crate) async fn token_exchange(
    State(auth): State<AuthState>,
    Json(payload): Json<TokenExchangeRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Parse the raw ID token string into a CoreIdToken
    let id_token: CoreIdToken = serde_json::from_value(serde_json::Value::String(
        payload.id_token,
    ))
    .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Verify the token against the OIDC provider's JWKS.
    // We skip nonce verification because the mobile client handled the nonce
    // during its own authorization flow.
    let verifier = auth
        .oidc_client
        .id_token_verifier()
        .set_other_audience_verifier_fn(|_| true);
    let claims = id_token
        .claims(&verifier, |_: Option<&Nonce>| Ok(()))
        .map_err(|e| {
            tracing::error!("ID token verification failed: {}", e);
            StatusCode::UNAUTHORIZED
        })?;

    // Build a Phos session JWT with the same structure as the web flow.
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
    let token = encode(&Header::default(), &session, &auth.jwt_encoding_key)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "token": token,
        "expires_in": auth.jwt_ttl_secs
    })))
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
        .route("/api/auth/token", post(token_exchange))
        .route("/api/auth/config", get(auth_config))
        .with_state(auth)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_session_token(
    headers: &HeaderMap,
    key: &DecodingKey,
) -> Result<SessionClaims, StatusCode> {
    // Try Authorization: Bearer header first, then fall back to session cookie
    let token = if let Some(auth_header) = headers.get(header::AUTHORIZATION) {
        let value = auth_header
            .to_str()
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        value
            .strip_prefix("Bearer ")
            .ok_or(StatusCode::UNAUTHORIZED)?
            .to_string()
    } else {
        get_cookie_value(headers, SESSION_COOKIE).ok_or(StatusCode::UNAUTHORIZED)?
    };
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
