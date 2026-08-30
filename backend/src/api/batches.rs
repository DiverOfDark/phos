//! Sending a library at a line, and watching what it does.
//!
//! Five endpoints and one of them matters most. `POST /batches/preview` writes
//! nothing and answers with the numbers the confirm sheet shows — how many
//! shots matched, how many already have output from this line, what the rest
//! comes to in tasks, GPU hours and disk. Nothing is queued until somebody has
//! seen those and pressed Send, because at this scale the way it goes wrong is
//! not a crash but a mountain nobody asked for.
//!
//! `POST /batches` then takes the same body and writes one row. Not fifty
//! thousand — see [`crate::comfyui::batch`] for why that is the whole design.

use axum::{extract::Path, http::StatusCode, Json};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::comfyui::batch::{
    plan::{Caps, Estimate},
    selection::Selection,
    store::{self, BatchRow},
};

use super::comfyui::{require_comfyui, ApiError};
use super::UState;

// ===== Payloads =============================================================

/// The caps a batch is sent with. Every one is optional; a small selection
/// sensibly has none.
#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
pub(super) struct CapsPayload {
    /// Tasks this batch may open per day. The day is the *user's*, not
    /// Greenwich's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_task_cap: Option<i64>,
    /// Minutes from local midnight at which the batch may feed. `[start, end)`,
    /// and it may wrap midnight — `1320`/`360` is 22:00 to 06:00. A window only
    /// paces work; it never starts a batch that was not sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start_minute: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_end_minute: Option<i32>,
    /// Pause while free space on the library volume is at or below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_floor_bytes: Option<i64>,
    /// Pause while this many of the batch's runs are waiting for a verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_outstanding_holds: Option<i64>,
}

impl CapsPayload {
    fn to_caps(&self) -> Caps {
        Caps {
            daily_task_cap: self.daily_task_cap.filter(|n| *n > 0),
            window: match (self.window_start_minute, self.window_end_minute) {
                (Some(start), Some(end)) => Some((start.clamp(0, 1439), end.clamp(0, 1440))),
                _ => None,
            },
            disk_floor_bytes: self.disk_floor_bytes.filter(|n| *n > 0),
            max_outstanding_holds: self.max_outstanding_holds.filter(|n| *n > 0),
            lead: None,
        }
    }

    fn from_caps(caps: &Caps) -> Self {
        CapsPayload {
            daily_task_cap: caps.daily_task_cap,
            window_start_minute: caps.window.map(|w| w.0),
            window_end_minute: caps.window.map(|w| w.1),
            disk_floor_bytes: caps.disk_floor_bytes,
            max_outstanding_holds: caps.max_outstanding_holds,
        }
    }
}

/// What to send, and where.
#[derive(Debug, Deserialize, ToSchema)]
pub(super) struct BatchPayload {
    pub line_id: String,
    /// Either `{"kind":"ids","ids":[...]}` or `{"kind":"query","query":{...}}`,
    /// where the query is exactly what `GET /api/shots` takes.
    pub selection: Selection,
    /// What each stage of the line left open, same shape as `POST /runs`.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub stage_values: crate::comfyui::runs::SuppliedByStage,
    /// Skip shots that already have output from this line. On by default: at
    /// batch scale this is a filter, not a warning.
    #[serde(default = "yes")]
    pub skip_if_generated: bool,
    #[serde(default)]
    pub caps: CapsPayload,
    /// Overrides the generated one, which reads `<line> · <what was selected>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn yes() -> bool {
    true
}

/// The confirm sheet, in one object.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct BatchPreview {
    pub line_id: String,
    pub line_name: String,
    pub label: String,
    #[serde(flatten)]
    pub estimate: Estimate,
    /// What each stage is expected to cost per task, and whether that came from
    /// this library's own history or from a guess. The sheet says so out loud
    /// rather than presenting an invented number as a measurement.
    pub stages: Vec<StagePreview>,
    /// Present when the batch cannot be sent at all, with the reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(super) struct StagePreview {
    pub stage_idx: i32,
    pub workflow_name: String,
    /// Tasks each upstream task becomes — 1 ordinarily, 4 for `×4 seeds`.
    pub fanout: i64,
    pub seconds_per_task: f64,
    pub bytes_per_task: i64,
    pub measured: bool,
    pub keeps_output: bool,
    pub holds: bool,
}

