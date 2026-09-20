//! `/mcp` over Streamable HTTP.
//!
//! Mounted as its own router rather than inside `create_router`, because its
//! authentication is its own: a bearer token, checked the same way whether or
//! not OIDC is configured, where the REST API is either wide open (single-user)
//! or session-only (multi-user). Merging it alongside the protected API keeps
//! that one rule in one place — see [`super::auth`].
//!
//! The one wrinkle is that `StreamableHttpService` is built from a factory that
//! never sees the request, so it cannot resolve a per-user library by itself.
//! Each user therefore gets their own service, built on first use and cached:
//! the handler resolves the library the normal way (the middleware stack has
//! already run) and dispatches into that user's service.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::sync::RwLock;

use super::PhosMcp;
use crate::api::AppState;

type McpService = StreamableHttpService<PhosMcp, LocalSessionManager>;

#[derive(Clone)]
struct McpHttpState {
    app: AppState,
    writable: bool,
    /// One service per library, keyed by that library's root. Built lazily
    /// because in multi-user mode the set of users is not known at startup.
    services: Arc<RwLock<HashMap<String, Arc<McpService>>>>,
}

/// The `/mcp` router, to merge beside the API.
///
/// `writable` comes from `PHOS_MCP_WRITE`; see [`super::PhosMcp::new`] for what
/// it changes.
pub fn create_router(state: AppState, writable: bool) -> Router {
    let mcp_state = McpHttpState {
        app: state.clone(),
        writable,
        services: Arc::new(RwLock::new(HashMap::new())),
    };

    Router::new()
        .route("/mcp", any(handle))
        .with_state(mcp_state)
        // Innermost first: the token check names the user, then the usual
        // resolver turns that into a pool and a library root.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api::resolve_user_db,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state,
            super::auth::require_mcp_token,
        ))
}

async fn handle(State(mcp): State<McpHttpState>, request: axum::extract::Request) -> Response {
    // Not the `UState` extractor: it only resolves against a router whose state
    // is `AppState`, and this router's state is its own. The resolver put the
    // per-user state in the extensions, which is the same thing one layer down.
    let user_state = request
        .extensions()
        .get::<AppState>()
        .cloned()
        .unwrap_or_else(|| mcp.app.clone());
    let key = user_state.library_root.to_string_lossy().to_string();

    let service = {
        let cached = mcp.services.read().await.get(&key).cloned();
        match cached {
            Some(service) => service,
            None => {
                let mut services = mcp.services.write().await;
                services
                    .entry(key)
                    .or_insert_with(|| Arc::new(build_service(user_state, mcp.writable)))
                    .clone()
            }
        }
    };

    service.handle(request).await.into_response()
}

fn build_service(state: AppState, writable: bool) -> McpService {
    let config = StreamableHttpServerConfig::default()
        // The SDK's default Host allowlist is localhost-only, which is the right
        // default for a desktop server reachable by a browser that could be
        // tricked into DNS rebinding. Phos is a server people put on a LAN or a
        // domain of their choosing, and `/mcp` is already gated on a bearer
        // token that no cross-origin page can read, so the allowlist would only
        // break legitimate hostnames.
        .disable_allowed_hosts();

    StreamableHttpService::new(
        move || Ok(PhosMcp::new(state.clone(), writable)),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

/// Whether writes are enabled, read from the environment once.
pub fn writes_enabled() -> bool {
    matches!(
        std::env::var("PHOS_MCP_WRITE").ok().as_deref(),
        Some("1") | Some("true")
    )
}
