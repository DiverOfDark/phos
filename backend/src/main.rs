mod db;
mod scanner;
mod ai;
mod api;

use axum::Router;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let root_path = Path::new("./test_library");
    if !root_path.exists() {
        std::fs::create_dir_all(root_path).unwrap();
    }

    let db_path = root_path.join(".phos.db");
    let conn = db::init_db(&db_path).expect("Failed to initialize database");
    info!("Database initialized at {:?}", db_path);

    let shared_conn = Arc::new(Mutex::new(conn));
    let state = api::AppState {
        db: shared_conn.clone(),
    };

    // Run a scan in the background
    let scan_path = root_path.to_path_buf();
    let scanner_db_path = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let scanner = scanner::Scanner::new(scanner_db_path);
        if let Err(e) = scanner.scan(&scan_path) {
            tracing::error!("Scan failed: {}", e);
        }
    });

    let api_router = api::create_router(state);
    
    // Serve files from the library for the gallery
    let serve_dir = ServeDir::new(root_path);

    let app = Router::new()
        .merge(api_router)
        .fallback_service(serve_dir)
        .layer(CorsLayer::permissive());

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    info!("listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
