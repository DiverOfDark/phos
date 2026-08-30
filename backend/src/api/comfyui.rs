use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::comfyui::{ContractCorrections, ParameterMap, StageContract, VaryMap};
use crate::models::{NewComfyuiWorkflow, NewEnhancementTask, NewWorkflowPreset};
use crate::schema::{
    comfyui_workflows, enhancement_tasks, files, line_stages, people, production_lines, shots,
    workflow_presets,
};

use super::{AppState, UState};

/// Helper: return 503 if ComfyUI is not configured.
pub(super) fn require_comfyui(state: &AppState) -> Result<String, StatusCode> {
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
#[derive(Debug)]
pub(super) struct ApiError(pub(super) StatusCode, pub(super) Option<String>);

impl ApiError {
    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, Some(message.into()))
    }

    /// The request was well formed and refused anyway: editing a line while a
    /// run of it is still walking, say. Worth its own status, because the
    /// caller's fix is to wait rather than to change what they sent.
    pub(super) fn conflict(message: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, Some(message.into()))
    }

    pub(super) fn internal() -> Self {
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

/// Read the node catalogue off the blocking thread pool.
///
/// `None` covers every way ComfyUI can decline to describe itself — down, too
/// old to have `/object_info`, or answering something unparseable — and every
/// caller treats that as ordinary rather than as an error.
pub(super) async fn node_catalog(url: &str) -> Option<std::sync::Arc<crate::comfyui::NodeCatalog>> {
    read_node_catalog(url, false).await
}

/// `refresh` bypasses the cache and asks ComfyUI now.
async fn read_node_catalog(
    url: &str,
    refresh: bool,
) -> Option<std::sync::Arc<crate::comfyui::NodeCatalog>> {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        let client = crate::comfyui::ComfyUiClient::new(&url);
        if refresh {
            crate::comfyui::refresh_node_catalog(&client)
        } else {
            crate::comfyui::node_catalog(&client)
        }
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!("Reading ComfyUI node info panicked: {}", e);
        None
    })
}

/// GET /api/comfyui/nodes?classes=A,B&refresh=true — what ComfyUI says its nodes take
#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct NodesQuery {
    /// Comma-separated class names to return. Omit for the whole catalogue,
    /// which on a loaded install is several megabytes.
    classes: Option<String>,
    /// Read `/object_info` from ComfyUI now instead of serving the cached
    /// copy. For after installing a model or a node pack.
    #[serde(default)]
    refresh: bool,
}

