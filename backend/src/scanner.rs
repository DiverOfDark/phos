use crate::ai::{cosine_similarity, AiPipeline, MAX_FACE_DISTANCE};
use crate::db;
use exif::{In, Tag};
use ffmpeg_next as ffmpeg;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, RgbImage};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};
use uuid::Uuid;
use walkdir::WalkDir;

/// Open an image file using FFmpeg, falling back to the `image` crate.
///
/// FFmpeg handles a wider range of formats (extended WebP, HEIC, AVIF, etc.)
/// but its pipe-based demuxers sometimes fail for formats like PNG where codec
/// parameters can't be determined without extra probing.  In those cases we
/// fall back to the `image` crate which handles PNG/JPEG/GIF/BMP well.
///
/// After loading, applies EXIF orientation so the image is correctly rotated.
pub fn open_image(path: &Path) -> anyhow::Result<DynamicImage> {
    let img = match extract_first_video_frame(path) {
        Ok(img) => img,
        Err(ffmpeg_err) => {
            debug!(
                "FFmpeg failed to open {:?} ({}), trying image crate fallback",
                path, ffmpeg_err
            );
            image::open(path).map_err(|image_err| {
                anyhow::anyhow!(
                    "Failed to open image {:?}: FFmpeg: {}, image crate: {}",
                    path,
                    ffmpeg_err,
                    image_err
                )
            })?
        }
    };
    Ok(apply_exif_orientation(img, path))
}

/// Read the EXIF orientation tag and transform the image accordingly.
fn apply_exif_orientation(img: DynamicImage, path: &Path) -> DynamicImage {
    let orientation = (|| -> Option<u32> {
        let file = File::open(path).ok()?;
        let mut bufreader = BufReader::new(file);
        let exif = exif::Reader::new()
            .read_from_container(&mut bufreader)
            .ok()?;
        let field = exif.get_field(Tag::Orientation, In::PRIMARY)?;
        field.value.get_uint(0)
    })();

    match orientation {
        Some(2) => img.fliph(),
        Some(3) => img.rotate180(),
        Some(4) => img.flipv(),
        Some(5) => img.rotate90().fliph(),
        Some(6) => img.rotate90(),
        Some(7) => img.rotate270().fliph(),
        Some(8) => img.rotate270(),
        _ => img, // 1 or unknown — no rotation needed
    }
}

/// Compute a difference hash (dHash) for a given image.
///
/// Resizes the image to 9x8 grayscale, then compares each pixel to its right
/// neighbour, producing 64 bits (8 bytes). This is a simple perceptual hash
/// that is resilient to minor scaling and compression changes.
pub fn compute_dhash(img: &DynamicImage) -> [u8; 8] {
    let gray = img.resize_exact(9, 8, FilterType::Triangle).to_luma8();
    let mut hash = [0u8; 8];
    let mut bit_index: usize = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let left = gray.get_pixel(x, y)[0];
            let right = gray.get_pixel(x + 1, y)[0];
            if left > right {
                hash[bit_index / 8] |= 1 << (7 - (bit_index % 8));
            }
            bit_index += 1;
        }
    }
    hash
}

