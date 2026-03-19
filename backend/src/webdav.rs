use std::convert::Infallible;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use base64::Engine;
use dav_server::davpath::DavPath;
use dav_server::fakels::FakeLs;
use dav_server::fs::*;
use dav_server::localfs::LocalFs;
use dav_server::DavHandler;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::db;

/// Fixed username used in single-user mode (no OIDC).
const SINGLE_USER_WEBDAV_USERNAME: &str = "phos";

/// Check if a filename should be hidden from WebDAV clients.
/// Hides Phos internal files (.phos.db, .phos_thumbnails/, .phos.db-wal, etc.)
/// and the duplicates staging folder.
fn is_hidden_name(name: &str) -> bool {
    name.starts_with(".phos") || name == ".duplicates"
}

/// Check if any component of a DavPath refers to a hidden internal path.
fn is_hidden_dav_path(path: &DavPath) -> bool {
    let pb = path.as_pathbuf();
    for component in pb.components() {
        if let std::path::Component::Normal(name) = component {
            let name_str = name.to_string_lossy();
            if is_hidden_name(&name_str) {
                return true;
            }
        }
    }
    false
}

/// Read-only, filtering DavFileSystem that wraps LocalFs.
/// - Hides internal Phos metadata files from directory listings and metadata requests
/// - Rejects all write operations (create, delete, rename, copy)
#[derive(Clone)]
struct PhosFs {
    inner: Box<LocalFs>,
}

impl PhosFs {
    fn new(root: &Path) -> Self {
        PhosFs {
            inner: LocalFs::new(root, false, false, false),
        }
    }
}

impl DavFileSystem for PhosFs {
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        if is_hidden_dav_path(path) {
            return Box::pin(async { Err(FsError::NotFound) });
        }
        // Read-only: reject any write flags
        if options.write || options.append || options.truncate || options.create || options.create_new
        {
            return Box::pin(async { Err(FsError::Forbidden) });
        }
        DavFileSystem::open(&*self.inner, path, options)
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        if is_hidden_dav_path(path) {
            return Box::pin(async { Err(FsError::NotFound) });
        }
        let fut = DavFileSystem::read_dir(&*self.inner, path, meta);
        Box::pin(async move {
            let stream = fut.await?;
            let filtered = stream.filter(|result| {
                let keep = match result {
                    Ok(entry) => {
                        let name_bytes = entry.name();
                        let name = String::from_utf8_lossy(&name_bytes);
                        !is_hidden_name(&name)
                    }
                    Err(_) => true, // pass through errors
                };
                async move { keep }
            });
            Ok(Box::pin(filtered) as FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        if is_hidden_dav_path(path) {
            return Box::pin(async { Err(FsError::NotFound) });
        }
        DavFileSystem::metadata(&*self.inner, path)
    }

    fn symlink_metadata<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        if is_hidden_dav_path(path) {
            return Box::pin(async { Err(FsError::NotFound) });
        }
        DavFileSystem::symlink_metadata(&*self.inner, path)
    }

    // Read-only enforcement: reject all write operations
    fn create_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::Forbidden) })
    }

    fn remove_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::Forbidden) })
    }

    fn remove_file<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::Forbidden) })
    }

    fn rename<'a>(&'a self, _from: &'a DavPath, _to: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::Forbidden) })
    }

    fn copy<'a>(&'a self, _from: &'a DavPath, _to: &'a DavPath) -> FsFuture<'a, ()> {
        Box::pin(async { Err(FsError::Forbidden) })
    }
}

/// Result of successful Basic Auth: the resolved library root for the user.
struct AuthResult {
    library_root: PathBuf,
}

/// Extract and validate Basic Auth credentials. Returns the library root for the authenticated user.
fn check_basic_auth(
    req: &Request<Body>,
    library_root: &Path,
    multi_user: bool,
) -> Result<AuthResult, Response<Body>> {
    // --- Extract Authorization header ---
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let auth_header = match auth_header {
        Some(h) => h,
        None => {
            tracing::debug!("WebDAV: no Authorization header, requesting credentials");
            return Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
                .body(Body::from("Authentication required"))
                .unwrap());
        }
    };

    if !auth_header.starts_with("Basic ") {
        tracing::warn!("WebDAV: non-Basic auth scheme received");
        return Err(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Basic authentication required"))
            .unwrap());
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&auth_header[6..])
        .map_err(|_| {
            tracing::warn!("WebDAV: failed to decode base64 credentials");
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
                .body(Body::from("Invalid credentials"))
                .unwrap()
        })?;

    let decoded_str = String::from_utf8(decoded).map_err(|_| {
        tracing::warn!("WebDAV: credentials are not valid UTF-8");
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Invalid credentials"))
            .unwrap()
    })?;

    let (username, password) = decoded_str.split_once(':').ok_or_else(|| {
        tracing::warn!("WebDAV: malformed credentials (no colon separator)");
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Invalid credentials"))
            .unwrap()
    })?;

    // --- Resolve the DB and library path for this user ---
    let user_library = if multi_user {
        // In multi-user mode, the username IS the OIDC sub, and each user's
        // library is at <root>/<sub>/
        let user_dir = library_root.join(username);
        if !user_dir.is_dir() {
            tracing::warn!("WebDAV: no library directory for user {:?}", username);
            return Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
                .body(Body::from("Invalid credentials"))
                .unwrap());
        }
        user_dir
    } else {
        // Single-user: username must be the fixed value
        if username != SINGLE_USER_WEBDAV_USERNAME {
            tracing::warn!(
                "WebDAV: wrong username {:?} (expected {:?})",
                username,
                SINGLE_USER_WEBDAV_USERNAME
            );
            return Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
                .body(Body::from("Invalid credentials"))
                .unwrap());
        }
        library_root.to_path_buf()
    };

    let db_path = user_library.join(".phos.db");
    let conn = db::open_connection(&db_path).map_err(|e| {
        tracing::error!("WebDAV: failed to open database at {:?}: {}", db_path, e);
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("Database error"))
            .unwrap()
    })?;

    let stored_password_hash = db::get_setting(&conn, "webdav_password");

    if stored_password_hash.is_none() {
        tracing::warn!(
            "WebDAV: no password configured for user {:?}",
            username
        );
        return Err(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::from(
                "WebDAV not configured. Set a password in Settings.",
            ))
            .unwrap());
    }

    let expected_hash = stored_password_hash.unwrap();
    let input_hash = format!("{:x}", Sha256::digest(password.as_bytes()));

    if input_hash != expected_hash {
        tracing::warn!("WebDAV: wrong password for user {:?}", username);
        return Err(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Invalid credentials"))
            .unwrap());
    }

    tracing::debug!("WebDAV: authenticated user {:?}", username);
    Ok(AuthResult {
        library_root: user_library,
    })
}

