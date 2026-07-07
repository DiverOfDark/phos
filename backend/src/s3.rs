use std::collections::BTreeMap;
use std::convert::Infallible;
use std::future::Future;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::SystemTime;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use base64::Engine;
use s3s::auth::{Credentials, S3Auth, SecretKey};
use s3s::dto::*;
use s3s::service::{S3Service, S3ServiceBuilder};
use s3s::{s3_error, S3Request, S3Response, S3Result, S3};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::db;
use crate::webdav::is_hidden_name;

/// Fixed access key used in single-user mode (no OIDC), mirroring WebDAV's username.
const SINGLE_USER_ACCESS_KEY: &str = "phos";

/// The single fixed bucket name each authenticated user sees. Also the URL path
/// segment the service is mounted at on the main port, so that path-style
/// requests (`/phos/<key>`) parse the bucket without any prefix stripping —
/// SigV4 signs the full request path, so the path must reach s3s unmodified.
pub const BUCKET_NAME: &str = "phos";

/// Settings table key holding the (plaintext) S3 secret key. SigV4 requires the
/// server to know the real secret for HMAC derivation, so unlike the WebDAV
/// password it cannot be stored hashed.
pub const SECRET_KEY_SETTING: &str = "s3_secret_key";

const MAX_KEYS_LIMIT: i32 = 1000;

/// Resolve the library root for an access key, mirroring WebDAV's username rules:
/// single-user mode requires the fixed key; multi-user mode uses the OIDC `sub`
/// and resolves to `<root>/<sub>/` (which must be an existing directory).
fn resolve_user_library(library_root: &Path, multi_user: bool, access_key: &str) -> Option<PathBuf> {
    if multi_user {
        if access_key.is_empty()
            || access_key.contains('/')
            || access_key.contains('\\')
            || access_key.contains("..")
            || access_key.starts_with('.')
        {
            return None;
        }
        let user_dir = library_root.join(access_key);
        user_dir.is_dir().then_some(user_dir)
    } else {
        (access_key == SINGLE_USER_ACCESS_KEY).then(|| library_root.to_path_buf())
    }
}

/// SigV4 auth provider: looks up the per-library secret key in `.phos.db`.
struct PhosS3Auth {
    library_root: PathBuf,
    multi_user: bool,
}

#[async_trait::async_trait]
impl S3Auth for PhosS3Auth {
    async fn get_secret_key(&self, access_key: &str) -> S3Result<SecretKey> {
        let library = resolve_user_library(&self.library_root, self.multi_user, access_key)
            .ok_or_else(|| s3_error!(InvalidAccessKeyId))?;
        let db_path = library.join(".phos.db");
        let mut conn = db::open_diesel_connection(&db_path).map_err(|e| {
            tracing::error!("S3: failed to open database at {:?}: {}", db_path, e);
            s3_error!(InternalError)
        })?;
        match db::get_setting(&mut conn, SECRET_KEY_SETTING) {
            Some(secret) => Ok(SecretKey::from(secret)),
            None => {
                tracing::warn!("S3: no credentials configured for access key {:?}", access_key);
                Err(s3_error!(
                    InvalidAccessKeyId,
                    "S3 access not configured. Generate credentials in Settings."
                ))
            }
        }
    }
}

/// Validate an object key and map it to a path under the library root.
/// Rejects empty/absolute keys, path traversal, and Phos internal files.
fn key_to_path(root: &Path, key: &str) -> S3Result<PathBuf> {
    if key.is_empty() || key.ends_with('/') {
        return Err(s3_error!(NoSuchKey));
    }
    let mut path = root.to_path_buf();
    for comp in key.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." || comp.contains('\\') || is_hidden_name(comp) {
            return Err(s3_error!(NoSuchKey));
        }
        path.push(comp);
    }
    Ok(path)
}

/// Synthetic ETag from mtime + size. Computing MD5 over a whole photo library
/// is prohibitive; size/modtime-based sync works, checksum verification won't.
fn synthetic_etag(mtime: Option<SystemTime>, size: u64) -> ETag {
    let secs = mtime
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ETag::Strong(format!("{:x}-{:x}", secs, size))
}