/// Hamming distance between two 8-byte dHash values (out of 64 bits).
pub fn hamming_distance(a: &[u8; 8], b: &[u8; 8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

/// Entry in the in-memory dHash cache used for shot grouping during scanning.
pub struct DHashCacheEntry {
    pub shot_id: String,
    pub dhash: [u8; 8],
}

pub struct Scanner {
    db_path: PathBuf,
    ai: Option<Arc<AiPipeline>>,
}

impl Scanner {
    pub fn new(db_path: PathBuf, ai: Option<AiPipeline>) -> Self {
        Self {
            db_path,
            ai: ai.map(Arc::new),
        }
    }

    /// Create a scanner that shares an existing AI pipeline but uses a different DB path.
    pub fn with_db_path(&self, db_path: PathBuf) -> Self {
        Self {
            db_path,
            ai: self.ai.clone(),
        }
    }

    /// Access the AI pipeline, if loaded.
    pub fn ai(&self) -> Option<&AiPipeline> {
        self.ai.as_deref()
    }

    /// Open a connection to the scanner's database with WAL mode and busy timeout.
    pub fn open_db(&self) -> anyhow::Result<Connection> {
        Ok(db::open_connection(&self.db_path)?)
    }

    /// Recompute SHA256 hashes for all files in the DB.
    /// Updates the hash if it changed; removes the record if the file is missing from disk.
    pub fn rehash_files(&self) -> anyhow::Result<()> {
        let conn = self.open_db()?;
        let library_root = self.db_path.parent().unwrap();

        let mut stmt = conn.prepare("SELECT id, path, hash FROM files")?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let total = rows.len();
        info!("Rehashing {} files...", total);

        let pb = ProgressBar::new(total as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                .unwrap()
                .progress_chars("=>-"),
        );

        let mut updated = 0u64;
        let mut removed = 0u64;

        for (file_id, file_path, old_hash) in &rows {
            let path = db::resolve_path(library_root, file_path);
            pb.set_message(
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );

            if !path.exists() {
                // File missing from disk — remove from DB
                let _ = conn.execute("DELETE FROM faces WHERE file_id = ?", params![file_id]);
                let _ = conn.execute(
                    "DELETE FROM video_keyframes WHERE video_file_id = ?",
                    params![file_id],
                );
                let _ = conn.execute("DELETE FROM files WHERE id = ?", params![file_id]);
                removed += 1;
                pb.inc(1);
                continue;
            }

            match calculate_hash(&path) {
                Ok(new_hash) => {
                    if new_hash != *old_hash {
                        let _ = conn.execute(
                            "UPDATE files SET hash = ? WHERE id = ?",
                            params![new_hash, file_id],
                        );
                        updated += 1;
                    }
                }
                Err(e) => {
                    error!("Failed to hash {:?}: {}", path, e);
                }
            }
            pb.inc(1);
        }

        pb.finish_and_clear();

        if updated > 0 || removed > 0 {
            info!(
                "Rehash complete: {} updated, {} removed (of {} total)",
                updated, removed, total
            );
        } else {
            info!("Rehash complete: all {} hashes up to date", total);
        }

        // Clean up orphaned shots (no files remaining)
        let orphaned_shots = conn.execute(
            "DELETE FROM shots WHERE id NOT IN (SELECT DISTINCT shot_id FROM files)",
            [],
        )?;
        if orphaned_shots > 0 {
            info!("Removed {} orphaned shots", orphaned_shots);
        }

        // Clean up orphaned people (no faces remaining)
        let _ = conn.execute(
            "UPDATE shots SET primary_person_id = NULL WHERE primary_person_id IN (SELECT p.id FROM people p WHERE NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = p.id))",
            [],
        );
        let orphaned_people = conn.execute(
            "DELETE FROM people WHERE NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = people.id)",
            [],
        )?;
        if orphaned_people > 0 {
            info!("Removed {} orphaned people", orphaned_people);
        }

        Ok(())
    }

    pub fn scan(&self, root: &Path) -> anyhow::Result<()> {
        let files: Vec<PathBuf> = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| {
                // Skip .phos* directories (thumbnails cache, db files)
                if e.file_type().is_dir() {
                    if let Some(name) = e.file_name().to_str() {
                        return !name.starts_with(".phos") && name != ".duplicates";
                    }
                }
                true
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_media_file(e.path()))
            .map(|e| e.path().to_path_buf())
            .collect();

        // Build an in-memory dHash cache from existing files in the DB
        let dhash_cache = {
            let conn = self.open_db()?;
            let mut stmt = conn.prepare(
                "SELECT f.shot_id, f.visual_embedding FROM files f WHERE f.visual_embedding IS NOT NULL"
            )?;
            let entries: Vec<DHashCacheEntry> = stmt
                .query_map([], |row| {
                    let shot_id: String = row.get(0)?;
                    let blob: Vec<u8> = row.get(1)?;
                    Ok((shot_id, blob))
                })?
                .filter_map(|r| r.ok())
                .filter_map(|(shot_id, blob)| {
                    if blob.len() == 8 {
                        let mut dhash = [0u8; 8];
                        dhash.copy_from_slice(&blob);
                        Some(DHashCacheEntry { shot_id, dhash })
                    } else {
                        None
                    }
                })
                .collect();
            std::sync::Mutex::new(entries)
        };

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(half_available_threads())
            .build()?;

        pool.install(|| {
            files.par_iter().for_each(|path| {
                let conn = match self.open_db() {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to open DB: {}", e);
                        return;
                    }
                };
                if let Err(e) = self.process_file(&conn, path, &dhash_cache) {
                    error!("Error processing {:?}: {}", path, e);
                }
            });
        });

        // After all files are processed, run face clustering
        let conn = self.open_db()?;
        self.cluster_faces(&conn)?;

        // Assign primary person to each shot based on largest face
        assign_primary_persons(&conn)?;

        // Assign folder numbers to shots that don't have one yet
        assign_folder_numbers(&conn)?;

        // Compact folder numbers to remove gaps from reassignments
        compact_folder_numbers(&conn)?;

        // Clean up empty directories left behind by duplicate moves or deletions
        crate::import::cleanup_empty_dirs(root)?;

        Ok(())
    }

    /// Remove a file from the database by its filesystem path.
    ///
    /// Deletes associated faces, video keyframes, and the file record itself.
    /// If the parent shot has no remaining files, the shot record is also removed.
    /// Orphaned person records (those with no remaining faces) are cleaned up too.
    pub fn remove_file(&self, conn: &Connection, path: &Path) -> anyhow::Result<()> {
        let library_root = self.db_path.parent().unwrap();
        let path_str = db::make_relative(library_root, path);

        // Look up the file by path
        let (file_id, shot_id): (String, String) = conn
            .query_row(
                "SELECT id, shot_id FROM files WHERE path = ?",
                params![path_str],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| anyhow::anyhow!("File not found in DB: {:?}", path))?;

        // Collect person_ids that might become orphaned after face deletion
        let mut stmt = conn.prepare(
            "SELECT DISTINCT person_id FROM faces WHERE file_id = ? AND person_id IS NOT NULL",
        )?;
        let affected_person_ids: Vec<String> = stmt
            .query_map(params![file_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        // Delete associated faces
        conn.execute("DELETE FROM faces WHERE file_id = ?", params![file_id])?;
        debug!("Deleted faces for file {}", file_id);

        // Delete associated video keyframes
        conn.execute(
            "DELETE FROM video_keyframes WHERE video_file_id = ?",
            params![file_id],
        )?;
        debug!("Deleted video keyframes for file {}", file_id);

        // Delete the file record
        conn.execute("DELETE FROM files WHERE id = ?", params![file_id])?;
        info!("Removed file record {} for {:?}", file_id, path);

        // Check if the shot has any remaining files
        let remaining_files: i64 = conn.query_row(
            "SELECT COUNT(*) FROM files WHERE shot_id = ?",
            params![shot_id],
            |row| row.get(0),
        )?;

        if remaining_files == 0 {
            conn.execute("DELETE FROM shots WHERE id = ?", params![shot_id])?;
            info!("Removed orphaned shot record {}", shot_id);
        }

        // Clean up orphaned person records (persons with no remaining faces)
        for person_id in &affected_person_ids {
            let face_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM faces WHERE person_id = ?",
                params![person_id],
                |row| row.get(0),
            )?;
            if face_count == 0 {
                conn.execute(
                    "UPDATE shots SET primary_person_id = NULL WHERE primary_person_id = ?",
                    params![person_id],
                )?;
                conn.execute("DELETE FROM people WHERE id = ?", params![person_id])?;
                info!("Removed orphaned person record {}", person_id);
            }
        }

        Ok(())
    }

    /// Cluster unassigned faces using centroid-based assignment.
    ///
    /// Each face is compared against existing person representative embeddings (centroids).
    /// If it matches within `MAX_FACE_DISTANCE`, it's assigned to that person and the
    /// centroid is updated as a running average. Otherwise a new person is created.
    /// This is O(n × k) where k = number of people, instead of the previous O(n²) pairwise approach.
    pub fn cluster_faces(&self, conn: &Connection) -> anyhow::Result<()> {
        // Load unassigned faces with embeddings
        let mut stmt = conn.prepare(
            "SELECT id, embedding FROM faces WHERE embedding IS NOT NULL AND person_id IS NULL",
        )?;
        let unassigned: Vec<(String, Vec<f32>)> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(id, blob)| {
                bincode::deserialize::<Vec<f32>>(&blob)
                    .ok()
                    .filter(|e| !e.is_empty())
                    .map(|embedding| (id, embedding))
            })
            .collect();

        if unassigned.is_empty() {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM faces WHERE embedding IS NOT NULL",
                [],
                |row| row.get(0),
            )?;
            info!("All {} faces already assigned, nothing to cluster", total);
            return Ok(());
        }

        info!("Clustering: {} unassigned faces", unassigned.len());

        // Load existing people centroids: (person_id, embedding_sum, face_count)
        // We track the unnormalized sum so we can update it incrementally.
        // cosine_similarity normalizes internally, so comparisons still work correctly.
        let mut centroids: Vec<(String, Vec<f32>, usize)> = {
            let mut pstmt = conn.prepare(
                "SELECT p.id, p.representative_embedding, \
                 (SELECT COUNT(*) FROM faces f WHERE f.person_id = p.id) \
                 FROM people p WHERE p.representative_embedding IS NOT NULL",
            )?;
            let rows: Vec<(String, Vec<u8>, usize)> = pstmt
                .query_map([], |row| {
                    let pid: String = row.get(0)?;
                    let blob: Vec<u8> = row.get(1)?;
                    let count: usize = row.get(2)?;
                    Ok((pid, blob, count))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows.into_iter()
                .filter_map(|(pid, blob, count)| {
                    bincode::deserialize::<Vec<f32>>(&blob)
                        .ok()
                        .filter(|e| !e.is_empty())
                        .map(|e| {
                            // Convert normalized centroid back to sum for running average
                            let sum: Vec<f32> =
                                e.iter().map(|v| v * count as f32).collect();
                            (pid, sum, count)
                        })
                })
                .collect()
        };

        // Also ensure people without representative_embedding get one computed
        // from their existing faces (handles old data from before this migration)
        {
            let orphan_pids: Vec<String> = conn
                .prepare(
                    "SELECT id FROM people WHERE representative_embedding IS NULL \
                     AND EXISTS (SELECT 1 FROM faces WHERE person_id = people.id AND embedding IS NOT NULL)",
                )?
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            for pid in orphan_pids {
                let face_embs: Vec<Vec<f32>> = conn
                    .prepare("SELECT embedding FROM faces WHERE person_id = ? AND embedding IS NOT NULL")?
                    .query_map(params![&pid], |row| {
                        let blob: Vec<u8> = row.get(0)?;
                        Ok(blob)
                    })?
                    .filter_map(|r| r.ok())
                    .filter_map(|blob| {
                        bincode::deserialize::<Vec<f32>>(&blob)
                            .ok()
                            .filter(|e| !e.is_empty())
                    })
                    .collect();

                if face_embs.is_empty() {
                    continue;
                }

                let dim = face_embs[0].len();
                let mut sum = vec![0.0f32; dim];
                for emb in &face_embs {
                    for (i, v) in emb.iter().enumerate() {
                        sum[i] += v;
                    }
                }
                let count = face_embs.len();

                // Save normalized centroid to DB
                let mean: Vec<f32> = sum.iter().map(|v| v / count as f32).collect();
                let norm: f32 = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 0.0 {
                    let normalized: Vec<f32> = mean.iter().map(|v| v / norm).collect();
                    let blob = bincode::serialize(&normalized)?;
                    conn.execute(
                        "UPDATE people SET representative_embedding = ? WHERE id = ?",
                        params![blob, &pid],
                    )?;
                }

                centroids.push((pid, sum, count));
            }
        }

        let pb = ProgressBar::new(unassigned.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                .unwrap()
                .progress_chars("=>-"),
        );
        pb.set_message("Assigning faces to people");

        let mut update_stmt = conn.prepare("UPDATE faces SET person_id = ? WHERE id = ?")?;
        let mut affected_people: HashSet<String> = HashSet::new();
        let mut assigned_count = 0usize;

        for (face_id, embedding) in &unassigned {
            // Find best matching person centroid
            let mut best: Option<(usize, f32)> = None;
            for (idx, (_, centroid_sum, _)) in centroids.iter().enumerate() {
                if centroid_sum.len() != embedding.len() {
                    continue;
                }
                // cosine_similarity normalizes internally, so using the sum works
                let dist = 1.0 - cosine_similarity(embedding, centroid_sum);
                if dist <= MAX_FACE_DISTANCE {
                    if let Some((_, best_dist)) = best {
                        if dist < best_dist {
                            best = Some((idx, dist));
                        }
                    } else {
                        best = Some((idx, dist));
                    }
                }
            }

            let person_id = if let Some((idx, _)) = best {
                // Assign to existing person and update running centroid
                let (ref pid, ref mut sum, ref mut count) = centroids[idx];
                for (i, v) in embedding.iter().enumerate() {
                    sum[i] += v;
                }
                *count += 1;
                pid.clone()
            } else {
                // No match — create a new person
                let pid = Uuid::new_v4().to_string();
                let emb_blob = bincode::serialize(embedding)?;
                conn.execute(
                    "INSERT INTO people (id, thumbnail_face_id, representative_embedding) VALUES (?, ?, ?)",
                    params![&pid, face_id, emb_blob],
                )?;
                centroids.push((pid.clone(), embedding.clone(), 1));
                pid
            };

            update_stmt.execute(params![&person_id, face_id])?;
            affected_people.insert(person_id);
            assigned_count += 1;
            pb.inc(1);
        }

        pb.finish_and_clear();

        // Recompute and save representative embeddings for all affected people
        for (pid, sum, count) in &centroids {
            if !affected_people.contains(pid) {
                continue;
            }
            if *count == 0 {
                continue;
            }
            let mean: Vec<f32> = sum.iter().map(|v| v / *count as f32).collect();
            let norm: f32 = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 0.0 {
                let normalized: Vec<f32> = mean.iter().map(|v| v / norm).collect();
                let blob = bincode::serialize(&normalized)?;
                conn.execute(
                    "UPDATE people SET representative_embedding = ? WHERE id = ?",
                    params![blob, pid],
                )?;
            }
        }

        // Update thumbnail_face_id for each person to the first face
        let person_ids: Vec<String> = conn
            .prepare("SELECT id FROM people")?
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        for pid in &person_ids {
            let first_face: Option<String> = conn
                .query_row(
                    "SELECT id FROM faces WHERE person_id = ? LIMIT 1",
                    params![pid],
                    |row| row.get(0),
                )
                .ok();
            if let Some(fid) = first_face {
                conn.execute(
                    "UPDATE people SET thumbnail_face_id = ? WHERE id = ?",
                    params![fid, pid],
                )?;
            }
        }

        // Clean up people with no remaining faces
        // First clear stale shots.primary_person_id references to avoid FK violations
        conn.execute(
            "UPDATE shots SET primary_person_id = NULL \
             WHERE primary_person_id IN ( \
                 SELECT p.id FROM people p \
                 WHERE NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = p.id) \
             )",
            [],
        )?;
        conn.execute(
            "DELETE FROM people \
             WHERE NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = people.id)",
            [],
        )?;

        let person_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;
        info!(
            "Clustering complete: {} faces assigned to {} persons",
            assigned_count, person_count
        );

        Ok(())
    }

    pub fn process_file(
        &self,
        conn: &Connection,
        path: &Path,
        dhash_cache: &std::sync::Mutex<Vec<DHashCacheEntry>>,
    ) -> anyhow::Result<()> {
        // --- Phase 1: CPU-heavy work (no DB writes) ---
        let library_root = self.db_path.parent().unwrap();
        let relative_path = db::make_relative(library_root, path);

        // Quick path duplicate check — catches concurrent processing of the
        // same file (e.g. upload handler + watcher race).
        {
            let mut stmt = conn.prepare("SELECT id FROM files WHERE path = ?")?;
            let mut rows = stmt.query(params![&relative_path])?;
            if rows.next()?.is_some() {
                debug!("File already indexed at path {:?}, skipping", path);
                return Ok(());
            }
        }

        let hash = calculate_hash(path)?;

        // Hash duplicate check — same content at a different path.
        // Move the duplicate into {root}/.duplicates/ preserving relative path.
        {
            let mut stmt = conn.prepare("SELECT id, path FROM files WHERE hash = ?")?;
            let mut rows = stmt.query(params![hash])?;
            if let Some(row) = rows.next()? {
                let existing_id: String = row.get(0)?;
                let existing_path: String = row.get(1)?;

                let root_dir = self.db_path.parent().unwrap();
                let rel = path
                    .strip_prefix(root_dir)
                    .unwrap_or(path);
                let duplicates_dir = root_dir.join(".duplicates");
                let mut target_path = duplicates_dir.join(rel);

                // Handle filename collisions
                if target_path.exists() {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("file");
                    let ext = path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    let target_dir = target_path.parent().unwrap().to_path_buf();
                    let mut i = 1u32;
                    loop {
                        let candidate = if ext.is_empty() {
                            target_dir.join(format!("{}_{}", stem, i))
                        } else {
                            target_dir.join(format!("{}_{}.{}", stem, i, ext))
                        };
                        if !candidate.exists() {
                            target_path = candidate;
                            break;
                        }
                        i += 1;
                    }
                }

                if let Some(parent) = target_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                // Try rename first (same filesystem), fall back to copy+delete
                if std::fs::rename(path, &target_path).is_err() {
                    std::fs::copy(path, &target_path)?;
                    std::fs::remove_file(path)?;
                }

                info!(
                    "Duplicate of {} (id {}) moved: {} -> {}",
                    existing_path,
                    existing_id,
                    path.display(),
                    target_path.display()
                );
                return Ok(());
            }
        }

        let id = Uuid::new_v4().to_string();
        let shot_id = Uuid::new_v4().to_string();

        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let mime_type = match ext.to_lowercase().as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "webp" => "image/webp",
            "mp4" => "video/mp4",
            "mov" => "video/quicktime",
            "mkv" => "video/x-matroska",
            "avi" => "video/x-msvideo",
            "webm" => "video/webm",
            _ => "application/octet-stream",
        };

        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;

        let (width, height) = if mime_type.starts_with("image/") || mime_type.starts_with("video/")
        {
            get_video_dimensions(path)
        } else {
            (None, None)
        };

        // EXIF metadata (images only)
        let exif_data = if mime_type.starts_with("image/") {
            let (ts, lat, lon) = extract_exif_metadata(path);
            if ts.is_some() || lat.is_some() || lon.is_some() {
                Some((ts, lat, lon))
            } else {
                None
            }
        } else {
            None
        };

        // Load image/video frame once and reuse for both dHash and face detection.
        // Previously the image was loaded twice (once for dHash, once for faces),
        // doubling per-thread memory usage.
        let mut image_faces: Vec<FaceResult> = Vec::new();
        let mut keyframe_results: Vec<KeyframeResult> = Vec::new();

        let dhash: Option<[u8; 8]> = if mime_type.starts_with("image/") {
            let img = open_image(path).ok();
            let dhash = img.as_ref().map(|i| compute_dhash(i));

            if let (Some(ai), Some(ref img)) = (&self.ai, &img) {
                image_faces = detect_faces_collect(ai, img);
            }
            // img dropped here — single load for both operations
            dhash
        } else if mime_type.starts_with("video/") {
            // Extract first frame for dHash (lightweight — single frame)
            let dhash = extract_first_video_frame(path)
                .ok()
                .map(|frame| compute_dhash(&frame));

            if let Some(ai) = &self.ai {
                // Stream keyframes one at a time: each DynamicImage is dropped
                // after face detection, so only one frame is in memory at a time.
                let _ = for_each_video_keyframe(path, 5.0, |timestamp_ms, image| {
                    keyframe_results.push(KeyframeResult {
                        kf_id: Uuid::new_v4().to_string(),
                        timestamp_ms,
                        kf_path: format!("memory://keyframe_{}", timestamp_ms),
                        faces: detect_faces_collect(ai, &image),
                    });
                });
            }
            dhash
        } else {
            None
        };

        // --- Phase 2: dHash shot grouping ---
        // Check the dhash_cache for a match (Hamming distance <= 10).
        // If match found: add file to existing shot (is_original = false).
        // No match: create new shot (is_original = true).
        let (actual_shot_id, is_new_shot, is_original) = if let Some(ref file_dhash) = dhash {
            let cache = dhash_cache.lock().unwrap();
            let matched = cache
                .iter()
                .find(|entry| hamming_distance(&entry.dhash, file_dhash) <= 10);
            if let Some(entry) = matched {
                (entry.shot_id.clone(), false, false)
            } else {
                (shot_id.clone(), true, true)
            }
        } else {
            (shot_id.clone(), true, true)
        };

        // --- Phase 3: Write everything to DB in a single transaction ---

        conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> anyhow::Result<()> {
            if is_new_shot {
                conn.execute(
                    "INSERT INTO shots (id, main_file_id, width, height) VALUES (?, ?, ?, ?)",
                    params![actual_shot_id, id, width, height],
                )?;
            }

            conn.execute(
                "INSERT INTO files (id, shot_id, path, hash, mime_type, file_size, is_original) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![id, actual_shot_id, relative_path, hash, mime_type, file_size, is_original],
            )?;

            if is_new_shot {
                if let Some((ts, lat, lon)) = &exif_data {
                    conn.execute(
                        "UPDATE shots SET timestamp = ?, latitude = ?, longitude = ? WHERE id = ?",
                        params![ts, lat, lon, actual_shot_id],
                    )?;
                }
            }

            if let Some(dhash) = &dhash {
                conn.execute(
                    "UPDATE files SET visual_embedding = ? WHERE id = ?",
                    params![dhash.as_slice(), id],
                )?;
            }

            for face in &image_faces {
                conn.execute(
                    "INSERT INTO faces (id, file_id, person_id, box_x1, box_y1, box_x2, box_y2, embedding, score) VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?)",
                    params![face.face_id, id, face.box_x1, face.box_y1, face.box_x2, face.box_y2, face.embedding_blob, face.score],
                )?;
            }

            for kfr in &keyframe_results {
                conn.execute(
                    "INSERT INTO video_keyframes (id, video_file_id, timestamp_ms, path) VALUES (?, ?, ?, ?)",
                    params![kfr.kf_id, id, kfr.timestamp_ms, kfr.kf_path],
                )?;
                for face in &kfr.faces {
                    conn.execute(
                        "INSERT INTO faces (id, file_id, person_id, box_x1, box_y1, box_x2, box_y2, embedding, score) VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?)",
                        params![face.face_id, id, face.box_x1, face.box_y1, face.box_x2, face.box_y2, face.embedding_blob, face.score],
                    )?;
                }
            }

            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;

                // Add to the dHash cache so subsequent files can match against this one
                if let Some(ref file_dhash) = dhash {
                    let mut cache = dhash_cache.lock().unwrap();
                    cache.push(DHashCacheEntry {
                        shot_id: actual_shot_id.clone(),
                        dhash: *file_dhash,
                    });
                }

                info!("Indexed: {:?}", path);
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Generate captions for all shots that don't have one yet.
    /// Runs sequentially — ONNX sessions are behind Mutex so parallelism wouldn't help.
    pub fn caption_shots(&self, root: &Path) -> anyhow::Result<()> {
        let ai = match &self.ai {
            Some(ai) => ai,
            None => return Ok(()),
        };

        if !ai.has_captioning() {
            info!("Captioning models not available, skipping caption generation");
            return Ok(());
        }

        let conn = self.open_db()?;
        let library_root = self.db_path.parent().unwrap_or(root);

        // Find shots without descriptions, joined with their main file path
        let mut stmt = conn.prepare(
            "SELECT s.id, f.path FROM shots s
             JOIN files f ON s.main_file_id = f.id
             WHERE s.description IS NULL"
        )?;
        let shots: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        if shots.is_empty() {
            info!("All shots already have captions");
            return Ok(());
        }

        info!("Generating captions for {} shots", shots.len());

        let pb = ProgressBar::new(shots.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} captioning")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut update_stmt = conn.prepare(
            "UPDATE shots SET description = ? WHERE id = ?"
        )?;

        for (shot_id, file_path) in &shots {
            let abs_path = db::resolve_path(library_root, file_path);
            info!("Captioning shot {} ({:?})", shot_id, abs_path);
            let result = (|| -> anyhow::Result<String> {
                let img = open_image(&abs_path)?;
                ai.generate_caption(&img)
            })();

            match result {
                Ok(ref caption) => {
                    info!("Captioned shot {}: {:?}", shot_id, caption);
                    if let Err(e) = update_stmt.execute(rusqlite::params![caption, shot_id]) {
                        error!("Failed to save caption for shot {}: {}", shot_id, e);
                    }
                }
                Err(e) => {
                    error!("Failed to generate caption for shot {} ({:?}): {}", shot_id, abs_path, e);
                }
            }

            pb.inc(1);
        }

        pb.finish_with_message("Captioning complete");
        info!("Caption generation finished");
        Ok(())
    }
}

/// Assign `primary_person_id` for each shot based on the largest face with a person_id.
///
/// For each shot where `review_status != 'confirmed'`, finds the face with the
/// largest bounding box area `(box_x2 - box_x1) * (box_y2 - box_y1)` that has a
/// `person_id` assigned. Sets `shots.primary_person_id` to that person. If no
/// faces have a `person_id`, sets it to NULL (unsorted).
pub fn assign_primary_persons(conn: &Connection) -> anyhow::Result<()> {
    // Find shots that need primary person assignment (not confirmed by user)
    let mut shot_stmt = conn.prepare(
        "SELECT id, primary_person_id FROM shots WHERE review_status != 'confirmed' OR review_status IS NULL"
    )?;
    let shots: Vec<(String, Option<String>)> = shot_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    if shots.is_empty() {
        return Ok(());
    }

    // When person changes, also reset folder_number so assign_folder_numbers will reassign it
    let mut update_with_reset_stmt =
        conn.prepare("UPDATE shots SET primary_person_id = ?, folder_number = NULL WHERE id = ?")?;
    let mut update_same_stmt =
        conn.prepare("UPDATE shots SET primary_person_id = ? WHERE id = ?")?;

    // For each shot, find the face with the largest bbox area that has a person_id
    let mut face_stmt = conn.prepare(
        "SELECT f.person_id, (f.box_x2 - f.box_x1) * (f.box_y2 - f.box_y1) AS area
         FROM faces f
         JOIN files fl ON f.file_id = fl.id
         WHERE fl.shot_id = ? AND f.person_id IS NOT NULL
         ORDER BY area DESC
         LIMIT 1",
    )?;

    let mut assigned = 0;
    let mut cleared = 0;
    let mut reassigned = 0;
    for (shot_id, old_person_id) in &shots {
        let best_person: Option<String> =
            face_stmt.query_row(params![shot_id], |row| row.get(0)).ok();

        match &best_person {
            Some(_) => assigned += 1,
            None => cleared += 1,
        }

        if &best_person != old_person_id {
            // Person changed — reset folder_number so it gets reassigned
            update_with_reset_stmt.execute(params![best_person, shot_id])?;
            reassigned += 1;
        } else {
            update_same_stmt.execute(params![best_person, shot_id])?;
        }
    }

    info!(
        "Primary person assignment: {} shots assigned, {} set to unsorted (NULL), {} reassigned (folder_number reset)",
        assigned, cleared, reassigned
    );

    Ok(())
}

/// Assign sequential `folder_number` values to shots that don't have one yet.
///
/// For each shot with a `primary_person_id` but NULL `folder_number`, assigns
/// `MAX(folder_number) + 1` for that person. Unsorted shots (NULL primary_person_id)
/// are treated as their own separate namespace.
pub fn assign_folder_numbers(conn: &Connection) -> anyhow::Result<()> {
    // Get all shots needing folder number assignment, grouped by primary_person_id
    // We handle NULL primary_person_id (unsorted) as a separate group
    let mut shots_stmt = conn.prepare(
        "SELECT id, primary_person_id FROM shots WHERE folder_number IS NULL ORDER BY created_at",
    )?;
    let shots: Vec<(String, Option<String>)> = shots_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    if shots.is_empty() {
        return Ok(());
    }

    let mut update_stmt = conn.prepare("UPDATE shots SET folder_number = ? WHERE id = ?")?;

    // Cache the current max folder_number per person (including NULL for unsorted)
    let mut max_numbers: HashMap<Option<String>, i64> = HashMap::new();

    // Load existing max folder numbers for each person
    let mut max_stmt = conn.prepare(
        "SELECT primary_person_id, MAX(folder_number) FROM shots WHERE folder_number IS NOT NULL GROUP BY primary_person_id"
    )?;
    let existing_maxes: Vec<(Option<String>, i64)> = max_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    for (person_id, max_num) in existing_maxes {
        max_numbers.insert(person_id, max_num);
    }

    let mut total_assigned = 0;
    for (shot_id, person_id) in &shots {
        let next_number = max_numbers.get(person_id).map(|n| n + 1).unwrap_or(1);

        update_stmt.execute(params![next_number, shot_id])?;
        max_numbers.insert(person_id.clone(), next_number);
        total_assigned += 1;
    }

    info!(
        "Folder number assignment: {} shots assigned numbers",
        total_assigned
    );

    Ok(())
}

/// Compact folder numbers so they are sequential (1, 2, 3, ...) per person, removing gaps.
///
/// For each person (including NULL for unsorted), queries all shots ordered by
/// current `folder_number` (with `created_at` as tiebreaker), then reassigns
/// folder_numbers as 1, 2, 3, ... Only updates shots whose number actually changed.
pub fn compact_folder_numbers(conn: &Connection) -> anyhow::Result<()> {
    // Get all distinct person IDs (including NULL for unsorted)
    let mut person_stmt = conn
        .prepare("SELECT DISTINCT primary_person_id FROM shots WHERE folder_number IS NOT NULL")?;
    let person_ids: Vec<Option<String>> = person_stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut update_stmt = conn.prepare("UPDATE shots SET folder_number = ? WHERE id = ?")?;

    let mut person_shots_stmt = conn.prepare(
        "SELECT id, folder_number FROM shots
         WHERE primary_person_id = ? AND folder_number IS NOT NULL
         ORDER BY folder_number, created_at",
    )?;
    let mut unsorted_shots_stmt = conn.prepare(
        "SELECT id, folder_number FROM shots
         WHERE primary_person_id IS NULL AND folder_number IS NOT NULL
         ORDER BY folder_number, created_at",
    )?;

    let mut total_compacted = 0;
    for person_id in &person_ids {
        // Get shots for this person ordered by folder_number, then created_at
        let shots: Vec<(String, i64)> = match person_id {
            Some(pid) => person_shots_stmt
                .query_map(params![pid], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect(),
            None => unsorted_shots_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect(),
        };

        for (i, (shot_id, current_number)) in shots.iter().enumerate() {
            let new_number = (i as i64) + 1;
            if new_number != *current_number {
                update_stmt.execute(params![new_number, shot_id])?;
                total_compacted += 1;
            }
        }
    }

    if total_compacted > 0 {
        info!(
            "Folder number compaction: {} shots renumbered",
            total_compacted
        );
    }

    Ok(())
}

/// Run face detection on an image and collect results without writing to DB.
fn detect_faces_collect(ai: &crate::ai::AiPipeline, img: &DynamicImage) -> Vec<FaceResult> {
    let (img_w, img_h) = img.dimensions();
    let detections = ai.detect_faces(img).unwrap_or_default();
    let mut results = Vec::new();

    for det in detections {
        let x1 = (det.box_x1 as u32).min(img_w.saturating_sub(1));
        let y1 = (det.box_y1 as u32).min(img_h.saturating_sub(1));
        let x2 = (det.box_x2 as u32).min(img_w);
        let y2 = (det.box_y2 as u32).min(img_h);

        let face_w = x2.saturating_sub(x1);
        let face_h = y2.saturating_sub(y1);

        if face_w < 10 || face_h < 10 {
            continue;
        }

        let bbox = (det.box_x1, det.box_y1, det.box_x2, det.box_y2);
        let embedding = ai
            .extract_embedding(img, det.landmarks.as_deref(), bbox)
            .unwrap_or_default();
        if embedding.is_empty() {
            continue;
        }

        let embedding_blob = match bincode::serialize(&embedding) {
            Ok(b) => b,
            Err(_) => continue,
        };

        results.push(FaceResult {
            face_id: Uuid::new_v4().to_string(),
            box_x1: det.box_x1,
            box_y1: det.box_y1,
            box_x2: det.box_x2,
            box_y2: det.box_y2,
            embedding_blob,
            score: det.score,
        });
    }

    results
}

/// Intermediate face detection result for batch DB writes.
struct FaceResult {
    face_id: String,
    box_x1: f32,
    box_y1: f32,
    box_x2: f32,
    box_y2: f32,
    embedding_blob: Vec<u8>,
    score: f32,
}

/// Intermediate keyframe result for batch DB writes.
struct KeyframeResult {
    kf_id: String,
    timestamp_ms: i64,
    kf_path: String,
    faces: Vec<FaceResult>,
}

/// Get video dimensions using ffmpeg.
fn get_video_dimensions(path: &Path) -> (Option<i32>, Option<i32>) {
    match ffmpeg::format::input(&path) {
        Ok(ictx) => {
            if let Some(stream) = ictx.streams().best(ffmpeg::media::Type::Video) {
                match ffmpeg::codec::context::Context::from_parameters(stream.parameters()) {
                    Ok(ctx) => match ctx.decoder().video() {
                        Ok(decoder) => {
                            (Some(decoder.width() as i32), Some(decoder.height() as i32))
                        }
                        Err(_) => (None, None),
                    },
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            }
        }
        Err(_) => (None, None),
    }
}

/// Process video keyframes one at a time via a callback, avoiding collecting all
/// decoded frames in memory simultaneously.  Each `DynamicImage` is dropped as
/// soon as the callback returns, so only one frame is in memory per call.
fn for_each_video_keyframe<F>(
    path: &Path,
    interval_secs: f64,
    mut on_keyframe: F,
) -> anyhow::Result<usize>
where
    F: FnMut(i64, DynamicImage),
{
    let mut ictx = ffmpeg::format::input(&path)?;

    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow::anyhow!("No video stream found in {:?}", path))?;
    let video_stream_index = stream.index();
    let time_base = stream.time_base();

    let context_decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = context_decoder.decoder().video()?;

    let width = decoder.width();
    let height = decoder.height();

    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!(
            "Video stream in {:?} has zero dimensions ({}x{})",
            path,
            width,
            height
        ));
    }

    let mut scaler = ffmpeg::software::scaling::Context::get(
        decoder.format(),
        width,
        height,
        ffmpeg::format::Pixel::RGB24,
        width,
        height,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;

    // Track the last extracted timestamp so we only extract at intervals
    let mut last_extracted_secs: f64 = -interval_secs; // Ensure we extract the first frame
    let mut count: usize = 0;

    let receive_and_process = |decoder: &mut ffmpeg::decoder::Video,
                               scaler: &mut ffmpeg::software::scaling::Context,
                               on_keyframe: &mut F,
                               last_extracted: &mut f64,
                               count: &mut usize,
                               time_base: ffmpeg::Rational| {
        let mut decoded = ffmpeg::frame::Video::empty();
        while decoder.receive_frame(&mut decoded).is_ok() {
            // Calculate frame timestamp in seconds
            let pts = decoded.timestamp().unwrap_or(0);
            let time_secs = pts as f64 * f64::from(time_base);
            let timestamp_ms = (time_secs * 1000.0) as i64;

            // Only extract if enough time has passed since last extraction
            if time_secs - *last_extracted < interval_secs {
                continue;
            }
            *last_extracted = time_secs;

            // Convert to RGB24
            let mut rgb_frame = ffmpeg::frame::Video::empty();
            if scaler.run(&decoded, &mut rgb_frame).is_err() {
                continue;
            }

            // Convert RGB frame data to DynamicImage
            let w = rgb_frame.width();
            let h = rgb_frame.height();
            let stride = rgb_frame.stride(0);
            let data = rgb_frame.data(0);

            // The frame data may have padding (stride > width*3), so we need to
            // copy row by row
            let mut rgb_data = Vec::with_capacity((w * h * 3) as usize);
            for row in 0..h {
                let start = (row as usize) * stride;
                let end = start + (w as usize) * 3;
                if end <= data.len() {
                    rgb_data.extend_from_slice(&data[start..end]);
                }
            }

            if let Some(img_buf) = RgbImage::from_raw(w, h, rgb_data) {
                let dynamic_img = DynamicImage::ImageRgb8(img_buf);
                on_keyframe(timestamp_ms, dynamic_img);
                // dynamic_img is dropped here — only one frame in memory at a time
                *count += 1;
            }
        }
    };

    for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            decoder.send_packet(&packet)?;
            receive_and_process(
                &mut decoder,
                &mut scaler,
                &mut on_keyframe,
                &mut last_extracted_secs,
                &mut count,
                time_base,
            );
        }
    }
    decoder.send_eof()?;
    receive_and_process(
        &mut decoder,
        &mut scaler,
        &mut on_keyframe,
        &mut last_extracted_secs,
        &mut count,
        time_base,
    );

    debug!("Processed {} keyframes from {:?}", count, path);

    Ok(count)
}