/// One batch on the board.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct BatchBrief {
    pub id: String,
    pub line_id: String,
    pub label: String,
    pub status: String,
    /// `window`, `daily_cap`, `disk_floor` or `holds` when paused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_reason: Option<String>,
    /// The same reason as a sentence, so a board can say why nothing has
    /// happened for six hours without inventing its own wording.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_note: Option<String>,
    pub skip_if_generated: bool,
    pub selection: Selection,
    pub caps: CapsPayload,
    /// What the sheet said when Send was pressed.
    pub matched_total: Option<i32>,
    pub skipped_total: Option<i32>,
    pub est_tasks: Option<i32>,
    pub est_gpu_seconds: Option<i32>,
    pub est_disk_bytes: Option<i64>,
    /// Runs of this batch, by state. These are *counted*, so they cannot
    /// disagree with the runs they describe.
    pub runs_running: i64,
    pub runs_held: i64,
    pub runs_completed: i64,
    pub runs_failed: i64,
    pub runs_cancelled: i64,
    pub runs_opened: i64,
    /// How far the query has been walked. `null` before the first tick.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_shot_id: Option<String>,
    pub created_at: Option<String>,
    pub finished_at: Option<String>,
}

fn brief(conn: &mut SqliteConnection, row: BatchRow) -> BatchBrief {
    let counts = store::run_counts(conn, &row.id).unwrap_or_default();
    let paused_note = row.paused_reason.as_deref().and_then(note_for);
    BatchBrief {
        id: row.id,
        line_id: row.line_id,
        label: row.label,
        status: row.state.as_str().to_string(),
        paused_reason: row.paused_reason,
        paused_note,
        skip_if_generated: row.skip_if_generated,
        selection: row.selection,
        caps: CapsPayload::from_caps(&row.caps),
        matched_total: row.matched_total,
        skipped_total: row.skipped_total,
        est_tasks: row.est_tasks,
        est_gpu_seconds: row.est_gpu_seconds,
        est_disk_bytes: row.est_disk_bytes,
        runs_running: counts.running,
        runs_held: counts.held,
        runs_completed: counts.completed,
        runs_failed: counts.failed,
        runs_cancelled: counts.cancelled,
        runs_opened: counts.opened(),
        cursor_shot_id: row.cursor.map(|c| c.shot_id),
        created_at: row.created_at,
        finished_at: row.finished_at,
    }
}

fn note_for(reason: &str) -> Option<String> {
    use crate::comfyui::batch::plan::PauseReason::*;
    let described = match reason {
        "window" => OutsideWindow,
        "daily_cap" => DailyCap,
        "disk_floor" => DiskFloor,
        "holds" => HoldCap,
        _ => return None,
    };
    Some(described.describe().to_string())
}

// ===== Handlers =============================================================

/// The confirm sheet. Writes nothing.
#[utoipa::path(
    post,
    path = "/api/comfyui/batches/preview",
    tag = "comfyui",
    summary = "What sending this selection to this line would cost",
    description = "Counts what the selection matches, how much of it already has output from \
                   this line, and what the rest comes to in tasks, GPU seconds and disk. Writes \
                   nothing: this is what the confirm sheet shows before Send.",
    request_body = BatchPayload,
    responses(
        (status = 200, body = BatchPreview),
        (status = 400, description = "The selection cannot be sent"),
        (status = 404, description = "No such line"),
    )
)]
pub(super) async fn preview_batch(
    UState(state): UState,
    Json(payload): Json<BatchPayload>,
) -> Result<Json<BatchPreview>, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    let (line_name, stages) = line_of(&mut conn, &payload.line_id)?;

    let refused = payload.selection.validate().err();

    let (matched, skipped) = counts_for(&mut conn, &payload)?;
    let (estimate, costs) = store::estimate_for(&mut conn, &payload.line_id, matched, skipped)
        .map_err(|e| {
            tracing::error!("Could not estimate batch: {}", e);
            ApiError::internal()
        })?;

    let stage_previews = costs
        .iter()
        .zip(&stages)
        .enumerate()
        .map(|(idx, (cost, name))| StagePreview {
            stage_idx: idx as i32,
            workflow_name: name.clone(),
            fanout: cost.fanout,
            seconds_per_task: cost.seconds,
            bytes_per_task: cost.bytes,
            measured: cost.seconds_measured && cost.bytes_measured,
            keeps_output: cost.keeps_output,
            holds: cost.holds,
        })
        .collect();

    Ok(Json(BatchPreview {
        label: payload
            .label
            .clone()
            .unwrap_or_else(|| format!("{} · {}", line_name, payload.selection.shorthand())),
        line_id: payload.line_id,
        line_name,
        estimate,
        stages: stage_previews,
        refused,
    }))
}

