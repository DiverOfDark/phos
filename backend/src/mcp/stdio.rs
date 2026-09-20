//! `phos mcp` — the same server over a pipe.
//!
//! For a client that runs Phos itself rather than talking to a hosted one:
//! Claude Code, Claude Desktop, any editor that spawns an MCP process. It opens
//! the library directly, so there is no HTTP hop, no token, and no server to
//! have started first — the filesystem permission to read the library *is* the
//! authorization.
//!
//! Nothing may be written to stdout except protocol frames, so this
//! deliberately does not initialize the usual tracing subscriber; diagnostics go
//! to stderr or nowhere.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::transport::io::stdio;
use rmcp::ServiceExt;

use super::PhosMcp;
use crate::api::AppState;

/// Serve one library over stdio until the client hangs up.
pub async fn run(library: Option<PathBuf>, writable: bool) -> Result<()> {
    let root = library.unwrap_or_else(|| {
        PathBuf::from(
            std::env::var("PHOS_LIBRARY_PATH").unwrap_or_else(|_| "./library".to_string()),
        )
    });
    if !root.exists() {
        anyhow::bail!("library {} does not exist", root.display());
    }

    let db_path = root.join(".phos.db");
    crate::db::init_db(&db_path).map_err(|e| anyhow::anyhow!("data migrations failed: {e}"))?;
    let pool = crate::db::establish_pool(&db_path)
        .map_err(|e| anyhow::anyhow!("could not open the database: {e}"))?;
    crate::db::run_migrations(&pool)
        .map_err(|e| anyhow::anyhow!("schema migrations failed: {e}"))?;

    // The AI pipeline is what `trigger_scan` needs; loading it costs a model
    // download on a cold cache, so it is skipped unless writes are on. A
    // read-only session never touches it and should not wait for it.
    let ai = if writable {
        Some(crate::ai::AiPipeline::new().context("failed to load the AI models")?)
    } else {
        None
    };
    let scanner = Arc::new(crate::scanner::Scanner::new(db_path.clone(), ai));

    let state = AppState {
        pool,
        scanner,
        library_root: root,
        // Always single-user: a stdio client is one person on one library, and
        // the per-user split is a property of the HTTP server's OIDC setup.
        multi_user: false,
        user_pools: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        shutdown_flag: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
        organizer: crate::organizer::Organizer::new(),
        ingest: crate::ingest::IngestQueue::new(),
    };

    let service = PhosMcp::new(state, writable)
        .serve(stdio())
        .await
        .context("MCP handshake failed")?;
    service.waiting().await.context("MCP session ended badly")?;
    Ok(())
}
