use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::models::{NewComfyuiWorkflow, NewEnhancementTask, NewWorkflowPreset};
use crate::schema::{comfyui_workflows, enhancement_tasks, files, shots, workflow_presets};

use super::{AppState, UState};

/// Helper: return 503 if ComfyUI is not configured.
fn require_comfyui(state: &AppState) -> Result<String, StatusCode> {
    state
        .comfyui_url
        .clone()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// GET /api/comfyui/health
#[utoipa::path(
    get,
    path = "/api/comfyui/health",
    tag = "comfyui",
    summary = "Check ComfyUI health",
    description = "Check whether ComfyUI is configured and reachable. Returns the connection status and system info.",
    responses(
        (status = 200, description = "ComfyUI health status"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_health(
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let url = require_comfyui(&state)?;
    let client = crate::comfyui::ComfyUiClient::new(&url);

    let result: Result<Result<(), anyhow::Error>, _> =
        tokio::task::spawn_blocking(move || client.health_check()).await;
    match result {
        Ok(Ok(())) => Ok(Json(serde_json::json!({"status": "ok"}))),
        Ok(Err(e)) => {
            tracing::warn!("ComfyUI health check failed: {}", e);
            Ok(Json(
                serde_json::json!({"status": "error", "message": e.to_string()}),
            ))
        }
        Err(e) => {
            tracing::error!("ComfyUI health check task panicked: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// GET /api/comfyui/workflows
#[utoipa::path(
    get,
    path = "/api/comfyui/workflows",
    tag = "comfyui",
    summary = "List workflows",
    description = "List all imported ComfyUI enhancement workflows available for use.",
    responses(
        (status = 200, description = "List of workflows"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_list_workflows(
    UState(state): UState,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows: Vec<crate::models::ComfyuiWorkflow> = comfyui_workflows::table
        .order(comfyui_workflows::created_at.desc())
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to list workflows: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let workflows: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|wf| {
            let inputs: serde_json::Value = wf
                .inputs_json
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Array(vec![]));
            let outputs: serde_json::Value = wf
                .outputs_json
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Array(vec![]));
            serde_json::json!({
                "id": wf.id,
                "name": wf.name,
                "description": wf.description,
                "inputs": inputs,
                "outputs": outputs,
                "created_at": wf.created_at,
            })
        })
        .collect();

    Ok(Json(workflows))
}

/// POST /api/comfyui/workflows — import a workflow template
#[derive(Deserialize, ToSchema)]
pub(super) struct ImportWorkflowPayload {
    name: String,
    description: Option<String>,
    workflow: serde_json::Value,
}

#[utoipa::path(
    post,
    path = "/api/comfyui/workflows",
    tag = "comfyui",
    summary = "Import a workflow",
    description = "Import a ComfyUI workflow JSON for use as an enhancement pipeline.",
    request_body = ImportWorkflowPayload,
    responses(
        (status = 200, description = "Workflow imported successfully"),
        (status = 400, description = "Invalid workflow payload"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_import_workflow(
    UState(state): UState,
    Json(payload): Json<ImportWorkflowPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;

    if payload.name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Validate: must be a JSON object
    if !payload.workflow.is_object() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Must have at least one LoadImage node
    let inputs = crate::comfyui::detect_inputs(&payload.workflow);
    let has_load_image = inputs.iter().any(|i| i.node_type == "LoadImage");
    if !has_load_image {
        return Err(StatusCode::BAD_REQUEST);
    }

    let outputs = crate::comfyui::detect_outputs(&payload.workflow);

    let id = uuid::Uuid::new_v4().to_string();
    let workflow_json =
        serde_json::to_string(&payload.workflow).map_err(|_| StatusCode::BAD_REQUEST)?;
    let inputs_json =
        serde_json::to_string(&inputs).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let outputs_json =
        serde_json::to_string(&outputs).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    diesel::insert_into(comfyui_workflows::table)
        .values(NewComfyuiWorkflow {
            id: &id,
            name: &payload.name,
            description: payload.description.as_deref(),
            workflow_json: &workflow_json,
            inputs_json: Some(&inputs_json),
            outputs_json: Some(&outputs_json),
        })
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to insert workflow: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": payload.name,
        "description": payload.description,
        "inputs": inputs,
        "outputs": outputs,
    })))
}

/// DELETE /api/comfyui/workflows/:id
#[utoipa::path(
    delete,
    path = "/api/comfyui/workflows/{id}",
    tag = "comfyui",
    summary = "Delete a workflow",
    description = "Delete an imported ComfyUI workflow by ID.",
    params(("id" = String, Path, description = "Workflow ID")),
    responses(
        (status = 200, description = "Workflow deleted successfully"),
        (status = 404, description = "Workflow not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_delete_workflow(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let deleted = diesel::delete(comfyui_workflows::table.filter(comfyui_workflows::id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete workflow: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// POST /api/comfyui/enhance — queue an enhancement task
#[derive(Deserialize, ToSchema)]
pub(super) struct EnhancePayload {
    shot_id: String,
    workflow_id: String,
    /// Optional: specific file to use as source. If omitted, the original file is used.
    source_file_id: Option<String>,
    #[serde(default)]
    text_overrides: std::collections::HashMap<String, String>,
}

#[utoipa::path(
    post,
    path = "/api/comfyui/enhance",
    tag = "comfyui",
    summary = "Queue enhancement task",
    description = "Queue an image enhancement task using a ComfyUI workflow. Creates a background task that processes the shot's original file.",
    request_body = EnhancePayload,
    responses(
        (status = 200, description = "Enhancement task queued"),
        (status = 404, description = "Shot or workflow not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_enhance(
    UState(state): UState,
    Json(payload): Json<EnhancePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify shot exists
    let shot_exists: bool = shots::table
        .filter(shots::id.eq(&payload.shot_id))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !shot_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Verify workflow exists
    let wf_exists: bool = comfyui_workflows::table
        .filter(comfyui_workflows::id.eq(&payload.workflow_id))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !wf_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let task_id = uuid::Uuid::new_v4().to_string();
    let text_overrides_json =
        serde_json::to_string(&payload.text_overrides).unwrap_or_else(|_| "{}".to_string());

    diesel::insert_into(enhancement_tasks::table)
        .values(NewEnhancementTask {
            id: &task_id,
            shot_id: &payload.shot_id,
            workflow_id: &payload.workflow_id,
            text_overrides: Some(&text_overrides_json),
            source_file_id: payload.source_file_id.as_deref(),
        })
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to insert enhancement task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "id": task_id,
        "status": "pending",
    })))
}

/// GET /api/comfyui/tasks?shot_id=X
#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct TasksQuery {
    shot_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/comfyui/tasks",
    tag = "comfyui",
    summary = "List enhancement tasks",
    description = "List ComfyUI enhancement tasks with optional status filtering and pagination.",
    params(TasksQuery),
    responses(
        (status = 200, description = "List of enhancement tasks"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_list_tasks(
    UState(state): UState,
    Query(query): Query<TasksQuery>,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let tasks: Vec<serde_json::Value> = if let Some(shot_id) = &query.shot_id {
        query_tasks(&mut conn, Some(shot_id))?
    } else {
        query_tasks(&mut conn, None)?
    };

    Ok(Json(tasks))
}

/// Task row from DSL join query.
struct TaskRow {
    id: String,
    shot_id: String,
    workflow_id: String,
    workflow_name: String,
    status: String,
    error_message: Option<String>,
    retry_count: Option<i32>,
    output_file_id: Option<String>,
    created_at: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    main_file_id: Option<String>,
}

type TaskTuple = (
    String, String, String, String, String,
    Option<String>, Option<i32>, Option<String>,
    Option<String>, Option<String>, Option<String>,
);

fn task_tuple_to_row(t: TaskTuple, main_file_id: Option<String>) -> TaskRow {
    TaskRow {
        id: t.0,
        shot_id: t.1,
        workflow_id: t.2,
        workflow_name: t.3,
        status: t.4,
        error_message: t.5,
        retry_count: t.6,
        output_file_id: t.7,
        created_at: t.8,
        started_at: t.9,
        completed_at: t.10,
        main_file_id,
    }
}

fn task_row_to_json(row: TaskRow) -> serde_json::Value {
    let thumbnail_url = row
        .main_file_id
        .map(|fid| format!("/api/files/{}/thumbnail", fid));
    serde_json::json!({
        "id": row.id,
        "shot_id": row.shot_id,
        "workflow_id": row.workflow_id,
        "workflow_name": row.workflow_name,
        "status": row.status,
        "error_message": row.error_message,
        "retry_count": row.retry_count.unwrap_or(0),
        "output_file_id": row.output_file_id,
        "created_at": row.created_at,
        "started_at": row.started_at,
        "completed_at": row.completed_at,
        "thumbnail_url": thumbnail_url,
    })
}

fn query_tasks(
    conn: &mut diesel::SqliteConnection,
    filter_shot_id: Option<&String>,
) -> Result<Vec<serde_json::Value>, StatusCode> {
    let task_select = (
        enhancement_tasks::id,
        enhancement_tasks::shot_id,
        enhancement_tasks::workflow_id,
        comfyui_workflows::name,
        enhancement_tasks::status,
        enhancement_tasks::error_message,
        enhancement_tasks::retry_count,
        enhancement_tasks::output_file_id,
        enhancement_tasks::created_at,
        enhancement_tasks::started_at,
        enhancement_tasks::completed_at,
    );

    let tuples: Vec<TaskTuple> = if let Some(sid) = filter_shot_id {
        enhancement_tasks::table
            .inner_join(comfyui_workflows::table)
            .select(task_select)
            .filter(enhancement_tasks::shot_id.eq(sid))
            .order(enhancement_tasks::created_at.desc())
            .load(conn)
    } else {
        enhancement_tasks::table
            .inner_join(comfyui_workflows::table)
            .select(task_select)
            .order(enhancement_tasks::created_at.desc())
            .limit(100)
            .load(conn)
    }
    .map_err(|e| {
        tracing::error!("Failed to query tasks: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Batch-fetch shot main_file_ids
    let shot_ids: Vec<&str> = tuples.iter().map(|t| t.1.as_str()).collect();
    let shot_main_files: std::collections::HashMap<String, Option<String>> = if !shot_ids.is_empty() {
        shots::table
            .filter(shots::id.eq_any(&shot_ids))
            .select((shots::id, shots::main_file_id))
            .load::<(String, Option<String>)>(conn)
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        std::collections::HashMap::new()
    };

    let rows: Vec<TaskRow> = tuples
        .into_iter()
        .map(|t| {
            let main_fid = shot_main_files.get(&t.1).cloned().flatten();
            task_tuple_to_row(t, main_fid)
        })
        .collect();

    Ok(rows.into_iter().map(task_row_to_json).collect())
}

/// GET /api/comfyui/tasks/:id
#[utoipa::path(
    get,
    path = "/api/comfyui/tasks/{id}",
    tag = "comfyui",
    summary = "Get enhancement task",
    description = "Retrieve details and current status of a specific enhancement task.",
    params(("id" = String, Path, description = "Enhancement task ID")),
    responses(
        (status = 200, description = "Task details"),
        (status = 404, description = "Task not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_get_task(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let tuple: TaskTuple = enhancement_tasks::table
        .inner_join(comfyui_workflows::table)
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            enhancement_tasks::workflow_id,
            comfyui_workflows::name,
            enhancement_tasks::status,
            enhancement_tasks::error_message,
            enhancement_tasks::retry_count,
            enhancement_tasks::output_file_id,
            enhancement_tasks::created_at,
            enhancement_tasks::started_at,
            enhancement_tasks::completed_at,
        ))
        .filter(enhancement_tasks::id.eq(&id))
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let main_fid: Option<String> = shots::table
        .select(shots::main_file_id)
        .filter(shots::id.eq(&tuple.1))
        .first::<Option<String>>(&mut conn)
        .ok()
        .flatten();

    Ok(Json(task_row_to_json(task_tuple_to_row(tuple, main_fid))))
}

/// POST /api/comfyui/tasks/:id/retry — retry a failed task
#[utoipa::path(
    post,
    path = "/api/comfyui/tasks/{id}/retry",
    tag = "comfyui",
    summary = "Retry enhancement task",
    description = "Retry a failed enhancement task. Resets it to pending status for reprocessing.",
    params(("id" = String, Path, description = "Enhancement task ID to retry")),
    responses(
        (status = 200, description = "Task retried successfully"),
        (status = 400, description = "Task is not in failed state"),
        (status = 404, description = "Task not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_retry_task(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Only allow retrying failed tasks
    let status: String = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq(&id))
        .select(enhancement_tasks::status)
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if status != "failed" {
        return Err(StatusCode::BAD_REQUEST);
    }

    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&id)))
        .set((
            enhancement_tasks::status.eq("pending"),
            enhancement_tasks::error_message.eq(None::<String>),
            enhancement_tasks::retry_count.eq(0),
        ))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to retry task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "pending"})))
}

