use crate::ai::{cosine_similarity, AiPipeline, FACE_SIMILARITY_THRESHOLD};
use exif::{In, Tag};
use ffmpeg_next as ffmpeg;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, RgbImage};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use walkdir::WalkDir;

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

pub struct Scanner {
    db_path: PathBuf,
    ai: Option<AiPipeline>,
}

/// A person record with their representative embedding loaded from the DB.
struct PersonRecord {
    id: String,
    embedding: Vec<f32>,
}

impl Scanner {
    pub fn new(db_path: PathBuf, ai: Option<AiPipeline>) -> Self {
        Self { db_path, ai }
    }

    /// Access the AI pipeline, if loaded.
    pub fn ai(&self) -> Option<&AiPipeline> {
        self.ai.as_ref()
    }

    /// Open a connection to the scanner's database.
    pub fn open_db(&self) -> anyhow::Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    pub fn scan(&self, root: &Path) -> anyhow::Result<()> {
        let conn = self.open_db()?;

        for entry in WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if is_media_file(path) {
                if let Err(e) = self.process_file(&conn, path) {
                    error!("Error processing {:?}: {}", path, e);
                }
            }
        }
        Ok(())
    }

    /// Remove a file from the database by its filesystem path.
    ///
    /// Deletes associated faces, video keyframes, and the file record itself.
    /// If the parent photo has no remaining files, the photo record is also removed.
    /// Orphaned person records (those with no remaining faces) are cleaned up too.
    pub fn remove_file(&self, conn: &Connection, path: &Path) -> anyhow::Result<()> {
        let path_str = path.to_string_lossy();

        // Look up the file by path
        let (file_id, photo_id): (String, String) = conn
            .query_row(
                "SELECT id, photo_id FROM files WHERE path = ?",
                params![path_str.as_ref()],
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

        // Check if the photo has any remaining files
        let remaining_files: i64 = conn.query_row(
            "SELECT COUNT(*) FROM files WHERE photo_id = ?",
            params![photo_id],
            |row| row.get(0),
        )?;

        if remaining_files == 0 {
            conn.execute("DELETE FROM photos WHERE id = ?", params![photo_id])?;
            info!("Removed orphaned photo record {}", photo_id);
        }

        // Clean up orphaned person records (persons with no remaining faces)
        for person_id in &affected_person_ids {
            let face_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM faces WHERE person_id = ?",
                params![person_id],
                |row| row.get(0),
            )?;
            if face_count == 0 {
                conn.execute("DELETE FROM people WHERE id = ?", params![person_id])?;
                info!("Removed orphaned person record {}", person_id);
            }
        }

        Ok(())
    }

    /// Load all person records with their representative embeddings from the DB.
    fn load_person_embeddings(conn: &Connection) -> Vec<PersonRecord> {
        let mut stmt = match conn
            .prepare("SELECT id, representative_embedding FROM people WHERE representative_embedding IS NOT NULL")
        {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to prepare person embeddings query: {}", e);
                return Vec::new();
            }
        };

        let rows = match stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((id, blob))
        }) {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to query person embeddings: {}", e);
                return Vec::new();
            }
        };

        let mut persons = Vec::new();
        for (id, blob) in rows.flatten() {
            match bincode::deserialize::<Vec<f32>>(&blob) {
                Ok(embedding) => {
                    if !embedding.is_empty() {
                        persons.push(PersonRecord { id, embedding });
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize embedding for person {}: {}", id, e);
                }
            }
        }
        persons
    }

    /// Find the best matching person for a given face embedding.
    /// Returns (person_id, similarity) if a match above threshold is found.
    fn find_matching_person(embedding: &[f32], persons: &[PersonRecord]) -> Option<(String, f32)> {
        let mut best_match: Option<(String, f32)> = None;

        for person in persons {
            if person.embedding.len() != embedding.len() {
                continue;
            }
            let sim = cosine_similarity(embedding, &person.embedding);
            if sim > FACE_SIMILARITY_THRESHOLD {
                if let Some((_, best_sim)) = &best_match {
                    if sim > *best_sim {
                        best_match = Some((person.id.clone(), sim));
                    }
                } else {
                    best_match = Some((person.id.clone(), sim));
                }
            }
        }

        best_match
    }

    /// Create a new person in the DB with the given embedding as the representative.
    fn create_new_person(
        conn: &Connection,
        embedding: &[f32],
        face_id: &str,
    ) -> anyhow::Result<String> {
        let person_id = Uuid::new_v4().to_string();
        let embedding_blob = bincode::serialize(embedding)?;

        conn.execute(
            "INSERT INTO people (id, thumbnail_face_id, representative_embedding) VALUES (?, ?, ?)",
            params![person_id, face_id, embedding_blob],
        )?;

        debug!("Created new person {} for face {}", person_id, face_id);
        Ok(person_id)
    }

    /// Match an embedding to an existing person, or create a new one.
    /// Returns the person_id. New persons are created with name = NULL,
    /// which serves as the "needs naming" flag for the UI.
    pub fn find_or_create_person(conn: &Connection, embedding: &[f32]) -> anyhow::Result<String> {
        let persons = Self::load_person_embeddings(conn);
        if let Some((person_id, _sim)) = Self::find_matching_person(embedding, &persons) {
            Ok(person_id)
        } else {
            let face_id = Uuid::new_v4().to_string();
            Self::create_new_person(conn, embedding, &face_id)
        }
    }

    /// Detect faces in an image and store them in the database, linking to the given file_id.
    fn detect_and_store_faces(
        &self,
        conn: &Connection,
        ai: &AiPipeline,
        img: &DynamicImage,
        file_id: &str,
    ) -> anyhow::Result<()> {
        let (img_w, img_h) = img.dimensions();
        let detections = ai.detect_faces(img).unwrap_or_default();

        debug!(
            "Detected {} faces in file {} ({}x{})",
            detections.len(),
            file_id,
            img_w,
            img_h
        );

        for det in detections {
            // Clamp bounding box to image dimensions
            let x1 = (det.box_x1 as u32).min(img_w.saturating_sub(1));
            let y1 = (det.box_y1 as u32).min(img_h.saturating_sub(1));
            let x2 = (det.box_x2 as u32).min(img_w);
            let y2 = (det.box_y2 as u32).min(img_h);

            let face_w = x2.saturating_sub(x1);
            let face_h = y2.saturating_sub(y1);

            if face_w < 10 || face_h < 10 {
                debug!(
                    "Skipping too-small face region: {}x{} at ({},{}) in file {}",
                    face_w, face_h, x1, y1, file_id
                );
                continue;
            }

            // Extract face chip
            let sub_img = img.crop_imm(x1, y1, face_w, face_h);

            let embedding = ai.extract_embedding(&sub_img).unwrap_or_default();
            if embedding.is_empty() {
                continue;
            }

            let face_id = Uuid::new_v4().to_string();
            let embedding_blob = bincode::serialize(&embedding)?;

            // Face clustering: find or create a person for this face
            let persons = Self::load_person_embeddings(conn);
            let person_id = if let Some((matched_person_id, sim)) =
                Self::find_matching_person(&embedding, &persons)
            {
                debug!(
                    "Matched face {} to person {} with similarity {:.3}",
                    face_id, matched_person_id, sim
                );
                matched_person_id
            } else {
                // Create a new person
                Self::create_new_person(conn, &embedding, &face_id)?
            };

            // Insert face with person_id and embedding
            conn.execute(
                "INSERT INTO faces (id, file_id, person_id, box_x1, box_y1, box_x2, box_y2, embedding) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![face_id, file_id, person_id, det.box_x1, det.box_y1, det.box_x2, det.box_y2, embedding_blob],
            )?;
        }

        Ok(())
    }

    pub fn process_file(&self, conn: &Connection, path: &Path) -> anyhow::Result<()> {
        let hash = calculate_hash(path)?;

        let mut stmt = conn.prepare("SELECT id, photo_id FROM files WHERE hash = ?")?;
        let mut rows = stmt.query(params![hash])?;

        if let Some(row) = rows.next()? {
            // File already exists, check if path matches or if it's a move
            let existing_id: String = row.get(0)?;
            info!(
                "File already exists with hash {}, ID: {}",
                hash, existing_id
            );
            return Ok(());
        }

        let id = Uuid::new_v4().to_string();
        let photo_id = Uuid::new_v4().to_string();

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

        // Try to get dimensions (image or video)
        let (width, height) = if mime_type.starts_with("image/") {
            match image::image_dimensions(path) {
                Ok((w, h)) => (Some(w as i32), Some(h as i32)),
                Err(_) => (None, None),
            }
        } else if mime_type.starts_with("video/") {
            get_video_dimensions(path)
        } else {
            (None, None)
        };

        // Insert into photos
        conn.execute(
            "INSERT INTO photos (id, main_file_id, width, height) VALUES (?, ?, ?, ?)",
            params![photo_id, id, width, height],
        )?;

        // Insert into files
        conn.execute(
            "INSERT INTO files (id, photo_id, path, hash, mime_type, file_size, is_original) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![id, photo_id, path.to_string_lossy(), hash, mime_type, file_size, 1],
        )?;

        // Extract EXIF metadata for images and update the photo record
        if mime_type.starts_with("image/") {
            let (timestamp, latitude, longitude) = extract_exif_metadata(path);
            if timestamp.is_some() || latitude.is_some() || longitude.is_some() {
                if let Err(e) = conn.execute(
                    "UPDATE photos SET timestamp = ?, latitude = ?, longitude = ? WHERE id = ?",
                    params![timestamp, latitude, longitude, photo_id],
                ) {
                    warn!(
                        "Failed to update EXIF metadata for photo {}: {}",
                        photo_id, e
                    );
                }
            }
        }

        // Compute perceptual hash (dHash) for images and videos
        if mime_type.starts_with("image/") {
            if let Ok(img) = image::open(path) {
                let dhash = compute_dhash(&img);
                conn.execute(
                    "UPDATE files SET visual_embedding = ? WHERE id = ?",
                    params![dhash.as_slice(), id],
                )?;
                debug!("Stored dHash for image file {}", id);
            }
        } else if mime_type.starts_with("video/") {
            match extract_first_video_frame(path) {
                Ok(frame) => {
                    let dhash = compute_dhash(&frame);
                    conn.execute(
                        "UPDATE files SET visual_embedding = ? WHERE id = ?",
                        params![dhash.as_slice(), id],
                    )?;
                    debug!("Stored dHash for video file {}", id);
                }
                Err(e) => {
                    warn!(
                        "Failed to extract first frame for dHash from {:?}: {}",
                        path, e
                    );
                }
            }
        }

        // AI Processing
        if let Some(ai) = &self.ai {
            if mime_type.starts_with("image/") {
                if let Ok(img) = image::open(path) {
                    self.detect_and_store_faces(conn, ai, &img, &id)?;
                }
            } else if mime_type.starts_with("video/") {
                // Extract keyframes from video and run face detection on each
                match extract_video_keyframes(path, 5.0) {
                    Ok(keyframes) => {
                        for kf in &keyframes {
                            let kf_id = Uuid::new_v4().to_string();
                            let kf_path = format!("memory://keyframe_{}", kf.timestamp_ms);

                            conn.execute(
                                "INSERT INTO video_keyframes (id, video_file_id, timestamp_ms, path) VALUES (?, ?, ?, ?)",
                                params![kf_id, id, kf.timestamp_ms, kf_path],
                            )?;

                            // Run face detection on this keyframe
                            self.detect_and_store_faces(conn, ai, &kf.image, &id)?;
                        }
                        debug!(
                            "Processed {} keyframes for video {:?}",
                            keyframes.len(),
                            path
                        );
                    }
                    Err(e) => {
                        warn!("Failed to extract keyframes from {:?}: {}", path, e);
                    }
                }
            }
        }

        info!("Indexed: {:?}", path);
        Ok(())
    }
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

