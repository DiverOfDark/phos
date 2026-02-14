mod ai;
mod api;
mod db;
mod import;
mod scanner;
mod watcher;

use axum::Router;
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::info;

#[derive(Parser)]
#[command(name = "phos", about = "AI-powered photo/video manager")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the web server (default)
    Serve,
    /// Import media files from source into organized target directory
    Import {
        /// Source directory to import from
        source: PathBuf,
        /// Target directory for organized files and database
        target: PathBuf,
        /// Move files instead of copying
        #[arg(long)]
        r#move: bool,
    },
    /// Reorganize files on disk to match current face clustering
    Reorganize {
        /// Library directory containing .phos.db
        library: PathBuf,
        /// Show what would be moved without actually moving anything
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    ffmpeg_next::init().expect("Failed to initialize ffmpeg");

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Import {
            source,
            target,
            r#move,
        }) => {
            if let Err(e) = import::run_import(&source, &target, r#move) {
                eprintln!("Import failed: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Reorganize { library, dry_run }) => {
            if let Err(e) = import::run_reorganize(&library, dry_run) {
                eprintln!("Reorganize failed: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Serve) | None => {
            run_server().await;
        }
    }
}

async fn run_server() {
    let library_path =
        std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string());
    let root_path = Path::new(&library_path);
    info!("Using library path: {:?}", root_path);
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

    // Run a scan in the background, then start the file watcher once done.
    let scan_path = root_path.to_path_buf();
    let scanner_db_path = db_path.to_path_buf();
    let watcher_library_path = root_path.to_path_buf();
    let watcher_db_path = db_path.to_path_buf();

    let ai = ai::AiPipeline::new().expect("Failed to load AI models");

    tokio::task::spawn_blocking(move || {
        let scanner = scanner::Scanner::new(scanner_db_path, Some(ai));
        if let Err(e) = scanner.scan(&scan_path) {
            tracing::error!("Scan failed: {}", e);
        }

        // Initial scan complete -- start watching for incremental changes.
        match watcher::start_watcher(watcher_library_path, watcher_db_path, None) {
            Ok(watcher_handle) => {
                info!("File watcher active after initial scan");
                // Keep the watcher alive for the lifetime of this thread.
                // The watcher handle must not be dropped, so we park the thread.
                // (The watcher's internal thread handles events independently.)
                // We use a channel that never sends to block forever.
                let (_tx, rx) = std::sync::mpsc::channel::<()>();
                let _ = rx.recv(); // blocks until program exit
                drop(watcher_handle);
            }
            Err(e) => {
                tracing::error!("Failed to start file watcher: {}", e);
            }
        }
    });

    let api_router = api::create_router(state);
    let static_dir = std::env::var("PHOS_STATIC_DIR").unwrap_or_else(|_| "static".to_string());
    let serve_static = ServeDir::new(static_dir);

    let app = Router::new()
        .merge(api_router)
        .fallback_service(serve_static)
        .layer(CorsLayer::permissive());

    let port = std::env::var("PHOS_PORT")
        .unwrap_or_else(|_| "33000".to_string())
        .parse::<u16>()
        .unwrap_or(33000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