fn guess_content_type(path: &Path) -> String {
    mime_guess::from_path(path).first_raw().unwrap_or("application/octet-stream").to_string()
}

fn read_only_error() -> s3s::S3Error {
    s3_error!(AccessDenied, "Phos S3 access is read-only")
}

/// One entry in a listing: either an object (file) or a common prefix (directory).
struct ListEntry {
    /// Object key, or prefix string ending with '/'.
    key: String,
    is_prefix: bool,
    size: u64,
    mtime: Option<SystemTime>,
}

/// Collect all entries matching `prefix`, honoring `delimiter`, sorted by key.
/// Delimiter "/" is served from a single directory read; any other case walks
/// the tree and rolls keys up by the delimiter. Hidden (.phos*) names are
/// filtered everywhere.
fn collect_entries(root: &Path, prefix: &str, delimiter: Option<&str>) -> S3Result<Vec<ListEntry>> {
    if prefix.contains('\\') {
        return Ok(Vec::new());
    }
    // Reject prefixes that reach into hidden or traversal paths: they can never match.
    for comp in prefix.split('/') {
        if comp == ".." || comp == "." {
            return Ok(Vec::new());
        }
    }

    if delimiter == Some("/") {
        // Fast path: only entries directly inside one directory are visible.
        let (dir_rel, name_pre) = match prefix.rsplit_once('/') {
            Some((d, n)) => (d, n),
            None => ("", prefix),
        };
        let mut dir_abs = root.to_path_buf();
        for comp in dir_rel.split('/').filter(|c| !c.is_empty()) {
            if is_hidden_name(comp) {
                return Ok(Vec::new());
            }
            dir_abs.push(comp);
        }
        let mut entries = Vec::new();
        let read_dir = match std::fs::read_dir(&dir_abs) {
            Ok(rd) => rd,
            Err(_) => return Ok(Vec::new()),
        };
        let key_base = if dir_rel.is_empty() { String::new() } else { format!("{dir_rel}/") };
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_hidden_name(&name) || !name.starts_with(name_pre) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                entries.push(ListEntry {
                    key: format!("{key_base}{name}/"),
                    is_prefix: true,
                    size: 0,
                    mtime: None,
                });
            } else if meta.is_file() {
                entries.push(ListEntry {
                    key: format!("{key_base}{name}"),
                    is_prefix: false,
                    size: meta.len(),
                    mtime: meta.modified().ok(),
                });
            }
        }
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(entries)
    } else {
        // Generic path: recursive walk, then optional roll-up by delimiter.
        let mut map: BTreeMap<String, ListEntry> = BTreeMap::new();
        let walker = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !is_hidden_name(&e.file_name().to_string_lossy()));
        for entry in walker.flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else { continue };
            let key = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if !key.starts_with(prefix) {
                continue;
            }
            let meta = entry.metadata().ok();
            let rolled = delimiter.and_then(|d| {
                key[prefix.len()..].find(d).map(|i| key[..prefix.len() + i + d.len()].to_string())
            });
            match rolled {
                Some(common) => {
                    map.entry(common.clone()).or_insert(ListEntry {
                        key: common,
                        is_prefix: true,
                        size: 0,
                        mtime: None,
                    });
                }
                None => {
                    map.insert(
                        key.clone(),
                        ListEntry {
                            key,
                            is_prefix: false,
                            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                            mtime: meta.and_then(|m| m.modified().ok()),
                        },
                    );
                }
            }
        }
        Ok(map.into_values().collect())
    }
}

fn encode_token(key: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.as_bytes())
}

fn decode_token(token: &str) -> S3Result<String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .map_err(|_| s3_error!(InvalidToken))?;
    String::from_utf8(bytes).map_err(|_| s3_error!(InvalidToken))
}

/// Result of the shared listing logic used by both ListObjects and ListObjectsV2.
struct ListPage {
    contents: Vec<Object>,
    common_prefixes: Vec<CommonPrefix>,
    is_truncated: bool,
    /// Key of the last returned entry, used for continuation.
    last_key: Option<String>,
    key_count: i32,
}

