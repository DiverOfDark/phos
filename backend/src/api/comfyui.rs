use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::models::{NewComfyuiWorkflow, NewEnhancementTask, NewWorkflowPreset};
use crate::schema::{comfyui_workflows, enhancement_tasks, files, people, shots, workflow_presets};

use super::{AppState, UState};

/// Helper: return 503 if ComfyUI is not configured.
fn require_comfyui(state: &AppState) -> Result<String, StatusCode> {
    state
        .comfyui_url
        .clone()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// A status plus, where it helps, something the user can act on.
///
/// Import is the one endpoint here where a bare 400 is actively unhelpful: the
/// user pasted a graph and needs to know which part of it Phos could not use.
/// The UI already reads `error` out of a JSON body.
pub(super) struct ApiError(StatusCode, Option<String>);

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, Some(message.into()))
    }

    fn internal() -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, None)
    }
}

impl From<StatusCode> for ApiError {
    fn from(status: StatusCode) -> Self {
        Self(status, None)
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        match self.1 {
            Some(message) => {
                (self.0, Json(serde_json::json!({ "error": message }))).into_response()
            }
            None => self.0.into_response(),
        }
    }
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
            // Loaders are derived on read rather than stored: workflows
            // imported before this existed have no record of theirs, and the
            // graph is already in memory here — the list only omits it from the
            // *response*, which is tens of kilobytes per row.
            let loaders = serde_json::from_str::<serde_json::Value>(&wf.workflow_json)
                .map(|graph| crate::comfyui::detect_loaders(&graph))
                .unwrap_or_default();
            let takes_video = loaders
                .iter()
                .any(|l| l.kind == crate::comfyui::LoaderKind::Video);
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
                "loaders": loaders,
                "takes_video": takes_video,
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
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;

    if payload.name.is_empty() {
        return Err(ApiError::bad_request("A workflow needs a name."));
    }

    // Validate: must be a JSON object
    if !payload.workflow.is_object() {
        return Err(ApiError::bad_request(
            "The workflow must be a ComfyUI API-format graph: a JSON object of nodes \
             keyed by node id. Export it with 'Save (API Format)', not 'Save'.",
        ));
    }

    // Must have somewhere to put the source: an image loader, or a video one.
    // Requiring `LoadImage` specifically 400'd every video workflow, which made
    // video→video unreachable however good the rest of the pipeline was.
    crate::comfyui::importable(&payload.workflow).map_err(ApiError::bad_request)?;

    let inputs = crate::comfyui::detect_inputs(&payload.workflow);
    let outputs = crate::comfyui::detect_outputs(&payload.workflow);

    let id = uuid::Uuid::new_v4().to_string();
    let workflow_json = serde_json::to_string(&payload.workflow)
        .map_err(|_| ApiError::bad_request("The workflow is not serialisable JSON."))?;
    let inputs_json = serde_json::to_string(&inputs).map_err(|_| ApiError::internal())?;
    let outputs_json = serde_json::to_string(&outputs).map_err(|_| ApiError::internal())?;

    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

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
            ApiError::internal()
        })?;

    Ok(Json(serde_json::json!({
        "id": id,
        "name": payload.name,
        "description": payload.description,
        "inputs": inputs,
        "outputs": outputs,
    })))
}

