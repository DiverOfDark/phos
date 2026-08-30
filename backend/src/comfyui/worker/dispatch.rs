//! `pending` → `uploading` → `queued`.
//!
//! Read the shot's source, push it to ComfyUI, rewrite the graph for this run,
//! and queue the prompt. Every step that can fail is tagged with a
//! [`FailureSite`] so [`super::complete::handle_failure`] can tell a dropped
//! connection (worth another go) from a graph ComfyUI refuses (never is).
//!
//! The graph is parsed *before* the source is read, because what the graph can
//! load is what decides the source's shape: a workflow with a video loader gets
//! the clip, one with only image loaders gets a frame of it.

use super::status::{handle_failure, live_task};
use crate::comfyui::client::ComfyUiClient;
use crate::comfyui::loaders::{
    bind_targets, role_directives, takes_video, LoaderKind, SourceBinding, SourceRole,
};
use crate::comfyui::policy::FailureSite;
use crate::comfyui::source::{read_source, resolve_source_file, SourceMode};
use crate::comfyui::timestamp::format_ts;
use crate::comfyui::workflow::{fresh_attempt_id, output_prefix_for_task, prepare_workflow};
use crate::schema::{comfyui_workflows, enhancement_tasks};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use serde_json::Value;
use std::path::Path;
use tracing::{error, info, warn};

/// A task waiting to be sent to ComfyUI.
struct PendingTask {
    id: String,
    shot_id: String,
    workflow_json: String,
    text_overrides: String,
    source_file_id: Option<String>,
    source_mode: Option<String>,
    retry_count: i32,
}

type PendingRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i32,
);

/// A step that failed, tagged with where it failed. The site is what decides
/// whether trying again could possibly help.
struct StepFailure {
    site: FailureSite,
    message: String,
}

impl StepFailure {
    fn new(site: FailureSite, context: &str, e: impl std::fmt::Display) -> Self {
        Self {
            site,
            message: format!("{}: {}", context, e),
        }
    }
}

/// Pick up pending tasks and start processing them.
pub(super) fn process_pending_tasks(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
) {
    let now = format_ts(chrono::Utc::now().naive_utc());

    let rows: Vec<PendingRow> = match enhancement_tasks::table
        .inner_join(
            comfyui_workflows::table.on(comfyui_workflows::id.eq(enhancement_tasks::workflow_id)),
        )
        .filter(
            enhancement_tasks::status.eq("pending").and(
                // A transient failure is re-queued with a backoff; do not pick it
                // up before that time.
                enhancement_tasks::next_attempt_at
                    .is_null()
                    .or(enhancement_tasks::next_attempt_at.le(&now)),
            ),
        )
        .order(enhancement_tasks::created_at.asc())
        .limit(5)
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            comfyui_workflows::workflow_json,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "COALESCE(enhancement_tasks.text_overrides, '{}')",
            ),
            enhancement_tasks::source_file_id,
            enhancement_tasks::source_mode,
            diesel::dsl::sql::<diesel::sql_types::Integer>(
                "COALESCE(enhancement_tasks.retry_count, 0)",
            ),
        ))
        .load::<PendingRow>(conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to query pending tasks: {}", e);
            return;
        }
    };

    for row in rows {
        let task = PendingTask {
            id: row.0,
            shot_id: row.1,
            workflow_json: row.2,
            text_overrides: row.3,
            source_file_id: row.4,
            source_mode: row.5,
            retry_count: row.6,
        };
        if let Err(failure) = dispatch_one(conn, client, library_root, &task, &now) {
            handle_failure(
                conn,
                &task.id,
                failure.site,
                &failure.message,
                task.retry_count,
            );
        }
    }
}

