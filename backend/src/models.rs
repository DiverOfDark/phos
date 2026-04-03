use diesel::prelude::*;
use crate::schema::*;

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
}

#[derive(Insertable)]
#[diesel(table_name = workflow_presets)]
pub struct NewWorkflowPreset<'a> {
    pub id: &'a str,
    pub workflow_id: &'a str,
    pub name: &'a str,
    pub text_overrides: &'a str,
    pub sort_order: Option<i32>,
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
