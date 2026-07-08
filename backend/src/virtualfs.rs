//! Presentational "virtual filesystem" shared by the WebDAV and S3 servers.
//!
//! On disk the library is organized as `{person}/{series}/{file}` (see
//! `import::run_reorganize`). Network clients instead see each top-level
//! directory as one flat list: `{person}/{series}_{file}`. The mapping is
//! purely presentational — the DB, scanner, and thumbnails keep using real
//! paths — and both views are read-only, so virtual→real resolution probes
//! the real filesystem instead of parsing names (underscores are legal in
//! person names and filenames alike).

use std::io;
use std::path::{Path, PathBuf};

use crate::webdav::is_hidden_name;

/// One entry in a virtual directory listing.
pub struct VirtualEntry {
    /// Name shown to clients (flattened for entries below the top level).
    pub name: String,
    pub real_path: PathBuf,
    pub metadata: std::fs::Metadata,
}

/// List the library root: top-level directories (persons) and root-level
/// files, as-is. Sorted by name.
pub fn list_root(root: &Path) -> io::Result<Vec<VirtualEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if is_hidden_name(&name) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        entries.push(VirtualEntry {
            name,
            real_path: entry.path(),
            metadata,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// List a top-level directory as a flat file list: every file in the subtree
/// at real path `p1/p2/…/file` appears once as `p1_p2_…_file`. Sorted by
/// virtual name; colliding names keep the first real path (lexicographic
/// walk order) and log a warning.
pub fn list_flattened(top_dir: &Path) -> io::Result<Vec<VirtualEntry>> {
    let mut entries: Vec<VirtualEntry> = Vec::new();
    let walker = walkdir::WalkDir::new(top_dir)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| !is_hidden_name(&e.file_name().to_string_lossy()));
    for entry in walker {
        let entry = entry.map_err(io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(top_dir) else { continue };
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("_");
        let Ok(metadata) = entry.metadata() else { continue };
        entries.push(VirtualEntry {
            name,
            real_path: entry.path().to_path_buf(),
            metadata,
        });
    }
    // On equal virtual names, keep the shallowest real path — matching
    // resolve(), where a literal file wins over a flattened one.
    entries.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            a.real_path
                .components()
                .count()
                .cmp(&b.real_path.components().count())
        })
    });
    entries.dedup_by(|b, a| {
        // dedup_by sees consecutive pairs (a earlier, b later); drop b on collision.
        if a.name == b.name {
            tracing::warn!(
                "virtualfs: {:?} hidden by {:?} (same virtual name {:?})",
                b.real_path,
                a.real_path,
                a.name
            );
            true
        } else {
            false
        }
    });
    Ok(entries)
}

/// Resolve a virtual path (`top/flattened_name`, or a bare root-level file
/// name) to the real file path. Returns None when nothing matches.
pub fn resolve(root: &Path, vpath: &str) -> Option<PathBuf> {
    if vpath.is_empty() || vpath.contains('\\') {
        return None;
    }
    match vpath.split_once('/') {
        None => {
            // Root-level file, served as-is.
            if is_hidden_name(vpath) || vpath == "." || vpath == ".." {
                return None;
            }
            let path = root.join(vpath);
            path.is_file().then_some(path)
        }
        Some((top, rest)) => {
            if top.is_empty()
                || rest.is_empty()
                || rest.contains('/')
                || is_hidden_name(top)
                || top == "."
                || top == ".."
            {
                return None;
            }
            let top_dir = root.join(top);
            if !top_dir.is_dir() {
                return None;
            }
            resolve_flattened(&top_dir, rest)
        }
    }
}