/// GET /api/comfyui/workflows/:id/graph — the stored node graph
///
/// The list endpoint deliberately omits `workflow_json`: a graph is tens of
/// kilobytes and listing ten workflows would ship all of them to draw none. The
/// console asks for one graph when it opens one workflow.
#[utoipa::path(
    get,
    path = "/api/comfyui/workflows/{id}/graph",
    tag = "comfyui",
    summary = "Get a workflow's node graph",
    description = "Return the stored ComfyUI API-format graph for one workflow, \
                   alongside the inputs and outputs detected at import time.",
    params(("id" = String, Path, description = "Workflow ID")),
    responses(
        (status = 200, description = "Workflow graph"),
        (status = 404, description = "Workflow not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_workflow_graph(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let wf: crate::models::ComfyuiWorkflow = comfyui_workflows::table
        .filter(comfyui_workflows::id.eq(&id))
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // A graph that no longer parses is a broken import, not a server fault: the
    // console gets an empty object and says so rather than a 500.
    let graph: serde_json::Value = serde_json::from_str(&wf.workflow_json)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let inputs: serde_json::Value = wf
        .inputs_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Array(vec![]));
    let outputs: serde_json::Value = wf
        .outputs_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Array(vec![]));

    Ok(Json(serde_json::json!({
        "id": wf.id,
        "name": wf.name,
        "graph": graph,
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
    /// Which part of a video source to feed the workflow: `first_frame`,
    /// `last_frame`, `at_time:<ms>`, `keyframe:<n>` or `whole_video`.
    ///
    /// Omit it to let the workflow decide — a graph with a video loader takes
    /// the whole clip, anything else takes the first frame. Ignored for stills.
    source_mode: Option<String>,
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
        (status = 400, description = "Unrecognised source_mode"),
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

    // Reject an unreadable source mode here rather than storing it and letting
    // the worker fall back to a default the caller did not ask for.
    if let Some(mode) = payload.source_mode.as_deref() {
        if mode.parse::<crate::comfyui::SourceMode>().is_err() {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

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
            source_mode: payload.source_mode.as_deref(),
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

/// GET /api/comfyui/tasks?shot_id=X&limit=N&cursor=TIMESTAMP
#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct TasksQuery {
    shot_id: Option<String>,
    /// Max items to return (default 50)
    limit: Option<i64>,
    /// Cursor: created_at value of the last item from the previous page
    cursor: Option<String>,
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
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let limit = query.limit.unwrap_or(50).min(200);
    let result = query_tasks(&mut conn, query.shot_id.as_ref(), query.cursor.as_ref(), limit)?;

    Ok(Json(result))
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
    source_file_id: Option<String>,
    main_file_id: Option<String>,
    /// Who the source shot belongs to, and the file the thumbnail shows.
    person_name: Option<String>,
    source_name: Option<String>,
}

type TaskTuple = (
    String, String, String, String, String,
    Option<String>, Option<i32>, Option<String>,
    Option<String>, Option<String>, Option<String>,
    Option<String>,
);

fn task_tuple_to_row(
    t: TaskTuple,
    main_file_id: Option<String>,
    person_name: Option<String>,
    source_name: Option<String>,
) -> TaskRow {
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
        source_file_id: t.11,
        main_file_id,
        person_name,
        source_name,
    }
}

fn task_row_to_json(row: TaskRow) -> serde_json::Value {
    let thumbnail_url = row
        .source_file_id
        .or(row.main_file_id)
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
        "person_name": row.person_name,
        "source_name": row.source_name,
    })
}

fn query_tasks(
    conn: &mut diesel::SqliteConnection,
    filter_shot_id: Option<&String>,
    cursor: Option<&String>,
    limit: i64,
) -> Result<serde_json::Value, StatusCode> {
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
        enhancement_tasks::source_file_id,
    );

    // Fetch limit+1 to detect if there's a next page
    let fetch_limit = limit + 1;

    let mut query = enhancement_tasks::table
        .inner_join(comfyui_workflows::table)
        .select(task_select)
        .order(enhancement_tasks::created_at.desc())
        .limit(fetch_limit)
        .into_boxed();

    if let Some(sid) = filter_shot_id {
        query = query.filter(enhancement_tasks::shot_id.eq(sid));
    }

    if let Some(c) = cursor {
        query = query.filter(enhancement_tasks::created_at.lt(c));
    }

    let mut tuples: Vec<TaskTuple> = query
        .load(conn)
        .map_err(|e| {
            tracing::error!("Failed to query tasks: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let has_more = tuples.len() as i64 > limit;
    if has_more {
        tuples.truncate(limit as usize);
    }

    // Batch-fetch what the queue needs to name its rows: the shot's main file
    // (for a thumbnail when the task recorded no source), who the shot belongs
    // to, and the filename being enhanced. Three batched lookups rather than
    // three per task, because a queue page is fifty rows.
    let shot_ids: Vec<&str> = tuples.iter().map(|t| t.1.as_str()).collect();
    let shot_rows: Vec<(String, Option<String>, Option<String>)> = if !shot_ids.is_empty() {
        shots::table
            .filter(shots::id.eq_any(&shot_ids))
            .select((shots::id, shots::main_file_id, shots::primary_person_id))
            .load(conn)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let shot_main_files: std::collections::HashMap<String, Option<String>> = shot_rows
        .iter()
        .map(|(id, fid, _)| (id.clone(), fid.clone()))
        .collect();
    let shot_people: std::collections::HashMap<String, Option<String>> = shot_rows
        .iter()
        .map(|(id, _, pid)| (id.clone(), pid.clone()))
        .collect();

    let person_ids: Vec<String> = shot_rows.iter().filter_map(|(_, _, pid)| pid.clone()).collect();
    let person_names: std::collections::HashMap<String, Option<String>> = if !person_ids.is_empty() {
        people::table
            .filter(people::id.eq_any(&person_ids))
            .select((people::id, people::name))
            .load(conn)
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        std::collections::HashMap::new()
    };

    // The filename shown is the one the thumbnail points at: the task's own
    // source file when it has one, otherwise the shot's main file.
    let file_ids: Vec<String> = tuples
        .iter()
        .filter_map(|t| {
            t.11.clone()
                .or_else(|| shot_main_files.get(&t.1).cloned().flatten())
        })
        .collect();
    let file_paths: std::collections::HashMap<String, String> = if !file_ids.is_empty() {
        files::table
            .filter(files::id.eq_any(&file_ids))
            .select((files::id, files::path))
            .load::<(String, String)>(conn)
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
            let person_name = shot_people
                .get(&t.1)
                .cloned()
                .flatten()
                .and_then(|pid| person_names.get(&pid).cloned().flatten());
            let source_name = t
                .11
                .clone()
                .or_else(|| main_fid.clone())
                .and_then(|fid| file_paths.get(&fid).cloned())
                .and_then(|path| path.rsplit('/').next().map(|s| s.to_string()));
            task_tuple_to_row(t, main_fid, person_name, source_name)
        })
        .collect();

    let next_cursor = if has_more {
        rows.last().and_then(|r| r.created_at.clone())
    } else {
        None
    };

    let items: Vec<serde_json::Value> = rows.into_iter().map(task_row_to_json).collect();

    Ok(serde_json::json!({
        "items": items,
        "next_cursor": next_cursor,
    }))
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
            enhancement_tasks::source_file_id,
        ))
        .filter(enhancement_tasks::id.eq(&id))
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let (main_fid, person_id): (Option<String>, Option<String>) = shots::table
        .select((shots::main_file_id, shots::primary_person_id))
        .filter(shots::id.eq(&tuple.1))
        .first(&mut conn)
        .unwrap_or((None, None));

    let person_name: Option<String> = person_id.and_then(|pid| {
        people::table
            .select(people::name)
            .filter(people::id.eq(pid))
            .first::<Option<String>>(&mut conn)
            .ok()
            .flatten()
    });

    let source_name: Option<String> = tuple
        .11
        .clone()
        .or_else(|| main_fid.clone())
        .and_then(|fid| {
            files::table
                .select(files::path)
                .filter(files::id.eq(fid))
                .first::<String>(&mut conn)
                .ok()
        })
        .and_then(|path| path.rsplit('/').next().map(|s| s.to_string()));

    Ok(Json(task_row_to_json(task_tuple_to_row(
        tuple,
        main_fid,
        person_name,
        source_name,
    ))))
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

    if status != "failed" && status != crate::comfyui::STATUS_CANCELLED {
        return Err(StatusCode::BAD_REQUEST);
    }

    // A hand-driven retry starts from scratch: clear the automatic retry budget,
    // the settle clock, the backoff, and the old prompt id.
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&id)))
        .set((
            enhancement_tasks::status.eq("pending"),
            enhancement_tasks::error_message.eq(None::<String>),
            enhancement_tasks::retry_count.eq(0),
            enhancement_tasks::settle_until.eq(None::<String>),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
            enhancement_tasks::comfyui_prompt_id.eq(None::<String>),
        ))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to retry task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "pending"})))
}