/// Take one task from `pending` to `queued`, or say where it fell over.
fn dispatch_one(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &PendingTask,
    now: &str,
) -> Result<(), StepFailure> {
    // Claim it. Zero rows means it was cancelled between the query and now;
    // there is nothing to send.
    let claimed = diesel::update(live_task(&task.id))
        .set((
            enhancement_tasks::status.eq("uploading"),
            enhancement_tasks::started_at.eq(now),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
        ))
        .execute(conn)
        .unwrap_or(0);
    if claimed == 0 {
        info!("Task {} was cancelled before dispatch; skipping", task.id);
        return Ok(());
    }

    // 1. Parse the workflow first: what the graph can load decides what the
    // source step should hand it.
    let workflow: Value = serde_json::from_str(&task.workflow_json)
        .map_err(|e| StepFailure::new(FailureSite::WorkflowJson, "Invalid workflow JSON", e))?;
    let text_overrides: std::collections::HashMap<String, String> =
        serde_json::from_str(&task.text_overrides).unwrap_or_default();

    // 2. Read the source in whatever shape this run asked for: a frame of the
    // video, or the video itself.
    let source = resolve_source_file(
        conn,
        &task.shot_id,
        task.source_file_id.as_deref(),
        library_root,
    )
    .map_err(|e| StepFailure::new(FailureSite::SourceImage, "Source file lookup failed", e))?;
    let mode = SourceMode::resolve(
        task.source_mode.as_deref(),
        takes_video(&workflow),
        source.is_video(),
    );
    let upload = read_source(conn, &source, mode).map_err(|e| {
        StepFailure::new(
            FailureSite::SourceImage,
            &format!("Source extraction failed ({})", mode),
            e,
        )
    })?;

    // 3. Upload to ComfyUI
    let uploaded_name = client
        .upload_file(&upload.filename, &upload.content_type, &upload.bytes)
        .map_err(|e| StepFailure::new(FailureSite::Upload, "Upload failed", e))?;

    // 4. Bind the upload to the loader nodes it belongs in. Writing it into
    // every loader is what made a start-frame/end-frame workflow impossible.
    let (target_role, role_overrides) = role_directives(&text_overrides);
    let binding = SourceBinding {
        uploaded_filename: &uploaded_name,
        kind: if mode == SourceMode::WholeVideo {
            LoaderKind::Video
        } else {
            LoaderKind::Image
        },
        role: target_role.unwrap_or(SourceRole::Start),
        role_overrides: &role_overrides,
    };
    let plan = bind_targets(&workflow, &binding);
    // An ambiguity is not a failure — the run still goes ahead with the first
    // candidate — but it is the difference between a clip that moves and one
    // that does not, so it is said out loud rather than swallowed.
    for warning in &plan.warnings {
        warn!("Task {}: {}", task.id, warning);
    }

    // Pin the output names before the run starts, and record the prefix so a
    // later poll can find the files even if history never mentions them. The
    // prefix is fresh per dispatch: ComfyUI keeps an earlier attempt's file and
    // advances the counter for the next one, so a reused prefix would let the
    // by-name probe import the stale first result.
    let output_prefix = output_prefix_for_task(&task.id, &fresh_attempt_id());
    let prepared = prepare_workflow(&workflow, &plan, &text_overrides, Some(&output_prefix));

    // 5. Queue prompt
    let prompt_id = client
        .queue_prompt(&prepared)
        .map_err(|e| StepFailure::new(FailureSite::Queue, "Queue failed", e))?;

    // 6. Set queued with comfyui_prompt_id. If the task was cancelled while we
    // were uploading, the row refuses the write — and the prompt we just queued
    // is one nobody will ever poll, so take it back off ComfyUI's queue.
    let still_ours = diesel::update(live_task(&task.id))
        .set((
            enhancement_tasks::status.eq("queued"),
            enhancement_tasks::comfyui_prompt_id.eq(&prompt_id),
            enhancement_tasks::output_prefix.eq(&output_prefix),
            enhancement_tasks::settle_until.eq(None::<String>),
        ))
        .execute(conn)
        .unwrap_or(0);
    if still_ours == 0 {
        info!(
            "Task {} was cancelled while dispatching; withdrawing prompt {}",
            task.id, prompt_id
        );
        if let Err(e) = client.delete_queued(&prompt_id) {
            error!("Could not withdraw prompt {}: {}", prompt_id, e);
        }
        return Ok(());
    }

    info!(
        "Task {} queued as ComfyUI prompt {} (output prefix {})",
        task.id, prompt_id, output_prefix
    );
    Ok(())
}