async fn list_page(
    root: PathBuf,
    prefix: String,
    delimiter: Option<String>,
    after: Option<String>,
    max_keys: i32,
) -> S3Result<ListPage> {
    let max_keys = max_keys.clamp(0, MAX_KEYS_LIMIT) as usize;
    let entries = tokio::task::spawn_blocking(move || {
        collect_entries(&root, &prefix, delimiter.as_deref())
    })
    .await
    .map_err(|_| s3_error!(InternalError))??;

    let mut contents = Vec::new();
    let mut common_prefixes = Vec::new();
    let mut is_truncated = false;
    let mut last_key = None;
    let mut count = 0usize;

    for entry in entries {
        if let Some(ref a) = after {
            if entry.key.as_str() <= a.as_str() {
                continue;
            }
        }
        if count == max_keys {
            is_truncated = true;
            break;
        }
        count += 1;
        last_key = Some(entry.key.clone());
        if entry.is_prefix {
            common_prefixes.push(CommonPrefix {
                prefix: Some(entry.key),
            });
        } else {
            contents.push(Object {
                key: Some(entry.key),
                size: Some(entry.size as i64),
                last_modified: entry.mtime.map(Timestamp::from),
                e_tag: Some(synthetic_etag(entry.mtime, entry.size)),
                storage_class: Some(ObjectStorageClass::from_static(ObjectStorageClass::STANDARD)),
                ..Default::default()
            });
        }
    }

    Ok(ListPage {
        contents,
        common_prefixes,
        is_truncated,
        last_key,
        key_count: count as i32,
    })
}

/// Read-only S3 implementation serving a Phos library from the local filesystem.
struct PhosS3 {
    library_root: PathBuf,
    multi_user: bool,
}

impl PhosS3 {
    /// Resolve the authenticated caller's library root. Anonymous requests are
    /// rejected here as a second line of defense (s3s already denies them when
    /// an auth provider is configured).
    fn resolve_library(&self, credentials: &Option<Credentials>) -> S3Result<PathBuf> {
        let creds = credentials
            .as_ref()
            .ok_or_else(|| s3_error!(AccessDenied, "Anonymous access is not allowed"))?;
        resolve_user_library(&self.library_root, self.multi_user, &creds.access_key)
            .ok_or_else(|| s3_error!(AccessDenied))
    }

    fn check_bucket(&self, bucket: &str) -> S3Result<()> {
        if bucket == BUCKET_NAME {
            Ok(())
        } else {
            Err(s3_error!(NoSuchBucket))
        }
    }
}

#[async_trait::async_trait]
impl S3 for PhosS3 {
    async fn list_buckets(
        &self,
        req: S3Request<ListBucketsInput>,
    ) -> S3Result<S3Response<ListBucketsOutput>> {
        let root = self.resolve_library(&req.credentials)?;
        let mtime = tokio::fs::metadata(&root).await.ok().and_then(|m| m.modified().ok());
        let bucket = Bucket {
            name: Some(BUCKET_NAME.to_string()),
            creation_date: mtime.map(Timestamp::from),
            ..Default::default()
        };
        Ok(S3Response::new(ListBucketsOutput {
            buckets: Some(vec![bucket]),
            ..Default::default()
        }))
    }

    async fn head_bucket(
        &self,
        req: S3Request<HeadBucketInput>,
    ) -> S3Result<S3Response<HeadBucketOutput>> {
        self.resolve_library(&req.credentials)?;
        self.check_bucket(&req.input.bucket)?;
        Ok(S3Response::new(HeadBucketOutput::default()))
    }

    async fn get_bucket_location(
        &self,
        req: S3Request<GetBucketLocationInput>,
    ) -> S3Result<S3Response<GetBucketLocationOutput>> {
        self.resolve_library(&req.credentials)?;
        self.check_bucket(&req.input.bucket)?;
        // Empty location constraint == us-east-1 in S3 semantics.
        Ok(S3Response::new(GetBucketLocationOutput::default()))
    }

