//! The background worker: the only part of this module that touches the
//! database or the network.
//!
//! One blocking thread per library, waking every three seconds to move tasks
//! along two paths:
//!
//! * [`dispatch`] takes a `pending` task through `uploading` to `queued` — read
//!   the source image, push it to ComfyUI, pin the output prefix, queue the
//!   prompt.
//! * [`complete`] follows a queued prompt through `processing`, possibly
//!   `awaiting_output`, to `completed` — or to `failed`, but only once the
//!   settle budget is spent and a retry could not help.
//!
//! It also carries the one piece of housekeeping that needs both a database and
//! a ComfyUI: [`contracts`] works out what each stored workflow accepts and
//! produces, for the rows imported before Phos asked that question.
//!
//! The decisions both paths make are pure functions in [`super::policy`] and
//! [`super::history`]; what is left here is the IO around them.

pub(super) mod advance;
mod complete;
mod contracts;
mod dispatch;
#[cfg(test)]
mod drain_tests;
mod status;
mod store;

use super::client::ComfyUiClient;
use super::STATUS_AWAITING_OUTPUT;
use crate::db;
use crate::schema::enhancement_tasks;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

/// How many three-second ticks between attempts to finish deriving contracts.
/// Five minutes: the only thing that changes the answer is ComfyUI coming back.
const CONTRACT_RETRY_TICKS: u64 = 100;

/// Spawn the enhancement worker. Returns a JoinHandle.
/// Follows the scanner.rs pattern: uses `spawn_blocking` with its own DB connection.
pub fn spawn_enhancement_worker(
    db_path: PathBuf,
    comfyui_url: String,
    shutdown: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let library_root = db_path.parent().unwrap().to_path_buf();
        let mut conn = match db::open_diesel_connection(&db_path) {
            Ok(c) => c,
            Err(e) => {
                error!("ComfyUI worker: failed to open DB: {}", e);
                return;
            }
        };
        let client = ComfyUiClient::new(&comfyui_url);
        info!("ComfyUI enhancement worker started (url: {})", comfyui_url);

        // Recover tasks that were mid-processing when we last shut down
        recover_interrupted_tasks(&mut conn);

        // Work out what the stored workflows accept and produce. A server that
        // is down right now leaves some of them under-typed, so keep asking on
        // a slow cadence rather than waiting for the next restart.
        let mut contracts_settled = contracts::backfill_contracts(&mut conn, &client);
        let mut contracts_due_in = CONTRACT_RETRY_TICKS;

        let (lock, cvar) = &*shutdown;
        loop {
            // Check shutdown
            if *lock.lock().unwrap() {
                info!("ComfyUI worker shutting down");
                break;
            }

            if !contracts_settled {
                contracts_due_in -= 1;
                if contracts_due_in == 0 {
                    contracts_due_in = CONTRACT_RETRY_TICKS;
                    contracts_settled = contracts::backfill_contracts(&mut conn, &client);
                }
            }

            dispatch::process_pending_tasks(&mut conn, &client, &library_root);
            complete::poll_active_tasks(&mut conn, &client, &library_root);
            // After completion and before cleanup: a task that just landed
            // queues the stage after it, and only then can the run it belongs
            // to be called finished and its intermediates swept.
            advance::advance_runs(&mut conn, &library_root);
            // And last, the batches: turn the next handful of matching shots
            // into runs. After `advance` rather than before, because the caps
            // are decided from how many of a batch's runs are still live — read
            // before this tick's completions landed, a batch would feed hardest
            // on the very tick the queue drained.
            crate::comfyui::batch::feed_batches(&mut conn, &library_root);
            cleanup_completed_tasks(&mut conn);

            // Sleep 3 seconds or until shutdown
            let guard = lock.lock().unwrap();
            let _ = cvar
                .wait_timeout(guard, std::time::Duration::from_secs(3))
                .unwrap();
        }
    })
}

/// Re-attach tasks that were mid-flight when we last shut down.
///
/// A task that already has a ComfyUI prompt id is *not* restarted: the prompt
/// may well have run, or still be running, while Phos was down. Restarting it
/// re-does the work and, worse, was one way a finished job came back as a
/// failure. Those go back to `processing` so the poller re-reads history (and,
/// if history is gone, probes the deterministic filenames). A task still
/// settling stays settling.
fn recover_interrupted_tasks(conn: &mut SqliteConnection) {
    // Had a prompt on ComfyUI: resume polling it rather than re-running it.
    if let Err(e) = diesel::update(
        enhancement_tasks::table.filter(
            enhancement_tasks::status
                .eq_any(&["queued", "processing", "downloading"])
                .and(enhancement_tasks::comfyui_prompt_id.is_not_null()),
        ),
    )
    .set(enhancement_tasks::status.eq("processing"))
    .execute(conn)
    {
        warn!("Failed to re-attach in-flight tasks: {}", e);
    }

    // Never reached ComfyUI: start over.
    if let Err(e) = diesel::update(
        enhancement_tasks::table.filter(
            enhancement_tasks::status
                .eq_any(&["uploading", "queued", "processing", "downloading"])
                .and(enhancement_tasks::comfyui_prompt_id.is_null()),
        ),
    )
    .set((
        enhancement_tasks::status.eq("pending"),
        enhancement_tasks::error_message.eq("Recovered after restart"),
        enhancement_tasks::next_attempt_at.eq(None::<String>),
    ))
    .execute(conn)
    {
        warn!("Failed to recover interrupted tasks: {}", e);
    }

    // `awaiting_output` is left exactly as it is — its deadline is still valid
    // and the poller picks it up — but the re-check clock is cleared so it is
    // looked at immediately rather than after a stale backoff.
    if let Err(e) = diesel::update(
        enhancement_tasks::table.filter(enhancement_tasks::status.eq(STATUS_AWAITING_OUTPUT)),
    )
    .set(enhancement_tasks::next_attempt_at.eq(None::<String>))
    .execute(conn)
    {
        warn!("Failed to resume settling tasks: {}", e);
    }
}

/// Remove completed tasks older than 5 minutes.
///
/// A task that is a step of a run is only swept once that whole run has landed.
/// Sweeping it earlier would take away the thing the next stage reads, the
/// parent link that says the continuation already happened, and the stage
/// history the board draws — and on a *failed* run, the intermediate a retry
/// from the failed stage resumes from. So only a run marked `completed` gives
/// its finished tasks up, and it gives them up on the same five-minute clock a
/// lone task always had.
fn cleanup_completed_tasks(conn: &mut SqliteConnection) {
    let cutoff = (chrono::Utc::now().naive_utc() - chrono::Duration::seconds(300))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let settled_runs = crate::schema::runs::table
        .filter(crate::schema::runs::status.eq(crate::comfyui::RunState::Completed.as_str()))
        .select(crate::schema::runs::id.nullable());
    match diesel::delete(
        enhancement_tasks::table.filter(
            enhancement_tasks::status
                .eq("completed")
                .and(enhancement_tasks::completed_at.is_not_null())
                .and(enhancement_tasks::completed_at.lt(&cutoff))
                .and(
                    enhancement_tasks::run_id
                        .is_null()
                        .or(enhancement_tasks::run_id.nullable().eq_any(settled_runs)),
                ),
        ),
    )
    .execute(conn)
    {
        Ok(n) if n > 0 => info!("Cleaned up {} completed enhancement tasks", n),
        Err(e) => warn!("Failed to clean up completed tasks: {}", e),
        _ => {}
    }
}
