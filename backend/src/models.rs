use crate::schema::*;
use diesel::prelude::*;

// ── People ──

#[derive(Insertable)]
#[diesel(table_name = people)]
pub struct NewPerson<'a> {
    pub id: &'a str,
    pub name: Option<&'a str>,
    pub thumbnail_face_id: Option<&'a str>,
    pub representative_embedding: Option<&'a [u8]>,
    pub folder_name: Option<&'a str>,
}

#[derive(AsChangeset)]
#[diesel(table_name = people)]
pub struct PersonChangeset<'a> {
    pub name: Option<&'a str>,
    pub thumbnail_face_id: Option<&'a str>,
    pub representative_embedding: Option<&'a [u8]>,
    pub folder_name: Option<&'a str>,
}

// ── Shots ──

#[derive(Insertable)]
#[diesel(table_name = shots)]
pub struct NewShot<'a> {
    pub id: &'a str,
    pub main_file_id: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub latitude: Option<f32>,
    pub longitude: Option<f32>,
    pub primary_person_id: Option<&'a str>,
    pub folder_number: Option<i32>,
    pub review_status: Option<&'a str>,
    pub description: Option<&'a str>,
}

#[derive(AsChangeset, Default)]
#[diesel(table_name = shots)]
pub struct ShotChangeset<'a> {
    pub main_file_id: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub latitude: Option<f32>,
    pub longitude: Option<f32>,
    pub primary_person_id: Option<Option<&'a str>>,
    pub folder_number: Option<i32>,
    pub review_status: Option<&'a str>,
    pub description: Option<&'a str>,
}

// ── Files ──

#[derive(Insertable)]
#[diesel(table_name = files)]
pub struct NewFile<'a> {
    pub id: &'a str,
    pub shot_id: &'a str,
    pub path: &'a str,
    pub hash: &'a str,
    pub mime_type: Option<&'a str>,
    pub file_size: Option<i32>,
    pub is_original: Option<bool>,
    pub visual_embedding: Option<&'a [u8]>,
    pub source_workflow_id: Option<&'a str>,
    pub source_text_overrides: Option<&'a str>,
    /// Made by a machine, not a camera. `None` leaves the column's `false`
    /// default in place — only the generators say `Some(true)`.
    pub synthetic: Option<bool>,
    /// [`crate::comfyui::ProvenanceManifest`], serialized.
    pub manifest_json: Option<&'a str>,
}

/// What a generator writes onto a file once it knows what it made.
#[derive(AsChangeset)]
#[diesel(table_name = files)]
pub struct SyntheticProvenance<'a> {
    pub synthetic: bool,
    pub manifest_json: Option<&'a str>,
    pub source_workflow_id: Option<&'a str>,
    pub source_text_overrides: Option<&'a str>,
}

// ── Faces ──

#[derive(Insertable)]
#[diesel(table_name = faces)]
pub struct NewFace<'a> {
    pub id: &'a str,
    pub file_id: &'a str,
    pub person_id: Option<&'a str>,
    pub box_x1: Option<f32>,
    pub box_y1: Option<f32>,
    pub box_x2: Option<f32>,
    pub box_y2: Option<f32>,
    pub embedding: Option<&'a [u8]>,
    pub score: Option<f32>,
}

// ── Video Keyframes ──

#[derive(Insertable)]
#[diesel(table_name = video_keyframes)]
pub struct NewVideoKeyframe<'a> {
    pub id: &'a str,
    pub video_file_id: &'a str,
    pub timestamp_ms: Option<i32>,
    pub path: &'a str,
}

// ── ComfyUI Workflows ──

#[derive(Queryable, Selectable, Debug)]
#[diesel(table_name = comfyui_workflows)]
#[allow(dead_code)]
pub struct ComfyuiWorkflow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub workflow_json: String,
    pub inputs_json: Option<String>,
    pub outputs_json: Option<String>,
    pub created_at: Option<String>,
    /// What this workflow accepts and produces — a `comfyui::StageContract`.
    /// `None` on a row imported before contracts existed; the worker backfills
    /// those, and the API derives one on the fly meanwhile.
    pub contract_json: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = comfyui_workflows)]
pub struct NewComfyuiWorkflow<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub workflow_json: &'a str,
    pub inputs_json: Option<&'a str>,
    pub outputs_json: Option<&'a str>,
    pub contract_json: Option<&'a str>,
}

// ── Enhancement Tasks ──