    async fn list_objects_v2(
        &self,
        req: S3Request<ListObjectsV2Input>,
    ) -> S3Result<S3Response<ListObjectsV2Output>> {
        let root = self.resolve_library(&req.credentials)?;
        self.check_bucket(&req.input.bucket)?;
        let input = req.input;
        let prefix = input.prefix.clone().unwrap_or_default();
        let after = match (&input.continuation_token, &input.start_after) {
            (Some(token), _) => Some(decode_token(token)?),
            (None, Some(start_after)) => Some(start_after.clone()),
            (None, None) => None,
        };
        let page = list_page(
            root,
            prefix,
            input.delimiter.clone(),
            after,
            input.max_keys.unwrap_or(MAX_KEYS_LIMIT),
        )
        .await?;

        Ok(S3Response::new(ListObjectsV2Output {
            name: Some(input.bucket),
            prefix: input.prefix,
            delimiter: input.delimiter,
            max_keys: input.max_keys,
            continuation_token: input.continuation_token,
            key_count: Some(page.key_count),
            is_truncated: Some(page.is_truncated),
            next_continuation_token: page
                .is_truncated
                .then(|| page.last_key.as_deref().map(encode_token))
                .flatten(),
            contents: (!page.contents.is_empty()).then_some(page.contents),
            common_prefixes: (!page.common_prefixes.is_empty()).then_some(page.common_prefixes),
            ..Default::default()
        }))
    }

    async fn list_objects(
        &self,
        req: S3Request<ListObjectsInput>,
    ) -> S3Result<S3Response<ListObjectsOutput>> {
        let root = self.resolve_library(&req.credentials)?;
        self.check_bucket(&req.input.bucket)?;
        let input = req.input;
        let prefix = input.prefix.clone().unwrap_or_default();
        let page = list_page(
            root,
            prefix,
            input.delimiter.clone(),
            input.marker.clone(),
            input.max_keys.unwrap_or(MAX_KEYS_LIMIT),
        )
        .await?;

        Ok(S3Response::new(ListObjectsOutput {
            name: Some(input.bucket),
            prefix: input.prefix,
            delimiter: input.delimiter,
            max_keys: input.max_keys,
            marker: input.marker,
            is_truncated: Some(page.is_truncated),
            next_marker: page.is_truncated.then(|| page.last_key.clone()).flatten(),
            contents: (!page.contents.is_empty()).then_some(page.contents),
            common_prefixes: (!page.common_prefixes.is_empty()).then_some(page.common_prefixes),
            ..Default::default()
        }))
    }

    async fn head_object(
        &self,
        req: S3Request<HeadObjectInput>,
    ) -> S3Result<S3Response<HeadObjectOutput>> {
        let root = self.resolve_library(&req.credentials)?;
        self.check_bucket(&req.input.bucket)?;
        let path = key_to_path(&root, &req.input.key)?;
        let meta = tokio::fs::metadata(&path).await.map_err(|_| s3_error!(NoSuchKey))?;
        if !meta.is_file() {
            return Err(s3_error!(NoSuchKey));
        }
        let mtime = meta.modified().ok();
        Ok(S3Response::new(HeadObjectOutput {
            content_length: Some(meta.len() as i64),
            last_modified: mtime.map(Timestamp::from),
            e_tag: Some(synthetic_etag(mtime, meta.len())),
            content_type: Some(guess_content_type(&path)),
            accept_ranges: Some("bytes".to_string()),
            ..Default::default()
        }))
    }

