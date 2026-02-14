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

pub fn run_import(source: &Path, target: &Path, move_files: bool) -> anyhow::Result<()> {
    // AI is mandatory for import — reject dummy mode
    if std::env::var("PHOS_DUMMY_AI").is_ok() {
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

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(scanner::half_available_threads())
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
        image::open(source_path).ok()
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
    let scan_conn = scanner.open_db()?;
    scanner.process_file(&scan_conn, &target_path)?;

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

/// Reorganize files on disk to match current face clustering.
///
/// Opens the `.phos.db` in the library, re-clusters faces, then moves each file
/// into `library/<person_id>/filename` (or `library/unsorted/filename` if no
/// person is associated). Updates `files.path` in the DB after each move and
/// cleans up empty directories afterwards.
pub fn run_reorganize(library: &Path, dry_run: bool) -> anyhow::Result<()> {
    let db_path = library.join(".phos.db");
    if !db_path.exists() {
        anyhow::bail!("No .phos.db found in {:?}", library);
    }

    let conn = db::init_db(&db_path)?;
    info!("Database opened at {:?}", db_path);

    // Re-cluster faces (no AI models needed — uses existing embeddings)
    let scanner = Scanner::new(db_path.clone(), None);
    info!("Running face clustering...");
    scanner.cluster_faces(&conn)?;

    // Load all photos with their files from the DB
    // Group files by photo_id so we maintain <person>/<photo>/<files> structure
    let mut stmt = conn.prepare(
        "SELECT p.id, f.id, f.path FROM photos p JOIN files f ON f.photo_id = p.id"
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |row| {
            let photo_id: String = row.get(0)?;
            let file_id: String = row.get(1)?;
            let path: String = row.get(2)?;
            Ok((photo_id, file_id, path))
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Group by photo_id
    let mut photos: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    for (photo_id, file_id, path) in &rows {
        photos
            .entry(photo_id.clone())
            .or_default()
            .push((file_id.clone(), path.clone()));
    }

    let total = rows.len() as u64;
    let action = if dry_run { "Planning" } else { "Reorganizing" };
    println!("{} {} files across {} photos...", action, total, photos.len());

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

    for (photo_id, files) in &photos {
        // Find the primary person for this photo:
        // Look across ALL files of this photo, pick face with highest score / largest bbox
        let file_ids: Vec<&str> = files.iter().map(|(fid, _)| fid.as_str()).collect();
        let placeholders: String = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT person_id FROM faces WHERE file_id IN ({}) AND person_id IS NOT NULL \
             ORDER BY score DESC, (box_x2 - box_x1) * (box_y2 - box_y1) DESC LIMIT 1",
            placeholders
        );
        let mut person_stmt = conn.prepare(&query)?;
        let primary_person: Option<String> = person_stmt
            .query_row(
                rusqlite::params_from_iter(file_ids.iter()),
                |row| row.get(0),
            )
            .ok();

        // Target: library/<person_id>/<photo_id>/ or library/unsorted/<photo_id>/
        let person_dir = match &primary_person {
            Some(person_id) => library.join(person_id),
            None => library.join("unsorted"),
        };
        let target_dir = person_dir.join(photo_id);

        for (file_id, current_path_str) in files {
            let current_path = PathBuf::from(current_path_str);
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
                .unwrap_or_else(|| format!("{}.unknown", file_id));

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
                    pb.println(format!("SKIP: {} (already correct)", current_path_str));
                }
                skipped += 1;
                pb.inc(1);
                continue;
            }

            if dry_run {
                pb.println(format!(
                    "MOVE: {} → {}",
                    current_path_str,
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
                    params![target_path.to_string_lossy().as_ref(), file_id],
                )?;

                Ok(())
            })() {
                pb.println(format!("ERROR: {} — {}", current_path_str, e));
                errors += 1;
                pb.inc(1);
                continue;
            }

            moved += 1;
            pb.inc(1);
        }
    }

    pb.finish_and_clear();

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
        println!("  (dry run — no files were actually moved)");
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
