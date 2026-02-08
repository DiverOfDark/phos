use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use sha2::{Sha256, Digest};
use std::fs::File;
use std::io::{self, Read};
use rusqlite::{params, Connection};
use uuid::Uuid;
use tracing::{info, error};

pub struct Scanner {
    db_path: PathBuf,
}

impl Scanner {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
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
        
        // Check if file already exists
        let mut stmt = conn.prepare("SELECT id FROM files WHERE hash = ?")?;
        let exists = stmt.exists(params![hash])?;
        
        if exists {
            return Ok(());
        }

        let id = Uuid::new_v4().to_string();
        let photo_id = Uuid::new_v4().to_string(); // In a real system, we might group by visual similarity later
        
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

        // Insert into photos (conceptual container)
        conn.execute(
            "INSERT INTO photos (id, main_file_id) VALUES (?, ?)",
            params![photo_id, id],
        )?;

        // Insert into files
        conn.execute(
            "INSERT INTO files (id, photo_id, path, hash, mime_type, file_size, is_original) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![id, photo_id, path.to_string_lossy(), hash, mime_type, file_size, 1],
        )?;

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