    async fn get_object(
        &self,
        req: S3Request<GetObjectInput>,
    ) -> S3Result<S3Response<GetObjectOutput>> {
        let root = self.resolve_library(&req.credentials)?;
        self.check_bucket(&req.input.bucket)?;
        let path = key_to_path(&root, &req.input.key)?;
        let meta = tokio::fs::metadata(&path).await.map_err(|_| s3_error!(NoSuchKey))?;
        if !meta.is_file() {
            return Err(s3_error!(NoSuchKey));
        }
        let file_len = meta.len();

        let (start, content_length) = match req.input.range {
            None => (0, file_len),
            Some(Range::Int { first, last }) => {
                if first >= file_len {
                    return Err(s3_error!(InvalidRange));
                }
                let last = last.map_or(file_len - 1, |l| l.min(file_len - 1));
                (first, last - first + 1)
            }
            Some(Range::Suffix { length }) => {
                let len = length.min(file_len);
                (file_len - len, len)
            }
        };

        let mut file = tokio::fs::File::open(&path).await.map_err(|_| s3_error!(NoSuchKey))?;
        if start > 0 {
            file.seek(SeekFrom::Start(start))
                .await
                .map_err(|_| s3_error!(InternalError))?;
        }
        let body = StreamingBlob::wrap(tokio_util::io::ReaderStream::new(file.take(content_length)));

        let mtime = meta.modified().ok();
        let output = GetObjectOutput {
            body: Some(body),
            content_length: Some(content_length as i64),
            content_range: req.input.range.map(|_| {
                format!(
                    "bytes {}-{}/{}",
                    start,
                    start + content_length.saturating_sub(1),
                    file_len
                )
            }),
            last_modified: mtime.map(Timestamp::from),
            e_tag: Some(synthetic_etag(mtime, file_len)),
            content_type: Some(guess_content_type(&path)),
            accept_ranges: Some("bytes".to_string()),
            ..Default::default()
        };
        let mut resp = S3Response::new(output);
        if req.input.range.is_some() {
            resp.status = Some(StatusCode::PARTIAL_CONTENT);
        }
        Ok(resp)
    }

    // --- Read-only enforcement: reject mutating operations with a clear error.
    // Everything not listed keeps the default NotImplemented response.

    async fn put_object(&self, _req: S3Request<PutObjectInput>) -> S3Result<S3Response<PutObjectOutput>> {
        Err(read_only_error())
    }

    async fn delete_object(
        &self,
        _req: S3Request<DeleteObjectInput>,
    ) -> S3Result<S3Response<DeleteObjectOutput>> {
        Err(read_only_error())
    }

    async fn delete_objects(
        &self,
        _req: S3Request<DeleteObjectsInput>,
    ) -> S3Result<S3Response<DeleteObjectsOutput>> {
        Err(read_only_error())
    }

    async fn copy_object(&self, _req: S3Request<CopyObjectInput>) -> S3Result<S3Response<CopyObjectOutput>> {
        Err(read_only_error())
    }

    async fn create_multipart_upload(
        &self,
        _req: S3Request<CreateMultipartUploadInput>,
    ) -> S3Result<S3Response<CreateMultipartUploadOutput>> {
        Err(read_only_error())
    }

    async fn create_bucket(
        &self,
        _req: S3Request<CreateBucketInput>,
    ) -> S3Result<S3Response<CreateBucketOutput>> {
        Err(read_only_error())
    }

    async fn delete_bucket(
        &self,
        _req: S3Request<DeleteBucketInput>,
    ) -> S3Result<S3Response<DeleteBucketOutput>> {
        Err(read_only_error())
    }
}

/// Axum-compatible wrapper around `s3s::service::S3Service`: converts the
/// response body type and maps errors to 500 (axum requires `Error = Infallible`).
#[derive(Clone)]
pub struct S3AxumService {
    inner: S3Service,
}

/// Build the S3 service for a library root. SigV4 credentials are looked up
/// per-request from the per-library `.phos.db`, so no credentials are needed
/// at construction time.
pub fn build_s3_service(library_root: &Path, multi_user: bool) -> S3AxumService {
    let mut builder = S3ServiceBuilder::new(PhosS3 {
        library_root: library_root.to_path_buf(),
        multi_user,
    });
    builder.set_auth(PhosS3Auth {
        library_root: library_root.to_path_buf(),
        multi_user,
    });
    S3AxumService {
        inner: builder.build(),
    }
}

