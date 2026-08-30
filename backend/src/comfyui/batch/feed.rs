//! The tick that turns a query into runs, a handful at a time.
//!
//! This is the whole runtime of FR7 and it is deliberately small: ask the caps
//! what may happen, ask the library for that many shots, open a run for each,
//! move the cursor. Everything that could be got wrong quietly — how far the
//! cursor moves, which cap bites first, what a shot costs — is in
//! [`super::plan`] and has no database anywhere near it.
//!
//! It runs on the same three-second pass that walks runs along their lines, and
//! it runs **after** `advance`, not before: a tick that opened runs first would
//! read a `live_runs` count from before this tick's completions and feed too
//! hard on the very tick the queue drained.

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::path::Path;

use crate::comfyui::line::RunState;
use crate::comfyui::runs::{start_line_run_for_batch, StartError, SuppliedByStage};
use crate::models::BatchChangeset;

use super::plan::{advance_cursor, decide, tasks_by_stage, Feed};
use super::selection::{next_page, Narrowing};
use super::store::{self, BatchRow, BatchState};

/// What one tick did, for the log and for the tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FedBatch {
    pub batch_id: String,
    /// Runs actually opened.
    pub opened: usize,
    /// Shots the line refused — a video where it wants a still, a shot with no
    /// original file. Skipped rather than fatal: one shot that does not fit is
    /// not a reason to stop a batch of twelve thousand.
    pub refused: usize,
    /// The state the batch is in after the tick.
    pub state: BatchState,
    pub paused_reason: Option<&'static str>,
}

/// Feed every batch that is still feeding. One pass of the worker loop.
pub fn feed_batches(conn: &mut SqliteConnection, library_root: &Path) -> Vec<FedBatch> {
    let batches = match store::feeding(conn) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Could not read batches: {}", e);
            return Vec::new();
        }
    };

    let now = chrono::Local::now().naive_local();
    let mut fed = Vec::new();
    for batch in batches {
        match feed_one(conn, library_root, &batch, now) {
            Ok(result) => fed.push(result),
            Err(e) => tracing::error!("Batch {} could not be fed: {}", batch.id, e),
        }
    }
    fed
}

/// One batch, one tick.
pub fn feed_one(
    conn: &mut SqliteConnection,
    library_root: &Path,
    batch: &BatchRow,
    now: chrono::NaiveDateTime,
) -> QueryResult<FedBatch> {
    let costs = store::stage_costs(conn, &batch.line_id)?;
    if costs.is_empty() {
        // The line was deleted out from under the batch. Nothing can be opened
        // and nothing ever will be, so the batch is over rather than stuck.
        finish(conn, batch, BatchState::Completed)?;
        tracing::warn!(
            "Batch {} names line {}, which no longer has stages; marking it completed",
            batch.id,
            batch.line_id
        );
        return Ok(FedBatch {
            batch_id: batch.id.clone(),
            state: BatchState::Completed,
            ..Default::default()
        });
    }
    let tasks_per_shot: i64 = tasks_by_stage(&costs).iter().sum();
    let pulse = store::pulse(conn, batch, library_root, now)?;

    // Asked first as "the query still has shots", because that is true almost
    // always and costs no query to assume. An empty page below re-asks with
    // `exhausted`, which is the only case where the answer differs.
    let room = match decide(&batch.caps, &pulse, false, tasks_per_shot) {
        Feed::Open(n) => n,
        Feed::Idle => return settle(conn, batch, BatchState::Running, None, 0, 0),
        Feed::Pause(reason) => {
            return settle(
                conn,
                batch,
                BatchState::Paused,
                Some(reason.as_str()),
                0,
                0,
            )
        }
        // Unreachable with `exhausted = false`, but a `Done` here would be a
        // batch declared over while its query still had shots.
        Feed::Done => return settle(conn, batch, BatchState::Running, None, 0, 0),
    };

    let final_workflow = if batch.skip_if_generated {
        final_workflow_id(conn, &batch.line_id)?
    } else {
        None
    };
    let narrow = Narrowing {
        skip_line_id: batch.skip_if_generated.then_some(batch.line_id.as_str()),
        skip_workflow_id: final_workflow.as_deref(),
        after: batch.cursor.as_ref(),
        limit: Some(room),
    };
    let page = next_page(conn, &batch.selection, &narrow)?;

    if page.is_empty() {
        // The query has no more shots past the cursor. Whether that is "done"
        // depends on whether anything this batch already opened is still going.
        let state = match decide(&batch.caps, &pulse, true, tasks_per_shot) {
            Feed::Done => BatchState::Completed,
            _ => BatchState::Running,
        };
        return settle(conn, batch, state, None, 0, 0);
    }

    let answers: SuppliedByStage = batch
        .stage_values
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let mut opened = 0usize;
    let mut refused = 0usize;
    for cursor in &page {
        match start_line_run_for_batch(
            conn,
            &batch.line_id,
            &cursor.shot_id,
            &answers,
            Some(&batch.id),
        ) {
            Ok(_) => opened += 1,
            Err(StartError::Rejected(e)) => {
                refused += 1;
                tracing::debug!(
                    "Batch {} skipped shot {}: {}",
                    batch.id,
                    cursor.shot_id,
                    e.message
                );
            }
            Err(StartError::NotFound(what)) => {
                // The shot went away between the page and the open. The cursor
                // still moves past it; it is not coming back.
                refused += 1;
                tracing::debug!("Batch {} skipped a missing {}", batch.id, what);
            }
            Err(StartError::Db(e)) => {
                // Stop *this* batch's tick here rather than plough on: the
                // cursor is advanced below only as far as what actually got
                // looked at, so the next tick resumes from the right place.
                tracing::error!("Batch {} could not open a run: {}", batch.id, e);
                break;
            }
        }
    }

    // The cursor moves past everything the page named, opened or refused. A
    // refused shot re-offered every tick is a batch that never finishes.
    let looked_at = &page[..(opened + refused).min(page.len())];
    let cursor = advance_cursor(batch.cursor.clone(), looked_at);
    apply_cursor(conn, batch, cursor.as_ref())?;

    settle(conn, batch, BatchState::Running, None, opened, refused)
}

