use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::Deserialize;
use utoipa::ToSchema;

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
    let db = state.db.lock().await;

    let mut stmt = db
        .prepare(
            "SELECT id, name, description, inputs_json, outputs_json, created_at FROM comfyui_workflows ORDER BY created_at DESC",
        )
        .map_err(|e| {
            tracing::error!("Failed to list workflows: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let workflows: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let description: Option<String> = row.get(2)?;
            let inputs_str: Option<String> = row.get(3)?;
            let outputs_str: Option<String> = row.get(4)?;
            let created_at: String = row.get(5)?;

            let inputs: serde_json::Value = inputs_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Array(vec![]));
            let outputs: serde_json::Value = outputs_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Array(vec![]));

            Ok(serde_json::json!({
                "id": id,
                "name": name,
                "description": description,
                "inputs": inputs,
                "outputs": outputs,
                "created_at": created_at,
            }))
        })
        .map_err(|e| {
            tracing::error!("Failed to query workflows: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
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

    let db = state.db.lock().await;
    db.execute(
        "INSERT INTO comfyui_workflows (id, name, description, workflow_json, inputs_json, outputs_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, payload.name, payload.description, workflow_json, inputs_json, outputs_json],
    )
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
    let db = state.db.lock().await;

    let deleted = db
        .execute("DELETE FROM comfyui_workflows WHERE id = ?1", params![id])
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
    let db = state.db.lock().await;

    // Verify shot exists
    let shot_exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM shots WHERE id = ?1",
            params![payload.shot_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !shot_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Verify workflow exists
    let wf_exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM comfyui_workflows WHERE id = ?1",
            params![payload.workflow_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !wf_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let task_id = uuid::Uuid::new_v4().to_string();
    let text_overrides_json =
        serde_json::to_string(&payload.text_overrides).unwrap_or_else(|_| "{}".to_string());

    db.execute(
        "INSERT INTO enhancement_tasks (id, shot_id, workflow_id, text_overrides) VALUES (?1, ?2, ?3, ?4)",
        params![task_id, payload.shot_id, payload.workflow_id, text_overrides_json],
    )
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
    let db = state.db.lock().await;

    let tasks: Vec<serde_json::Value> = if let Some(shot_id) = &query.shot_id {
        query_tasks(&db, Some(shot_id))?
    } else {
        query_tasks(&db, None)?
    };

    Ok(Json(tasks))
}

fn task_row_to_json(row: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
    let shot_id: String = row.get(1)?;
    let main_file_id: Option<String> = row.get(11)?;
    let thumbnail_url = main_file_id.map(|fid| format!("/api/files/{}/thumbnail", fid));
    Ok(serde_json::json!({
        "id": row.get::<_, String>(0)?,
        "shot_id": shot_id,
        "workflow_id": row.get::<_, String>(2)?,
        "workflow_name": row.get::<_, String>(3)?,
        "status": row.get::<_, String>(4)?,
        "error_message": row.get::<_, Option<String>>(5)?,
        "retry_count": row.get::<_, i64>(6)?,
        "output_file_id": row.get::<_, Option<String>>(7)?,
        "created_at": row.get::<_, String>(8)?,
        "started_at": row.get::<_, Option<String>>(9)?,
        "completed_at": row.get::<_, Option<String>>(10)?,
        "thumbnail_url": thumbnail_url,
    }))
}

fn query_tasks(
    db: &rusqlite::Connection,
    shot_id: Option<&String>,
) -> Result<Vec<serde_json::Value>, StatusCode> {
    if let Some(shot_id) = shot_id {
        let mut stmt = db
            .prepare(
                "SELECT t.id, t.shot_id, t.workflow_id, w.name, t.status, t.error_message,
                        t.retry_count, t.output_file_id, t.created_at, t.started_at, t.completed_at,
                        s.main_file_id
                 FROM enhancement_tasks t
                 JOIN comfyui_workflows w ON t.workflow_id = w.id
                 LEFT JOIN shots s ON t.shot_id = s.id
                 WHERE t.shot_id = ?1
                 ORDER BY t.created_at DESC",
            )
            .map_err(|e| {
                tracing::error!("Failed to prepare tasks query: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        let rows: Vec<serde_json::Value> = stmt
            .query_map(params![shot_id], task_row_to_json)
            .map_err(|e| {
                tracing::error!("Failed to query tasks: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    } else {
        let mut stmt = db
            .prepare(
                "SELECT t.id, t.shot_id, t.workflow_id, w.name, t.status, t.error_message,
                        t.retry_count, t.output_file_id, t.created_at, t.started_at, t.completed_at,
                        s.main_file_id
                 FROM enhancement_tasks t
                 JOIN comfyui_workflows w ON t.workflow_id = w.id
                 LEFT JOIN shots s ON t.shot_id = s.id
                 ORDER BY t.created_at DESC
                 LIMIT 100",
            )
            .map_err(|e| {
                tracing::error!("Failed to prepare tasks query: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        let rows: Vec<serde_json::Value> = stmt
            .query_map([], task_row_to_json)
            .map_err(|e| {
                tracing::error!("Failed to query tasks: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
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
    let db = state.db.lock().await;

    let task = db
        .query_row(
            "SELECT t.id, t.shot_id, t.workflow_id, w.name, t.status, t.error_message,
                    t.retry_count, t.output_file_id, t.created_at, t.started_at, t.completed_at,
                    s.main_file_id
             FROM enhancement_tasks t
             JOIN comfyui_workflows w ON t.workflow_id = w.id
             LEFT JOIN shots s ON t.shot_id = s.id
             WHERE t.id = ?1",
            params![id],
            task_row_to_json,
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(task))
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
    let db = state.db.lock().await;

    // Only allow retrying failed tasks
    let status: String = db
        .query_row(
            "SELECT status FROM enhancement_tasks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if status != "failed" {
        return Err(StatusCode::BAD_REQUEST);
    }

    db.execute(
        "UPDATE enhancement_tasks SET status = 'pending', error_message = NULL, retry_count = 0 WHERE id = ?1",
        params![id],
    )
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
    let db = state.db.lock().await;

    let mut stmt = db
        .prepare(
            "SELECT f.id, f.source_workflow_id, f.source_text_overrides
             FROM files f
             WHERE f.shot_id = ?1 AND f.source_workflow_id IS NOT NULL
             ORDER BY f.created_at DESC",
        )
        .map_err(|e| {
            tracing::error!("Failed to query generations: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let generations: Vec<serde_json::Value> = stmt
        .query_map(params![shot_id], |row| {
            let overrides_str: Option<String> = row.get(2)?;
            let overrides: serde_json::Value = overrides_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::json!({}));
            Ok(serde_json::json!({
                "file_id": row.get::<_, String>(0)?,
                "workflow_id": row.get::<_, String>(1)?,
                "text_overrides": overrides,
            }))
        })
        .map_err(|e| {
            tracing::error!("Failed to query generations: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
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
    let db = state.db.lock().await;

    let mut stmt = db
        .prepare(
            "SELECT id, name, text_overrides, sort_order, created_at
             FROM workflow_presets
             WHERE workflow_id = ?1
             ORDER BY sort_order ASC, created_at ASC",
        )
        .map_err(|e| {
            tracing::error!("Failed to list presets: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let presets: Vec<serde_json::Value> = stmt
        .query_map(params![workflow_id], |row| {
            let overrides_str: String = row.get(2)?;
            let overrides: serde_json::Value =
                serde_json::from_str(&overrides_str).unwrap_or(serde_json::json!({}));
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "text_overrides": overrides,
                "sort_order": row.get::<_, i64>(3)?,
                "created_at": row.get::<_, String>(4)?,
            }))
        })
        .map_err(|e| {
            tracing::error!("Failed to query presets: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter_map(|r| r.ok())
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
    let db = state.db.lock().await;

    // Verify workflow exists
    let wf_exists: bool = db
        .query_row(
            "SELECT COUNT(*) FROM comfyui_workflows WHERE id = ?1",
            params![workflow_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !wf_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let overrides_json =
        serde_json::to_string(&payload.text_overrides).unwrap_or_else(|_| "{}".to_string());
    let sort_order = payload.sort_order.unwrap_or(0);

    db.execute(
        "INSERT INTO workflow_presets (id, workflow_id, name, text_overrides, sort_order) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, workflow_id, payload.name, overrides_json, sort_order],
    )
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
    let db = state.db.lock().await;

    let overrides_json =
        serde_json::to_string(&payload.text_overrides).unwrap_or_else(|_| "{}".to_string());
    let sort_order = payload.sort_order.unwrap_or(0);

    let updated = db
        .execute(
            "UPDATE workflow_presets SET name = ?1, text_overrides = ?2, sort_order = ?3 WHERE id = ?4 AND workflow_id = ?5",
            params![payload.name, overrides_json, sort_order, preset_id, workflow_id],
        )
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
    let db = state.db.lock().await;

    let deleted = db
        .execute(
            "DELETE FROM workflow_presets WHERE id = ?1 AND workflow_id = ?2",
            params![preset_id, workflow_id],
        )
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
    let db = state.db.lock().await;

    // Only allow deleting failed or completed tasks
    let status: String = db
        .query_row(
            "SELECT status FROM enhancement_tasks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if status != "failed" && status != "completed" {
        return Err(StatusCode::BAD_REQUEST);
    }

    db.execute(
        "DELETE FROM enhancement_tasks WHERE id = ?1",
        params![id],
    )
    .map_err(|e| {
        tracing::error!("Failed to delete task: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}