// ===== Shot Generations =====

/// GET /api/comfyui/generations/:shot_id — get generation history for a shot
#[utoipa::path(
    get,
    path = "/api/comfyui/generations/{shot_id}",
    tag = "comfyui",
    summary = "Get shot generation history",
    description = "Returns the workflow and text overrides used to generate each non-original file for a shot.",
    params(("shot_id" = String, Path, description = "Shot ID")),
    responses(
        (status = 200, description = "List of generations"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_shot_generations(
    Path(shot_id): Path<String>,
    UState(state): UState,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows: Vec<(String, Option<String>, Option<String>)> = files::table
        .filter(files::shot_id.eq(&shot_id))
        .filter(files::source_workflow_id.is_not_null())
        .select((
            files::id,
            files::source_workflow_id,
            files::source_text_overrides,
        ))
        .order(files::created_at.desc())
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query generations: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let generations: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(file_id, workflow_id, overrides_str)| {
            let overrides: serde_json::Value = overrides_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "file_id": file_id,
                "workflow_id": workflow_id,
                "text_overrides": overrides,
            })
        })
        .collect();

    Ok(Json(generations))
}

// ===== Workflow Presets =====

/// GET /api/comfyui/workflows/:id/presets
#[derive(Deserialize, ToSchema)]
pub(super) struct PresetPayload {
    name: String,
    #[serde(default)]
    text_overrides: std::collections::HashMap<String, String>,
    sort_order: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/comfyui/workflows/{id}/presets",
    tag = "comfyui",
    summary = "List workflow presets",
    description = "List all prompt presets for a specific workflow.",
    params(("id" = String, Path, description = "Workflow ID")),
    responses(
        (status = 200, description = "List of presets"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_list_presets(
    Path(workflow_id): Path<String>,
    UState(state): UState,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let rows: Vec<crate::models::WorkflowPreset> = workflow_presets::table
        .filter(workflow_presets::workflow_id.eq(&workflow_id))
        .order((
            workflow_presets::sort_order.asc(),
            workflow_presets::created_at.asc(),
        ))
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to list presets: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let presets: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|p| {
            let overrides: serde_json::Value =
                serde_json::from_str(&p.text_overrides).unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "text_overrides": overrides,
                "sort_order": p.sort_order.unwrap_or(0),
                "created_at": p.created_at,
            })
        })
        .collect();

