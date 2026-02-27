use crate::ai::{cosine_similarity, AiPipeline, MAX_FACE_DISTANCE};
use crate::db;
use crate::scanner::{self, Scanner};
use image::DynamicImage;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;
use walkdir::WalkDir;

/// Hamming distance between two 8-byte dHash values (out of 64 bits).
fn hamming_distance(a: &[u8; 8], b: &[u8; 8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

/// A cached record of an already-imported file's dHash and target path.
struct StoredFileRecord {
    path: String,
    dhash: [u8; 8],
}

/// Load all files with visual_embedding (dHash) from the DB.
fn load_dhash_cache(conn: &Connection) -> Vec<StoredFileRecord> {
    let mut stmt = match conn.prepare("SELECT path, visual_embedding FROM files WHERE visual_embedding IS NOT NULL") {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to load dHash cache: {}", e);
            return Vec::new();
        }
    };

    let rows = match stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        Ok((path, blob))
    }) {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to query dHash cache: {}", e);
            return Vec::new();
        }
    };

    let mut records = Vec::new();
    for row in rows.flatten() {
        let (path, blob) = row;
        if blob.len() == 8 {
            let mut dhash = [0u8; 8];
            dhash.copy_from_slice(&blob);
            records.push(StoredFileRecord { path, dhash });
        }
    }
    records
}

/// Find the next counter (numbered subfolder) inside a directory.
fn next_counter(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()))
                .max()
                .unwrap_or(0)
                + 1
        })
        .unwrap_or(1)
}

/// Extract the parent folder structure from a stored file path to determine
/// which person/counter folder it belongs to.
/// Expected paths look like: `target/<person_id>/<counter>/filename`
/// Returns (person_or_unsorted_dir, counter_dir) as absolute paths.
fn parse_target_folder(file_path: &str) -> Option<(PathBuf, PathBuf)> {
    let p = Path::new(file_path);
    // counter dir is the parent
    let counter_dir = p.parent()?;
    // person dir is the grandparent
    let person_dir = counter_dir.parent()?;
    Some((person_dir.to_path_buf(), counter_dir.to_path_buf()))
}

/// Import summary statistics (thread-safe with atomic counters).
struct ImportStats {
    total: u64,
    imported: AtomicU64,
    variations: AtomicU64,
    skipped_duplicate: AtomicU64,
    skipped_error: AtomicU64,
}

pub fn run_import(source: &Path, target: &Path, move_files: bool, threads: usize) -> anyhow::Result<()> {
    // AI is mandatory for import — reject dummy mode
    if std::env::var("PHOS_DUMMY_AI").ok().is_some_and(|v| v == "1") {
        anyhow::bail!(
            "PHOS_DUMMY_AI is set but AI models are required for import. \
             Unset PHOS_DUMMY_AI and ensure models can be downloaded."
        );
    }
    info!("Loading AI models (mandatory for import)...");
    let ai = AiPipeline::new().map_err(|e| {
        anyhow::anyhow!("AI models are required for import but failed to load: {}", e)
    })?;

    // Ensure target directory exists
    fs::create_dir_all(target)?;

    // Initialize DB in target
    let db_path = target.join(".phos.db");
    let conn = db::init_db(&db_path)?;
    info!("Database initialized at {:?}", db_path);

    // Create scanner for process_file calls
    let scanner = Scanner::new(db_path.clone(), Some(ai));

    // Collect all media files from source
    let source_files: Vec<PathBuf> = WalkDir::new(source)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| scanner::is_media_file(e.path()))
        .map(|e| e.path().to_path_buf())
        .collect();

    let total = source_files.len() as u64;
    info!("Found {} media files in {:?}", total, source);

    if total == 0 {
        println!("No media files found in {:?}", source);
        return Ok(());
    }

    // Setup progress bar
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    let stats = ImportStats {
        total,
        imported: AtomicU64::new(0),
        variations: AtomicU64::new(0),
        skipped_duplicate: AtomicU64::new(0),
        skipped_error: AtomicU64::new(0),
    };

    // Load dHash cache (starts empty, grows as we import)
    let dhash_cache = Mutex::new(load_dhash_cache(&conn));

    info!("Using {} import threads", threads);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;

    pool.install(|| {
        source_files.par_iter().for_each(|source_path| {
            pb.set_message(
                source_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );

            match import_single_file(
                source_path,
                target,
                move_files,
                &db_path,
                &scanner,
                &dhash_cache,
                &stats,
            ) {
                Ok(()) => {}
                Err(e) => {
                    error!("Error importing {:?}: {}", source_path, e);
                    stats.skipped_error.fetch_add(1, Ordering::Relaxed);
                }
            }

            pb.inc(1);
        });
    });

    pb.finish_with_message("Import complete!");

    // Print import summary
    println!();
    println!("=== Import Summary ===");
    println!("  Total files found:     {}", stats.total);
    println!("  Successfully imported: {}", stats.imported.load(Ordering::Relaxed));
    println!("  Grouped as variations: {}", stats.variations.load(Ordering::Relaxed));
    println!("  Skipped (duplicate):   {}", stats.skipped_duplicate.load(Ordering::Relaxed));
    println!("  Skipped (error):       {}", stats.skipped_error.load(Ordering::Relaxed));
    println!("======================");
    println!();

    // Reorganize files to match clustering
    info!("Running post-import reorganize...");
    run_reorganize(target, false)?;

    Ok(())
}

