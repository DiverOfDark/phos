//! Following a queued prompt to its end.
//!
//! Reads `/history`, turns it into a [`HistoryVerdict`], and acts on it. The
//! only subtle case is the one this whole module exists for: ComfyUI says the
//! prompt finished but names no file. That is *not* a failure. The task moves
//! to `awaiting_output` with a deadline, and on every re-check we also probe
//! `/view` for the filenames we pinned before the run started — a file on disk
//! beats a silent history entry, and history is silent after a ComfyUI restart.

use super::status::{handle_failure, mark_completed, task_has_output};
use super::store::download_and_save_output;
use crate::comfyui::client::ComfyUiClient;
use crate::comfyui::history::{execution_error_traceback, interpret_history, HistoryVerdict};
use crate::comfyui::outputs::{fallback_output_candidates, OutputRef};
use crate::comfyui::policy::{decide_settle, settle_budget, FailureSite, SettleDecision};
use crate::comfyui::timestamp::{format_ts, parse_ts};
use crate::comfyui::STATUS_AWAITING_OUTPUT;
use crate::schema::{comfyui_workflows, enhancement_tasks};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use serde_json::Value;
use std::path::Path;
use tracing::{error, info, warn};

/// A task the poller is following, with everything it needs to decide.
pub(super) struct ActiveTask {
    pub id: String,
    pub shot_id: String,
    pub prompt_id: String,
    pub workflow_id: String,
    pub workflow_json: String,
    pub text_overrides: String,
    pub status: String,
    pub output_prefix: Option<String>,
    pub settle_until: Option<String>,
    pub retry_count: i32,
}

type ActiveTaskRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i32,
);

/// Poll tasks that are queued/processing/settling against ComfyUI history.
pub(super) fn poll_active_tasks(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
) {
    let now_dt = chrono::Utc::now().naive_utc();
    let now = format_ts(now_dt);

    let rows: Vec<ActiveTaskRow> = match enhancement_tasks::table
        .inner_join(
            comfyui_workflows::table.on(comfyui_workflows::id.eq(enhancement_tasks::workflow_id)),
        )
        .filter(
            enhancement_tasks::status
                .eq_any(&["queued", "processing", STATUS_AWAITING_OUTPUT])
                .and(enhancement_tasks::comfyui_prompt_id.is_not_null())
                // A settling task sets its own re-check time; leave it alone
                // until then.
                .and(
                    enhancement_tasks::next_attempt_at
                        .is_null()
                        .or(enhancement_tasks::next_attempt_at.le(&now)),
                ),
        )
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            enhancement_tasks::comfyui_prompt_id.assume_not_null(),
            enhancement_tasks::workflow_id,
            comfyui_workflows::workflow_json,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "COALESCE(enhancement_tasks.text_overrides, '{}')",
            ),
            enhancement_tasks::status,
            enhancement_tasks::output_prefix,
            enhancement_tasks::settle_until,
            diesel::dsl::sql::<diesel::sql_types::Integer>(
                "COALESCE(enhancement_tasks.retry_count, 0)",
            ),
        ))
        .load::<ActiveTaskRow>(conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to query active tasks: {}", e);
            return;
        }
    };

    for row in rows {
        let task = ActiveTask {
            id: row.0,
            shot_id: row.1,
            prompt_id: row.2,
            workflow_id: row.3,
            workflow_json: row.4,
            text_overrides: row.5,
            status: row.6,
            output_prefix: row.7,
            settle_until: row.8,
            retry_count: row.9,
        };
        poll_one_task(conn, client, library_root, &task, now_dt);
    }
}

fn poll_one_task(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &ActiveTask,
    now_dt: chrono::NaiveDateTime,
) {
    // Move out of `queued` as soon as we start watching it.
    if task.status == "queued" {
        let _ = diesel::update(
            enhancement_tasks::table.filter(
                enhancement_tasks::id
                    .eq(&task.id)
                    .and(enhancement_tasks::status.eq("queued")),
            ),
        )
        .set(enhancement_tasks::status.eq("processing"))
        .execute(conn);
    }

    let settling = task.status == STATUS_AWAITING_OUTPUT;

    let history = match fetch_history(conn, client, task) {
        Ok(h) => h,
        Err(()) => return,
    };

    let verdict = match history.as_ref() {
        Some(h) => interpret_history(h),
        // No history entry at all: treat it as "finished, named nothing" so the
        // settle path gets its chance to find the file by name.
        None => HistoryVerdict::NoOutputs,
    };

    match verdict {
        HistoryVerdict::Running => {
            // Still executing. If we were settling, ComfyUI re-queued the prompt;
            // drop back to processing and let the run finish.
            if settling {
                let _ = diesel::update(
                    enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)),
                )
                .set((
                    enhancement_tasks::status.eq("processing"),
                    enhancement_tasks::next_attempt_at.eq(None::<String>),
                ))
                .execute(conn);
            }
        }
        HistoryVerdict::Failed(message) => {
            // A node raised. Nothing about retrying changes that, so report the
            // real message and stop.
            if let Some(tb) = history.as_ref().and_then(execution_error_traceback) {
                error!("Task {} traceback:\n{}", task.id, tb);
            }
            handle_failure(
                conn,
                &task.id,
                FailureSite::Execution,
                &message,
                task.retry_count,
            );
        }
        HistoryVerdict::Outputs(refs) => {
            download_all(conn, client, library_root, task, &refs);
        }
        HistoryVerdict::NoOutputs => {
            settle_task(conn, client, library_root, task, now_dt, history.as_ref());
        }
    }
}

