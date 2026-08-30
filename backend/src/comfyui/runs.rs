//! Reading lines out of the library and writing runs into it.
//!
//! [`super::line`] decides; this puts the decisions on disk. It sits between
//! the API and the worker because both need the same two verbs and neither can
//! reach the other's private modules: the API starts a run at stage 0, the
//! worker continues one at stage k+1, and both do it by calling
//! [`queue_stage`].
//!
//! # One shape for a stage, wherever it came from
//!
//! A stage of a line and a single-workflow enhance are the same thing being
//! queued — same prompts, same typed parameters, same fan-out spec, same source
//! mode — so they go through one function. That is why `runs.line_id` is
//! nullable: an ad-hoc enhance is a one-stage run, the board has one kind of
//! row, and FR7's batch will extend one endpoint rather than two.
//!
//! # Fan-out is expanded here, once per continuation
//!
//! A stage's `vary` is handed to [`super::params::expand`] — the same function
//! the enhance endpoint uses — at the moment the stage is queued. Queue stage 2
//! once from a stage-1 output and you get four rows; each of those, when it
//! completes, queues stage 3 for itself. Nothing keeps a list of branches,
//! because a branch is just a task with a parent.

use super::contract::{MediaType, StageContract};
use super::line::{self, LineError, StageTyping};
use super::params::{ParameterMap, VaryMap};
use crate::models::{NewEnhancementTask, NewRun};
use crate::schema::{comfyui_workflows, enhancement_tasks, files, line_stages, runs, shots};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::collections::HashMap;

/// A `line_stages` row joined to its workflow: position, workflow id and name,
/// the three override maps, source mode, keep flag, stored contract and graph.
type StageTuple = (
    i32,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    bool,
    Option<String>,
    String,
);

/// One step of a line, read out of the database with its workflow's contract.
pub(crate) struct StageRow {
    pub stage_idx: i32,
    pub workflow_id: String,
    pub workflow_name: String,
    pub text_overrides: String,
    pub parameters: String,
    pub vary: String,
    pub source_mode: Option<String>,
    pub keep_output: bool,
    pub contract: StageContract,
}

impl StageRow {
    pub fn typing(&self) -> StageTyping {
        StageTyping {
            stage_idx: self.stage_idx,
            name: self.workflow_name.clone(),
            accepts: self.contract.accepts,
            produces: self.contract.produces,
        }
    }

    /// The stage as something queueable: its stored JSON parsed, and the
    /// loader-role corrections a person made on the workflow's contract folded
    /// into the override map exactly as the enhance endpoint folds them.
    pub fn plan(&self) -> StagePlan<'_> {
        let mut text_overrides: HashMap<String, String> =
            serde_json::from_str(&self.text_overrides).unwrap_or_default();
        self.contract.apply_role_corrections(&mut text_overrides);
        StagePlan {
            stage_idx: self.stage_idx,
            workflow_id: &self.workflow_id,
            text_overrides,
            parameters: serde_json::from_str(&self.parameters).unwrap_or_default(),
            vary: serde_json::from_str(&self.vary).unwrap_or_default(),
            source_mode: self.source_mode.as_deref(),
        }
    }
}

/// Everything queueing one stage needs, whether it came from a line or from a
/// single enhance request.
pub(crate) struct StagePlan<'a> {
    pub stage_idx: i32,
    pub workflow_id: &'a str,
    pub text_overrides: HashMap<String, String>,
    pub parameters: ParameterMap,
    pub vary: VaryMap,
    pub source_mode: Option<&'a str>,
}

/// Every stage of a line, in order, with the contract each workflow carries.
///
/// A workflow imported before contracts existed has `contract_json` NULL; one
/// is derived on the spot without asking ComfyUI, the same fallback the
/// workflow list uses. It comes back under-typed rather than missing, and the
/// worker's backfill replaces it within a few minutes.
pub(crate) fn stages_of_line(
    conn: &mut SqliteConnection,
    line_id: &str,
) -> QueryResult<Vec<StageRow>> {
    let rows: Vec<StageTuple> = line_stages::table
        .inner_join(comfyui_workflows::table)
        .filter(line_stages::line_id.eq(line_id))
        .order(line_stages::stage_idx.asc())
        .select((
            line_stages::stage_idx,
            line_stages::workflow_id,
            comfyui_workflows::name,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "COALESCE(line_stages.text_overrides, '{}')",
            ),
            diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(line_stages.parameters, '{}')"),
            diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(line_stages.vary, '{}')"),
            line_stages::source_mode,
            line_stages::keep_output,
            comfyui_workflows::contract_json,
            comfyui_workflows::workflow_json,
        ))
        .load(conn)?;

    Ok(rows
        .into_iter()
        .map(|r| StageRow {
            stage_idx: r.0,
            workflow_id: r.1,
            workflow_name: r.2,
            text_overrides: r.3,
            parameters: r.4,
            vary: r.5,
            source_mode: r.6,
            keep_output: r.7,
            contract: contract_of(r.8.as_deref(), &r.9),
        })
        .collect())
}