#[utoipa::path(
    get,
    path = "/api/comfyui/nodes",
    tag = "comfyui",
    summary = "Get ComfyUI node definitions",
    description = "What ComfyUI says its installed node classes take: every input's name, \
                   type, default and range, and for an enum its contents — the checkpoints, \
                   samplers, schedulers and LoRAs installed on that server. Cached in memory \
                   for five minutes and re-read when ComfyUI comes back after being down; \
                   `refresh=true` re-reads it now. Always 200: a server that is unreachable \
                   or too old to have /object_info answers `available: false`, and the \
                   console falls back to plain text boxes.",
    params(NodesQuery),
    responses(
        (status = 200, description = "Node definitions, or available=false"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_nodes(
    UState(state): UState,
    Query(query): Query<NodesQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let url = require_comfyui(&state)?;
    let Some(catalog) = read_node_catalog(&url, query.refresh).await else {
        return Ok(Json(serde_json::json!({
            "available": false,
            "nodes": {},
        })));
    };

    let nodes = match query.classes.as_deref() {
        Some(list) => {
            let mut picked = serde_json::Map::new();
            for name in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(class) = catalog.get(name) {
                    if let Ok(v) = serde_json::to_value(class) {
                        picked.insert(name.to_string(), v);
                    }
                }
            }
            serde_json::Value::Object(picked)
        }
        None => serde_json::to_value(&catalog.classes).unwrap_or_else(|_| serde_json::json!({})),
    };

    Ok(Json(serde_json::json!({
        "available": true,
        "node_count": catalog.classes.len(),
        "nodes": nodes,
    })))
}

/// What a stored workflow accepts and produces, ready to put on the wire.
///
/// The column is the answer; deriving here is the fallback for a row imported
/// before contracts existed and not yet reached by the worker's backfill pass.
/// That derivation deliberately does *not* fetch `/object_info`: a workflow list
/// must not stall for however long a dead ComfyUI takes to refuse a connection.
/// It comes back carrying `no_catalog`, and the worker replaces it with a typed
/// one within a few minutes.
fn contract_json_of(stored: Option<&str>, workflow_json: &str) -> serde_json::Value {
    if let Some(parsed) = stored.and_then(|s| serde_json::from_str::<StageContract>(s).ok()) {
        return serde_json::to_value(parsed).unwrap_or(serde_json::Value::Null);
    }
    let graph: serde_json::Value =
        serde_json::from_str(workflow_json).unwrap_or(serde_json::Value::Null);
    serde_json::to_value(StageContract::derive(&graph, None)).unwrap_or(serde_json::Value::Null)
}

/// GET /api/comfyui/workflows
#[utoipa::path(
    get,
    path = "/api/comfyui/workflows",
    tag = "comfyui",
    summary = "List workflows",
    description = "List all imported ComfyUI enhancement workflows available for use. \
                   Each carries its stage contract: what it accepts (image / video / text / \
                   none), what it produces (image / video / text), which loader fills which \
                   slot, the prompt slots a person or a describe stage fills, and the typed \
                   parameters a line can set.",
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
            let graph = serde_json::from_str::<serde_json::Value>(&wf.workflow_json).ok();
            let loaders = graph
                .as_ref()
                .map(crate::comfyui::detect_loaders)
                .unwrap_or_default();
            let takes_video = loaders
                .iter()
                .any(|l| l.kind == crate::comfyui::LoaderKind::Video);
            // Slots more than one loader claims with nothing to tell them
            // apart. Answered here, before a run, because after the run the
            // evidence is a clip that does not move.
            let warnings = graph
                .as_ref()
                .map(crate::comfyui::default_binding_warnings)
                .unwrap_or_default();
            let inputs: serde_json::Value = wf
                .inputs_json
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Array(vec![]));
            let outputs: serde_json::Value = wf
                .outputs_json
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Array(vec![]));
            let contract = contract_json_of(wf.contract_json.as_deref(), &wf.workflow_json);
            serde_json::json!({
                "id": wf.id,
                "name": wf.name,
                "description": wf.description,
                "inputs": inputs,
                "outputs": outputs,
                "loaders": loaders,
                "takes_video": takes_video,
                "warnings": warnings,
                "contract": contract,
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
    let url = require_comfyui(&state)?;

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

    // Ask ComfyUI what its nodes take, so the stored inputs carry real types
    // and ranges. A server that cannot say falls the import back to the old
    // heuristics rather than refusing it.
    let catalog = node_catalog(&url).await;

    let inputs = crate::comfyui::detect_inputs(&payload.workflow, catalog.as_deref());
    let outputs = crate::comfyui::detect_outputs(&payload.workflow);

    // What this workflow accepts and produces, so it can be a stage in a line.
    // Derived here rather than on read because the answer depends on what
    // ComfyUI said at this moment, and because a person may then correct it —
    // a correction has to have somewhere to live.
    let contract = StageContract::derive(&payload.workflow, catalog.as_deref());

    let id = uuid::Uuid::new_v4().to_string();
    let workflow_json = serde_json::to_string(&payload.workflow)
        .map_err(|_| ApiError::bad_request("The workflow is not serialisable JSON."))?;
    let inputs_json = serde_json::to_string(&inputs).map_err(|_| ApiError::internal())?;
    let outputs_json = serde_json::to_string(&outputs).map_err(|_| ApiError::internal())?;
    let contract_json = serde_json::to_string(&contract).map_err(|_| ApiError::internal())?;

    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    diesel::insert_into(comfyui_workflows::table)
        .values(NewComfyuiWorkflow {
            id: &id,
            name: &payload.name,
            description: payload.description.as_deref(),
            workflow_json: &workflow_json,
            inputs_json: Some(&inputs_json),
            outputs_json: Some(&outputs_json),
            contract_json: Some(&contract_json),
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
        "contract": contract,
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
        "contract": contract_json_of(wf.contract_json.as_deref(), &wf.workflow_json),
    })))
}