pub fn run_remote_import(source_str: &str, target_url: &str, threads: usize) -> anyhow::Result<()> {
    let source = Path::new(source_str);
    if !source.exists() {
        anyhow::bail!("Source directory does not exist: {:?}", source);
    }

    // Collect files
    let source_files: Vec<PathBuf> = WalkDir::new(source)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| scanner::is_media_file(e.path()))
        .map(|e| e.path().to_path_buf())
        .collect();

    let total = source_files.len() as u64;
    info!("Found {} media files for remote import", total);

    if total == 0 {
        return Ok(());
    }

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .unwrap(),
    );

    let base_url = format!("{}/api/import/upload", target_url.trim_end_matches('/'));

    info!("Using {} upload threads", threads);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;

    pool.install(|| {
        source_files.par_iter().for_each(|path| {
            let filename = path.file_name().unwrap().to_string_lossy().to_string();
            pb.set_message(filename.clone());

            let file_bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    error!("Failed to read {:?}: {}", path, e);
                    pb.inc(1);
                    return;
                }
            };

            let client = ureq::Agent::new_with_defaults();
            let encoded_filename = urlencoding::encode(&filename);
            let url = format!("{}?filename={}", base_url, encoded_filename);

            match client.put(&url)
                .header("Content-Type", "application/octet-stream")
                .send(file_bytes)
            {
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to upload {}: {}", filename, e);
                }
            }

            pb.inc(1);
        });
    });

    pb.finish_with_message("Upload complete!");

    // Trigger server-side face clustering and reorganization
    info!("Triggering post-import finalization on server...");
    let finalize_url = format!(
        "{}/api/import/finalize",
        target_url.trim_end_matches('/')
    );
    let client = ureq::Agent::new_with_defaults();
    match client.post(&finalize_url).send_empty() {
        Ok(_) => {
            println!("Post-import finalization complete (face clustering + reorganization).");
        }
        Err(e) => {
            error!("Finalize request failed: {}", e);
            println!("Warning: post-import finalization failed: {}. You can trigger it manually via the UI.", e);
        }
    }

    Ok(())
}

