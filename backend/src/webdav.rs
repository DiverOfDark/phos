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

/// Validate Basic Auth credentials against the database.
/// Returns Ok(()) on success, or an HTTP error response on failure.
fn check_basic_auth(req: &Request<Body>, db_path: &Path) -> Result<(), Response<Body>> {
    let conn = db::open_connection(db_path).map_err(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("Database error"))
            .unwrap()
    })?;

    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let auth_header = match auth_header {
        Some(h) => h,
        None => {
            return Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
                .body(Body::from("Authentication required"))
                .unwrap());
        }
    };

    let stored_username = db::get_setting(&conn, "webdav_username");
    let stored_password_hash = db::get_setting(&conn, "webdav_password");

    // If no credentials configured, WebDAV is not enabled
    if stored_username.is_none() || stored_password_hash.is_none() {
        return Err(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::from(
                "WebDAV not configured. Set credentials in Settings.",
            ))
            .unwrap());
    }

    if !auth_header.starts_with("Basic ") {
        return Err(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Basic authentication required"))
            .unwrap());
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&auth_header[6..])
        .map_err(|_| {
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
                .body(Body::from("Invalid credentials"))
                .unwrap()
        })?;

    let decoded_str = String::from_utf8(decoded).map_err(|_| {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Invalid credentials"))
            .unwrap()
    })?;

    let (username, password) = decoded_str.split_once(':').ok_or_else(|| {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Invalid credentials"))
            .unwrap()
    })?;

    let expected_username = stored_username.unwrap();
    let expected_hash = stored_password_hash.unwrap();
    let input_hash = format!("{:x}", Sha256::digest(password.as_bytes()));

    if username != expected_username || input_hash != expected_hash {
        return Err(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"phos\"")
            .body(Body::from("Invalid credentials"))
            .unwrap());
    }

    Ok(())
}

/// Tower service that wraps DavHandler with Basic Auth.
/// Used with axum's `nest_service` to handle all WebDAV methods (including PROPFIND, etc.).
#[derive(Clone)]
pub struct WebDavService {
    handler: DavHandler,
    db_path: PathBuf,
}

impl WebDavService {
    pub fn new(library_root: &Path, db_path: &Path) -> Self {
        let fs = PhosFs::new(library_root);
        let handler = DavHandler::builder()
            .filesystem(Box::new(fs))
            .locksystem(FakeLs::new())
            .build_handler();

        WebDavService {
            handler,
            db_path: db_path.to_path_buf(),
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
        let handler = self.handler.clone();
        let db_path = self.db_path.clone();

        Box::pin(async move {
            if let Err(resp) = check_basic_auth(&req, &db_path) {
                return Ok(resp);
            }

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