/// Build a DavHandler for the given library root.
/// When `prefix` is non-empty, configure `strip_prefix` so that response hrefs
/// include the mount prefix (e.g. `/webdav/photo.jpg` instead of `/photo.jpg`).
fn build_dav_handler(library_root: &Path, prefix: &str) -> DavHandler {
    let fs = PhosFs::new(library_root);
    let mut builder = DavHandler::builder()
        .filesystem(Box::new(fs))
        .locksystem(FakeLs::new());
    if !prefix.is_empty() {
        builder = builder.strip_prefix(prefix);
    }
    builder.build_handler()
}

/// Tower service that wraps DavHandler with Basic Auth.
/// Used with axum's `nest_service` to handle all WebDAV methods (including PROPFIND, etc.).
#[derive(Clone)]
pub struct WebDavService {
    /// Pre-built handler for single-user mode (avoids rebuilding per request).
    single_user_handler: DavHandler,
    library_root: PathBuf,
    multi_user: bool,
    /// URL prefix that axum strips (e.g. "/webdav"). Empty when served at root.
    prefix: String,
}

impl WebDavService {
    pub fn new(library_root: &Path, multi_user: bool, prefix: &str) -> Self {
        let single_user_handler = build_dav_handler(library_root, prefix);
        WebDavService {
            single_user_handler,
            library_root: library_root.to_path_buf(),
            multi_user,
            prefix: prefix.to_string(),
        }
    }
}

impl tower_service::Service<Request<Body>> for WebDavService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let single_user_handler = self.single_user_handler.clone();
        let library_root = self.library_root.clone();
        let multi_user = self.multi_user;
        let prefix = self.prefix.clone();

        Box::pin(async move {
            let auth = match check_basic_auth(&req, &library_root, multi_user) {
                Ok(a) => a,
                Err(resp) => return Ok(resp),
            };

            // In multi-user mode, build a handler for this user's library.
            // In single-user mode, reuse the pre-built handler.
            let handler = if multi_user {
                build_dav_handler(&auth.library_root, &prefix)
            } else {
                single_user_handler
            };

            // axum's nest_service strips the mount prefix (e.g. "/webdav") from
            // the request URI, but dav-server needs the full URI to generate
            // correct hrefs in PROPFIND responses. Reconstruct it here.
            let req = if !prefix.is_empty() {
                let (mut parts, body) = req.into_parts();
                let path = parts.uri.path();
                let new_pq = if let Some(q) = parts.uri.query() {
                    format!("{prefix}{path}?{q}")
                } else {
                    format!("{prefix}{path}")
                };
                let mut uri_parts = parts.uri.into_parts();
                uri_parts.path_and_query = Some(new_pq.parse().unwrap());
                parts.uri = axum::http::Uri::from_parts(uri_parts).unwrap();
                Request::from_parts(parts, body)
            } else {
                req
            };

            let resp = handler.handle(req).await;
            let (parts, dav_body) = resp.into_parts();
            let body = Body::new(dav_body);
            Ok(Response::from_parts(parts, body))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_hidden_name() {
        assert!(is_hidden_name(".phos.db"));
        assert!(is_hidden_name(".phos.db-wal"));
        assert!(is_hidden_name(".phos.db-shm"));
        assert!(is_hidden_name(".phos_thumbnails"));
        assert!(is_hidden_name(".phos_jwt_secret"));
        assert!(is_hidden_name(".duplicates"));

        assert!(!is_hidden_name("photo.jpg"));
        assert!(!is_hidden_name("John"));
        assert!(!is_hidden_name("001"));
        assert!(!is_hidden_name(".gitignore"));
    }
}