/// The workflow of the line's last stage — what "output from this line" means
/// in the generations data.
fn final_workflow_id(conn: &mut SqliteConnection, line_id: &str) -> QueryResult<Option<String>> {
    use crate::schema::line_stages;
    line_stages::table
        .filter(line_stages::line_id.eq(line_id))
        .order(line_stages::stage_idx.desc())
        .select(line_stages::workflow_id)
        .first(conn)
        .optional()
}

fn apply_cursor(
    conn: &mut SqliteConnection,
    batch: &BatchRow,
    cursor: Option<&super::plan::Cursor>,
) -> QueryResult<()> {
    let Some(cursor) = cursor else {
        return Ok(());
    };
    store::apply(
        conn,
        &batch.id,
        BatchChangeset {
            cursor_key: Some(Some(&cursor.key)),
            cursor_shot_id: Some(Some(&cursor.shot_id)),
            ..Default::default()
        },
    )
}

/// Write back what the tick concluded, and only when it changed something.
fn settle(
    conn: &mut SqliteConnection,
    batch: &BatchRow,
    state: BatchState,
    reason: Option<&'static str>,
    opened: usize,
    refused: usize,
) -> QueryResult<FedBatch> {
    if state != batch.state || reason.map(str::to_string) != batch.paused_reason {
        let finished_at = state
            .is_terminal()
            .then(|| chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
        store::apply(
            conn,
            &batch.id,
            BatchChangeset {
                status: Some(state.as_str()),
                paused_reason: Some(reason),
                finished_at: finished_at.as_deref().map(Some),
                ..Default::default()
            },
        )?;
    }
    Ok(FedBatch {
        batch_id: batch.id.clone(),
        opened,
        refused,
        state,
        paused_reason: reason,
    })
}

fn finish(
    conn: &mut SqliteConnection,
    batch: &BatchRow,
    state: BatchState,
) -> QueryResult<()> {
    let finished_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    store::apply(
        conn,
        &batch.id,
        BatchChangeset {
            status: Some(state.as_str()),
            paused_reason: Some(None),
            finished_at: Some(Some(&finished_at)),
            ..Default::default()
        },
    )
}

/// What a STOP left to do on ComfyUI's side.
#[derive(Debug, Clone, Default)]
pub struct Stopped {
    /// Runs of the batch that were cancelled.
    pub cancelled_runs: usize,
    /// Prompts to drop from ComfyUI's own queue. The caller does that, because
    /// it is network and this is not.
    pub prompt_ids: Vec<String>,
}

/// STOP: instant, because most of the batch was never rows.
///
/// The order is load-bearing. The batch's status goes to `stopped` **first**,
/// so a feeder tick running concurrently sees a terminal batch and opens
/// nothing, and only then are the runs that do exist cancelled. The other order
/// leaves a window in which the feeder opens runs behind the cancel and the
/// batch stops with work still queued.
///
/// A held run is cancelled through its hold's own Cancel verdict, so an
/// abandoned hold is recorded like every other verdict rather than becoming a
/// second spelling of cancel that differs by which button was pressed.
pub fn stop(
    conn: &mut SqliteConnection,
    library_root: &Path,
    batch_id: &str,
) -> QueryResult<Stopped> {
    let prompt_ids = store::live_prompt_ids(conn, batch_id)?;

    let finished_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    store::apply(
        conn,
        batch_id,
        BatchChangeset {
            status: Some(BatchState::Stopped.as_str()),
            paused_reason: Some(None),
            finished_at: Some(Some(&finished_at)),
            ..Default::default()
        },
    )?;

    let live = store::live_run_ids(conn, batch_id)?;
    let mut cancelled = 0usize;
    for run_id in &live {
        let status: Option<String> = crate::schema::runs::table
            .filter(crate::schema::runs::id.eq(run_id))
            .select(crate::schema::runs::status)
            .first(conn)
            .optional()?;
        let held = status.as_deref() == Some(RunState::Held.as_str());
        if held
            && crate::comfyui::holds::give_verdict(
                conn,
                library_root,
                run_id,
                crate::comfyui::Verdict::Cancel,
                &[],
                None,
            )
            .is_ok()
        {
            cancelled += 1;
            continue;
        }
        // Either it was not held, or it was marked held with nothing to hold —
        // FR5c's one non-self-evidencing marker. Stopping a run is the thing
        // that must always work, so this falls through rather than refusing.
        match crate::comfyui::cancel_run(conn, run_id) {
            Ok(_) => cancelled += 1,
            Err(e) => tracing::warn!("Could not cancel run {} of batch {}: {}", run_id, batch_id, e),
        }
    }

    Ok(Stopped {
        cancelled_runs: cancelled,
        prompt_ids,
    })
}