fn import_single_file(
    source_path: &Path,
    target: &Path,
    move_files: bool,
    db_path: &Path,
    scanner: &Scanner,
    dhash_cache: &Mutex<Vec<StoredFileRecord>>,
    stats: &ImportStats,
) -> anyhow::Result<()> {
    // 1. SHA256 hash — check for exact duplicate
    let hash = scanner::calculate_hash(source_path)?;

    {
        let conn = db::open_connection(db_path)?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM files WHERE hash = ?",
                params![hash],
                |row| row.get(0),
            )
            .ok();

        if existing.is_some() {
            warn!("Skipping exact duplicate: {:?} (hash {})", source_path, hash);
            stats.skipped_duplicate.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }

    // 2. Load image (or first video frame) for dHash and face detection
    let filename = source_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("No filename for {:?}", source_path))?
        .to_string_lossy()
        .to_string();

    let ext = source_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let is_video = matches!(ext.as_str(), "mp4" | "mkv" | "mov" | "avi" | "webm");

    let img: Option<DynamicImage> = if is_video {
        scanner::extract_first_video_frame(source_path).ok()
    } else {
        scanner::open_image(source_path).ok()
    };

    // 3. Compute dHash and check for variations
    let target_dir: PathBuf;

    if let Some(ref img) = img {
        let dhash = scanner::compute_dhash(img);

        // Check against all stored dHashes (brief lock)
        let variation_match = {
            let cache = dhash_cache.lock().unwrap();
            cache.iter().find(|stored| {
                hamming_distance(&dhash, &stored.dhash) <= 10
            }).map(|stored| stored.path.clone())
        };

        if let Some(matched_path) = variation_match {
            // This is a variation — place in the same folder as the match
            if let Some((_person_dir, counter_dir)) = parse_target_folder(&matched_path) {
                target_dir = counter_dir;
                stats.variations.fetch_add(1, Ordering::Relaxed);
            } else {
                // Fallback: couldn't parse path, treat as new
                let conn = db::open_connection(db_path)?;
                target_dir = determine_person_folder(img, target, &conn, scanner)?;
            }
        } else {
            // 4. Not a variation — run face detection for person assignment
            let conn = db::open_connection(db_path)?;
            target_dir = determine_person_folder(img, target, &conn, scanner)?;
        }
    } else {
        // Couldn't load image — put in unsorted
        let unsorted = target.join("unsorted");
        let counter = next_counter(&unsorted);
        target_dir = unsorted.join(counter.to_string());
    }

    // 5. Copy/move file to target
    fs::create_dir_all(&target_dir)?;
    let target_path = target_dir.join(&filename);

    // Handle filename collision
    let target_path = if target_path.exists() {
        let stem = source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = source_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let mut i = 1u32;
        loop {
            let candidate = if ext.is_empty() {
                target_dir.join(format!("{}_{}", stem, i))
            } else {
                target_dir.join(format!("{}_{}.{}", stem, i, ext))
            };
            if !candidate.exists() {
                break candidate;
            }
            i += 1;
        }
    } else {
        target_path
    };

    if move_files {
        // Try rename first (same filesystem), fall back to copy+delete
        if fs::rename(source_path, &target_path).is_err() {
            fs::copy(source_path, &target_path)?;
            fs::remove_file(source_path)?;
        }
    } else {
        fs::copy(source_path, &target_path)?;
    }

    // 6. Full DB indexing via scanner
    // Create a scanner-specific dHash cache for process_file.
    // Import already handles variation detection separately via its own cache,
    // so this cache starts empty — its purpose is just to satisfy the API.
    let scan_dhash_cache = std::sync::Mutex::new(Vec::<scanner::DHashCacheEntry>::new());
    let scan_conn = scanner.open_db()?;
    scanner.process_file(&scan_conn, &target_path, &scan_dhash_cache)?;

    // Update dHash cache with the newly imported file (brief lock)
    if let Some(ref img) = img {
        let dhash = scanner::compute_dhash(img);
        dhash_cache.lock().unwrap().push(StoredFileRecord {
            path: target_path.to_string_lossy().to_string(),
            dhash,
        });
    }

    stats.imported.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Determine the target folder based on face detection.