    Ok(Json(presets))
}

/// POST /api/comfyui/workflows/:id/presets
#[utoipa::path(
    post,
    path = "/api/comfyui/workflows/{id}/presets",
    tag = "comfyui",
    summary = "Create workflow preset",
    description = "Create a new prompt preset for a workflow with saved text overrides.",
    params(("id" = String, Path, description = "Workflow ID")),
    request_body = PresetPayload,
    responses(
        (status = 200, description = "Preset created"),
        (status = 404, description = "Workflow not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_create_preset(
    Path(workflow_id): Path<String>,
    UState(state): UState,
    Json(payload): Json<PresetPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify workflow exists
    let wf_exists: bool = comfyui_workflows::table
        .filter(comfyui_workflows::id.eq(&workflow_id))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !wf_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let overrides_json =
        serde_json::to_string(&payload.text_overrides).unwrap_or_else(|_| "{}".to_string());
    let sort_order = payload.sort_order.unwrap_or(0) as i32;

    diesel::insert_into(workflow_presets::table)
        .values(NewWorkflowPreset {
            id: &id,
            workflow_id: &workflow_id,
            name: &payload.name,
            text_overrides: &overrides_json,
            sort_order: Some(sort_order),
        })
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to insert preset: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": payload.name,
        "text_overrides": payload.text_overrides,
        "sort_order": sort_order,
    })))
}