/// Send it.
#[utoipa::path(
    post,
    path = "/api/comfyui/batches",
    tag = "comfyui",
    summary = "Send a selection of shots to a line",
    description = "Writes one batch row holding the query and a cursor. No runs are opened here: \
                   the worker pulls the next handful each tick, which is what makes STOP instant \
                   and keeps the queue board a board.",
    request_body = BatchPayload,
    responses(
        (status = 200, body = BatchBrief),
        (status = 400, description = "The selection cannot be sent"),
        (status = 404, description = "No such line"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn create_batch(
    UState(state): UState,
    Json(payload): Json<BatchPayload>,
) -> Result<Json<BatchBrief>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    let (line_name, _) = line_of(&mut conn, &payload.line_id)?;

    payload
        .selection
        .validate()
        .map_err(ApiError::bad_request)?;

    let (matched, skipped) = counts_for(&mut conn, &payload)?;
    if matched - skipped <= 0 {
        return Err(ApiError::bad_request(
            "Nothing to run: every shot this matches already has output from this line.",
        ));
    }

    let (estimate, _) = store::estimate_for(&mut conn, &payload.line_id, matched, skipped)
        .map_err(|_| ApiError::internal())?;
    let stage_values = (!payload.stage_values.is_empty())
        .then(|| serde_json::to_string(&payload.stage_values).ok())
        .flatten();
    let label = payload
        .label
        .clone()
        .unwrap_or_else(|| format!("{} · {}", line_name, payload.selection.shorthand()));

    let id = store::create(
        &mut conn,
        &payload.line_id,
        &label,
        &payload.selection,
        stage_values.as_deref(),
        payload.skip_if_generated,
        &payload.caps.to_caps(),
        &estimate,
    )
    .map_err(|e| {
        tracing::error!("Could not create batch: {}", e);
        ApiError::internal()
    })?;

    let row = store::load(&mut conn, &id)
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::internal)?;
    Ok(Json(brief(&mut conn, row)))
}

/// The board.
#[utoipa::path(
    get,
    path = "/api/comfyui/batches",
    tag = "comfyui",
    summary = "Every batch, newest first",
    responses((status = 200, body = Vec<BatchBrief>))
)]
pub(super) async fn list_batches(UState(state): UState) -> Result<Json<Vec<BatchBrief>>, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    let rows = store::list(&mut conn).map_err(|_| ApiError::internal())?;
    Ok(Json(
        rows.into_iter().map(|r| brief(&mut conn, r)).collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/comfyui/batches/{id}",
    tag = "comfyui",
    summary = "One batch",
    responses(
        (status = 200, body = BatchBrief),
        (status = 404, description = "No such batch"),
    )
)]
pub(super) async fn get_batch(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<BatchBrief>, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    let row = store::load(&mut conn, &id)
        .map_err(|_| ApiError::internal())?
        .ok_or(ApiError::from(StatusCode::NOT_FOUND))?;
    Ok(Json(brief(&mut conn, row)))
}

