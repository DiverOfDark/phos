#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[test]
    fn test_scanner_basic() {
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