/// PUT /api/comfyui/workflows/:workflow_id/presets/:preset_id
#[utoipa::path(
    put,
    path = "/api/comfyui/workflows/{workflow_id}/presets/{preset_id}",
    tag = "comfyui",
    summary = "Update workflow preset",
    description = "Update an existing prompt preset's name, text overrides, or sort order.",
    params(
        ("workflow_id" = String, Path, description = "Workflow ID"),
        ("preset_id" = String, Path, description = "Preset ID"),
    ),
    request_body = PresetPayload,
    responses(
        (status = 200, description = "Preset updated"),
        (status = 404, description = "Preset not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_update_preset(
    Path((workflow_id, preset_id)): Path<(String, String)>,
    UState(state): UState,
    Json(payload): Json<PresetPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let overrides_json =
        serde_json::to_string(&payload.text_overrides).unwrap_or_else(|_| "{}".to_string());
    let sort_order = payload.sort_order.unwrap_or(0) as i32;

    let updated = diesel::update(
        workflow_presets::table
            .filter(workflow_presets::id.eq(&preset_id))
            .filter(workflow_presets::workflow_id.eq(&workflow_id)),
    )
    .set((
        workflow_presets::name.eq(&payload.name),
        workflow_presets::text_overrides.eq(&overrides_json),
        workflow_presets::sort_order.eq(sort_order),
    ))
    .execute(&mut conn)
    .map_err(|e| {
        tracing::error!("Failed to update preset: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if updated == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(serde_json::json!({
        "id": preset_id,
        "name": payload.name,
        "text_overrides": payload.text_overrides,
        "sort_order": sort_order,
    })))
}

