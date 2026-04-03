#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[test]
    fn test_scanner_basic() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let media_dir = dir.path().join("media");
        fs::create_dir(&media_dir).unwrap();

        let file_path = media_dir.join("test.jpg");
        fs::write(&file_path, b"fake image data").unwrap();

        crate::db::init_and_migrate(&db_path).unwrap();
        let scanner = Scanner::new(db_path.clone(), None);

        scanner.scan(&media_dir).unwrap();

        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        use diesel::prelude::*;
        let count: i64 = crate::schema::files::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 1);
    }
}