#[derive(Insertable)]
#[diesel(table_name = enhancement_tasks)]
pub struct NewEnhancementTask<'a> {
    pub id: &'a str,
    pub shot_id: &'a str,
    pub workflow_id: &'a str,
    pub text_overrides: Option<&'a str>,
    pub source_file_id: Option<&'a str>,
    /// Which part of a video source the run consumes. `None` means "decide from
    /// the workflow" — see `SourceMode` in `comfyui::source`.
    pub source_mode: Option<&'a str>,
    /// This run's typed parameters, `{"<node_id>.<field>": <value>}`, already
    /// resolved — a swept seed is drawn before the row is written. `None` is a
    /// run that set none, which is every task queued before FR4.
    pub parameters: Option<&'a str>,
    /// The run this task is a step of. Every task queued since FR5 has one:
    /// a single-workflow enhance is a one-stage run, so the board has one kind
    /// of row rather than two.
    pub run_id: Option<&'a str>,
    /// Which step of the run's line this is, 0-based.
    pub stage_idx: Option<i32>,
    /// The task whose output this one eats. `None` at stage 0. It is also the
    /// marker that says a completed task has already been advanced: a row
    /// naming it as parent exists exactly when its continuation was queued.
    pub parent_task_id: Option<&'a str>,
}

// ── Production Lines ──

#[derive(Insertable)]
#[diesel(table_name = production_lines)]
pub struct NewProductionLine<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
}

/// One step of a line: a workflow, plus everything one run of it needs that
/// the workflow itself does not carry.
#[derive(Insertable)]
#[diesel(table_name = line_stages)]
pub struct NewLineStage<'a> {
    pub id: &'a str,
    pub line_id: &'a str,
    pub stage_idx: i32,
    pub workflow_id: &'a str,
    /// Prompt bindings and `role:<node>` directives, as a task's are.
    pub text_overrides: Option<&'a str>,
    /// Typed parameter overrides, as a task's are.
    pub parameters: Option<&'a str>,
    /// The fan-out spec, expanded once per continuation.
    pub vary: Option<&'a str>,
    /// Which part of an upstream video this stage consumes. `None` lets the
    /// graph decide.
    pub source_mode: Option<&'a str>,
    /// Whether this stage's intermediate survives the run.
    pub keep_output: bool,
}

// ── Runs ──

#[derive(Insertable)]
#[diesel(table_name = runs)]
pub struct NewRun<'a> {
    pub id: &'a str,
    /// `None` for a single-workflow enhance, which is a one-stage run with no
    /// line behind it.
    pub line_id: Option<&'a str>,
    pub shot_id: &'a str,
    /// The line's name, or the workflow's — snapshotted, so the run still reads
    /// correctly after the line is renamed or deleted.
    pub label: &'a str,
    pub stage_count: i32,
}

#[derive(AsChangeset, Default)]
#[diesel(table_name = runs)]
pub struct RunChangeset<'a> {
    pub status: Option<&'a str>,
    pub error_message: Option<Option<&'a str>>,
    pub finished_at: Option<Option<&'a str>>,
}

#[derive(AsChangeset)]
#[diesel(table_name = enhancement_tasks)]
pub struct EnhancementTaskChangeset<'a> {
    pub status: Option<&'a str>,
    pub comfyui_prompt_id: Option<&'a str>,
    pub output_file_id: Option<&'a str>,
    pub error_message: Option<&'a str>,
    pub retry_count: Option<i32>,
    pub started_at: Option<&'a str>,
    pub completed_at: Option<&'a str>,
    /// `filename_prefix` the task's save nodes were pinned to, so a finished run
    /// can be found by name when history says nothing.
    pub output_prefix: Option<&'a str>,
    /// Deadline for `awaiting_output`.
    pub settle_until: Option<&'a str>,
    /// Earliest time the worker may pick this row up again.
    pub next_attempt_at: Option<&'a str>,
}

// ── Workflow Presets ──

#[derive(Queryable, Selectable, Debug)]
#[diesel(table_name = workflow_presets)]
#[allow(dead_code)]
pub struct WorkflowPreset {
    pub id: String,
    pub workflow_id: String,
    pub name: String,
    pub text_overrides: String,
    pub sort_order: Option<i32>,
    pub created_at: Option<String>,
    /// The preset's typed parameters, same shape as a task's. `None` is a
    /// preset saved before FR4: prompts only.
    pub parameters: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = workflow_presets)]
pub struct NewWorkflowPreset<'a> {
    pub id: &'a str,
    pub workflow_id: &'a str,
    pub name: &'a str,
    pub text_overrides: &'a str,
    pub sort_order: Option<i32>,
    pub parameters: Option<&'a str>,
}

// ── Ignored Merges ──

#[derive(Queryable, Selectable, Insertable, Debug)]
#[diesel(table_name = ignored_merges)]
pub struct IgnoredMerge {
    pub shot_id_1: String,
    pub shot_id_2: String,
    pub created_at: Option<String>,
}

// ── Settings ──

#[derive(Queryable, Selectable, Insertable, Debug)]
#[diesel(table_name = settings)]
pub struct Setting {
    pub key: String,
    pub value: String,
}
