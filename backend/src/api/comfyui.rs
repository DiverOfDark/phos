use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::Deserialize;

use super::{AppState, UState};

/// Helper: return 503 if ComfyUI is not configured.
fn require_comfyui(state: &AppState) -> Result<String, StatusCode> {
    state
        .comfyui_url
        .clone()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// GET /api/comfyui/health
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
#[derive(Deserialize)]
pub(super) struct ImportWorkflowPayload {
    name: String,
    description: Option<String>,
    workflow: serde_json::Value,
}

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
#[derive(Deserialize)]
pub(super) struct EnhancePayload {
    shot_id: String,
    workflow_id: String,
    #[serde(default)]
    text_overrides: std::collections::HashMap<String, String>,
}

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
#[derive(Deserialize)]
pub(super) struct TasksQuery {
    shot_id: Option<String>,
}

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

/// DELETE /api/comfyui/tasks/:id — remove a failed or completed task
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
