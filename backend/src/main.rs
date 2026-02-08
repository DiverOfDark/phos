mod db;
mod scanner;

use axum::{routing::get, Router};
use std::net::SocketAddr;
use std::path::Path;
use tracing::info;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let root_path = Path::new("./test_library");
    if !root_path.exists() {
        std::fs::create_dir_all(root_path).unwrap();
    }

    let db_path = root_path.join(".phos.db");
    let _conn = db::init_db(&db_path).expect("Failed to initialize database");
    info!("Database initialized at {:?}", db_path);

    // Run a scan in the background
    let scan_path = root_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        scanner::scan_directory(&scan_path);
    });

    let app = Router::new().route("/", get(|| async { "Phos API is online" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    info!("listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
