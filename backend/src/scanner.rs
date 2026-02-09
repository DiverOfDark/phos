use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use sha2::{Sha256, Digest};
use std::fs::File;
use std::io::{self, Read};
use rusqlite::{params, Connection};
use uuid::Uuid;
use tracing::{info, error};
use image::GenericImageView;
use crate::ai::AiPipeline;

pub struct Scanner {
    db_path: PathBuf,
    ai: Option<AiPipeline>,
}

impl Scanner {
    pub fn new(db_path: PathBuf, ai: Option<AiPipeline>) -> Self {
        Self { db_path, ai }
    }

    pub fn scan(&self, root: &Path) -> anyhow::Result<()> {
        let conn = Connection::open(&self.db_path)?;
        
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

    fn process_file(&self, conn: &Connection, path: &Path) -> anyhow::Result<()> {
        let hash = calculate_hash(path)?;
        
        let mut stmt = conn.prepare("SELECT id, photo_id FROM files WHERE hash = ?")?;
        let mut rows = stmt.query(params![hash])?;
        
        if let Some(row) = rows.next()? {
            // File already exists, check if path matches or if it's a move
            let existing_id: String = row.get(0)?;
            info!("File already exists with hash {}, ID: {}", hash, existing_id);
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
            _ => "application/octet-stream",
        };

        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;

        // Try to get image dimensions
        let (width, height) = if mime_type.starts_with("image/") {
            match image::image_dimensions(path) {
                Ok((w, h)) => (Some(w as i32), Some(h as i32)),
                Err(_) => (None, None),
            }
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

        // AI Processing
        if let Some(ai) = &self.ai {
            if mime_type.starts_with("image/") {
                if let Ok(img) = image::open(path) {
                    let detections = ai.detect_faces(&img).unwrap_or_default();
                    for det in detections {
                        // Extract face chip
                        let sub_img = img.view(
                            det.box_x1 as u32,
                            det.box_y1 as u32,
                            (det.box_x2 - det.box_x1) as u32,
                            (det.box_y2 - det.box_y1) as u32,
                        ).to_image();
                        
                        let embedding = ai.extract_embedding(&image::DynamicImage::ImageRgba8(sub_img)).unwrap_or_default();
                        if embedding.is_empty() { continue; }
                        
                        let embedding_blob = bincode::serialize(&embedding)?;

                        conn.execute(
                            "INSERT INTO faces (id, file_id, box_x1, box_y1, box_x2, box_y2, embedding) VALUES (?, ?, ?, ?, ?, ?, ?)",
                            params![Uuid::new_v4().to_string(), id, det.box_x1, det.box_y1, det.box_x2, det.box_y2, embedding_blob],
                        )?;
                    }
                }
            }
        }

        info!("Indexed: {:?}", path);
        Ok(())
    }
}


fn is_media_file(path: &Path) -> bool {
    let ext = path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "mp4" | "mkv" | "mov")
}

fn calculate_hash(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 1024 * 1024];

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 { break; }
        hasher.update(&buffer[..count]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

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
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
    }
}