/// STOP.
#[utoipa::path(
    post,
    path = "/api/comfyui/batches/{id}/stop",
    tag = "comfyui",
    summary = "Stop a batch, and purge what it has queued on ComfyUI",
    description = "Instant, because most of the batch was never rows. The batch is marked stopped \
                   first — so a worker tick running at the same moment opens nothing behind the \
                   cancel — and only then are its live runs cancelled and their prompts dropped \
                   from ComfyUI's own queue.",
    responses(
        (status = 200, description = "Stopped"),
        (status = 404, description = "No such batch"),
    )
)]
pub(super) async fn stop_batch(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    if store::load(&mut conn, &id)
        .map_err(|_| ApiError::internal())?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND.into());
    }

    let stopped =
        crate::comfyui::batch::stop(&mut conn, &state.library_root, &id).map_err(|e| {
            tracing::error!("Could not stop batch {}: {}", id, e);
            ApiError::internal()
        })?;

    let purged = stopped.prompt_ids.len();
    if let (Some(url), false) = (state.comfyui_url.clone(), stopped.prompt_ids.is_empty()) {
        let prompt_ids = stopped.prompt_ids.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let client = crate::comfyui::ComfyUiClient::new(&url);
            for prompt_id in prompt_ids {
                // Only the running prompt is worth interrupting: /interrupt
                // stops whatever is executing, so firing it for a merely-queued
                // prompt would kill somebody else's job.
                if let Ok(true) = client.is_prompt_running(&prompt_id) {
                    if let Err(e) = client.interrupt() {
                        tracing::warn!("Interrupt for prompt {} failed: {}", prompt_id, e);
                    }
                }
                if let Err(e) = client.delete_queued(&prompt_id) {
                    tracing::warn!("Dropping prompt {} from the queue failed: {}", prompt_id, e);
                }
            }
        })
        .await;
    }

    Ok(Json(serde_json::json!({
        "status": "stopped",
        "cancelled_runs": stopped.cancelled_runs,
        "purged_prompts": purged,
    })))
}

// ===== Saved selections =====================================================

#[derive(Debug, Deserialize, ToSchema)]
pub(super) struct SavedSelectionPayload {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    pub selection: Selection,
    #[serde(default)]
    pub caps: CapsPayload,
    #[serde(default = "yes")]
    pub skip_if_generated: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(super) struct SavedSelectionBrief {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    pub selection: Selection,
    pub caps: CapsPayload,
    pub skip_if_generated: bool,
    pub created_at: Option<String>,
}

/// A query plus the line you usually send it to.
///
/// It makes a repeat one click and it **never fires on its own** — there is no
/// schedule here and there is not going to be one. Saving one starts nothing.
#[utoipa::path(
    post,
    path = "/api/comfyui/selections",
    tag = "comfyui",
    summary = "Save a selection for re-sending",
    description = "A query plus the line you usually send it to, so a repeat is one click. It \
                   never fires on its own: a batch exists because a person pressed Send.",
    request_body = SavedSelectionPayload,
    responses((status = 200, body = SavedSelectionBrief))
)]
pub(super) async fn save_selection(
    UState(state): UState,
    Json(payload): Json<SavedSelectionPayload>,
) -> Result<Json<SavedSelectionBrief>, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    payload
        .selection
        .validate()
        .map_err(ApiError::bad_request)?;
    if payload.name.trim().is_empty() {
        return Err(ApiError::bad_request("Give the selection a name."));
    }

    let caps_json = serde_json::to_string(&payload.caps).ok();
    let id = store::save_selection(
        &mut conn,
        payload.name.trim(),
        payload.line_id.as_deref(),
        &payload.selection,
        caps_json.as_deref(),
        payload.skip_if_generated,
    )
    .map_err(|_| ApiError::internal())?;

    Ok(Json(SavedSelectionBrief {
        id,
        name: payload.name.trim().to_string(),
        line_id: payload.line_id,
        selection: payload.selection,
        caps: payload.caps,
        skip_if_generated: payload.skip_if_generated,
        created_at: None,
    }))
}

