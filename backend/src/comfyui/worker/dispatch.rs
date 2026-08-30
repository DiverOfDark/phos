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

use super::status::handle_failure;
use crate::comfyui::client::ComfyUiClient;
use crate::comfyui::loaders::{
    bind_targets, role_directives, takes_video, LoaderKind, SourceBinding, SourceRole,
};
use crate::comfyui::params::ParameterMap;
use crate::comfyui::policy::FailureSite;
use crate::comfyui::source::{read_source, resolve_source_file, SourceMode};
use crate::comfyui::timestamp::format_ts;
use crate::comfyui::workflow::{output_prefix_for_task, prepare_workflow};
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
    /// This run's typed parameters as stored JSON. Already resolved when the
    /// row was written, so dispatch decides nothing about them.
    parameters: String,
    source_file_id: Option<String>,
    source_mode: Option<String>,
    retry_count: i32,
}

type PendingRow = (
    String,
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

    let tasks = match pending_tasks(conn, &now) {
        Ok(tasks) => tasks,
        Err(e) => {
            error!("Failed to query pending tasks: {}", e);
            return;
        }
    };

    for task in tasks {
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

/// The next few tasks waiting to go out, oldest first.
///
/// Every nullable column a run needs is read through `COALESCE`, so a row
/// written before the column existed answers with the empty map rather than
/// with a `NULL` the caller has to think about.
fn pending_tasks(
    conn: &mut SqliteConnection,
    now: &str,
) -> Result<Vec<PendingTask>, diesel::result::Error> {
    let rows: Vec<PendingRow> = enhancement_tasks::table
        .inner_join(
            comfyui_workflows::table.on(comfyui_workflows::id.eq(enhancement_tasks::workflow_id)),
        )
        .filter(
            enhancement_tasks::status.eq("pending").and(
                // A transient failure is re-queued with a backoff; do not pick it
                // up before that time.
                enhancement_tasks::next_attempt_at
                    .is_null()
                    .or(enhancement_tasks::next_attempt_at.le(now)),
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
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "COALESCE(enhancement_tasks.parameters, '{}')",
            ),
            enhancement_tasks::source_file_id,
            enhancement_tasks::source_mode,
            diesel::dsl::sql::<diesel::sql_types::Integer>(
                "COALESCE(enhancement_tasks.retry_count, 0)",
            ),
        ))
        .load::<PendingRow>(conn)?;

    Ok(rows
        .into_iter()
        .map(|row| PendingTask {
            id: row.0,
            shot_id: row.1,
            workflow_json: row.2,
            text_overrides: row.3,
            parameters: row.4,
            source_file_id: row.5,
            source_mode: row.6,
            retry_count: row.7,
        })
        .collect())
}

/// Take one task from `pending` to `queued`, or say where it fell over.
fn dispatch_one(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &PendingTask,
    now: &str,
) -> Result<(), StepFailure> {
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
        .set((
            enhancement_tasks::status.eq("uploading"),
            enhancement_tasks::started_at.eq(now),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
        ))
        .execute(conn);

    // 1. Parse the workflow first: what the graph can load decides what the
    // source step should hand it.
    let workflow: Value = serde_json::from_str(&task.workflow_json)
        .map_err(|e| StepFailure::new(FailureSite::WorkflowJson, "Invalid workflow JSON", e))?;
    let text_overrides: std::collections::HashMap<String, String> =
        serde_json::from_str(&task.text_overrides).unwrap_or_default();
    // A parameter map that will not parse is a corrupt row, not a reason to fail
    // the run: the graph's own defaults are still a valid thing to submit.
    let parameters: ParameterMap = serde_json::from_str(&task.parameters).unwrap_or_default();

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
    // later poll can find the files even if history never mentions them.
    let output_prefix = output_prefix_for_task(&task.id);
    let prepared = prepare_workflow(
        &workflow,
        &plan,
        &text_overrides,
        &parameters,
        Some(&output_prefix),
    );

    // 5. Queue prompt
    let prompt_id = client
        .queue_prompt(&prepared)
        .map_err(|e| StepFailure::new(FailureSite::Queue, "Queue failed", e))?;

    // 6. Set queued with comfyui_prompt_id
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
        .set((
            enhancement_tasks::status.eq("queued"),
            enhancement_tasks::comfyui_prompt_id.eq(&prompt_id),
            enhancement_tasks::output_prefix.eq(&output_prefix),
            enhancement_tasks::settle_until.eq(None::<String>),
        ))
        .execute(conn);

    info!(
        "Task {} queued as ComfyUI prompt {} (output prefix {})",
        task.id, prompt_id, output_prefix
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::connection::SimpleConnection;
    use serde_json::json;

    /// The graph every task in these tests runs.
    const GRAPH: &str = r#"{
        "3": {"class_type": "KSampler",
              "inputs": {"seed": 1, "steps": 20, "cfg": 8.0, "model": ["4", 0]}},
        "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a photograph"}},
        "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}}
    }"#;

    /// A migrated library with one workflow and the given task rows.
    fn library(rows: &str) -> (tempfile::TempDir, diesel::SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json) VALUES ('wf-1', 'Portrait', '{}');
             {}",
            GRAPH.replace('\'', "''"),
            rows
        ))
        .unwrap();
        (dir, conn)
    }

    /// The graph the dispatcher would submit for one queued task — the same
    /// three lines `dispatch_one` runs, minus the parts that need a server.
    fn submitted(task: &PendingTask) -> Value {
        let workflow: Value = serde_json::from_str(&task.workflow_json).unwrap();
        let text_overrides: std::collections::HashMap<String, String> =
            serde_json::from_str(&task.text_overrides).unwrap_or_default();
        let parameters: ParameterMap = serde_json::from_str(&task.parameters).unwrap_or_default();
        let (target_role, role_overrides) = role_directives(&text_overrides);
        let binding = SourceBinding {
            uploaded_filename: "phos_upload.png",
            kind: LoaderKind::Image,
            role: target_role.unwrap_or(SourceRole::Start),
            role_overrides: &role_overrides,
        };
        let plan = bind_targets(&workflow, &binding);
        prepare_workflow(
            &workflow,
            &plan,
            &text_overrides,
            &parameters,
            Some(&output_prefix_for_task(&task.id)),
        )
    }

    #[test]
    fn a_tasks_parameters_survive_the_database_and_reach_the_graph() {
        // The whole point of the column: what the console sent is what the
        // server runs, off a row nobody kept in memory.
        let (_dir, mut conn) = library(
            r#"INSERT INTO enhancement_tasks (id, shot_id, workflow_id, status, text_overrides, parameters)
               VALUES ('task-1', 'shot-1', 'wf-1', 'pending',
                       '{"6.text":"a lighthouse at dusk"}',
                       '{"3.seed":4242,"3.steps":28,"3.cfg":6.5}');"#,
        );

        let tasks = pending_tasks(&mut conn, "2026-08-30 12:00:00").unwrap();
        assert_eq!(tasks.len(), 1);
        let prepared = submitted(&tasks[0]);
        assert_eq!(prepared["3"]["inputs"]["seed"], json!(4242));
        assert_eq!(prepared["3"]["inputs"]["steps"], json!(28));
        assert_eq!(prepared["3"]["inputs"]["cfg"], json!(6.5));
        // The other channel still works, and so do the binding and the pin.
        assert_eq!(
            prepared["6"]["inputs"]["text"],
            json!("a lighthouse at dusk")
        );
        assert_eq!(prepared["4"]["inputs"]["image"], json!("phos_upload.png"));
    }

    #[test]
    fn a_task_queued_before_the_column_existed_runs_exactly_as_it_did() {
        // Every row in an upgraded library: `parameters` is NULL.
        let (_dir, mut conn) = library(
            r#"INSERT INTO enhancement_tasks (id, shot_id, workflow_id, status, text_overrides)
               VALUES ('task-old', 'shot-1', 'wf-1', 'pending', '{"6.text":"unchanged"}');"#,
        );

        let tasks = pending_tasks(&mut conn, "2026-08-30 12:00:00").unwrap();
        assert_eq!(tasks[0].parameters, "{}", "NULL must read as no parameters");
        let prepared = submitted(&tasks[0]);
        assert_eq!(prepared["3"]["inputs"]["seed"], json!(1));
        assert_eq!(prepared["3"]["inputs"]["steps"], json!(20));
        assert_eq!(prepared["6"]["inputs"]["text"], json!("unchanged"));
    }

    #[test]
    fn a_fanned_out_sweep_is_four_rows_that_each_know_their_own_seed() {
        // What the enhance endpoint writes for `{"3.seed": {"count": 4}}`: four
        // independent rows, each carrying its whole resolved map. Nothing is
        // left for dispatch to decide, so the four runs differ by seed alone.
        let base: ParameterMap = [("3.seed".to_string(), json!(1000))].into_iter().collect();
        let vary: crate::comfyui::VaryMap =
            serde_json::from_value(json!({ "3.seed": { "count": 4, "mode": "increment" } }))
                .unwrap();
        let runs = crate::comfyui::expand(&base, &vary).unwrap();

        let inserts: String = runs
            .iter()
            .enumerate()
            .map(|(i, run)| {
                format!(
                    "INSERT INTO enhancement_tasks \
                     (id, shot_id, workflow_id, status, created_at, parameters) \
                     VALUES ('task-{}', 'shot-1', 'wf-1', 'pending', \
                     '2026-08-30 12:00:0{}', '{}');",
                    i,
                    i,
                    serde_json::to_string(run).unwrap()
                )
            })
            .collect();
        let (_dir, mut conn) = library(&inserts);

        let seeds: Vec<i64> = pending_tasks(&mut conn, "2026-08-30 13:00:00")
            .unwrap()
            .iter()
            .map(|task| submitted(task)["3"]["inputs"]["seed"].as_i64().unwrap())
            .collect();
        assert_eq!(seeds, [1000, 1001, 1002, 1003]);
    }
}
