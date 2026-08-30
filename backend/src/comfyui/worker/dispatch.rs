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
//!
//! One task never leaves this module at all: a **describe** stage whose shot has
//! already been described. The description is on `shots.analysis_json`, the task
//! completes from it, and no photograph is uploaded and no GPU is asked for
//! anything — see [`serve_describe_from_cache`].

use super::status::{handle_failure, live_task, mark_completed};
use crate::comfyui::client::ComfyUiClient;
use crate::comfyui::contract::MediaType;
use crate::comfyui::line::{admits_upstream_output, media_type_of_mime, StageTyping};
use crate::comfyui::loaders::{
    bind_targets, check_source_kind, role_directives, takes_video, SourceBinding, SourceRole,
};
use crate::comfyui::params::ParameterMap;
use crate::comfyui::policy::FailureSite;
use crate::comfyui::prompt;
use crate::comfyui::queue;
use crate::comfyui::source::{read_source, resolve_source_file, SourceMode};
use crate::comfyui::timestamp::format_ts;
use crate::comfyui::workflow::{fresh_attempt_id, output_prefix_for_task, prepare_workflow};
use crate::schema::{comfyui_workflows, enhancement_tasks};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use serde_json::Value;
use std::path::Path;
use tracing::{error, info, warn};

/// How many tasks one pass takes off the queue.
///
/// Small on purpose, and unchanged by FR8: the pass runs every three seconds
/// and each task here is an upload. What FR8 changed is *which* five.
const DISPATCH_CHUNK: i64 = 5;

/// A task waiting to be sent to ComfyUI.
struct PendingTask {
    id: String,
    shot_id: String,
    workflow_json: String,
    /// The workflow's stored stage contract, read for its role corrections.
    /// They are applied here, at dispatch, rather than written into the task
    /// row: a task records what the caller asked for, and `role:` directives
    /// persisted in `text_overrides` would surface as provenance and prompts.
    contract_json: Option<String>,
    text_overrides: String,
    /// This run's typed parameters as stored JSON. Already resolved when the
    /// row was written, so dispatch decides nothing about them.
    parameters: String,
    source_file_id: Option<String>,
    source_mode: Option<String>,
    retry_count: i32,
    /// Which step of a line this is, 0-based. `None` on a row written before
    /// runs existed; `Some(0)` is a run's first stage, which reads the shot
    /// rather than another stage's output.
    stage_idx: Option<i32>,
    /// The workflow's name, so a stage handed something it cannot read can
    /// say which stage and what it wanted.
    workflow_name: String,
    /// The three columns that, with `id` and `stage_idx`, are this task's place
    /// in the queue. Read back rather than assumed, so the order the database
    /// returned can be checked against the pure one.
    workflow_id: String,
    created_at: String,
    priority: String,
}

impl PendingTask {
    /// Where this task sits in the drain order, as
    /// [`crate::comfyui::queue`] describes it with no database involved.
    fn drain_key(&self) -> queue::DrainKey {
        queue::DrainKey {
            priority: queue::Priority::parse(&self.priority),
            stage_idx: self.stage_idx.unwrap_or(0),
            workflow_id: self.workflow_id.clone(),
            created_at: self.created_at.clone(),
            id: self.id.clone(),
        }
    }
}

type PendingRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    i32,
    Option<i32>,
    String,
    String,
    String,
    String,
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

    let tasks = match pending_tasks(conn, &now, DISPATCH_CHUNK) {
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

/// The next few tasks waiting to go out, in drain order.
///
/// **Not** oldest first, and that is FR8. The order is
/// [`crate::comfyui::queue`]'s and is expressed there twice — once as the SQL
/// fragments this builds its `ORDER BY` from, once as [`queue::DrainKey`]'s
/// [`Ord`] — so what this returns can be checked against a sort that needs no
/// database. Interactive before batch, then lower stage first, then grouped by
/// workflow, then oldest, then by id so it is total.
///
/// Every nullable column a run needs is read through `COALESCE`, so a row
/// written before the column existed answers with the empty map rather than
/// with a `NULL` the caller has to think about.
fn pending_tasks(
    conn: &mut SqliteConnection,
    now: &str,
    limit: i64,
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
        .order((
            diesel::dsl::sql::<diesel::sql_types::Integer>(queue::PRIORITY_RANK_SQL),
            diesel::dsl::sql::<diesel::sql_types::Integer>(queue::STAGE_RANK_SQL),
            enhancement_tasks::workflow_id.asc(),
            enhancement_tasks::created_at.asc(),
            enhancement_tasks::id.asc(),
        ))
        .limit(limit)
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            comfyui_workflows::workflow_json,
            comfyui_workflows::contract_json,
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
            enhancement_tasks::stage_idx,
            comfyui_workflows::name,
            enhancement_tasks::workflow_id,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "COALESCE(enhancement_tasks.created_at, '')",
            ),
            enhancement_tasks::priority,
        ))
        .load::<PendingRow>(conn)?;

    Ok(rows
        .into_iter()
        .map(|row| PendingTask {
            id: row.0,
            shot_id: row.1,
            workflow_json: row.2,
            contract_json: row.3,
            text_overrides: row.4,
            parameters: row.5,
            source_file_id: row.6,
            source_mode: row.7,
            retry_count: row.8,
            stage_idx: row.9,
            workflow_name: row.10,
            workflow_id: row.11,
            created_at: row.12,
            priority: row.13,
        })
        .collect())
}