/// Extract the first frame from a video file and return it as a DynamicImage.
pub fn extract_first_video_frame(path: &Path) -> anyhow::Result<DynamicImage> {
    let mut ictx = ffmpeg::format::input(&path)?;

    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow::anyhow!("No video stream found in {:?}", path))?;
    let video_stream_index = stream.index();

    let context_decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = context_decoder.decoder().video()?;

    let width = decoder.width();
    let height = decoder.height();

    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!(
            "Video stream in {:?} has zero dimensions ({}x{})",
            path,
            width,
            height
        ));
    }

    let mut scaler = ffmpeg::software::scaling::Context::get(
        decoder.format(),
        width,
        height,
        ffmpeg::format::Pixel::RGB24,
        width,
        height,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;

    for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            decoder.send_packet(&packet)?;

            let mut decoded = ffmpeg::frame::Video::empty();
            if decoder.receive_frame(&mut decoded).is_ok() {
                let mut rgb_frame = ffmpeg::frame::Video::empty();
                scaler.run(&decoded, &mut rgb_frame)?;

                let w = rgb_frame.width();
                let h = rgb_frame.height();
                let stride = rgb_frame.stride(0);
                let data = rgb_frame.data(0);

                let mut rgb_data = Vec::with_capacity((w * h * 3) as usize);
                for row in 0..h {
                    let start = (row as usize) * stride;
                    let end = start + (w as usize) * 3;
                    if end <= data.len() {
                        rgb_data.extend_from_slice(&data[start..end]);
                    }
                }

                if let Some(img_buf) = RgbImage::from_raw(w, h, rgb_data) {
                    return Ok(DynamicImage::ImageRgb8(img_buf));
                }
            }
        }
    }

    Err(anyhow::anyhow!(
        "Could not extract any frame from {:?}",
        path
    ))
}