#[utoipa::path(
    get,
    path = "/api/comfyui/selections",
    tag = "comfyui",
    summary = "Saved selections",
    responses((status = 200, body = Vec<SavedSelectionBrief>))
)]
pub(super) async fn list_selections(
    UState(state): UState,
) -> Result<Json<Vec<SavedSelectionBrief>>, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    let rows = store::list_selections(&mut conn).map_err(|_| ApiError::internal())?;
    Ok(Json(
        rows.into_iter()
            .map(|r| SavedSelectionBrief {
                id: r.id,
                name: r.name,
                line_id: r.line_id,
                selection: r.selection,
                caps: r
                    .caps_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default(),
                skip_if_generated: r.skip_if_generated,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/comfyui/selections/{id}",
    tag = "comfyui",
    summary = "Forget a saved selection",
    responses(
        (status = 200, description = "Deleted"),
        (status = 404, description = "No such selection"),
    )
)]
pub(super) async fn delete_selection(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    match store::delete_selection(&mut conn, &id).map_err(|_| ApiError::internal())? {
        0 => Err(StatusCode::NOT_FOUND.into()),
        _ => Ok(StatusCode::OK),
    }
}

// ===== Shared =============================================================

/// The line's name and its stages' workflow names, or a 404.
fn line_of(conn: &mut SqliteConnection, line_id: &str) -> Result<(String, Vec<String>), ApiError> {
    use crate::schema::production_lines;
    let name: Option<String> = production_lines::table
        .filter(production_lines::id.eq(line_id))
        .select(production_lines::name)
        .first(conn)
        .optional()
        .map_err(|_| ApiError::internal())?;
    let name = name.ok_or(ApiError::from(StatusCode::NOT_FOUND))?;

    let stages = crate::comfyui::runs::stages_of_line(conn, line_id)
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .map(|s| s.workflow_name)
        .collect();
    Ok((name, stages))
}

/// How many shots the selection names, and how many of those already have
/// output from this line.
///
/// Two queries rather than one, because the sheet shows both numbers and a
/// single `COUNT` cannot give them. They are taken back to back, so a very busy
/// import could in principle make `skipped` exceed `matched` between them —
/// which is why the estimate clamps rather than trusting the subtraction.
fn counts_for(conn: &mut SqliteConnection, payload: &BatchPayload) -> Result<(i64, i64), ApiError> {
    use crate::comfyui::batch::selection::{count, Narrowing};
    use crate::schema::line_stages;

    let matched = count(conn, &payload.selection, &Narrowing::default()).map_err(|e| {
        tracing::error!("Could not count a batch selection: {}", e);
        ApiError::internal()
    })?;

    if !payload.skip_if_generated {
        return Ok((matched, 0));
    }

    let final_workflow: Option<String> = line_stages::table
        .filter(line_stages::line_id.eq(&payload.line_id))
        .order(line_stages::stage_idx.desc())
        .select(line_stages::workflow_id)
        .first(conn)
        .optional()
        .map_err(|_| ApiError::internal())?;

    let remaining = count(
        conn,
        &payload.selection,
        &Narrowing {
            skip_line_id: Some(&payload.line_id),
            skip_workflow_id: final_workflow.as_deref(),
            ..Default::default()
        },
    )
    .map_err(|_| ApiError::internal())?;

    Ok((matched, (matched - remaining).max(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::shots::ShotsQuery;
    use diesel::connection::SimpleConnection;

    const IMAGE_GRAPH: &str = r#"{
        "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
        "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out", "images": ["4", 0]}}
    }"#;

    fn library() -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json) \
             VALUES ('wf-1', 'Restore', '{}');
             INSERT INTO production_lines (id, name) VALUES ('line-1', 'Restore & upscale');
             INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, keep_output) \
             VALUES ('st-1', 'line-1', 0, 'wf-1', 1);
             INSERT INTO people (id, name) VALUES ('p-gran', 'Grandma');",
            IMAGE_GRAPH.replace('\'', "''")
        ))
        .unwrap();
        // Eight shots at five-year intervals from 1950; the first five are
        // Grandma's, and three of those are before 1990.
        for i in 0..8 {
            conn.batch_execute(&format!(
                "INSERT INTO shots (id, timestamp, primary_person_id) \
                 VALUES ('s-{i}', '{}-01-01 00:00:00', {});
                 INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, synthetic) \
                 VALUES ('f-{i}', 's-{i}', 'p{i}.jpg', 'h{i}', 'image/jpeg', 1, 0);
                 UPDATE shots SET main_file_id = 'f-{i}' WHERE id = 's-{i}';",
                1950 + i * 5,
                if i < 5 { "'p-gran'" } else { "NULL" },
                i = i
            ))
            .unwrap();
        }
        (dir, conn)
    }

    fn payload(selection: Selection, skip: bool) -> BatchPayload {
        BatchPayload {
            line_id: "line-1".to_string(),
            selection,
            stage_values: Default::default(),
            skip_if_generated: skip,
            caps: CapsPayload::default(),
            label: None,
        }
    }

    fn whole_library() -> Selection {
        Selection::Query {
            query: ShotsQuery::default(),
        }
    }

    #[test]
    fn the_sheets_two_counts_come_from_the_same_query_narrowed_twice() {
        let (_dir, mut conn) = library();
        // Two of the eight already have output from this line: one through a
        // completed run, one through a file the workflow made.
        conn.batch_execute(
            "INSERT INTO runs (id, line_id, shot_id, label, status, stage_count) \
             VALUES ('r1','line-1','s-0','x','completed',1);
             INSERT INTO files (id, shot_id, path, hash, source_workflow_id, synthetic) \
             VALUES ('g1','s-1','g1.png','hg1','wf-1',1);",
        )
        .unwrap();

        let (matched, skipped) = counts_for(&mut conn, &payload(whole_library(), true)).unwrap();
        assert_eq!(matched, 8);
        assert_eq!(skipped, 2);

        // With redo, nothing is skipped and the second query is not even asked.
        let (matched, skipped) = counts_for(&mut conn, &payload(whole_library(), false)).unwrap();
        assert_eq!(matched, 8);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn a_query_narrows_the_sheet_the_way_the_gallery_narrows() {
        // The point of reusing `ShotsQuery`: what the person saw in the gallery
        // is what the batch runs.
        let (_dir, mut conn) = library();
        let (all, _) = counts_for(&mut conn, &payload(whole_library(), false)).unwrap();
        assert_eq!(all, 8);

        // Grandma has five of the eight.
        let grandma = |to: Option<&str>| Selection::Query {
            query: ShotsQuery {
                person_id: Some("p-gran".into()),
                to: to.map(str::to_string),
                ..Default::default()
            },
        };
        let (matched, _) = counts_for(&mut conn, &payload(grandma(None), false)).unwrap();
        assert_eq!(matched, 5);

        // And two of those are before 1960 — a bare year, which is the bound a
        // person types and the one SQLite's column affinity used to swallow.
        let (matched, _) = counts_for(&mut conn, &payload(grandma(Some("1960")), false)).unwrap();
        assert_eq!(matched, 2);
    }

    #[test]
    fn caps_round_trip_through_the_payload_and_drop_nonsense() {
        let sent = CapsPayload {
            daily_task_cap: Some(400),
            window_start_minute: Some(0),
            window_end_minute: Some(420),
            disk_floor_bytes: Some(50_000_000_000),
            max_outstanding_holds: Some(200),
        };
        let caps = sent.to_caps();
        assert_eq!(caps.window, Some((0, 420)));
        assert_eq!(caps.daily_task_cap, Some(400));
        let back = CapsPayload::from_caps(&caps);
        assert_eq!(back.window_start_minute, Some(0));
        assert_eq!(back.max_outstanding_holds, Some(200));

        // A cap of zero is somebody clearing the field, not a batch that may
        // never run — it reads as "no cap".
        let cleared = CapsPayload {
            daily_task_cap: Some(0),
            max_outstanding_holds: Some(0),
            disk_floor_bytes: Some(0),
            ..Default::default()
        }
        .to_caps();
        assert_eq!(cleared.daily_task_cap, None);
        assert_eq!(cleared.max_outstanding_holds, None);
        assert_eq!(cleared.disk_floor_bytes, None);

        // Half a window is no window: a start with no end paces nothing.
        let half = CapsPayload {
            window_start_minute: Some(60),
            ..Default::default()
        }
        .to_caps();
        assert_eq!(half.window, None);
    }

    #[test]
    fn every_pause_reason_has_a_sentence_and_nothing_else_does() {
        for reason in ["window", "daily_cap", "disk_floor", "holds"] {
            assert!(
                note_for(reason).is_some(),
                "{} has no sentence for the board",
                reason
            );
        }
        assert_eq!(note_for("something_else"), None);
    }

    #[test]
    fn a_missing_line_is_a_404_before_anything_is_counted() {
        let (_dir, mut conn) = library();
        let err = line_of(&mut conn, "no-such-line").unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);

        let (name, stages) = line_of(&mut conn, "line-1").unwrap();
        assert_eq!(name, "Restore & upscale");
        assert_eq!(stages, vec!["Restore".to_string()]);
    }
}
