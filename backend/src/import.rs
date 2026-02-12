use crate::ai::AiPipeline;
use crate::db;
use crate::scanner::{self, Scanner};
use image::DynamicImage;
use indicatif::{ProgressBar, ProgressStyle};
use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};
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

/// Import summary statistics.
struct ImportStats {
    total: u64,
    imported: u64,
    variations: u64,
    skipped_duplicate: u64,
    skipped_error: u64,
    people_detected: usize,
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

    let mut stats = ImportStats {
        total,
        imported: 0,
        variations: 0,
        skipped_duplicate: 0,
        skipped_error: 0,
        people_detected: 0,
    };

    // Load dHash cache (starts empty, grows as we import)
    let mut dhash_cache = load_dhash_cache(&conn);

    for source_path in &source_files {
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
            &conn,
            &scanner,
            &mut dhash_cache,
            &mut stats,
        ) {
            Ok(()) => {}
            Err(e) => {
                error!("Error importing {:?}: {}", source_path, e);
                stats.skipped_error += 1;
            }
        }

        pb.inc(1);
    }

    // Count unique people
    let people_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))
        .unwrap_or(0);
    stats.people_detected = people_count as usize;

    pb.finish_with_message("Import complete!");

    // Print summary
    println!();
    println!("=== Import Summary ===");
    println!("  Total files found:     {}", stats.total);
    println!("  Successfully imported: {}", stats.imported);
    println!("  Grouped as variations: {}", stats.variations);
    println!("  Skipped (duplicate):   {}", stats.skipped_duplicate);
    println!("  Skipped (error):       {}", stats.skipped_error);
    println!("  People detected:       {}", stats.people_detected);
    println!("======================");

    Ok(())
}

fn import_single_file(
    source_path: &Path,
    target: &Path,
    move_files: bool,
    conn: &Connection,
    scanner: &Scanner,
    dhash_cache: &mut Vec<StoredFileRecord>,
    stats: &mut ImportStats,
) -> anyhow::Result<()> {
    // 1. SHA256 hash — check for exact duplicate
    let hash = scanner::calculate_hash(source_path)?;

    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM files WHERE hash = ?",
            params![hash],
            |row| row.get(0),
        )
        .ok();

    if existing.is_some() {
        warn!("Skipping exact duplicate: {:?} (hash {})", source_path, hash);
        stats.skipped_duplicate += 1;
        return Ok(());
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

        // Check against all stored dHashes
        let variation_match = dhash_cache.iter().find(|stored| {
            hamming_distance(&dhash, &stored.dhash) <= 10
        });

        if let Some(matched) = variation_match {
            // This is a variation — place in the same folder as the match
            if let Some((_person_dir, counter_dir)) = parse_target_folder(&matched.path) {
                target_dir = counter_dir;
                stats.variations += 1;
            } else {
                // Fallback: couldn't parse path, treat as new
                target_dir = determine_person_folder(img, target, conn, scanner)?;
            }
        } else {
            // 4. Not a variation — run face detection for person assignment
            target_dir = determine_person_folder(img, target, conn, scanner)?;
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

    // Update dHash cache with the newly imported file
    if let Some(ref img) = img {
        let dhash = scanner::compute_dhash(img);
        dhash_cache.push(StoredFileRecord {
            path: target_path.to_string_lossy().to_string(),
            dhash,
        });
    }

    stats.imported += 1;
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
                let face_chip = img.crop_imm(x1, y1, face_w, face_h);
                let embedding = ai.extract_embedding(&face_chip).unwrap_or_default();

                if !embedding.is_empty() {
                    let person_id = Scanner::find_or_create_person(conn, &embedding)?;
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

/// Access the AI pipeline from a Scanner instance.
/// This uses a helper that exposes the AI reference.
fn get_ai_from_scanner(scanner: &Scanner) -> Option<&AiPipeline> {
    scanner.ai()
}