/// PUT /api/comfyui/workflows/:id/contract — correct what Phos worked out
///
/// The body is the *whole* set of corrections, not a patch: a PUT replaces
/// them. `{}` therefore means "forget what I said and derive it again", which
/// is the one thing a correction UI must never make people re-import to get.
///
/// Corrections are stored beside the derived contract rather than instead of
/// it, and take part in the next derivation instead of being applied over its
/// result — so saying "node 7 is the negative prompt" can name a text box the
/// heuristics never offered, and still holds after ComfyUI comes back and the
/// parameters can finally be typed.
#[utoipa::path(
    put,
    path = "/api/comfyui/workflows/{id}/contract",
    tag = "comfyui",
    summary = "Correct a workflow's stage contract",
    description = "Replace the corrections applied to a workflow's derived stage contract \
                   and return the contract that results. The heuristics are wrong on \
                   unusual graphs — a title that means something else, a saver Phos cannot \
                   classify, a prompt box wired somewhere unexpected — and this is how that \
                   is fixed without re-importing. Send an empty object to discard every \
                   correction and take the derivation as it stands.",
    params(("id" = String, Path, description = "Workflow ID")),
    request_body = ContractCorrections,
    responses(
        (status = 200, description = "The contract after the corrections"),
        (status = 404, description = "Workflow not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_correct_contract(
    Path(id): Path<String>,
    UState(state): UState,
    Json(corrections): Json<ContractCorrections>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let url = require_comfyui(&state)?;

    let workflow_json: String = {
        let mut conn = state
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(&id))
            .select(comfyui_workflows::workflow_json)
            .first(&mut conn)
            .map_err(|_| StatusCode::NOT_FOUND)?
    };

    // Worth the round trip here, unlike on a list: a person is waiting on the
    // answer to one workflow, and a correction should be re-derived against the
    // best information available rather than against a blind guess.
    let catalog = node_catalog(&url).await;
    let graph: serde_json::Value =
        serde_json::from_str(&workflow_json).unwrap_or(serde_json::Value::Null);
    let contract = StageContract::derive_with(&graph, catalog.as_deref(), corrections);
    let encoded =
        serde_json::to_string(&contract).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut conn = state
        .pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    diesel::update(comfyui_workflows::table.filter(comfyui_workflows::id.eq(&id)))
        .set(comfyui_workflows::contract_json.eq(&encoded))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to store contract for workflow {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(
        serde_json::to_value(contract).unwrap_or(serde_json::Value::Null),
    ))
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
        (status = 409, description = "A production line still uses this workflow"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_delete_workflow(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    // A line holds this workflow as a stage. SQLite is not enforcing the
    // foreign key, and deleting anyway would not fail the line — worse, it
    // would silently shorten it: `stages_of_line`'s join just skips the orphan,
    // and a three-stage line quietly delivers a two-stage product.
    let mut lines_using: Vec<String> = line_stages::table
        .inner_join(production_lines::table)
        .filter(line_stages::workflow_id.eq(&id))
        .select(production_lines::name)
        .distinct()
        .load(&mut conn)
        .map_err(|_| ApiError::internal())?;
    if !lines_using.is_empty() {
        lines_using.sort();
        return Err(ApiError::conflict(format!(
            "This workflow is a stage of {}. Remove it from the line, or delete the line, first.",
            lines_using
                .iter()
                .map(|n| format!("'{}'", n))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let deleted = diesel::delete(comfyui_workflows::table.filter(comfyui_workflows::id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete workflow: {}", e);
            ApiError::internal()
        })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND.into());
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
    /// Typed values for the workflow's non-text inputs, keyed
    /// `"<node_id>.<field_name>"`: `{"3.seed": 4242, "3.cfg": 6.5,
    /// "4.ckpt_name": "sd_xl_base_1.0.safetensors"}`. Each value keeps its own
    /// JSON type, which is what `text_overrides` cannot do.
    #[serde(default)]
    #[schema(value_type = Object)]
    parameters: ParameterMap,
    /// Parameters to sweep rather than pin, keyed like `parameters`. Each entry
    /// multiplies the number of tasks queued.
    ///
    /// A value is one of three spellings: a count (`{"3.seed": 4}` — four runs
    /// with four fresh seeds), an explicit list (`{"3.cfg": [4.0, 6.0, 8.0]}` —
    /// three runs), or a `VarySpec` for the long form. Both of those together is
    /// twelve runs; past 64 the request is refused.
    ///
    /// Every value is resolved here, so each task is written with its own
    /// complete parameter map and is replayable from its row.
    ///
    /// Described as a free-form object rather than as a union of the three: the
    /// generated Android client builds every model in the spec even though it
    /// calls none of these endpoints, and an untagged union of a number, an
    /// array and an object is not worth making it compile.
    #[serde(default)]
    #[schema(value_type = Object)]
    vary: VaryMap,
}

#[utoipa::path(
    post,
    path = "/api/comfyui/enhance",
    tag = "comfyui",
    summary = "Queue enhancement task",
    description = "Queue an image enhancement task using a ComfyUI workflow. Creates a background \
                   task that processes the shot's original file. A `vary` map queues one task per \
                   combination instead, and the response lists them in order.",
    request_body = EnhancePayload,
    responses(
        (status = 200, description = "Enhancement task(s) queued"),
        (status = 400, description = "Unrecognised source_mode, one this workflow cannot take, or an unusable sweep"),
        (status = 404, description = "Shot or workflow not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn comfyui_enhance(
    UState(state): UState,
    Json(mut payload): Json<EnhancePayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;

    // Reject an unreadable source mode here rather than storing it and letting
    // the worker fall back to a default the caller did not ask for.
    let explicit_mode = payload
        .source_mode
        .as_deref()
        .map(|m| m.parse::<crate::comfyui::SourceMode>())
        .transpose()
        .map_err(ApiError::bad_request)?;

    // Work out what this request actually asks for before touching the
    // database: a sweep that cannot be read is the caller's mistake, and a
    // half-queued fan-out is nobody's idea of one.
    let runs = crate::comfyui::expand(&payload.parameters, &payload.vary)
        .map_err(ApiError::bad_request)?;

    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    // Verify shot exists
    let shot_exists: bool = shots::table
        .filter(shots::id.eq(&payload.shot_id))
        .count()
        .get_result::<i64>(&mut conn)
        .map(|c| c > 0)
        .unwrap_or(false);

    if !shot_exists {
        return Err(StatusCode::NOT_FOUND.into());
    }

    // Verify the workflow exists and that it can take the mode the caller
    // picked — a `whole_video` against a graph with only image loaders (or a
    // frame against a video-only graph) would otherwise queue a task
    // guaranteed to fail.
    let stored: Option<(String, Option<String>)> = comfyui_workflows::table
        .filter(comfyui_workflows::id.eq(&payload.workflow_id))
        .select((comfyui_workflows::workflow_json, comfyui_workflows::contract_json))
        .first(&mut conn)
        .optional()
        .map_err(|_| ApiError::internal())?;
    let Some((workflow_json, stored_contract)) = stored else {
        return Err(StatusCode::NOT_FOUND.into());
    };
    // A sweep whose key names no rewritable field would queue N tasks that all
    // run the same graph while their rows claim distinct values — refuse it
    // here, where it is still one request and one message.
    if !payload.vary.is_empty() {
        if let Ok(workflow) = serde_json::from_str::<serde_json::Value>(&workflow_json) {
            crate::comfyui::check_sweep_targets(&workflow, &payload.vary)
                .map_err(ApiError::bad_request)?;
        }
    }
    if explicit_mode.is_some() {
        if let Ok(workflow) = serde_json::from_str::<serde_json::Value>(&workflow_json) {
            // A still resolves every mode to a frame, so `whole_video` is only
            // checked when the source really is a video; frame modes always
            // produce an image and are checkable regardless.
            let source_is_video = source_mime(&mut conn, &payload)
                .map(|m| m.starts_with("video/"))
                .unwrap_or(false);
            let resolved = crate::comfyui::SourceMode::resolve(
                payload.source_mode.as_deref(),
                crate::comfyui::takes_video(&workflow),
                source_is_video,
            );
            crate::comfyui::check_source_kind(&workflow, resolved.loader_kind())
                .map_err(ApiError::bad_request)?;
        }
    }

    // A describe stage run on its own still gets the instruction Phos
    // composed — the names, the date, the place — unless the caller wrote one
    // into its prompt box themselves. Compiled here, exactly as a line's
    // queue path compiles it, so the task row records the prompt that was
    // actually sent.
    let contract = crate::comfyui::runs::contract_of(
        stored_contract.as_deref(),
        &workflow_json,
    );
    crate::comfyui::runs::compile_describe_instruction(
        &mut conn,
        &payload.shot_id,
        &contract,
        &mut payload.text_overrides,
    );

    // Contract role corrections are deliberately *not* folded in here: a task
    // row records what the caller asked for, and `role:` directives written
    // into it would be persisted as provenance and shown back as prompts. The
    // dispatcher reads the contract beside the graph and applies them at run
    // time instead — which also means correcting a contract fixes tasks
    // already queued against it.
    queue_enhancement(&mut conn, &payload, &runs)
}

/// The mime type of the file a task would read: the one the payload names, or
/// the shot's original. `None` when there is nothing to say — the worker will
/// name the real problem if the file is missing.
fn source_mime(
    conn: &mut diesel::sqlite::SqliteConnection,
    payload: &EnhancePayload,
) -> Option<String> {
    let mime_sql = diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')");
    if let Some(file_id) = &payload.source_file_id {
        files::table
            .filter(
                files::id
                    .eq(file_id)
                    .and(files::shot_id.eq(&payload.shot_id)),
            )
            .select(mime_sql)
            .first::<String>(conn)
            .ok()
    } else {
        files::table
            .filter(
                files::shot_id
                    .eq(&payload.shot_id)
                    .and(files::is_original.eq(true)),
            )
            .order(files::created_at.asc())
            .select(mime_sql)
            .first::<String>(conn)
            .ok()
    }
}

fn queue_enhancement(
    conn: &mut diesel::sqlite::SqliteConnection,
    payload: &EnhancePayload,
    runs: &[crate::comfyui::ParameterMap],
) -> Result<Json<serde_json::Value>, ApiError> {
    let text_overrides_json =
        serde_json::to_string(&payload.text_overrides).unwrap_or_else(|_| "{}".to_string());

    // A single-workflow enhance is a one-stage run. Modelling it that way costs
    // one row and buys a board with one kind of entry on it rather than two,
    // and an advance pass with no special case for "a task that belongs to
    // nothing".
    let label: String = comfyui_workflows::table
        .filter(comfyui_workflows::id.eq(&payload.workflow_id))
        .select(comfyui_workflows::name)
        .first(&mut *conn)
        .unwrap_or_else(|_| "Enhancement".to_string());

    // One transaction for the whole fan-out. Four tasks that are four separate
    // rows are still one thing the user asked for; queueing two of them and
    // then failing would leave a sweep with holes in it.
    let (run_id, task_ids): (String, Vec<String>) = conn
        .transaction::<_, diesel::result::Error, _>(|conn| {
            let run_id = crate::comfyui::runs::open_run(conn, None, &payload.shot_id, &label, 1)?;
            let mut ids = Vec::with_capacity(runs.len());
            for run in runs {
                let task_id = uuid::Uuid::new_v4().to_string();
                let parameters_json =
                    serde_json::to_string(run).unwrap_or_else(|_| "{}".to_string());
                diesel::insert_into(enhancement_tasks::table)
                    .values(NewEnhancementTask {
                        id: &task_id,
                        shot_id: &payload.shot_id,
                        workflow_id: &payload.workflow_id,
                        text_overrides: Some(&text_overrides_json),
                        source_file_id: payload.source_file_id.as_deref(),
                        source_mode: payload.source_mode.as_deref(),
                        parameters: Some(&parameters_json),
                        run_id: Some(&run_id),
                        stage_idx: Some(0),
                        parent_task_id: None,
                    })
                    .execute(conn)?;
                ids.push(task_id);
            }
            Ok((run_id, ids))
        })
        .map_err(|e| {
            tracing::error!("Failed to insert enhancement task(s): {}", e);
            ApiError::internal()
        })?;

    // `id` and `status` are what a single-task caller has always read; `tasks`
    // is the ordered list a sweep produced, and `run_id` is what the board
    // groups them under.
    Ok(Json(serde_json::json!({
        "id": task_ids.first(),
        "status": "pending",
        "tasks": task_ids,
        "count": task_ids.len(),
        "run_id": run_id,
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
    let result = query_tasks(
        &mut conn,
        query.shot_id.as_ref(),
        query.cursor.as_ref(),
        limit,
    )?;

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
    /// The run this task is a step of, and which step. Every task queued since
    /// FR5 has both: a lone enhance is a one-stage run.
    run_id: Option<String>,
    stage_idx: Option<i32>,
    /// What a describe stage said. `None` on every stage that makes a file,
    /// which is all of them but one.
    text_output: Option<String>,
    main_file_id: Option<String>,
    /// Who the source shot belongs to, and the file the thumbnail shows.
    person_name: Option<String>,
    source_name: Option<String>,
}

type TaskTuple = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i32>,
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
        run_id: t.12,
        stage_idx: t.13,
        text_output: t.14,
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
        "run_id": row.run_id,
        "stage_idx": row.stage_idx,
        "text_output": row.text_output,
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
        enhancement_tasks::run_id,
        enhancement_tasks::stage_idx,
        enhancement_tasks::text_output,
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

    let mut tuples: Vec<TaskTuple> = query.load(conn).map_err(|e| {
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

    let person_ids: Vec<String> = shot_rows
        .iter()
        .filter_map(|(_, _, pid)| pid.clone())
        .collect();
    let person_names: std::collections::HashMap<String, Option<String>> = if !person_ids.is_empty()
    {
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
            let source_name =
                t.11.clone()
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
            enhancement_tasks::run_id,
            enhancement_tasks::stage_idx,
            enhancement_tasks::text_output,
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
    // the settle clock, the backoff, the old prompt id, and the timestamp a
    // cancellation stamped on it — a pending task is not "completed at" anything.
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&id)))
        .set((
            enhancement_tasks::status.eq("pending"),
            enhancement_tasks::error_message.eq(None::<String>),
            enhancement_tasks::retry_count.eq(0),
            enhancement_tasks::settle_until.eq(None::<String>),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
            enhancement_tasks::comfyui_prompt_id.eq(None::<String>),
            enhancement_tasks::completed_at.eq(None::<String>),
        ))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to retry task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Its run is walking again. Without this the run stays `failed`, and the
    // advance pass — which only looks at running runs — would never queue the
    // stage after this one when it finally lands.
    reopen_run_of(&mut conn, &id);

    Ok(Json(serde_json::json!({"status": "pending"})))
}

/// Put a task's run back into `running`.
///
/// A single task retried by hand is a run resumed, and the advance pass only
/// looks at running runs. Nothing happens for a run that is already walking.
fn reopen_run_of(conn: &mut diesel::SqliteConnection, task_id: &str) {
    let run_id: Option<Option<String>> = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq(task_id))
        .select(enhancement_tasks::run_id)
        .first(conn)
        .optional()
        .unwrap_or(None);
    let Some(Some(run_id)) = run_id else { return };
    let _ = diesel::update(crate::schema::runs::table.filter(crate::schema::runs::id.eq(&run_id)))
        .set((
            crate::schema::runs::status.eq(crate::comfyui::RunState::Running.as_str()),
            crate::schema::runs::error_message.eq(None::<String>),
            crate::schema::runs::finished_at.eq(None::<String>),
        ))
        .execute(conn);
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

    // Claim the row first, in one conditional update, and only then talk to
    // ComfyUI. Two races live here otherwise: the worker can overwrite a
    // `cancelled` it never saw with `queued` or `completed`, and this handler
    // can overwrite a task that completed while the remote calls were in
    // flight. The worker's own writes all refuse to touch a cancelled row, so
    // once this update lands the task is ours.
    //
    // The connection is dropped before the remote calls: the pool holds two,
    // and holding one for up to thirty seconds of ComfyUI timeouts would let two
    // cancellations block every other request on this library.
    let prompt_id: Option<String> = {
        let mut conn = state
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let now = chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let claimed = diesel::update(
            enhancement_tasks::table.filter(enhancement_tasks::id.eq(&id).and(
                enhancement_tasks::status.ne_all(&[
                    "completed",
                    "failed",
                    crate::comfyui::STATUS_CANCELLED,
                ]),
            )),
        )
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

        if claimed == 0 {
            // Either no such task, or nothing left to stop.
            let exists: i64 = enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(&id))
                .count()
                .get_result(&mut conn)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Err(if exists == 0 {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            });
        }

        // Read the prompt id *after* the claim: the worker cannot attach one to a
        // cancelled row, so what we see now is what ComfyUI has.
        enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&id))
            .select(enhancement_tasks::comfyui_prompt_id)
            .first(&mut conn)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    // A task that never reached ComfyUI has no prompt id and is already done.
    if let Some(prompt_id) = prompt_id {
        let _ = tokio::task::spawn_blocking(move || cancel_on_comfyui(&url, &prompt_id)).await;
    }

    Ok(Json(
        serde_json::json!({"status": crate::comfyui::STATUS_CANCELLED}),
    ))
}

/// Stop a prompt on ComfyUI, wherever it is.
///
/// Pending first, then running: a prompt checked for "running" *before* it is
/// deleted from the pending queue can start executing in between, at which
/// point the delete is a no-op and the interrupt was never sent. Deleting
/// first closes that gap — anything still there afterwards is running, and
/// `/interrupt` stops whatever is executing, so it is only fired when that is
/// our prompt and not somebody else's job.
fn cancel_on_comfyui(url: &str, prompt_id: &str) {
    let client = crate::comfyui::ComfyUiClient::new(url);
    if let Err(e) = client.delete_queued(prompt_id) {
        tracing::warn!("Dropping prompt {} from the queue failed: {}", prompt_id, e);
    }
    match client.is_prompt_running(prompt_id) {
        Ok(true) => {
            if let Err(e) = client.interrupt() {
                tracing::warn!("Interrupt for prompt {} failed: {}", prompt_id, e);
            }
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("Could not read ComfyUI queue: {}", e),
    }
}

// ===== Shot Generations =====

/// GET /api/comfyui/generations/:shot_id — get generation history for a shot
#[utoipa::path(
    get,
    path = "/api/comfyui/generations/{shot_id}",
    tag = "comfyui",
    summary = "Get shot generation history",
    description = "Returns the workflow, text overrides and typed parameters used to generate each non-original file for a shot.",
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

    // file id, workflow id, text overrides, provenance manifest
    type GenerationRow = (String, Option<String>, Option<String>, Option<String>);
    let rows: Vec<GenerationRow> = files::table
        .filter(files::shot_id.eq(&shot_id))
        .filter(files::source_workflow_id.is_not_null())
        .select((
            files::id,
            files::source_workflow_id,
            files::source_text_overrides,
            files::manifest_json,
        ))
        .order(files::created_at.desc())
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to query generations: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let generations: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(file_id, workflow_id, overrides_str, manifest_str)| {
            let overrides: serde_json::Value = overrides_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::json!({}));
            // The typed values that made this file live in its provenance
            // manifest; a file written before they existed reads back `{}`,
            // which is also what a run that set nothing recorded.
            let parameters = manifest_str
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .map(|m| m["parameters"].clone())
                .filter(serde_json::Value::is_object)
                .unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "file_id": file_id,
                "workflow_id": workflow_id,
                "text_overrides": overrides,
                "parameters": parameters,
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
    /// The preset's typed values, keyed like a task's `parameters`. A preset
    /// that can pin a prompt but not a seed or a step count is half a preset.
    #[serde(default)]
    #[schema(value_type = Object)]
    parameters: ParameterMap,
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
            // A preset saved before FR4 has no typed half; it reads back as an
            // empty map rather than as a missing key, so the console does not
            // have to know which era it came from.
            let parameters: serde_json::Value = p
                .parameters
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "text_overrides": overrides,
                "parameters": parameters,
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
    let parameters_json =
        serde_json::to_string(&payload.parameters).unwrap_or_else(|_| "{}".to_string());
    let sort_order = payload.sort_order.unwrap_or(0) as i32;

    diesel::insert_into(workflow_presets::table)
        .values(NewWorkflowPreset {
            id: &id,
            workflow_id: &workflow_id,
            name: &payload.name,
            text_overrides: &overrides_json,
            sort_order: Some(sort_order),
            parameters: Some(&parameters_json),
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
        "parameters": payload.parameters,
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
    let parameters_json =
        serde_json::to_string(&payload.parameters).unwrap_or_else(|_| "{}".to_string());
    let sort_order = payload.sort_order.unwrap_or(0) as i32;

    let updated = diesel::update(
        workflow_presets::table
            .filter(workflow_presets::id.eq(&preset_id))
            .filter(workflow_presets::workflow_id.eq(&workflow_id)),
    )
    .set((
        workflow_presets::name.eq(&payload.name),
        workflow_presets::text_overrides.eq(&overrides_json),
        workflow_presets::parameters.eq(&parameters_json),
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
        "parameters": payload.parameters,
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

    // A step of a run that is still walking is not spare: the stage after it
    // reads what it made, and the row is what says that continuation already
    // happened. Deleting it would strand the run halfway.
    let live_run: i64 = enhancement_tasks::table
        .inner_join(
            crate::schema::runs::table.on(crate::schema::runs::id
                .nullable()
                .eq(enhancement_tasks::run_id)),
        )
        .filter(
            enhancement_tasks::id
                .eq(&id)
                .and(crate::schema::runs::status.eq(crate::comfyui::RunState::Running.as_str())),
        )
        .count()
        .get_result(&mut conn)
        .unwrap_or(0);
    if live_run > 0 {
        return Err(StatusCode::CONFLICT);
    }

    diesel::delete(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&id)))
        .execute(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to delete task: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::connection::SimpleConnection;

    /// A migrated library with one workflow to hang presets off.
    fn library() -> (tempfile::TempDir, diesel::SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        conn.batch_execute(
            "INSERT INTO comfyui_workflows (id, name, workflow_json)
             VALUES ('wf-1', 'Portrait', '{}');",
        )
        .unwrap();
        (dir, conn)
    }

    #[test]
    fn a_preset_saves_a_seed_and_a_step_count_alongside_its_prompts() {
        let (_dir, mut conn) = library();
        let parameters: ParameterMap = serde_json::from_value(serde_json::json!({
            "3.seed": 4242, "3.steps": 28, "4.ckpt_name": "sd_xl_base_1.0.safetensors"
        }))
        .unwrap();

        diesel::insert_into(workflow_presets::table)
            .values(NewWorkflowPreset {
                id: "preset-1",
                workflow_id: "wf-1",
                name: "Golden hour",
                text_overrides: r#"{"6.text":"a lighthouse at dusk"}"#,
                sort_order: Some(0),
                parameters: Some(&serde_json::to_string(&parameters).unwrap()),
            })
            .execute(&mut conn)
            .unwrap();

        let stored: crate::models::WorkflowPreset = workflow_presets::table
            .filter(workflow_presets::id.eq("preset-1"))
            .first(&mut conn)
            .unwrap();
        let read_back: ParameterMap =
            serde_json::from_str(stored.parameters.as_deref().unwrap()).unwrap();
        assert_eq!(read_back, parameters);
        assert_eq!(read_back["3.seed"], serde_json::json!(4242));
        assert!(read_back["3.seed"].is_i64(), "a preset's seed is a number");
        // The prompt half is untouched by any of this.
        assert_eq!(
            stored.text_overrides,
            r#"{"6.text":"a lighthouse at dusk"}"#
        );
    }

    #[test]
    fn a_preset_saved_before_the_column_existed_still_loads() {
        let (_dir, mut conn) = library();
        conn.batch_execute(
            "INSERT INTO workflow_presets (id, workflow_id, name, text_overrides)
             VALUES ('preset-old', 'wf-1', 'Old', '{\"6.text\":\"a photograph\"}');",
        )
        .unwrap();

        let stored: crate::models::WorkflowPreset = workflow_presets::table
            .filter(workflow_presets::id.eq("preset-old"))
            .first(&mut conn)
            .unwrap();
        assert_eq!(stored.parameters, None);
        // Which the list endpoint hands the console as an empty map, so it does
        // not have to know which era a preset came from.
        let as_served: serde_json::Value = stored
            .parameters
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::json!({}));
        assert_eq!(as_served, serde_json::json!({}));
    }
}