/// Extract EXIF metadata from an image file.
/// Returns (timestamp, latitude, longitude) where any field may be None.
fn extract_exif_metadata(path: &Path) -> (Option<String>, Option<f64>, Option<f64>) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None, None),
    };
    let mut bufreader = BufReader::new(file);
    let exifreader = exif::Reader::new();
    let exif = match exifreader.read_from_container(&mut bufreader) {
        Ok(e) => e,
        Err(_) => return (None, None, None),
    };

    // Extract DateTimeOriginal
    let timestamp = exif
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .map(|f| f.display_value().to_string());

    // Extract GPS coordinates
    let latitude = extract_gps_coord(&exif, Tag::GPSLatitude, Tag::GPSLatitudeRef);
    let longitude = extract_gps_coord(&exif, Tag::GPSLongitude, Tag::GPSLongitudeRef);

    (timestamp, latitude, longitude)
}

/// Parse a GPS coordinate from EXIF DMS (degrees/minutes/seconds) fields.
fn extract_gps_coord(exif: &exif::Exif, coord_tag: Tag, ref_tag: Tag) -> Option<f64> {
    let field = exif.get_field(coord_tag, In::PRIMARY)?;
    let ref_field = exif.get_field(ref_tag, In::PRIMARY)?;

    // The coordinate value should contain 3 rationals: degrees, minutes, seconds
    if let exif::Value::Rational(ref rationals) = field.value {
        if rationals.len() >= 3 {
            let degrees = rationals[0].to_f64();
            let minutes = rationals[1].to_f64();
            let seconds = rationals[2].to_f64();
            let mut coord = degrees + minutes / 60.0 + seconds / 3600.0;

            // Check reference direction (S or W means negative)
            let ref_str = ref_field.display_value().to_string();
            if ref_str.contains('S') || ref_str.contains('W') {
                coord = -coord;
            }

            return Some(coord);
        }
    }
    None
}