/// Resolve a flattened name within a real directory by probing: a literal
/// file wins; otherwise try each subdirectory whose name + `_` prefixes the
/// name, in lexicographic order, recursing on the remainder.
fn resolve_flattened(dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || is_hidden_name(name) || name == "." || name == ".." {
        return None;
    }
    let literal = dir.join(name);
    if literal.is_file() {
        return Some(literal);
    }

    let mut subdirs: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !is_hidden_name(n))
        .collect();
    subdirs.sort();

    for sub in subdirs {
        if let Some(rest) = name.strip_prefix(&format!("{sub}_")) {
            if let Some(found) = resolve_flattened(&dir.join(&sub), rest) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical layout plus edge cases:
    ///   unsorted/001/b.mp4, unsorted/002/photo_1.jpg, person (2)/001/a.png,
    ///   album/nested/deep/c.jpg, root.txt, .phos.db (hidden)
    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("unsorted/001")).unwrap();
        std::fs::create_dir_all(root.join("unsorted/002")).unwrap();
        std::fs::create_dir_all(root.join("person (2)/001")).unwrap();
        std::fs::create_dir_all(root.join("album/nested/deep")).unwrap();
        std::fs::create_dir_all(root.join(".phos_thumbnails")).unwrap();
        std::fs::write(root.join("unsorted/001/b.mp4"), b"vid").unwrap();
        std::fs::write(root.join("unsorted/002/photo_1.jpg"), b"jpg1").unwrap();
        std::fs::write(root.join("person (2)/001/a.png"), b"png").unwrap();
        std::fs::write(root.join("album/nested/deep/c.jpg"), b"deep").unwrap();
        std::fs::write(root.join("root.txt"), b"root").unwrap();
        std::fs::write(root.join(".phos.db"), b"db").unwrap();
        std::fs::write(root.join(".phos_thumbnails/t.jpg"), b"thumb").unwrap();
        dir
    }

    #[test]
    fn test_list_root() {
        let dir = setup();
        let entries = list_root(dir.path()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["album", "person (2)", "root.txt", "unsorted"]);
        assert!(entries[0].metadata.is_dir());
        assert!(entries[2].metadata.is_file());
    }

    #[test]
    fn test_list_flattened() {
        let dir = setup();
        let entries = list_flattened(&dir.path().join("unsorted")).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["001_b.mp4", "002_photo_1.jpg"]);
        assert!(entries.iter().all(|e| e.metadata.is_file()));

        let deep = list_flattened(&dir.path().join("album")).unwrap();
        let names: Vec<&str> = deep.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["nested_deep_c.jpg"]);
    }

    #[test]
    fn test_resolve_canonical() {
        let dir = setup();
        let root = dir.path();
        assert_eq!(
            resolve(root, "unsorted/001_b.mp4"),
            Some(root.join("unsorted/001/b.mp4"))
        );
        // Underscore in the original filename still resolves.
        assert_eq!(
            resolve(root, "unsorted/002_photo_1.jpg"),
            Some(root.join("unsorted/002/photo_1.jpg"))
        );
        assert_eq!(
            resolve(root, "album/nested_deep_c.jpg"),
            Some(root.join("album/nested/deep/c.jpg"))
        );
        assert_eq!(resolve(root, "root.txt"), Some(root.join("root.txt")));
    }

    #[test]
    fn test_resolve_misses() {
        let dir = setup();
        let root = dir.path();
        assert_eq!(resolve(root, "unsorted/001_missing.jpg"), None);
        assert_eq!(resolve(root, "unsorted/003_b.mp4"), None);
        assert_eq!(resolve(root, "nobody/001_b.mp4"), None);
        // Virtual paths never have more than one slash.
        assert_eq!(resolve(root, "unsorted/001/b.mp4"), None);
        assert_eq!(resolve(root, ""), None);
        assert_eq!(resolve(root, ".phos.db"), None);
        assert_eq!(resolve(root, ".phos_thumbnails/t.jpg"), None);
        assert_eq!(resolve(root, "../root.txt"), None);
        assert_eq!(resolve(root, "unsorted/.._b.mp4"), None);
    }

    #[test]
    fn test_resolve_literal_wins() {
        let dir = setup();
        let root = dir.path();
        // A literal file whose name looks flattened takes priority.
        std::fs::write(root.join("unsorted/001_b.mp4"), b"literal").unwrap();
        assert_eq!(
            resolve(root, "unsorted/001_b.mp4"),
            Some(root.join("unsorted/001_b.mp4"))
        );
        // And the listing dedupes the collision to the same (literal) file.
        let entries = list_flattened(&root.join("unsorted")).unwrap();
        let kept: Vec<&VirtualEntry> = entries.iter().filter(|e| e.name == "001_b.mp4").collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].real_path, root.join("unsorted/001_b.mp4"));
    }
}
