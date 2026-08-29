//! Writing a task's fate back to its row.
//!
//! The interesting one is [`handle_failure`], which is where `retry_count`
//! finally earns its column. It was read in half a dozen places and never once
//! incremented, so a dropped connection was as terminal as a broken graph —
//! which is why a workflow that worked could still need several manual reruns.

use crate::comfyui::policy::{
    plan_failure, retry_resumes_prompt, FailureAction, FailureSite, MAX_ATTEMPTS,
};
use crate::comfyui::timestamp::format_ts;
use crate::schema::enhancement_tasks;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use tracing::{error, info, warn};

/// Record a failure, retrying it if the site says another attempt could help.
pub(super) fn handle_failure(
    conn: &mut SqliteConnection,
    task_id: &str,
    site: FailureSite,
    message: &str,
    retry_count: i32,
) {
    match plan_failure(site, message, retry_count) {
        FailureAction::Retry {
            attempt,
            delay,
            message,
        } => {
            let retry_at = format_ts(
                chrono::Utc::now().naive_utc() + chrono::Duration::seconds(delay.as_secs() as i64),
            );
            warn!(
                "Task {} hit a transient failure, attempt {}/{} in {}s: {}",
                task_id,
                attempt,
                MAX_ATTEMPTS,
                delay.as_secs(),
                message
            );
            let note = format!(
                "Retrying (attempt {}/{}): {}",
                attempt, MAX_ATTEMPTS, message
            );
            let filter = enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id));
            let _ = if retry_resumes_prompt(site) {
                // The prompt already reached ComfyUI; go back to watching it
                // rather than paying for the whole graph a second time.
                diesel::update(filter)
                    .set((
                        enhancement_tasks::status.eq("processing"),
                        enhancement_tasks::retry_count.eq(retry_count + 1),
                        enhancement_tasks::next_attempt_at.eq(&retry_at),
                        enhancement_tasks::error_message.eq(&note),
                    ))
                    .execute(conn)
            } else {
                diesel::update(filter)
                    .set((
                        enhancement_tasks::status.eq("pending"),
                        enhancement_tasks::retry_count.eq(retry_count + 1),
                        enhancement_tasks::next_attempt_at.eq(&retry_at),
                        enhancement_tasks::settle_until.eq(None::<String>),
                        enhancement_tasks::comfyui_prompt_id.eq(None::<String>),
                        enhancement_tasks::error_message.eq(&note),
                    ))
                    .execute(conn)
            };
        }
        FailureAction::Fail(message) => mark_failed(conn, task_id, &message),
    }
}

/// Mark a task as failed with an error message.
fn mark_failed(conn: &mut SqliteConnection, task_id: &str, error_msg: &str) {
    error!("Task {} failed: {}", task_id, error_msg);
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
        .set((
            enhancement_tasks::status.eq("failed"),
            enhancement_tasks::error_message.eq(error_msg),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
        ))
        .execute(conn);
}

pub(super) fn mark_completed(conn: &mut SqliteConnection, task_id: &str) {
    let now = format_ts(chrono::Utc::now().naive_utc());
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
        .set((
            enhancement_tasks::status.eq("completed"),
            enhancement_tasks::completed_at.eq(&now),
            enhancement_tasks::error_message.eq(None::<String>),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
            enhancement_tasks::settle_until.eq(None::<String>),
        ))
        .execute(conn);
    info!("Task {} completed successfully", task_id);
}

/// Did an earlier attempt already save a file for this task?
pub(super) fn task_has_output(conn: &mut SqliteConnection, task_id: &str) -> bool {
    enhancement_tasks::table
        .filter(
            enhancement_tasks::id
                .eq(task_id)
                .and(enhancement_tasks::output_file_id.is_not_null()),
        )
        .count()
        .get_result::<i64>(conn)
        .map(|c| c > 0)
        .unwrap_or(false)
}
