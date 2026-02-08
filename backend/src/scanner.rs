use sha2::{Sha256, Digest};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use walkdir::WalkDir;
use tracing::{info, warn};

pub fn calculate_hash(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 1024 * 1024]; // 1MB buffer

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 { break; }
        hasher.update(&buffer[..count]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn scan_directory(root: &Path) {
    info!("Starting scan of {:?}", root);
    
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file()) 
    {
        let path = entry.path();
        
        // Simple extension check for now
        let ext = path.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
            
        if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "mp4" | "mkv" | "mov") {
            info!("Found media: {:?}", path);
            match calculate_hash(path) {
                Ok(hash) => info!("Hash for {:?}: {}", path.file_name().unwrap(), hash),
                Err(e) => warn!("Failed to hash {:?}: {}", path, e),
            }
        }
    }
    
    info!("Scan complete.");
}