/// Does this stage's contract still admit what it has actually been handed?
///
/// The line was checked when it was drawn — every join asked
/// [`crate::comfyui::Accepts::admits`] — but a workflow can be re-imported, or
/// its contract corrected, at any point between then and now. This is the cheap
/// second look, taken where the source has just been read and its real type is
/// known rather than declared.
///
/// Only asked of a continuation. Stage 1 reads the shot, and what a person may
/// run against their own photograph is not this function's business: an ad-hoc
/// enhance that ComfyUI will refuse should be refused by ComfyUI, with its
/// message, exactly as it always has been.
fn check_stage_admits(task: &PendingTask, source_mime: &str) -> Result<(), StepFailure> {
    let Some(stage_idx) = task.stage_idx.filter(|idx| *idx > 0) else {
        return Ok(());
    };
    let contract =
        crate::comfyui::runs::contract_of(task.contract_json.as_deref(), &task.workflow_json);
    let graph: serde_json::Value =
        serde_json::from_str(&task.workflow_json).unwrap_or(serde_json::Value::Null);
    let typing = StageTyping {
        stage_idx,
        name: task.workflow_name.clone(),
        accepts: contract.accepts,
        produces: contract.produces,
        // What this task was queued with, and what its graph can load: the two
        // things `reads_as` needs to give the answer the picker offered on and
        // the validator accepted.
        source_mode: task.source_mode.clone(),
        takes_video: takes_video(&graph),
    };
    // Not a second rule: the same function the line editor's validation calls,
    // asked of the file that actually turned up.
    match media_type_of_mime(source_mime) {
        Some(handed) => admits_upstream_output(&typing, handed).map_err(|e| StepFailure {
            site: FailureSite::StageMismatch,
            message: e.message,
        }),
        None => Err(StepFailure {
            site: FailureSite::StageMismatch,
            message: format!(
                "Stage {} ({}) was handed a {}, which a line cannot carry.",
                stage_idx + 1,
                task.workflow_name,
                source_mime
            ),
        }),
    }
}