/// DELETE /api/comfyui/workflows/:workflow_id/presets/:preset_id
#[utoipa::path(
    delete,
    path = "/api/comfyui/workflows/{workflow_id}/presets/{preset_id}",
    tag = "comfyui",
    summary = "Delete workflow preset",
    description = "Delete a prompt preset from a workflow.",
    params(
        ("workflow_id" = String, Path, description = "Workflow ID"),
        ("preset_id" = String, Path, description = "Preset ID"),
    ),
    responses(
        (status = 200, description = "Preset deleted"),
        (status = 404, description = "Preset not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_delete_preset(
    Path((workflow_id, preset_id)): Path<(String, String)>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let deleted = diesel::delete(
        workflow_presets::table
            .filter(workflow_presets::id.eq(&preset_id))
            .filter(workflow_presets::workflow_id.eq(&workflow_id)),
    )
    .execute(&mut conn)
    .map_err(|e| {
        tracing::error!("Failed to delete preset: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(serde_json::json!({"status": "ok"})))
}

/// DELETE /api/comfyui/tasks/:id — remove a failed or completed task
#[utoipa::path(
    delete,
    path = "/api/comfyui/tasks/{id}",
    tag = "comfyui",
    summary = "Delete enhancement task",
    description = "Delete a completed or failed enhancement task record.",
    params(("id" = String, Path, description = "Enhancement task ID to delete")),
    responses(
        (status = 200, description = "Task deleted successfully"),
        (status = 400, description = "Task is not in failed or completed state"),
        (status = 404, description = "Task not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_delete_task(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Only allow deleting failed or completed tasks
    let status: String = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq(&id))
        .select(enhancement_tasks::status)
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if status != "failed" && status != "completed" {
        return Err(StatusCode::BAD_REQUEST);
    }

    diesel::delete(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}