/// The stored contract, or one derived on the spot for a workflow that has
/// none. Deliberately does not fetch `/object_info`: neither drawing a line nor
/// advancing a run should stall for however long a dead ComfyUI takes to refuse
/// a connection.
pub(crate) fn contract_of(stored: Option<&str>, workflow_json: &str) -> StageContract {
    stored
        .and_then(|s| serde_json::from_str::<StageContract>(s).ok())
        .unwrap_or_else(|| {
            let graph: serde_json::Value =
                serde_json::from_str(workflow_json).unwrap_or(serde_json::Value::Null);
            StageContract::derive(&graph, None)
        })
}

/// What the shot a run is against actually is.
///
/// The same file [`super::source::resolve_source_file`] would read: the shot's
/// original. `None` when the shot has no original, or its MIME type is
/// something a line cannot carry.
pub(crate) fn shot_media_type(conn: &mut SqliteConnection, shot_id: &str) -> Option<MediaType> {
    let mime: String = files::table
        .filter(files::shot_id.eq(shot_id).and(files::is_original.eq(true)))
        .order(files::created_at.asc())
        .select(diesel::dsl::sql::<diesel::sql_types::Text>(
            "COALESCE(mime_type, '')",
        ))
        .first(conn)
        .ok()?;
    line::media_type_of_mime(&mime)
}

/// Queue one stage of a run: one task per combination its `vary` asks for.
///
/// `source_file_id` is what this stage reads — the upstream task's output at
/// stage k+1, and whatever the caller named (usually nothing, meaning the
/// shot's original) at stage 0. `parent_task_id` is the task that made it, and
/// is what tells the advance pass this continuation already exists.
///
/// Returns the ids in the order [`super::params::expand`] produced them, which
/// is the order a sweep's takes are numbered in.
pub(crate) fn queue_stage(
    conn: &mut SqliteConnection,
    run_id: &str,
    shot_id: &str,
    plan: &StagePlan<'_>,
    source_file_id: Option<&str>,
    parent_task_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let expanded = super::params::expand(&plan.parameters, &plan.vary)?;
    let text_overrides_json =
        serde_json::to_string(&plan.text_overrides).unwrap_or_else(|_| "{}".to_string());

    let mut ids = Vec::with_capacity(expanded.len());
    for run in &expanded {
        let task_id = uuid::Uuid::new_v4().to_string();
        let parameters_json = serde_json::to_string(run).unwrap_or_else(|_| "{}".to_string());
        diesel::insert_into(enhancement_tasks::table)
            .values(NewEnhancementTask {
                id: &task_id,
                shot_id,
                workflow_id: plan.workflow_id,
                text_overrides: Some(&text_overrides_json),
                source_file_id,
                source_mode: plan.source_mode,
                parameters: Some(&parameters_json),
                run_id: Some(run_id),
                stage_idx: Some(plan.stage_idx),
                parent_task_id,
            })
            .execute(conn)
            .map_err(|e| format!("Failed to queue stage {}: {}", plan.stage_idx, e))?;
        ids.push(task_id);
    }
    Ok(ids)
}

/// Open a run row. Callers queue its first stage in the same transaction.
pub(crate) fn open_run(
    conn: &mut SqliteConnection,
    line_id: Option<&str>,
    shot_id: &str,
    label: &str,
    stage_count: i32,
) -> QueryResult<String> {
    let run_id = uuid::Uuid::new_v4().to_string();
    diesel::insert_into(runs::table)
        .values(NewRun {
            id: &run_id,
            line_id,
            shot_id,
            label,
            stage_count,
        })
        .execute(conn)?;
    Ok(run_id)
}

/// Why a line could not be started.
#[derive(Debug)]
pub(crate) enum StartError {
    /// No such line, or no such shot.
    NotFound(&'static str),
    /// The line, or this shot's fit to it, is the problem. Reported to the
    /// caller as-is: it names the stage and says what does not fit.
    Rejected(LineError),
    Db(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NotFound(what) => write!(f, "{} not found", what),
            StartError::Rejected(e) => f.write_str(&e.message),
            StartError::Db(e) => f.write_str(e),
        }
    }
}

