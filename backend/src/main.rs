mod ai;
mod api;
mod auth;
mod comfyui;
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

    let comfyui_url = std::env::var("PHOS_COMFYUI_URL").ok();
    if let Some(ref url) = comfyui_url {
        info!("ComfyUI integration enabled (url: {})", url);
    }

    let state = api::AppState {
        db: shared_conn.clone(),
        scanner: scanner.clone(),
        comfyui_url: comfyui_url.clone(),
    };

    // Shutdown coordination: a condvar that the blocking background task
    // can wait on, and the signal handler sets.
    let shutdown_flag = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));

    // Run a scan in the background, then start the file watcher once done.
    let scan_path = root_path.to_path_buf();
    let watcher_library_path = root_path.to_path_buf();
    let watcher_db_path = db_path.to_path_buf();
    let bg_shutdown = shutdown_flag.clone();

    let bg_handle = tokio::task::spawn_blocking(move || {
        let (lock, cvar) = &*bg_shutdown;
        if *lock.lock().unwrap() {
            return;
        }
        if let Err(e) = scanner.scan(&scan_path) {
            tracing::error!("Scan failed: {}", e);
        }

        if *lock.lock().unwrap() {
            return;
        }
        // Reorganize files on disk to match clustering results.
        if let Err(e) = import::run_reorganize(&scan_path, false) {
            tracing::error!("Post-scan reorganize failed: {}", e);
        }

        if *lock.lock().unwrap() {
            return;
        }
        // Initial scan complete -- start watching for incremental changes.
        match watcher::start_watcher(watcher_library_path, watcher_db_path, None) {
            Ok(watcher_handle) => {
                info!("File watcher active after initial scan");
                // Block until shutdown is requested. Dropping the watcher handle
                // closes the notify channel, which makes the watcher loop flush
                // pending events and exit cleanly.
                let guard = lock.lock().unwrap();
                let _guard = cvar.wait_while(guard, |stopped| !*stopped).unwrap();
                info!("Shutdown signal received, stopping file watcher");
                drop(watcher_handle);
            }
            Err(e) => {
                tracing::error!("Failed to start file watcher: {}", e);
            }
        }
    });

    // Optionally spawn the ComfyUI enhancement worker
    let _comfyui_handle = if let Some(ref url) = comfyui_url {
        let comfyui_shutdown = shutdown_flag.clone();
        Some(comfyui::spawn_enhancement_worker(
            db_path.to_path_buf(),
            url.clone(),
            comfyui_shutdown,
        ))
    } else {
        None
    };

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
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    info!("HTTP server stopped, signalling background tasks...");
    // Wake the blocking background task so it can exit.
    {
        let (lock, cvar) = &*shutdown_flag;
        let mut stopped = lock.lock().unwrap();
        *stopped = true;
        cvar.notify_all();
    }
    // Give it a moment to flush cleanly.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), bg_handle).await;
    info!("Shutdown complete");
}

/// Wait for SIGTERM or Ctrl-C.
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => info!("Received Ctrl-C, shutting down"),
            _ = sigterm.recv() => info!("Received SIGTERM, shutting down"),
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to listen for Ctrl-C");
        info!("Received Ctrl-C, shutting down");
    }
}