/// A keyframe extracted from a video file.
struct ExtractedKeyframe {
    timestamp_ms: i64,
    image: DynamicImage,
}

/// Extract keyframes from a video at approximately every `interval_secs` seconds.
/// Returns a vector of (timestamp_ms, DynamicImage) pairs.
fn extract_video_keyframes(
    path: &Path,
    interval_secs: f64,
) -> anyhow::Result<Vec<ExtractedKeyframe>> {
    let mut keyframes = Vec::new();

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

    let receive_frames = |decoder: &mut ffmpeg::decoder::Video,
                          scaler: &mut ffmpeg::software::scaling::Context,
                          keyframes: &mut Vec<ExtractedKeyframe>,
                          last_extracted: &mut f64,
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
                keyframes.push(ExtractedKeyframe {
                    timestamp_ms,
                    image: dynamic_img,
                });
            }
        }
    };

    for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            decoder.send_packet(&packet)?;
            receive_frames(
                &mut decoder,
                &mut scaler,
                &mut keyframes,
                &mut last_extracted_secs,
                time_base,
            );
        }
    }
    decoder.send_eof()?;
    receive_frames(
        &mut decoder,
        &mut scaler,
        &mut keyframes,
        &mut last_extracted_secs,
        time_base,
    );

    debug!("Extracted {} keyframes from {:?}", keyframes.len(), path);

    Ok(keyframes)
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
        let photos_dir = dir.path().join("photos");
        fs::create_dir(&photos_dir).unwrap();

        let photo_path = photos_dir.join("test.jpg");
        fs::write(&photo_path, b"fake image data").unwrap();

        let _conn = crate::db::init_db(&db_path).unwrap();
        let scanner = Scanner::new(db_path.clone(), None);

        scanner.scan(&photos_dir).unwrap();

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
        let photos_dir = dir.path().join("photos");
        fs::create_dir(&photos_dir).unwrap();

        let photo_path = photos_dir.join("remove_me.jpg");
        fs::write(&photo_path, b"fake image data for removal").unwrap();

        let _conn = crate::db::init_db(&db_path).unwrap();
        let scanner = Scanner::new(db_path.clone(), None);

        // Process the file first
        {
            let conn = scanner.open_db().unwrap();
            scanner.process_file(&conn, &photo_path).unwrap();

            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 1);

            let photo_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))
                .unwrap();
            assert_eq!(photo_count, 1);
        }

        // Now remove it
        {
            let conn = scanner.open_db().unwrap();
            scanner.remove_file(&conn, &photo_path).unwrap();

            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);

            // Photo should also be removed since it has no remaining files
            let photo_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))
                .unwrap();
            assert_eq!(photo_count, 0);
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