/// What starting a run produced.
#[derive(Debug)]
pub(crate) struct RunStart {
    pub run_id: String,
    pub task_ids: Vec<String>,
    pub stage_count: i32,
}

/// Start a line against a shot: check it fits, open the run, queue stage 0.
///
/// The whole thing is one transaction. A run that exists with no tasks would
/// sit on the board as "running" forever, and a fan-out queued halfway is
/// nobody's idea of a fan-out.
pub(crate) fn start_line_run(
    conn: &mut SqliteConnection,
    line_id: &str,
    shot_id: &str,
) -> Result<RunStart, StartError> {
    let label: String = crate::schema::production_lines::table
        .filter(crate::schema::production_lines::id.eq(line_id))
        .select(crate::schema::production_lines::name)
        .first(conn)
        .optional()
        .map_err(|e| StartError::Db(e.to_string()))?
        .ok_or(StartError::NotFound("Line"))?;

    let shot_exists: i64 = shots::table
        .filter(shots::id.eq(shot_id))
        .count()
        .get_result(conn)
        .map_err(|e| StartError::Db(e.to_string()))?;
    if shot_exists == 0 {
        return Err(StartError::NotFound("Shot"));
    }

    let stages = stages_of_line(conn, line_id).map_err(|e| StartError::Db(e.to_string()))?;
    let typings: Vec<StageTyping> = stages.iter().map(StageRow::typing).collect();

    // Checked when the line was drawn, and again now: a workflow can be
    // re-imported, or its contract corrected, between the two.
    line::validate_chain(&typings).map_err(StartError::Rejected)?;

    // And the one check the design-time pass cannot make, because a line is
    // drawn once and run against many shots.
    if let Some(source) = shot_media_type(conn, shot_id) {
        line::admits_source(&typings[0], source).map_err(StartError::Rejected)?;
    } else if line::reads_source(typings[0].accepts) {
        return Err(StartError::Rejected(LineError {
            stage_idx: 0,
            message: "This shot has no original file for the line to read.".to_string(),
        }));
    }

    let stage_count = stages.len() as i32;
    conn.transaction::<_, diesel::result::Error, _>(|conn| {
        let run_id = open_run(conn, Some(line_id), shot_id, &label, stage_count)?;
        let plan = stages[0].plan();
        let task_ids = queue_stage(conn, &run_id, shot_id, &plan, None, None)
            .map_err(|e| diesel::result::Error::QueryBuilderError(e.into()))?;
        Ok(RunStart {
            run_id,
            task_ids,
            stage_count,
        })
    })
    .map_err(|e| StartError::Db(e.to_string()))
}