/// Finish a describe stage from what the library already knows, if it can.
///
/// The cache is per *shot*, not per shot-and-workflow: a second line over the
/// same photograph should not pay the GPU again for what the first one found
/// out, and the description is about the photograph rather than about the run.
/// What differs between runs — the intent, the style, the constraints — is
/// applied when the prompt is *compiled* downstream, not baked in here.
///
/// It is tied to the *file* the stage would read, though: a mid-line describe
/// stage reads the stage before it, and a description of that intermediate
/// must not answer for the photograph — nor a stale one for a file since
/// promoted to original. [`prompt::cached_analysis_for`] makes that check.
///
/// A run that wants a fresh look says so with the `phos:refresh` directive.
///
/// Returns whether the task was finished here.
fn serve_describe_from_cache(
    conn: &mut SqliteConnection,
    task: &PendingTask,
    text_overrides: &std::collections::HashMap<String, String>,
) -> bool {
    let contract =
        crate::comfyui::runs::contract_of(task.contract_json.as_deref(), &task.workflow_json);
    if contract.produces != MediaType::Text || prompt::wants_refresh(text_overrides) {
        return false;
    }
    let Some(cached) =
        prompt::cached_analysis_for(conn, &task.shot_id, task.source_file_id.as_deref())
    else {
        return false;
    };
    if cached.text.trim().is_empty() {
        return false;
    }
    if diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
        .set(enhancement_tasks::text_output.eq(&cached.text))
        .execute(conn)
        .is_err()
    {
        // Fall through and describe it properly rather than completing a task
        // that carries nothing.
        return false;
    }
    info!(
        "Task {} answered from the description already on shot {}",
        task.id, task.shot_id
    );
    mark_completed(conn, &task.id);
    true
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
    let mut text_overrides: std::collections::HashMap<String, String> =
        serde_json::from_str(&task.text_overrides).unwrap_or_default();
    // A loader role a person corrected on the workflow's contract joins the
    // run here, in memory only — anything the task itself says wins, and the
    // row on disk never carries a `role:` directive it did not ask for.
    if let Some(contract) = task
        .contract_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<crate::comfyui::StageContract>(s).ok())
    {
        contract.apply_role_corrections(&mut text_overrides);
    }
    // A parameter map that will not parse is a corrupt row, not a reason to fail
    // the run: the graph's own defaults are still a valid thing to submit.
    let parameters: ParameterMap = serde_json::from_str(&task.parameters).unwrap_or_default();

    // A description this shot already has is a description. Answering from it
    // costs nothing and skips everything below — the upload, the queue, the
    // poll and the GPU.
    if serve_describe_from_cache(conn, task, &text_overrides) {
        return Ok(());
    }

    // 2. Read the source in whatever shape this run asked for: a frame of the
    // video, or the video itself.
    let source = resolve_source_file(
        conn,
        &task.shot_id,
        task.source_file_id.as_deref(),
        library_root,
    )
    .map_err(|e| StepFailure::new(FailureSite::SourceImage, "Source file lookup failed", e))?;

    // A continuation reads what the stage before it made. Check that is still
    // something this stage can read before uploading a gigabyte of it.
    check_stage_admits(task, &source.mime_type)?;

    let mode = SourceMode::resolve(
        task.source_mode.as_deref(),
        takes_video(&workflow),
        source.is_video(),
    );
    // A mode the graph has no loader for fails here, before anything is
    // extracted or uploaded — falling back to a loader of the wrong kind would
    // only move the failure into ComfyUI, with a worse message.
    check_source_kind(&workflow, mode.loader_kind()).map_err(|e| {
        StepFailure::new(
            FailureSite::SourceImage,
            &format!("Source mode {} does not fit this workflow", mode),
            e,
        )
    })?;
    let upload = read_source(conn, &source, mode).map_err(|e| {
        StepFailure::new(
            FailureSite::SourceImage,
            &format!("Source extraction failed ({})", mode),
            e,
        )
    })?;

    // 3. Upload to ComfyUI, streaming — a whole video is not read into memory.
    let (mut body, body_len) = upload
        .open()
        .map_err(|e| StepFailure::new(FailureSite::SourceImage, "Source open failed", e))?;
    let uploaded_name = client
        .upload_file(
            &upload.filename,
            &upload.content_type,
            body.as_mut(),
            body_len,
        )
        .map_err(|e| StepFailure::new(FailureSite::Upload, "Upload failed", e))?;
    drop(body);

    // 4. Bind the upload to the loader nodes it belongs in. Writing it into
    // every loader is what made a start-frame/end-frame workflow impossible.
    let (target_role, role_overrides) = role_directives(&text_overrides);
    let binding = SourceBinding {
        uploaded_filename: &uploaded_name,
        kind: mode.loader_kind(),
        role: target_role.unwrap_or(SourceRole::Start),
        role_overrides: &role_overrides,
    };
    let plan = bind_targets(&workflow, &binding).map_err(|e| {
        StepFailure::new(
            FailureSite::SourceImage,
            "Source does not fit this workflow",
            e,
        )
    })?;
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
        let mut text_overrides: std::collections::HashMap<String, String> =
            serde_json::from_str(&task.text_overrides).unwrap_or_default();
        if let Some(contract) = task
            .contract_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<crate::comfyui::StageContract>(s).ok())
        {
            contract.apply_role_corrections(&mut text_overrides);
        }
        let parameters: ParameterMap = serde_json::from_str(&task.parameters).unwrap_or_default();
        let (target_role, role_overrides) = role_directives(&text_overrides);
        let binding = SourceBinding {
            uploaded_filename: "phos_upload.png",
            kind: crate::comfyui::loaders::LoaderKind::Image,
            role: target_role.unwrap_or(SourceRole::Start),
            role_overrides: &role_overrides,
        };
        let plan = bind_targets(&workflow, &binding).unwrap();
        prepare_workflow(
            &workflow,
            &plan,
            &text_overrides,
            &parameters,
            Some(&output_prefix_for_task(&task.id, &fresh_attempt_id())),
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

        let tasks = pending_tasks(&mut conn, "2026-08-30 12:00:00", DISPATCH_CHUNK).unwrap();
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

        let tasks = pending_tasks(&mut conn, "2026-08-30 12:00:00", DISPATCH_CHUNK).unwrap();
        assert_eq!(tasks[0].parameters, "{}", "NULL must read as no parameters");
        let prepared = submitted(&tasks[0]);
        assert_eq!(prepared["3"]["inputs"]["seed"], json!(1));
        assert_eq!(prepared["3"]["inputs"]["steps"], json!(20));
        assert_eq!(prepared["6"]["inputs"]["text"], json!("unchanged"));
    }

    #[test]
    fn a_contract_role_correction_reaches_the_run_without_touching_the_row() {
        // Two untitled image loaders both default to `start`, so the upload
        // would land in both. The person corrected node 5 to `end` on the
        // workflow's contract — the run must honour that, and the task row must
        // stay exactly what the caller asked for, with no `role:` directive
        // persisted where provenance would pick it up.
        let two_loader_graph = r#"{
            "3": {"class_type": "KSampler", "inputs": {"seed": 1, "model": ["4", 0]}},
            "4": {"class_type": "LoadImage", "inputs": {"image": "first.png"}},
            "5": {"class_type": "LoadImage", "inputs": {"image": "second.png"}}
        }"#;
        let graph: Value = serde_json::from_str(two_loader_graph).unwrap();
        let contract = crate::comfyui::StageContract::derive_with(
            &graph,
            None,
            crate::comfyui::ContractCorrections {
                roles: [("5".to_string(), SourceRole::End)].into_iter().collect(),
                ..Default::default()
            },
        );
        let rows = format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json, contract_json) \
             VALUES ('wf-2', 'Interpolate', '{}', '{}');\n\
             INSERT INTO enhancement_tasks (id, shot_id, workflow_id, status) \
             VALUES ('task-1', 'shot-1', 'wf-2', 'pending');",
            two_loader_graph.replace('\'', "''"),
            serde_json::to_string(&contract)
                .unwrap()
                .replace('\'', "''")
        );
        let (_dir, mut conn) = library(&rows);

        let tasks = pending_tasks(&mut conn, "2026-08-30 12:00:00").unwrap();
        assert_eq!(tasks.len(), 1);
        // The row carries no role directive of its own…
        assert_eq!(tasks[0].text_overrides, "{}");
        // …and the run still binds the upload to the start frame alone.
        let prepared = submitted(&tasks[0]);
        assert_eq!(prepared["4"]["inputs"]["image"], json!("phos_upload.png"));
        assert_eq!(prepared["5"]["inputs"]["image"], json!("second.png"));
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

        let seeds: Vec<i64> = pending_tasks(&mut conn, "2026-08-30 13:00:00", DISPATCH_CHUNK)
            .unwrap()
            .iter()
            .map(|task| submitted(task)["3"]["inputs"]["seed"].as_i64().unwrap())
            .collect();
        assert_eq!(seeds, [1000, 1001, 1002, 1003]);
    }

    // === FR9 — the shot's description is paid for once ======================

    /// A describe task, and a library where the shot may or may not already
    /// have been described.
    fn describe_library(
        analysis: Option<&str>,
        directives: &str,
    ) -> (tempfile::TempDir, diesel::SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json, contract_json) \
               VALUES ('wf-describe', 'Describe', '{{}}', \
                       '{{\"accepts\":\"image\",\"produces\":\"text\"}}');
             INSERT INTO shots (id, analysis_json) VALUES ('shot-1', {analysis});
             INSERT INTO files (id, shot_id, path, hash, mime_type, is_original) \
               VALUES ('file-orig', 'shot-1', 'original.jpg', 'h0', 'image/jpeg', 1);
             INSERT INTO enhancement_tasks \
               (id, shot_id, workflow_id, status, text_overrides) \
               VALUES ('task-1', 'shot-1', 'wf-describe', 'pending', '{directives}');",
            analysis = match analysis {
                Some(a) => format!("'{}'", a.replace('\'', "''")),
                None => "NULL".to_string(),
            },
            directives = directives.replace('\'', "''"),
        ))
        .unwrap();
        (dir, conn)
    }

    /// The one pending task, in the shape `dispatch_one` reads it.
    fn the_task(conn: &mut diesel::SqliteConnection) -> PendingTask {
        pending_tasks(conn, "2099-01-01 00:00:00", DISPATCH_CHUNK)
            .unwrap()
            .pop()
            .expect("a pending task")
    }

    const CACHED: &str = r#"{"version":1,"text":"a woman on a jetty at dusk",
        "workflow_id":"wf-describe","source_file_id":"file-orig",
        "generated_at":"2026-08-30 12:00:00"}"#;

    #[test]
    fn a_second_line_over_the_same_shot_does_not_describe_it_again() {
        let (_dir, mut conn) = describe_library(Some(CACHED), "{}");
        let task = the_task(&mut conn);
        let overrides: std::collections::HashMap<String, String> =
            serde_json::from_str(&task.text_overrides).unwrap();

        // No upload, no queued prompt, no GPU: the answer was already known.
        assert!(serve_describe_from_cache(&mut conn, &task, &overrides));

        let (status, text, prompt_id): (String, Option<String>, Option<String>) =
            enhancement_tasks::table
                .filter(enhancement_tasks::id.eq("task-1"))
                .select((
                    enhancement_tasks::status,
                    enhancement_tasks::text_output,
                    enhancement_tasks::comfyui_prompt_id,
                ))
                .first(&mut conn)
                .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(text.as_deref(), Some("a woman on a jetty at dusk"));
        assert_eq!(prompt_id, None, "nothing was ever queued");
    }

    #[test]
    fn the_whole_dispatch_pass_answers_from_the_cache_with_no_server_at_all() {
        // The strong form of the claim: the same pass that would upload the
        // photograph and queue a prompt is pointed at a port nothing is
        // listening on, and the describe task still completes. Nothing was
        // asked of ComfyUI because nothing needed to be.
        let dir = tempfile::tempdir().unwrap();
        let (_dir2, mut conn) = describe_library(Some(CACHED), "{}");
        let dead = crate::comfyui::ComfyUiClient::new("http://127.0.0.1:1");

        process_pending_tasks(&mut conn, &dead, dir.path());

        let (status, text): (String, Option<String>) = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq("task-1"))
            .select((enhancement_tasks::status, enhancement_tasks::text_output))
            .first(&mut conn)
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(text.as_deref(), Some("a woman on a jetty at dusk"));

        // And with no description on the shot, the same pass needs the server
        // it cannot reach — which is what makes the run above a saving rather
        // than a coincidence.
        let (_dir3, mut fresh) = describe_library(None, "{}");
        process_pending_tasks(&mut fresh, &dead, dir.path());
        let status: String = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq("task-1"))
            .select(enhancement_tasks::status)
            .first(&mut fresh)
            .unwrap();
        assert_ne!(status, "completed");
    }

    #[test]
    fn a_shot_nobody_has_described_yet_goes_to_the_gpu() {
        let (_dir, mut conn) = describe_library(None, "{}");
        let task = the_task(&mut conn);
        assert!(!serve_describe_from_cache(
            &mut conn,
            &task,
            &std::collections::HashMap::new()
        ));
    }

    #[test]
    fn a_run_can_insist_on_a_fresh_look() {
        let (_dir, mut conn) = describe_library(Some(CACHED), r#"{"phos:refresh": "1"}"#);
        let task = the_task(&mut conn);
        let overrides: std::collections::HashMap<String, String> =
            serde_json::from_str(&task.text_overrides).unwrap();
        assert!(!serve_describe_from_cache(&mut conn, &task, &overrides));
    }

    #[test]
    fn a_description_of_a_different_file_does_not_answer_for_this_one() {
        // A mid-line describe stage reads the stage before it, not the shot's
        // photograph, and a description of that intermediate must not be
        // served to it — nor the intermediate's description to a later stage
        // reading the original.
        let (_dir, mut conn) = describe_library(Some(CACHED), "{}");
        conn.batch_execute(
            "INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, synthetic) \
               VALUES ('file-mid', 'shot-1', 'phos/mid.png', 'h1', 'image/png', 0, 1);
             UPDATE enhancement_tasks SET source_file_id = 'file-mid' WHERE id = 'task-1';",
        )
        .unwrap();
        let task = the_task(&mut conn);
        assert!(!serve_describe_from_cache(
            &mut conn,
            &task,
            &std::collections::HashMap::new()
        ));
    }

    #[test]
    fn a_stage_that_makes_a_picture_never_reads_the_description_cache() {
        // The cache is a describe stage's, and only a describe stage's. A
        // generation stage completing without ever running would be a disaster
        // that looked like a speed-up.
        let (_dir, mut conn) = describe_library(Some(CACHED), "{}");
        diesel::update(comfyui_workflows::table)
            .set(comfyui_workflows::contract_json.eq(r#"{"accepts":"image","produces":"image"}"#))
            .execute(&mut conn)
            .unwrap();
        let task = the_task(&mut conn);
        assert!(!serve_describe_from_cache(
            &mut conn,
            &task,
            &std::collections::HashMap::new()
        ));
    }
}
