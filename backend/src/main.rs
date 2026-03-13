mod ai;
mod api;
mod auth;
mod cli_auth;
mod comfyui;
mod db;
mod import;
mod scanner;
mod watcher;

use axum::Router;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

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
    /// Export OpenAPI spec as JSON
    #[command(name = "openapi")]
    OpenApi {
        /// Output file path (defaults to stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // OpenAPI export doesn't need any runtime initialization
    if let Some(Commands::OpenApi { output }) = cli.command {
        let spec = api::ApiDoc::openapi()
            .to_pretty_json()
            .expect("Failed to serialize OpenAPI spec");
        if let Some(path) = output {
            std::fs::write(&path, &spec).expect("Failed to write OpenAPI spec file");
            eprintln!("OpenAPI spec written to {}", path.display());
        } else {
            println!("{}", spec);
        }
        return;
    }

    tracing_subscriber::fmt::init();
    info!("Phos {} starting", env!("PHOS_VERSION"));
    ffmpeg_next::init().expect("Failed to initialize ffmpeg");
    // Suppress noisy FFmpeg warnings (deprecated pixel formats, probesize hints)
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);

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
                    let token = match cli_auth::authenticate_if_needed(target_str) {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!("Authentication failed: {}", e);
                            std::process::exit(1);
                        }
                    };
                    if let Err(e) =
                        import::run_remote_import(&source, target_str, threads, token.as_deref())
                    {
                        eprintln!("Remote import failed: {}", e);
                        std::process::exit(1);
                    }
                } else if let Err(e) =
                    import::run_import(Path::new(&source), Path::new(target_str), r#move, threads)
                {
                    eprintln!("Import failed: {}", e);
                    std::process::exit(1);
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
        Some(Commands::OpenApi { .. }) => unreachable!("handled above"),
        Some(Commands::Serve) | None => {
            run_server().await;
        }
    }
}

fn init_shared_db(path: &Path) -> Arc<Mutex<rusqlite::Connection>> {
    let conn = db::init_db(path)
        .map_err(|e| {
            tracing::error!("Failed to initialize database at {:?}: {}", path, e);
            e
        })
        .expect("Failed to initialize database");
    Arc::new(Mutex::new(conn))
}