/// Returns the full path to the counter subfolder (e.g., `target/<person_id>/3/`).
fn determine_person_folder(
    img: &DynamicImage,
    target: &Path,
    conn: &Connection,
    scanner: &Scanner,
) -> anyhow::Result<PathBuf> {
    // Access AI from scanner — we need to detect faces
    // Use the AI pipeline via the scanner's internal reference
    let ai = get_ai_from_scanner(scanner);

    if let Some(ai) = ai {
        let detections = ai.detect_faces(img).unwrap_or_default();

        if !detections.is_empty() {
            // Find the largest face (by bounding box area)
            let largest = detections
                .iter()
                .max_by(|a, b| {
                    let area_a = (a.box_x2 - a.box_x1) * (a.box_y2 - a.box_y1);
                    let area_b = (b.box_x2 - b.box_x1) * (b.box_y2 - b.box_y1);
                    area_a
                        .partial_cmp(&area_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();

            // Extract face chip and embedding
            let (img_w, img_h) = image::GenericImageView::dimensions(img);
            let x1 = (largest.box_x1 as u32).min(img_w.saturating_sub(1));
            let y1 = (largest.box_y1 as u32).min(img_h.saturating_sub(1));
            let x2 = (largest.box_x2 as u32).min(img_w);
            let y2 = (largest.box_y2 as u32).min(img_h);
            let face_w = x2.saturating_sub(x1);
            let face_h = y2.saturating_sub(y1);

            if face_w >= 10 && face_h >= 10 {
                let bbox = (largest.box_x1, largest.box_y1, largest.box_x2, largest.box_y2);
                let embedding = ai
                    .extract_embedding(img, largest.landmarks.as_deref(), bbox)
                    .unwrap_or_default();

                if !embedding.is_empty() {
                    // Simple nearest-neighbor match for folder sorting
                    let person_id = find_or_create_person_for_import(conn, &embedding)?;
                    let person_dir = target.join(&person_id);
                    let counter = next_counter(&person_dir);
                    return Ok(person_dir.join(counter.to_string()));
                }
            }
        }
    }

    // No faces or failed — unsorted
    let unsorted = target.join("unsorted");
    let counter = next_counter(&unsorted);
    Ok(unsorted.join(counter.to_string()))
}

/// Simple greedy person matching for import folder sorting.
/// Matches against person representative embeddings, or creates a new person.
/// This is only used for deciding which folder to place a file in during import.
fn find_or_create_person_for_import(
    conn: &Connection,
    embedding: &[f32],
) -> anyhow::Result<String> {
    // Match against person representative embeddings (populated during import)
    let mut stmt = conn.prepare(
        "SELECT id, representative_embedding FROM people WHERE representative_embedding IS NOT NULL",
    )?;
    let rows: Vec<(String, Vec<f32>)> = stmt
        .query_map([], |row| {
            let pid: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((pid, blob))
        })?
        .filter_map(|r| r.ok())
        .filter_map(|(pid, blob)| {
            bincode::deserialize::<Vec<f32>>(&blob)
                .ok()
                .filter(|e| e.len() == embedding.len())
                .map(|e| (pid, e))
        })
        .collect();

    let mut best: Option<(String, f32)> = None;
    for (pid, emb) in &rows {
        let dist = 1.0 - cosine_similarity(embedding, emb);
        if dist <= MAX_FACE_DISTANCE {
            if let Some((_, best_dist)) = &best {
                if dist < *best_dist {
                    best = Some((pid.clone(), dist));
                }
            } else {
                best = Some((pid.clone(), dist));
            }
        }
    }

    if let Some((person_id, _)) = best {
        Ok(person_id)
    } else {
        let person_id = Uuid::new_v4().to_string();
        let embedding_blob = bincode::serialize(embedding)?;
        conn.execute(
            "INSERT INTO people (id, representative_embedding) VALUES (?, ?)",
            params![person_id, embedding_blob],
        )?;
        Ok(person_id)
    }
}

/// Access the AI pipeline from a Scanner instance.
/// This uses a helper that exposes the AI reference.
fn get_ai_from_scanner(scanner: &Scanner) -> Option<&AiPipeline> {
    scanner.ai()
}

/// Sanitize a name for use as a filesystem folder name.
/// Strips characters that are illegal on common filesystems: `/\:*?"<>|`
/// and trims leading/trailing whitespace. Returns the sanitized string.
pub fn sanitize_folder_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Ensure every person in the DB has a non-NULL `folder_name`.
/// For people with a `name`, sanitize it. For unnamed people, use the UUID.
/// Handles collisions by appending " (2)", " (3)", etc.
fn ensure_folder_names(conn: &Connection) -> anyhow::Result<()> {
    // Find people whose folder_name is NULL
    let mut stmt = conn.prepare("SELECT id, name FROM people WHERE folder_name IS NULL")?;
    let people: Vec<(String, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (person_id, name) in people {
        let base = match name {
            Some(ref n) if !n.trim().is_empty() => sanitize_folder_name(n),
            _ => person_id.clone(),
        };

        let folder_name = find_unique_folder_name(conn, &base, Some(&person_id))?;

        conn.execute(
            "UPDATE people SET folder_name = ? WHERE id = ?",
            params![folder_name, person_id],
        )?;
    }
    Ok(())
}

/// Find a folder_name that doesn't collide with existing ones in the people table.
/// `exclude_person_id` allows excluding the person being renamed from collision checks.
fn find_unique_folder_name(
    conn: &Connection,
    base_name: &str,
    exclude_person_id: Option<&str>,
) -> anyhow::Result<String> {
    let candidate = base_name.to_string();
    if !folder_name_exists(conn, &candidate, exclude_person_id)? {
        return Ok(candidate);
    }

    let mut suffix = 2u32;
    loop {
        let candidate = format!("{} ({})", base_name, suffix);
        if !folder_name_exists(conn, &candidate, exclude_person_id)? {
            return Ok(candidate);
        }
        suffix += 1;
        if suffix > 1000 {
            anyhow::bail!("Could not find a unique folder name for '{}'", base_name);
        }
    }
}

/// Check if a folder_name already exists in the people table,
/// optionally excluding a specific person_id from the check.
fn folder_name_exists(
    conn: &Connection,
    folder_name: &str,
    exclude_person_id: Option<&str>,
) -> anyhow::Result<bool> {
    let count: i64 = match exclude_person_id {
        Some(pid) => conn.query_row(
            "SELECT COUNT(*) FROM people WHERE folder_name = ? AND id != ?",
            params![folder_name, pid],
            |row| row.get(0),
        )?,
        None => conn.query_row(
            "SELECT COUNT(*) FROM people WHERE folder_name = ?",
            params![folder_name],
            |row| row.get(0),
        )?,
    };
    Ok(count > 0)
}

/// Rename a person's folder on disk and update all related DB records.
///
/// 1. Computes a new `folder_name` by sanitizing `new_name` and handling collisions.
/// 2. Renames the directory on disk from old folder_name to new folder_name.
/// 3. Batch-updates `files.path` in the DB for all files under that person's shots.
/// 4. Updates `people.folder_name` and `people.name`.
pub fn rename_person_folder(
    conn: &Connection,
    library: &Path,
    person_id: &str,
    new_name: &str,
) -> anyhow::Result<()> {
    // Get the current folder_name for this person
    let old_folder_name: Option<String> = conn
        .query_row(
            "SELECT folder_name FROM people WHERE id = ?",
            params![person_id],
            |row| row.get(0),
        )
        .map_err(|_| anyhow::anyhow!("Person '{}' not found", person_id))?;

    let old_folder_name = old_folder_name
        .unwrap_or_else(|| person_id.to_string());

    // Compute new folder_name with collision handling
    let sanitized = sanitize_folder_name(new_name);
    if sanitized.is_empty() {
        anyhow::bail!("Sanitized folder name is empty for '{}'", new_name);
    }
    let new_folder_name = find_unique_folder_name(conn, &sanitized, Some(person_id))?;

    if old_folder_name == new_folder_name {
        // Just update the display name, folder_name stays the same
        conn.execute(
            "UPDATE people SET name = ? WHERE id = ?",
            params![new_name, person_id],
        )?;
        return Ok(());
    }

    let old_dir = library.join(&old_folder_name);
    let new_dir = library.join(&new_folder_name);

    // Rename directory on disk if it exists
    if old_dir.exists() {
        if new_dir.exists() {
            anyhow::bail!(
                "Target directory {:?} already exists on disk",
                new_dir
            );
        }
        fs::rename(&old_dir, &new_dir)?;
        info!(
            "Renamed directory {:?} -> {:?}",
            old_dir, new_dir
        );
    }

    // Batch-update files.path: replace old folder_name prefix with new one
    // Find all files that belong to shots assigned to this person
    let old_prefix = format!("{}/", old_dir.to_string_lossy());
    let new_prefix = format!("{}/", new_dir.to_string_lossy());

    let mut stmt = conn.prepare(
        "SELECT f.id, f.path FROM files f
         JOIN shots s ON f.shot_id = s.id
         WHERE s.primary_person_id = ?",
    )?;
    let files: Vec<(String, String)> = stmt
        .query_map(params![person_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (file_id, file_path) in &files {
        if file_path.starts_with(&old_prefix) {
            let new_path = format!("{}{}", new_prefix, &file_path[old_prefix.len()..]);
            conn.execute(
                "UPDATE files SET path = ? WHERE id = ?",
                params![new_path, file_id],
            )?;
        }
    }

    // Update people table
    conn.execute(
        "UPDATE people SET name = ?, folder_name = ? WHERE id = ?",
        params![new_name, new_folder_name, person_id],
    )?;

    info!(
        "Person '{}' renamed to '{}' (folder: '{}' -> '{}')",
        person_id, new_name, old_folder_name, new_folder_name
    );

    Ok(())
}

/// Reorganize files on disk to match current face clustering.
///
/// Opens the `.phos.db` in the library, re-clusters faces, then moves each file
/// into `library/<person.folder_name>/<folder_number>/filename` (or
/// `library/unsorted/<folder_number>/filename` if no person is associated).
/// Updates `files.path` in the DB after each move and cleans up empty directories afterwards.
pub fn run_reorganize(library: &Path, dry_run: bool) -> anyhow::Result<()> {
    let db_path = library.join(".phos.db");
    if !db_path.exists() {
        anyhow::bail!("No .phos.db found in {:?}", library);
    }

    let conn = db::open_connection(&db_path)?;
    info!("Database opened at {:?}", db_path);

    // Re-cluster faces (no AI models needed — uses existing embeddings)
    let scanner = Scanner::new(db_path.clone(), None);
    info!("Running face clustering...");
    scanner.cluster_faces(&conn)?;

    // Ensure all people have folder_name set (use UUID for unnamed people)
    ensure_folder_names(&conn)?;

    // Load all shots with their files, joining people for folder_name and
    // using shots.primary_person_id and shots.folder_number
    let mut stmt = conn.prepare(
        "SELECT s.id, f.id, f.path, s.primary_person_id, s.folder_number,
                p.folder_name
         FROM shots s
         JOIN files f ON f.shot_id = s.id
         LEFT JOIN people p ON s.primary_person_id = p.id"
    )?;

    struct FileRow {
        shot_id: String,
        file_id: String,
        path: String,
        primary_person_id: Option<String>,
        folder_number: Option<i64>,
        person_folder_name: Option<String>,
    }

    let rows: Vec<FileRow> = stmt
        .query_map([], |row| {
            Ok(FileRow {
                shot_id: row.get(0)?,
                file_id: row.get(1)?,
                path: row.get(2)?,
                primary_person_id: row.get(3)?,
                folder_number: row.get(4)?,
                person_folder_name: row.get(5)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Group by shot_id
    let mut shots: std::collections::BTreeMap<String, Vec<&FileRow>> =
        std::collections::BTreeMap::new();
    for row in &rows {
        shots.entry(row.shot_id.clone()).or_default().push(row);
    }

    let total = rows.len() as u64;
    let action = if dry_run { "Planning" } else { "Reorganizing" };
    println!("{} {} files across {} shots...", action, total, shots.len());

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    let mut moved = 0u64;
    let mut skipped = 0u64;
    let mut errors = 0u64;

    for (_shot_id, files) in &shots {
        // All files in a shot share the same primary_person_id and folder_number,
        // so use the first file's metadata.
        let first = files[0];

        // Determine the person folder name:
        // - If primary_person_id is set, use person.folder_name (or person_id as fallback)
        // - Otherwise, use "unsorted"
        let person_folder = match &first.primary_person_id {
            Some(pid) => first
                .person_folder_name
                .clone()
                .unwrap_or_else(|| pid.clone()),
            None => "unsorted".to_string(),
        };

        // Determine the folder_number subfolder. If NULL, use "000" as a fallback
        // (shots without an assigned folder_number haven't been through assign_folder_numbers yet)
        let folder_num_str = match first.folder_number {
            Some(n) => format!("{:03}", n),
            None => "000".to_string(),
        };

        // Target: library/<person_folder_name>/<folder_number>/
        let target_dir = library.join(&person_folder).join(&folder_num_str);

        for file_row in files {
            let current_path = PathBuf::from(&file_row.path);
            pb.set_message(
                current_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );

            // Compute target path preserving the original filename
            let filename = current_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("{}.unknown", file_row.file_id));

            let mut target_path = target_dir.join(&filename);

            // Handle filename collisions (skip if it would collide with itself)
            if target_path != current_path && target_path.exists() {
                let stem = current_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file");
                let ext = current_path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                let mut i = 1u32;
                loop {
                    let candidate = if ext.is_empty() {
                        target_dir.join(format!("{}_{}", stem, i))
                    } else {
                        target_dir.join(format!("{}_{}.{}", stem, i, ext))
                    };
                    if !candidate.exists() || candidate == current_path {
                        target_path = candidate;
                        break;
                    }
                    i += 1;
                }
            }

            // Compare paths
            if target_path == current_path {
                if dry_run {
                    pb.println(format!("SKIP: {} (already correct)", file_row.path));
                }
                skipped += 1;
                pb.inc(1);
                continue;
            }

            if dry_run {
                pb.println(format!(
                    "MOVE: {} -> {}",
                    file_row.path,
                    target_path.to_string_lossy()
                ));
                moved += 1;
                pb.inc(1);
                continue;
            }

            // Actually move the file
            if let Err(e) = (|| -> anyhow::Result<()> {
                fs::create_dir_all(&target_dir)?;

                // Try rename first (same filesystem), fall back to copy+delete
                if fs::rename(&current_path, &target_path).is_err() {
                    fs::copy(&current_path, &target_path)?;
                    fs::remove_file(&current_path)?;
                }

                // Update DB path
                conn.execute(
                    "UPDATE files SET path = ? WHERE id = ?",
                    params![target_path.to_string_lossy().as_ref(), file_row.file_id],
                )?;

                Ok(())
            })() {
                error!("Reorganize failed for {}: {}", file_row.path, e);
                pb.println(format!("ERROR: {} -- {}", file_row.path, e));
                errors += 1;
                pb.inc(1);
                continue;
            }

            moved += 1;
            pb.inc(1);
        }
    }

    pb.finish_and_clear();

    // Clean up orphaned people (persons with no remaining faces)
    let orphaned = conn.execute(
        "UPDATE shots SET primary_person_id = NULL \
         WHERE primary_person_id IN ( \
             SELECT p.id FROM people p \
             WHERE NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = p.id) \
         )",
        [],
    )?;
    let deleted_people = conn.execute(
        "DELETE FROM people WHERE NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = people.id)",
        [],
    )?;
    if deleted_people > 0 {
        info!(
            "Removed {} orphaned people ({} shot references cleared)",
            deleted_people, orphaned
        );
    }

    // Clean up empty directories (but never the library root or .phos.db)
    if !dry_run {
        println!("Cleaning up empty directories...");
        cleanup_empty_dirs(library)?;
    }

    // Count unique people
    let people_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))
        .unwrap_or(0);

    println!();
    println!("=== Reorganize Summary ===");
    println!("  Files moved:     {}", moved);
    println!("  Files unchanged: {}", skipped);
    if errors > 0 {
        println!("  Errors:          {}", errors);
    }
    println!("  People count:    {}", people_count);
    if dry_run {
        println!("  (dry run -- no files were actually moved)");
    }
    println!("==========================");

    Ok(())
}

/// Remove empty directories under `root`, walking bottom-up.
/// Never removes the root directory itself.
fn cleanup_empty_dirs(root: &Path) -> anyhow::Result<()> {
    let mut dirs: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
        .map(|e| e.path().to_path_buf())
        .filter(|p| p != root)
        .collect();

    // Sort by path length descending so we process deepest dirs first
    dirs.sort_by(|a, b| b.as_os_str().len().cmp(&a.as_os_str().len()));

    for dir in dirs {
        if let Ok(mut entries) = fs::read_dir(&dir) {
            if entries.next().is_none() {
                fs::remove_dir(&dir).ok();
            }
        }
    }

    Ok(())
}