/// A stage's declared typing without loading the whole line — what the advance
/// pass needs to know before queueing stage k+1.
pub(crate) fn stage_at(
    conn: &mut SqliteConnection,
    line_id: &str,
    stage_idx: i32,
) -> QueryResult<Option<StageRow>> {
    Ok(stages_of_line(conn, line_id)?
        .into_iter()
        .find(|s| s.stage_idx == stage_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::line::RunState;
    use crate::comfyui::Accepts;
    use diesel::connection::SimpleConnection;

    /// A graph that takes a still and saves a still.
    pub(super) const IMAGE_GRAPH: &str = r#"{
        "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20, "cfg": 8.0}},
        "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
        "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out", "images": ["3", 0]}}
    }"#;

    fn library() -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let conn = crate::db::open_diesel_connection(&db_path).unwrap();
        (dir, conn)
    }

    #[test]
    fn a_stage_reads_back_with_its_workflows_contract() {
        let (_dir, mut conn) = library();
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json) \
             VALUES ('wf-1', 'Portrait', '{}');
             INSERT INTO production_lines (id, name) VALUES ('line-1', '4K Restore');
             INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, keep_output) \
             VALUES ('st-1', 'line-1', 0, 'wf-1', 1);",
            IMAGE_GRAPH.replace('\'', "''")
        ))
        .unwrap();

        let stages = stages_of_line(&mut conn, "line-1").unwrap();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].workflow_name, "Portrait");
        assert!(stages[0].keep_output);
        // No `contract_json` on the row, so it was derived on the spot.
        assert_eq!(stages[0].contract.accepts, Accepts::Image);
        assert_eq!(stages[0].contract.produces, MediaType::Image);
        // And the nullable override columns read as empty maps, not NULL.
        assert_eq!(stages[0].text_overrides, "{}");
        assert_eq!(stages[0].vary, "{}");
    }

    #[test]
    fn a_run_is_refused_when_the_shot_is_not_what_stage_one_eats() {
        let (_dir, mut conn) = library();
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json) \
             VALUES ('wf-1', 'Portrait', '{}');
             INSERT INTO production_lines (id, name) VALUES ('line-1', 'Restore');
             INSERT INTO line_stages (id, line_id, stage_idx, workflow_id) \
             VALUES ('st-1', 'line-1', 0, 'wf-1');
             INSERT INTO shots (id) VALUES ('shot-1');
             INSERT INTO files (id, shot_id, path, hash, mime_type, is_original) \
             VALUES ('file-1', 'shot-1', 'clip.mp4', 'h1', 'video/mp4', 1);",
            IMAGE_GRAPH.replace('\'', "''")
        ))
        .unwrap();

        let err = start_line_run(&mut conn, "line-1", "shot-1").unwrap_err();
        assert!(
            matches!(&err, StartError::Rejected(e) if e.message.contains("this shot is video")),
            "{}",
            err
        );
        // And nothing was written: no half-open run on the board.
        let runs_count: i64 = runs::table.count().get_result(&mut conn).unwrap();
        assert_eq!(runs_count, 0);
    }

    #[test]
    fn starting_a_line_opens_a_run_and_queues_only_its_first_stage() {
        let (_dir, mut conn) = library();
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json) \
             VALUES ('wf-1', 'Portrait', '{g}'), ('wf-2', 'Upscale', '{g}');
             INSERT INTO production_lines (id, name) VALUES ('line-1', '4K Restore');
             INSERT INTO line_stages (id, line_id, stage_idx, workflow_id) \
             VALUES ('st-1', 'line-1', 0, 'wf-1'), ('st-2', 'line-1', 1, 'wf-2');
             INSERT INTO shots (id) VALUES ('shot-1');
             INSERT INTO files (id, shot_id, path, hash, mime_type, is_original) \
             VALUES ('file-1', 'shot-1', 'a.jpg', 'h1', 'image/jpeg', 1);",
            g = IMAGE_GRAPH.replace('\'', "''")
        ))
        .unwrap();

        let started = start_line_run(&mut conn, "line-1", "shot-1").unwrap();
        assert_eq!(started.stage_count, 2);
        assert_eq!(started.task_ids.len(), 1, "stage 2 waits for stage 1");

        let (label, status, count): (String, String, i32) = runs::table
            .filter(runs::id.eq(&started.run_id))
            .select((runs::label, runs::status, runs::stage_count))
            .first(&mut conn)
            .unwrap();
        assert_eq!(label, "4K Restore", "the line's name, snapshotted");
        assert_eq!(status, RunState::Running.as_str());
        assert_eq!(count, 2);

        let (stage_idx, parent, wf): (Option<i32>, Option<String>, String) =
            enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(&started.task_ids[0]))
                .select((
                    enhancement_tasks::stage_idx,
                    enhancement_tasks::parent_task_id,
                    enhancement_tasks::workflow_id,
                ))
                .first(&mut conn)
                .unwrap();
        assert_eq!(stage_idx, Some(0));
        assert_eq!(parent, None, "stage 1 eats the shot, not another task");
        assert_eq!(wf, "wf-1");
    }

    #[test]
    fn a_stages_sweep_becomes_that_many_independent_tasks() {
        let (_dir, mut conn) = library();
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json) \
             VALUES ('wf-1', 'Portrait', '{}');
             INSERT INTO production_lines (id, name) VALUES ('line-1', 'Takes');
             INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, parameters, vary) \
             VALUES ('st-1', 'line-1', 0, 'wf-1', '{{\"3.seed\":1000}}', \
                     '{{\"3.seed\":{{\"count\":4,\"mode\":\"increment\"}}}}');
             INSERT INTO shots (id) VALUES ('shot-1');
             INSERT INTO files (id, shot_id, path, hash, mime_type, is_original) \
             VALUES ('file-1', 'shot-1', 'a.jpg', 'h1', 'image/jpeg', 1);",
            IMAGE_GRAPH.replace('\'', "''")
        ))
        .unwrap();

        let started = start_line_run(&mut conn, "line-1", "shot-1").unwrap();
        assert_eq!(started.task_ids.len(), 4);
        let mut seeds: Vec<i64> = enhancement_tasks::table
            .filter(enhancement_tasks::run_id.eq(&started.run_id))
            .select(enhancement_tasks::parameters)
            .load::<Option<String>>(&mut conn)
            .unwrap()
            .into_iter()
            .map(|p| {
                serde_json::from_str::<serde_json::Value>(&p.unwrap()).unwrap()["3.seed"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        seeds.sort();
        assert_eq!(seeds, [1000, 1001, 1002, 1003]);
    }
}
