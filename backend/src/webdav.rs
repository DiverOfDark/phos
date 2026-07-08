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
use sha2::{Digest, Sha256};

use crate::db;

/// Fixed username used in single-user mode (no OIDC).
const SINGLE_USER_WEBDAV_USERNAME: &str = "phos";

/// Check if a filename should be hidden from WebDAV/S3 clients.
/// Hides Phos internal files (.phos.db, .phos_thumbnails/, .phos.db-wal, etc.)
pub(crate) fn is_hidden_name(name: &str) -> bool {
    name.starts_with(".phos")
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

/// Metadata for virtual entries, backed by the real file's std metadata.
#[derive(Debug, Clone)]
struct VirtualMetaData(std::fs::Metadata);

impl DavMetaData for VirtualMetaData {
    fn len(&self) -> u64 {
        self.0.len()
    }

    fn modified(&self) -> FsResult<std::time::SystemTime> {
        self.0.modified().map_err(|_| FsError::GeneralFailure)
    }

    fn is_dir(&self) -> bool {
        self.0.is_dir()
    }

    fn created(&self) -> FsResult<std::time::SystemTime> {
        self.0.created().map_err(|_| FsError::NotImplemented)
    }

    fn accessed(&self) -> FsResult<std::time::SystemTime> {
        self.0.accessed().map_err(|_| FsError::NotImplemented)
    }
}

/// Directory entry with a virtual (flattened) name and real-file metadata.
struct VirtualDirEntry {
    name: String,
    meta: std::fs::Metadata,
}

impl DavDirEntry for VirtualDirEntry {
    fn name(&self) -> Vec<u8> {
        self.name.clone().into_bytes()
    }

    fn metadata(&self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        let meta = VirtualMetaData(self.meta.clone());
        Box::pin(async move { Ok(Box::new(meta) as Box<dyn DavMetaData>) })
    }
}

/// The virtual path a client requested, classified by depth.
enum VirtualPath {
    Root,
    /// One segment: a top-level dir (person) or a root-level file.
    TopLevel(String),
    /// Two segments: `{top}/{flattened_name}` — always a file in the virtual view.
    Flattened(String),
    /// Deeper paths don't exist in the virtual namespace.
    NotFound,
}

fn classify(path: &DavPath) -> VirtualPath {
    let rel = path.as_rel_ospath();
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    match comps.len() {
        0 => VirtualPath::Root,
        1 => VirtualPath::TopLevel(comps.into_iter().next().unwrap()),
        2 => VirtualPath::Flattened(comps.join("/")),
        _ => VirtualPath::NotFound,
    }
}

/// Read-only DavFileSystem presenting the flattened virtual view (see
/// `crate::virtualfs`): top-level person dirs each contain one flat list of
/// `{series}_{file}` entries.
/// - Hides internal Phos metadata files
/// - Rejects all write operations (create, delete, rename, copy)
#[derive(Clone)]
struct PhosFs {
    root: PathBuf,
    /// Used only to open real files (ranged reads etc.) after virtual→real mapping.
    inner: Box<LocalFs>,
}

impl PhosFs {
    fn new(root: &Path) -> Self {
        PhosFs {
            root: root.to_path_buf(),
            inner: LocalFs::new(root, false, false, false),
        }
    }

    /// Resolve a virtual DavPath to the real absolute path (files and the
    /// root/top-level dirs that exist verbatim).
    fn resolve_real(&self, path: &DavPath) -> Option<PathBuf> {
        match classify(path) {
            VirtualPath::Root => Some(self.root.clone()),
            VirtualPath::TopLevel(name) => {
                if is_hidden_name(&name) || name == "." || name == ".." || name.contains('\\') {
                    return None;
                }
                let p = self.root.join(&name);
                p.exists().then_some(p)
            }
            VirtualPath::Flattened(vpath) => crate::virtualfs::resolve(&self.root, &vpath),
            VirtualPath::NotFound => None,
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
        Box::pin(async move {
            let real = self.resolve_real(path).ok_or(FsError::NotFound)?;
            let rel = real.strip_prefix(&self.root).map_err(|_| FsError::NotFound)?;
            // Hand LocalFs the real path (it joins its base with the DavPath's
            // relative path), percent-encoding each segment.
            let encoded = rel
                .components()
                .map(|c| urlencoding::encode(&c.as_os_str().to_string_lossy()).into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let real_dav = DavPath::new(&format!("/{encoded}")).map_err(|_| FsError::NotFound)?;
            DavFileSystem::open(&*self.inner, &real_dav, options).await
        })
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        if is_hidden_dav_path(path) {
            return Box::pin(async { Err(FsError::NotFound) });
        }
        Box::pin(async move {
            let entries = match classify(path) {
                VirtualPath::Root => {
                    crate::virtualfs::list_root(&self.root).map_err(|_| FsError::GeneralFailure)?
                }
                VirtualPath::TopLevel(_) => {
                    let dir = self
                        .resolve_real(path)
                        .filter(|p| p.is_dir())
                        .ok_or(FsError::NotFound)?;
                    crate::virtualfs::list_flattened(&dir).map_err(|_| FsError::GeneralFailure)?
                }
                // Flattened entries are files; deeper paths don't exist.
                VirtualPath::Flattened(_) | VirtualPath::NotFound => return Err(FsError::NotFound),
            };
            let stream = futures_util::stream::iter(entries.into_iter().map(|e| {
                Ok(Box::new(VirtualDirEntry {
                    name: e.name,
                    meta: e.metadata,
                }) as Box<dyn DavDirEntry>)
            }));
            Ok(Box::pin(stream) as FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        if is_hidden_dav_path(path) {
            return Box::pin(async { Err(FsError::NotFound) });
        }
        Box::pin(async move {
            let real = self.resolve_real(path).ok_or(FsError::NotFound)?;
            let meta = std::fs::metadata(&real).map_err(|_| FsError::NotFound)?;
            Ok(Box::new(VirtualMetaData(meta)) as Box<dyn DavMetaData>)
        })
    }

    fn symlink_metadata<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        DavFileSystem::metadata(self, path)
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
    let mut conn = db::open_diesel_connection(&db_path).map_err(|e| {
        tracing::error!("WebDAV: failed to open database at {:?}: {}", db_path, e);
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("Database error"))
            .unwrap()
    })?;

    let stored_password_hash = db::get_setting(&mut conn, "webdav_password");

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
    use futures_util::StreamExt;

    fn setup_library() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("unsorted/001")).unwrap();
        std::fs::create_dir_all(root.join("unsorted/002")).unwrap();
        std::fs::write(root.join("unsorted/001/b.mp4"), b"vid").unwrap();
        std::fs::write(root.join("unsorted/002/photo_1.jpg"), b"jpg").unwrap();
        std::fs::write(root.join(".phos.db"), b"db").unwrap();
        dir
    }

    async fn list_names(fs: &PhosFs, path: &str) -> Vec<String> {
        let dav_path = DavPath::new(path).unwrap();
        let mut stream = DavFileSystem::read_dir(fs, &dav_path, ReadDirMeta::None)
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(entry) = stream.next().await {
            names.push(String::from_utf8(entry.unwrap().name()).unwrap());
        }
        names
    }

    #[tokio::test]
    async fn test_virtual_view() {
        let dir = setup_library();
        let fs = PhosFs::new(dir.path());

        // Root lists person dirs as-is; hidden files are absent.
        assert_eq!(list_names(&fs, "/").await, vec!["unsorted"]);

        // A person dir is one flat list of {series}_{file} entries.
        assert_eq!(
            list_names(&fs, "/unsorted/").await,
            vec!["001_b.mp4", "002_photo_1.jpg"]
        );

        // Virtual file metadata comes from the real file.
        let meta = DavFileSystem::metadata(&fs, &DavPath::new("/unsorted/001_b.mp4").unwrap())
            .await
            .unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.len(), 3);

        // The raw nested path is not part of the virtual namespace.
        assert!(
            DavFileSystem::metadata(&fs, &DavPath::new("/unsorted/001/b.mp4").unwrap())
                .await
                .is_err()
        );

        // Opening a virtual file reads the real one.
        let opts = OpenOptions {
            read: true,
            ..Default::default()
        };
        assert!(DavFileSystem::open(
            &fs,
            &DavPath::new("/unsorted/002_photo_1.jpg").unwrap(),
            opts
        )
        .await
        .is_ok());
    }

    #[test]
    fn test_is_hidden_name() {
        assert!(is_hidden_name(".phos.db"));
        assert!(is_hidden_name(".phos.db-wal"));
        assert!(is_hidden_name(".phos.db-shm"));
        assert!(is_hidden_name(".phos_thumbnails"));
        assert!(is_hidden_name(".phos_jwt_secret"));
        assert!(!is_hidden_name("photo.jpg"));
        assert!(!is_hidden_name("John"));
        assert!(!is_hidden_name("001"));
        assert!(!is_hidden_name(".gitignore"));
    }
}