impl tower_service::Service<Request<Body>> for S3AxumService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        Box::pin(async move {
            match tower_service::Service::call(&mut inner, req).await {
                Ok(resp) => {
                    let (parts, s3_body) = resp.into_parts();
                    Ok(Response::from_parts(parts, Body::new(s3_body)))
                }
                Err(err) => {
                    tracing::error!("S3 service error: {:?}", err);
                    Ok(Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from("S3 service error"))
                        .unwrap())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_service::Service;

    fn setup_library() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("photo.jpg"), b"jpegdata").unwrap();
        std::fs::write(root.join(".phos.db"), b"secret").unwrap();
        std::fs::create_dir(root.join(".phos_thumbnails")).unwrap();
        std::fs::write(root.join(".phos_thumbnails/t.jpg"), b"thumb").unwrap();
        std::fs::create_dir_all(root.join("album/nested")).unwrap();
        std::fs::write(root.join("album/a.png"), b"pngdata").unwrap();
        std::fs::write(root.join("album/nested/b.mp4"), b"videodata").unwrap();
        dir
    }

    /// Drive the full axum-facing service with a raw HTTP request.
    async fn call_service(root: &Path, req: Request<Body>) -> Response<Body> {
        let mut svc = build_s3_service(root, false);
        svc.call(req).await.unwrap()
    }

    #[tokio::test]
    async fn test_service_rejects_anonymous() {
        let dir = setup_library();
        let req = Request::builder()
            .method("GET")
            .uri("/phos?list-type=2")
            .body(Body::empty())
            .unwrap();
        let resp = call_service(dir.path(), req).await;
        // No SigV4 signature: the request must be rejected, never served.
        assert!(resp.status().is_client_error(), "got {}", resp.status());
    }

    #[test]
    fn test_key_to_path() {
        let root = Path::new("/lib");
        assert!(key_to_path(root, "photo.jpg").is_ok());
        assert!(key_to_path(root, "album/nested/b.mp4").is_ok());
        assert!(key_to_path(root, "").is_err());
        assert!(key_to_path(root, "album/").is_err());
        assert!(key_to_path(root, "/etc/passwd").is_err());
        assert!(key_to_path(root, "../outside").is_err());
        assert!(key_to_path(root, "album/../../outside").is_err());
        assert!(key_to_path(root, ".phos.db").is_err());
        assert!(key_to_path(root, ".phos_thumbnails/t.jpg").is_err());
        assert!(key_to_path(root, "album/.phos.db").is_err());
        assert!(key_to_path(root, "a\\b").is_err());
    }

    #[test]
    fn test_collect_entries_delimiter_hides_phos_files() {
        let dir = setup_library();
        let entries = collect_entries(dir.path(), "", Some("/")).unwrap();
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["album/", "photo.jpg"]);
        assert!(entries[0].is_prefix);
        assert!(!entries[1].is_prefix);
    }

    #[test]
    fn test_collect_entries_recursive() {
        let dir = setup_library();
        let entries = collect_entries(dir.path(), "", None).unwrap();
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["album/a.png", "album/nested/b.mp4", "photo.jpg"]);
    }

    #[test]
    fn test_collect_entries_prefix() {
        let dir = setup_library();
        let entries = collect_entries(dir.path(), "album/", Some("/")).unwrap();
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["album/a.png", "album/nested/"]);
    }

    #[test]
    fn test_collect_entries_traversal_prefix() {
        let dir = setup_library();
        assert!(collect_entries(dir.path(), "../", Some("/")).unwrap().is_empty());
        assert!(collect_entries(dir.path(), ".phos_thumbnails/", Some("/")).unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_list_page_truncation() {
        let dir = setup_library();
        let page = list_page(dir.path().to_path_buf(), String::new(), None, None, 2)
            .await
            .unwrap();
        assert!(page.is_truncated);
        assert_eq!(page.key_count, 2);
        let after = page.last_key.clone();
        let page2 = list_page(dir.path().to_path_buf(), String::new(), None, after, 2)
            .await
            .unwrap();
        assert!(!page2.is_truncated);
        assert_eq!(page2.contents[0].key.as_deref(), Some("photo.jpg"));
    }

    #[test]
    fn test_resolve_user_library() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("user-1")).unwrap();

        assert_eq!(resolve_user_library(root, false, "phos"), Some(root.to_path_buf()));
        assert_eq!(resolve_user_library(root, false, "other"), None);
        assert_eq!(resolve_user_library(root, true, "user-1"), Some(root.join("user-1")));
        assert_eq!(resolve_user_library(root, true, "user-2"), None);
        assert_eq!(resolve_user_library(root, true, "../user-1"), None);
        assert_eq!(resolve_user_library(root, true, ".phos_thumbnails"), None);
    }

    #[test]
    fn test_synthetic_etag() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(0x1234);
        assert_eq!(synthetic_etag(Some(t), 0xff), ETag::Strong("1234-ff".to_string()));
    }
}