/// POST /api/comfyui/tasks/:id/cancel — stop a task, on both sides
#[utoipa::path(
    post,
    path = "/api/comfyui/tasks/{id}/cancel",
    tag = "comfyui",
    summary = "Cancel enhancement task",
    description = "Stop an in-flight enhancement task: interrupt it on ComfyUI if it is the \
                   running prompt, drop it from ComfyUI's pending queue, and mark the task \
                   cancelled.",
    params(("id" = String, Path, description = "Enhancement task ID to cancel")),
    responses(
        (status = 200, description = "Task cancelled"),
        (status = 400, description = "Task has already finished"),
        (status = 404, description = "Task not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_cancel_task(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let url = require_comfyui(&state)?;
    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (status, prompt_id): (String, Option<String>) = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq(&id))
        .select((
            enhancement_tasks::status,
            enhancement_tasks::comfyui_prompt_id,
        ))
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Nothing to stop once it has landed.
    if matches!(
        status.as_str(),
        "completed" | "failed" | crate::comfyui::STATUS_CANCELLED
    ) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Tell ComfyUI first, then record it. A task that never reached ComfyUI has
    // no prompt id and only needs the local row updated.
    if let Some(prompt_id) = prompt_id {
        let _ = tokio::task::spawn_blocking(move || {
            let client = crate::comfyui::ComfyUiClient::new(&url);
            // Only the running prompt is worth interrupting — /interrupt stops
            // whatever is executing, so firing it for a merely-queued prompt
            // would kill somebody else's job.
            match client.is_prompt_running(&prompt_id) {
                Ok(true) => {
                    if let Err(e) = client.interrupt() {
                        tracing::warn!("Interrupt for prompt {} failed: {}", prompt_id, e);
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::warn!("Could not read ComfyUI queue: {}", e),
            }
            if let Err(e) = client.delete_queued(&prompt_id) {
                tracing::warn!("Dropping prompt {} from the queue failed: {}", prompt_id, e);
            }
        })
        .await;
    }

    let now = chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&id)))
        .set((
            enhancement_tasks::status.eq(crate::comfyui::STATUS_CANCELLED),
            enhancement_tasks::error_message.eq("Cancelled"),
            enhancement_tasks::completed_at.eq(&now),
            enhancement_tasks::settle_until.eq(None::<String>),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
        ))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to cancel task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(
        serde_json::json!({"status": crate::comfyui::STATUS_CANCELLED}),
    ))
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

    if !matches!(
        status.as_str(),
        "failed" | "completed" | crate::comfyui::STATUS_CANCELLED
    ) {
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