/// Return half of the available CPU threads (minimum 1).
pub fn half_available_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(1)
}

pub fn is_media_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "mp4" | "mkv" | "mov" | "avi" | "webm"
    )
}

pub fn calculate_hash(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 1024 * 1024];

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_is_media_file() {
        assert!(is_media_file(Path::new("test.jpg")));
        assert!(is_media_file(Path::new("test.PNG")));
        assert!(!is_media_file(Path::new("test.txt")));
    }

    #[test]
    fn test_scanner_integration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let media_dir = dir.path().join("media");
        fs::create_dir(&media_dir).unwrap();

        let file_path = media_dir.join("test.jpg");
        fs::write(&file_path, b"fake image data").unwrap();

        let _conn = crate::db::init_db(&db_path).unwrap();
        let scanner = Scanner::new(db_path.clone(), None);

        scanner.scan(&media_dir).unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_remove_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let shots_dir = dir.path().join("shots");
        fs::create_dir(&shots_dir).unwrap();

        let shot_path = shots_dir.join("remove_me.jpg");
        fs::write(&shot_path, b"fake image data for removal").unwrap();

        let _conn = crate::db::init_db(&db_path).unwrap();
        let scanner = Scanner::new(db_path.clone(), None);

        // Process the file first
        {
            let conn = scanner.open_db().unwrap();
            let dhash_cache = std::sync::Mutex::new(Vec::<DHashCacheEntry>::new());
            scanner
                .process_file(&conn, &shot_path, &dhash_cache)
                .unwrap();

            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 1);

            let shot_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM shots", [], |r| r.get(0))
                .unwrap();
            assert_eq!(shot_count, 1);
        }

        // Now remove it
        {
            let conn = scanner.open_db().unwrap();
            scanner.remove_file(&conn, &shot_path).unwrap();

            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);

            // Shot should also be removed since it has no remaining files
            let shot_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM shots", [], |r| r.get(0))
                .unwrap();
            assert_eq!(shot_count, 0);
        }
    }

    #[test]
    fn test_remove_file_not_found() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let _conn = crate::db::init_db(&db_path).unwrap();
        let scanner = Scanner::new(db_path.clone(), None);

        let conn = scanner.open_db().unwrap();
        let result = scanner.remove_file(&conn, Path::new("/nonexistent/file.jpg"));
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_dhash_deterministic() {
        // A solid-color image should produce a consistent hash
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| {
            image::Rgb([128, 128, 128])
        }));
        let hash1 = compute_dhash(&img);
        let hash2 = compute_dhash(&img);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_compute_dhash_different_images() {
        // A black image vs a checkerboard image should produce different hashes.
        // The checkerboard has alternating bright/dark columns so neighbouring
        // pixels differ, guaranteeing non-zero hash bits.
        let black =
            DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| image::Rgb([0, 0, 0])));
        let checkerboard = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |x, _| {
            if x % 2 == 0 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        }));
        let hash_black = compute_dhash(&black);
        let hash_checker = compute_dhash(&checkerboard);
        assert_ne!(hash_black, hash_checker);
    }

    #[test]
    fn test_compute_dhash_uniform_is_zero() {
        // A uniform image: every pixel equals its neighbor, so all bits are 0
        let img =
            DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| image::Rgb([42, 42, 42])));
        let hash = compute_dhash(&img);
        assert_eq!(hash, [0u8; 8]);
    }
}