/// Read the prompt's history entry.
///
/// `Ok(None)` means ComfyUI has never heard of it *and* it is not queued — the
/// job is lost, most likely to a restart, which clears history but not the
/// output directory. `Err(())` means there is nothing to decide this round.
fn fetch_history(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    task: &ActiveTask,
) -> Result<Option<Value>, ()> {
    match client.get_history(&task.prompt_id) {
        Ok(Some(h)) => Ok(Some(h)),
        Ok(None) => match client.is_prompt_in_queue(&task.prompt_id) {
            // Still pending or running: check again next cycle.
            Ok(true) => Err(()),
            Ok(false) => Ok(None),
            Err(e) => {
                warn!("Failed to check queue for prompt {}: {}", task.prompt_id, e);
                Err(())
            }
        },
        Err(e) => {
            // History is unreadable — a transport problem, not a workflow
            // problem. Keep the task alive; only give up after MAX_ATTEMPTS.
            handle_failure(
                conn,
                &task.id,
                FailureSite::History,
                &format!("History fetch failed for prompt {}: {}", task.prompt_id, e),
                task.retry_count,
            );
            Err(())
        }
    }
}

/// Download everything history named. Succeeding on any one file completes the
/// task; failing on all of them is transient, because a 404 from `/view` is very
/// often a file that is written but not yet closed.
fn download_all(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &ActiveTask,
    refs: &[OutputRef],
) {
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
        .set(enhancement_tasks::status.eq("downloading"))
        .execute(conn);

    let mut errors: Vec<String> = Vec::new();
    let mut downloaded = false;
    for out in refs {
        match download_and_save_output(conn, client, task, out, library_root) {
            Ok(_) => downloaded = true,
            Err(e) => {
                error!(
                    "Failed to download output {} for task {}: {}",
                    out.describe(),
                    task.id,
                    e
                );
                errors.push(e.to_string());
            }
        }
    }

    if downloaded || task_has_output(conn, &task.id) {
        mark_completed(conn, &task.id);
        return;
    }

    // Say what actually went wrong. The old message ("No output images found in
    // ComfyUI response") blamed the workflow for what was usually a 404.
    let detail = errors
        .first()
        .cloned()
        .unwrap_or_else(|| "no reason reported".to_string());
    let message = format!(
        "ComfyUI named {} output file(s) but none could be downloaded. First error: {}",
        refs.len(),
        detail
    );
    handle_failure(
        conn,
        &task.id,
        FailureSite::Download,
        &message,
        task.retry_count,
    );
}

/// ComfyUI says it is done but has named no file. That is a state, not a
/// verdict: wait, and meanwhile look for the file under the name we pinned.
fn settle_task(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &ActiveTask,
    now_dt: chrono::NaiveDateTime,
    history: Option<&Value>,
) {
    let workflow: Value = serde_json::from_str(&task.workflow_json).unwrap_or(Value::Null);

    // The file may already be on disk under the deterministic prefix even though
    // history is silent about it. One hit finishes the task.
    if let Some(prefix) = task.output_prefix.as_deref() {
        for candidate in fallback_output_candidates(prefix) {
            // A miss is the normal case for most candidates — only the right
            // extension exists — so this is not worth logging per file.
            if download_and_save_output(conn, client, task, &candidate, library_root).is_ok() {
                info!(
                    "Task {} recovered output {} by name; history never listed it",
                    task.id,
                    candidate.describe()
                );
                mark_completed(conn, &task.id);
                return;
            }
        }
    }

    if task_has_output(conn, &task.id) {
        mark_completed(conn, &task.id);
        return;
    }

    let budget = settle_budget(&workflow);
    let settle_until = task.settle_until.as_deref().and_then(parse_ts);

    match decide_settle(now_dt, settle_until, budget) {
        SettleDecision::Start {
            deadline,
            recheck_at,
        } => {
            info!(
                "Task {} finished with no files listed; waiting up to {}s for them",
                task.id,
                budget.as_secs()
            );
            let _ =
                diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
                    .set((
                        enhancement_tasks::status.eq(STATUS_AWAITING_OUTPUT),
                        enhancement_tasks::settle_until.eq(format_ts(deadline)),
                        enhancement_tasks::next_attempt_at.eq(format_ts(recheck_at)),
                        enhancement_tasks::error_message.eq(None::<String>),
                    ))
                    .execute(conn);
        }
        SettleDecision::Wait { recheck_at } => {
            let _ =
                diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
                    .set((
                        enhancement_tasks::status.eq(STATUS_AWAITING_OUTPUT),
                        enhancement_tasks::next_attempt_at.eq(format_ts(recheck_at)),
                    ))
                    .execute(conn);
        }
        SettleDecision::Expired => {
            let message = gave_up_message(task, history, budget);
            error!("Task {} gave up settling: {}", task.id, message);
            handle_failure(
                conn,
                &task.id,
                FailureSite::Settle,
                &message,
                task.retry_count,
            );
        }
    }
}

/// "Finished but silent" and "vanished from ComfyUI entirely" are different
/// problems, and the user needs to be told which one.
fn gave_up_message(
    task: &ActiveTask,
    history: Option<&Value>,
    budget: std::time::Duration,
) -> String {
    let prefix = task.output_prefix.as_deref().unwrap_or("(none)");
    match history {
        Some(h) => {
            let outputs_debug = h
                .get("outputs")
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "N/A".to_string()))
                .unwrap_or_else(|| "null".to_string());
            format!(
                "ComfyUI reported prompt {} finished but published no file within {}s, \
                 and nothing was found under the pinned prefix {}. Outputs: {}",
                task.prompt_id,
                budget.as_secs(),
                prefix,
                outputs_debug
            )
        }
        None => format!(
            "Prompt {} is in neither ComfyUI's history nor its queue, and no file \
             appeared under the pinned prefix {} within {}s (job lost, most likely a \
             ComfyUI restart)",
            task.prompt_id,
            prefix,
            budget.as_secs()
        ),
    }
}