async fn run_server() {
    let library_path =
        std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string());
    let root_path = Path::new(&library_path);
    info!("Using library path: {:?}", root_path);
    if !root_path.exists() {
        std::fs::create_dir_all(root_path).unwrap();
    }

    let multi_user = std::env::var("PHOS_OIDC_ISSUER").is_ok();

    let db_path = root_path.join(".phos.db");
    if multi_user {
        info!("Multi-user mode: each SSO user gets their own library subfolder");
    } else {
        info!("Initializing database at {:?}", db_path);
    }
    let shared_conn = init_shared_db(&db_path);

    let ai = ai::AiPipeline::new().expect("Failed to load AI models");
    let scanner = Arc::new(scanner::Scanner::new(db_path.to_path_buf(), Some(ai)));

    let comfyui_url = std::env::var("PHOS_COMFYUI_URL").ok();
    if let Some(ref url) = comfyui_url {
        info!("ComfyUI integration enabled (url: {})", url);
    }

    let user_dbs: Arc<RwLock<HashMap<String, Arc<Mutex<rusqlite::Connection>>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Shutdown coordination: a condvar that the blocking background task
    // can wait on, and the signal handler sets.
    let shutdown_flag = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));

    let state = api::AppState {
        db: shared_conn.clone(),
        scanner: scanner.clone(),
        comfyui_url: comfyui_url.clone(),
        library_root: root_path.to_path_buf(),
        multi_user,
        user_dbs: user_dbs.clone(),
        shutdown_flag: shutdown_flag.clone(),
    };

    let bg_shutdown = shutdown_flag.clone();

    let bg_handle = if multi_user {
        // Multi-user mode: scan all existing user subdirectories at startup
        let root = root_path.to_path_buf();
        let scanner_ref = scanner.clone();
        let comfyui_url_bg = comfyui_url.clone();
        let comfyui_shutdown = shutdown_flag.clone();
        tokio::task::spawn_blocking(move || {
            let (lock, _cvar) = &*bg_shutdown;
            if *lock.lock().unwrap() {
                return;
            }

            // Find existing user directories (those with a .phos.db)
            let user_dirs: Vec<PathBuf> = match std::fs::read_dir(&root) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter(|e| {
                        let name = e.file_name();
                        let name_str = name.to_string_lossy();
                        !name_str.starts_with('.')
                    })
                    .filter(|e| e.path().join(".phos.db").exists())
                    .map(|e| e.path())
                    .collect(),
                Err(e) => {
                    tracing::error!("Failed to read library root: {}", e);
                    Vec::new()
                }
            };

            for user_dir in &user_dirs {
                if *lock.lock().unwrap() {
                    return;
                }
                let user_db_path = user_dir.join(".phos.db");
                let user_scanner = scanner_ref.with_db_path(user_db_path.clone());
                let user_name = user_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                info!("Scanning user library: {}", user_name);

                if let Err(e) = user_scanner.rehash_files() {
                    tracing::error!("Rehash failed for user {}: {}", user_name, e);
                }
                if let Err(e) = user_scanner.scan(user_dir) {
                    tracing::error!("Scan failed for user {}: {}", user_name, e);
                }
                if let Err(e) = import::run_reorganize(user_dir, false) {
                    tracing::error!("Reorganize failed for user {}: {}", user_name, e);
                }

                // Spawn a ComfyUI worker for each existing user
                if let Some(ref url) = comfyui_url_bg {
                    info!("Spawning ComfyUI worker for user {}", user_name);
                    comfyui::spawn_enhancement_worker(
                        user_db_path,
                        url.clone(),
                        comfyui_shutdown.clone(),
                    );
                }
            }
            info!(
                "Multi-user startup scan complete ({} user libraries)",
                user_dirs.len()
            );
        })
    } else {
        // Single-user mode: scan the root library as before
        let scan_path = root_path.to_path_buf();
        let watcher_library_path = root_path.to_path_buf();
        let watcher_db_path = db_path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let (lock, cvar) = &*bg_shutdown;
            if *lock.lock().unwrap() {
                return;
            }
            if let Err(e) = scanner.rehash_files() {
                tracing::error!("Rehash failed: {}", e);
            }

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
                    // Periodically re-run reorganize (every 30 minutes) until shutdown.
                    let reorganize_interval = std::time::Duration::from_secs(30 * 60);
                    let mut guard = lock.lock().unwrap();
                    loop {
                        let (g, timeout) = cvar
                            .wait_timeout_while(guard, reorganize_interval, |stopped| !*stopped)
                            .unwrap();
                        guard = g;
                        if *guard {
                            break;
                        }
                        if timeout.timed_out() {
                            info!("Periodic reorganize triggered");
                            if let Err(e) = import::run_reorganize(&scan_path, false) {
                                tracing::error!("Periodic reorganize failed: {}", e);
                            }
                        }
                    }
                    info!("Shutdown signal received, stopping file watcher");
                    drop(watcher_handle);
                }
                Err(e) => {
                    tracing::error!("Failed to start file watcher: {}", e);
                }
            }
        })
    };

    // Spawn ComfyUI enhancement worker for single-user mode.
    // In multi-user mode, workers are spawned per user in the background scan
    // and dynamically when new users are created.
    let _comfyui_handle = if !multi_user {
        if let Some(ref url) = comfyui_url {
            let comfyui_shutdown = shutdown_flag.clone();
            Some(comfyui::spawn_enhancement_worker(
                db_path.to_path_buf(),
                url.clone(),
                comfyui_shutdown,
            ))
        } else {
            None
        }
    } else {
        None
    };

    let api_router = api::create_router(state);
    let static_dir = std::env::var("PHOS_STATIC_DIR").unwrap_or_else(|_| "static".to_string());
    let index_path = format!("{}/index.html", static_dir);
    let serve_static = ServeDir::new(&static_dir).not_found_service(ServeFile::new(index_path));

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
            .merge(Scalar::with_url("/api/docs", api::ApiDoc::openapi()))
            .fallback_service(serve_static)
            .layer(CorsLayer::permissive())
    } else {
        info!("OIDC authentication disabled (set PHOS_OIDC_ISSUER to enable)");
        Router::new()
            .merge(api_router)
            .merge(Scalar::with_url("/api/docs", api::ApiDoc::openapi()))
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
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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
