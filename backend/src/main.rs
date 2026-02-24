mod ai;
mod api;
mod auth;
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
use tower_http::services::{ServeDir, ServeFile};
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
        /// Source directory or URL to import from
        source: String,
        /// Target directory for organized files and database (local) or Remote URL
        target: Option<String>,
        /// Move files instead of copying (local only)
        #[arg(long)]
        r#move: bool,
        /// Number of parallel threads (default: 4)
        #[arg(long, short = 'j', default_value = "4")]
        threads: usize,
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
    // Suppress noisy FFmpeg warnings (deprecated pixel formats, probesize hints)
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Import {
            source,
            target,
            r#move,
            threads,
        }) => {
            let threads = threads.max(1);
            if let Some(ref target_str) = target {
                if target_str.starts_with("http://") || target_str.starts_with("https://") {
                    if let Err(e) = import::run_remote_import(&source, target_str, threads) {
                        eprintln!("Remote import failed: {}", e);
                        std::process::exit(1);
                    }
                } else {
                    if let Err(e) = import::run_import(Path::new(&source), Path::new(target_str), r#move, threads) {
                        eprintln!("Import failed: {}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                eprintln!("Target directory or URL is required for import");
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
    info!("Initializing database at {:?}", db_path);
    let conn = db::init_db(&db_path).map_err(|e| {
        tracing::error!("Failed to initialize database: {}", e);
        e
    }).expect("Failed to initialize database");

    let shared_conn = Arc::new(Mutex::new(conn));

    let ai = ai::AiPipeline::new().expect("Failed to load AI models");
    let scanner = Arc::new(scanner::Scanner::new(db_path.to_path_buf(), Some(ai)));

    let state = api::AppState {
        db: shared_conn.clone(),
        scanner: scanner.clone(),
    };

    // Run a scan in the background, then start the file watcher once done.
    let scan_path = root_path.to_path_buf();
    let watcher_library_path = root_path.to_path_buf();
    let watcher_db_path = db_path.to_path_buf();

    tokio::task::spawn_blocking(move || {
        if let Err(e) = scanner.scan(&scan_path) {
            tracing::error!("Scan failed: {}", e);
        }

        // Reorganize files on disk to match clustering results.
        if let Err(e) = import::run_reorganize(&scan_path, false) {
            tracing::error!("Post-scan reorganize failed: {}", e);
        }

        // Initial scan complete -- start watching for incremental changes.
        match watcher::start_watcher(watcher_library_path, watcher_db_path, None) {
            Ok(watcher_handle) => {
                info!("File watcher active after initial scan");
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
    let index_path = format!("{}/index.html", static_dir);
    let serve_static = ServeDir::new(&static_dir)
        .not_found_service(ServeFile::new(index_path));

    let port = std::env::var("PHOS_PORT")
        .unwrap_or_else(|_| "33000".to_string())
        .parse::<u16>()
        .unwrap_or(33000);

    // OIDC authentication (optional — enabled when PHOS_OIDC_ISSUER is set)
    let app = if let Ok(issuer) = std::env::var("PHOS_OIDC_ISSUER") {
        let client_id = std::env::var("PHOS_OIDC_CLIENT_ID")
            .expect("PHOS_OIDC_CLIENT_ID is required when PHOS_OIDC_ISSUER is set");
        let client_secret = std::env::var("PHOS_OIDC_CLIENT_SECRET")
            .expect("PHOS_OIDC_CLIENT_SECRET is required when PHOS_OIDC_ISSUER is set");
        let redirect_uri = std::env::var("PHOS_OIDC_REDIRECT_URI")
            .unwrap_or_else(|_| format!("http://localhost:{}/api/auth/callback", port));
        let jwt_secret = std::env::var("PHOS_JWT_SECRET").unwrap_or_else(|_| {
            let secret_path = root_path.join(".phos_jwt_secret");
            if let Ok(s) = std::fs::read_to_string(&secret_path) {
                return s.trim().to_string();
            }
            let s = uuid::Uuid::new_v4().to_string();
            let _ = std::fs::write(&secret_path, &s);
            info!("Generated JWT secret at {:?}", secret_path);
            s
        });
        let jwt_ttl: u64 = std::env::var("PHOS_JWT_TTL")
            .unwrap_or_else(|_| "3600".to_string())
            .parse()
            .unwrap_or(3600);
        let scopes: Vec<String> = std::env::var("PHOS_OIDC_SCOPES")
            .unwrap_or_else(|_| "openid profile email".to_string())
            .split_whitespace()
            .map(String::from)
            .collect();

        info!("OIDC authentication enabled (issuer: {})", issuer);
        let auth_state = auth::init_oidc(
            &issuer,
            &client_id,
            &client_secret,
            &redirect_uri,
            &jwt_secret,
            jwt_ttl,
            scopes,
        )
        .await
        .expect("Failed to initialize OIDC provider");

        let auth_router = auth::create_auth_router(auth_state.clone());
        let protected_api = api_router.layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth::require_auth,
        ));

        Router::new()
            .merge(auth_router)
            .merge(protected_api)
            .fallback_service(serve_static)
            .layer(CorsLayer::permissive())
    } else {
        info!("OIDC authentication disabled (set PHOS_OIDC_ISSUER to enable)");
        Router::new()
            .merge(api_router)
            .fallback_service(serve_static)
            .layer(CorsLayer::permissive())
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
