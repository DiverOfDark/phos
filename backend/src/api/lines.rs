//! Production lines, and the runs that walk them.
//!
//! A *line* is a chain of workflows — photo → 5s clip → interpolate → 4K
//! upscale — stored in the library's own `.phos.db` like everything else Phos
//! knows. A *run* is one line applied to one shot.
//!
//! # Two things are deliberate about the shapes here
//!
//! **A line is rejected when it is drawn, not when it runs.** Every join is
//! checked with [`crate::comfyui::Accepts::admits`] on `POST` and `PUT`, so a
//! four-hour chain whose third stage cannot eat the second's output comes back
//! as a 400 naming the stage, rather than as a failure two hours in. It is
//! checked again when a run starts, because a workflow can be re-imported or
//! its contract corrected in between, and a third time at dispatch.
//!
//! **`POST /runs` is shaped for the batch it will become.** FR7 replaces
//! `shot_id` with a query and adds a cursor, so the handler already resolves a
//! *set* of shots and answers with a list of runs — today that set has one
//! member. The single-shot fields stay in the response so nothing has to be
//! rewritten twice.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use utoipa::ToSchema;

use crate::comfyui::line::{self, RunState};
use crate::comfyui::runs::{contract_of, StageRow, SuppliedByStage};
use crate::comfyui::{ParameterMap, StageTyping, VaryMap};
use crate::models::{NewLineStage, NewProductionLine};
use crate::schema::{
    comfyui_workflows, enhancement_tasks, files, line_stages, people, production_lines, runs, shots,
};

use super::comfyui::{require_comfyui, ApiError};
use super::UState;

// ===== Payloads =============================================================

/// One step of a line, as a client sends it.
///
/// Every field but `workflow_id` mirrors something a single-workflow enhance
/// already sends, and is stored and dispatched through exactly the same code —
/// which is why a stage of a line and an ad-hoc run cannot drift apart.
#[derive(Deserialize, ToSchema)]
pub(super) struct LineStagePayload {
    pub(super) workflow_id: String,
    /// Prompt bindings, plus the `role:<node_id>` directives the loader binder
    /// reads.
    #[serde(default)]
    pub(super) text_overrides: HashMap<String, String>,
    /// Typed values for this stage's non-text inputs, keyed
    /// `"<node_id>.<field_name>"`.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub(super) parameters: ParameterMap,
    /// Parameters to sweep rather than pin. Expanded once per continuation, so
    /// a count of 4 here means four takes and four independent runners through
    /// every stage after this one.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub(super) vary: VaryMap,
    /// Which part of an upstream video this stage consumes: `first_frame`,
    /// `last_frame`, `at_time:<ms>`, `keyframe:<n>` or `whole_video`. Omit it
    /// to let the graph decide.
    pub(super) source_mode: Option<String>,
    /// Keep this stage's output once the run completes. The last stage's output
    /// is the product and is kept regardless, and so are the takes of a stage
    /// that holds for review.
    #[serde(default)]
    pub(super) keep_output: bool,
    /// Keys this stage deliberately leaves open — the *exposed* disposition.
    /// Starting a run may supply values for exactly these, and only these.
    #[serde(default)]
    pub(super) exposed: Vec<String>,
    /// Park the run after this stage and ask which of its takes go on. Refused
    /// on the last stage, whose output is the product.
    #[serde(default)]
    pub(super) hold_for_review: bool,
}

#[derive(Deserialize, ToSchema)]
pub(super) struct LinePayload {
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) stages: Vec<LineStagePayload>,
}

impl LineStagePayload {
    /// One stage of an existing line, copied — what Duplicate and the
    /// promote-from-history flow both build a payload out of.
    pub(super) fn from_row(row: &StageRow) -> Self {
        LineStagePayload {
            workflow_id: row.workflow_id.clone(),
            text_overrides: serde_json::from_str(&row.text_overrides).unwrap_or_default(),
            parameters: serde_json::from_str(&row.parameters).unwrap_or_default(),
            vary: serde_json::from_str(&row.vary).unwrap_or_default(),
            source_mode: row.source_mode.clone(),
            keep_output: row.keep_output,
            exposed: row.exposed.clone(),
            hold_for_review: row.hold_for_review,
        }
    }

    pub(super) fn new(
        workflow_id: String,
        text_overrides: HashMap<String, String>,
        parameters: ParameterMap,
        source_mode: Option<String>,
        exposed: Vec<String>,
    ) -> Self {
        LineStagePayload {
            workflow_id,
            text_overrides,
            parameters,
            vary: VaryMap::new(),
            source_mode,
            keep_output: false,
            exposed,
            hold_for_review: false,
        }
    }
}

impl LinePayload {
    pub(super) fn new(
        name: String,
        description: Option<String>,
        stages: Vec<LineStagePayload>,
    ) -> Self {
        LinePayload {
            name,
            description,
            stages,
        }
    }

    pub(super) fn name(&self) -> &str {
        self.name.trim()
    }

    pub(super) fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Each stage's workflow and the part of an upstream video it consumes —
    /// the two things working out a draft's joins needs, without exposing the
    /// payload's fields to the rest of the API.
    pub(super) fn stage_specs(&self) -> Vec<(&str, Option<&str>)> {
        self.stages
            .iter()
            .map(|s| (s.workflow_id.as_str(), s.source_mode.as_deref()))
            .collect()
    }
}

/// One join of a *draft* line, worked out the same way a stored one's is.
///
/// A stage the editor has just added has no join yet — it has never been read
/// back from the database — so the validate endpoint answers with them, and the
/// editor draws its connectors from the same function that drew them before the
/// edit. There is no second derivation for "not saved yet".
pub(super) fn draft_stages_json(
    conn: &mut SqliteConnection,
    specs: &[(&str, Option<&str>)],
) -> Vec<serde_json::Value> {
    let mut out = Vec::with_capacity(specs.len());
    let mut above: Vec<StageTyping> = Vec::with_capacity(specs.len());
    for (idx, (workflow_id, source_mode)) in specs.iter().enumerate() {
        let row: Option<(String, Option<String>, String)> = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(workflow_id))
            .select((
                comfyui_workflows::name,
                comfyui_workflows::contract_json,
                comfyui_workflows::workflow_json,
            ))
            .first(conn)
            .optional()
            .unwrap_or(None);
        let Some((name, contract_json, workflow_json)) = row else {
            // A stage naming a workflow this library does not have. The
            // refusal says so; there is nothing to draw for it, and nothing is
            // known about what it would have handed on.
            out.push(serde_json::json!({ "stage_idx": idx }));
            above.clear();
            continue;
        };
        let contract = contract_of(contract_json.as_deref(), &workflow_json);
        let takes_video = graph_takes_video(&workflow_json);
        let takes_image = graph_takes_image(&workflow_json);
        above.push(StageTyping {
            stage_idx: idx as i32,
            name,
            accepts: contract.accepts,
            produces: contract.produces,
            source_mode: source_mode.map(str::to_string),
            takes_video,
        });
        let carried = line::carried_into(&above, above.len() - 1);
        let handoff = carried.map(|c| {
            let h = crate::comfyui::editor::handoff(
                c.produced,
                &contract,
                takes_video,
                takes_image,
                *source_mode,
            );
            serde_json::json!({
                "carries": h.carries,
                "resolved": h.resolved,
                "modes": h.modes,
                "roles": h.roles.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
                "is_a_question": h.is_a_question(),
                // What the stage below actually reads, which is the clip
                // itself unless the connector takes a frame out of it. The
                // validator admits on this, so the connector says it.
                "reads": c.reads,
            })
        });
        out.push(serde_json::json!({
            "stage_idx": idx,
            "accepts": contract.accepts,
            "produces": contract.produces,
            "handoff": handoff,
        }));
    }
    out
}

/// What starts a run.
///
/// FR7 adds a query in place of `shot_id` and a cursor beside it; the handler
/// is already written around "which shots does this ask for", so that is a new
/// field rather than a new endpoint.
#[derive(Deserialize, ToSchema)]
pub(super) struct StartRunPayload {
    line_id: String,
    shot_id: String,
    /// Values for the keys each stage left open, keyed by stage index as a
    /// string. A key a stage did not expose is refused by name rather than
    /// ignored.
    #[serde(default)]
    #[schema(value_type = Object)]
    stage_values: SuppliedByStage,
}

/// A `production_lines` row: name, description, and the two timestamps.
type LineRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// The same row without its id, which the caller already has.
type LineDetail = (String, Option<String>, Option<String>, Option<String>);

/// One task of a run, as the board reads it: run, stage, status, workflow, and
/// the file it read.
type RunTaskRow = (String, Option<i32>, String, String, Option<String>);

// ===== Lines: read ==========================================================

/// A line's stages, with each workflow's contract folded in — what the editor
/// draws and what validation reads.
///
/// `upstream` is what the stage before it produces, which is what turns a
/// stage into a *join*: the connector the editor draws between two rows states
/// what travels along it and whether there is anything to decide. Worked out
/// here rather than in the browser for the same reason validity is — the
/// dispatcher's rule lives on this side, and a second copy of it would be a
/// second copy to disagree with.
pub(super) fn stage_json(
    stage: &StageRow,
    upstream: Option<line::Carried>,
    takes_video: bool,
    takes_image: bool,
) -> serde_json::Value {
    let handoff = upstream.map(|c| {
        let h = crate::comfyui::editor::handoff(
            c.produced,
            &stage.contract,
            takes_video,
            takes_image,
            stage.source_mode.as_deref(),
        );
        serde_json::json!({
            "carries": h.carries,
            "resolved": h.resolved,
            "modes": h.modes,
            "roles": h.roles.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
            "is_a_question": h.is_a_question(),
            // What the stage below actually reads, which is the clip itself
            // unless the connector takes a frame out of it. The validator
            // admits on this, so the connector says it.
            "reads": c.reads,
        })
    });
    serde_json::json!({
        "stage_idx": stage.stage_idx,
        "workflow_id": stage.workflow_id,
        "workflow_name": stage.workflow_name,
        "accepts": stage.contract.accepts,
        "produces": stage.contract.produces,
        "keep_output": stage.keep_output,
        "hold_for_review": stage.hold_for_review,
        "source_mode": stage.source_mode,
        "exposed": stage.exposed,
        "handoff": handoff,
        "text_overrides": serde_json::from_str::<serde_json::Value>(&stage.text_overrides)
            .unwrap_or_else(|_| serde_json::json!({})),
        "parameters": serde_json::from_str::<serde_json::Value>(&stage.parameters)
            .unwrap_or_else(|_| serde_json::json!({})),
        "vary": serde_json::from_str::<serde_json::Value>(&stage.vary)
            .unwrap_or_else(|_| serde_json::json!({})),
    })
}

/// Every stage, each with the join above it.
///
/// The join is what the *media flow* carries into that stage, not simply what
/// the stage above produced: a stage that makes no file is transparent to it,
/// so the connector under a describe stage still says `image`. That rule is
/// [`line::carried_into`] — the one the validator admits on and the dispatcher
/// sends on — and it is asked here and nowhere else.
pub(super) fn stages_json(stages: &[StageRow]) -> Vec<serde_json::Value> {
    let typings: Vec<StageTyping> = stages.iter().map(StageRow::typing).collect();
    stages
        .iter()
        .enumerate()
        .map(|(i, s)| stage_json(s, line::carried_into(&typings, i), s.takes_video, s.takes_image))
        .collect()
}

/// Does this graph have a loader that can read a clip?
///
/// Read off the graph rather than off the contract, because that is where the
/// dispatcher reads it — see [`crate::comfyui::editor::handoff`].
pub(super) fn graph_takes_video(workflow_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(workflow_json)
        .ok()
        .map(|graph| crate::comfyui::takes_video(&graph))
        .unwrap_or(false)
}

/// Its still-reading half, asked the same way for the same reason.
pub(super) fn graph_takes_image(workflow_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(workflow_json)
        .ok()
        .map(|graph| crate::comfyui::takes_image(&graph))
        .unwrap_or(false)
}

pub(super) fn line_json(
    id: &str,
    name: &str,
    description: Option<&str>,
    created_at: Option<&str>,
    updated_at: Option<&str>,
    stages: &[StageRow],
    live_runs: i64,
) -> serde_json::Value {
    let typings: Vec<StageTyping> = stages.iter().map(StageRow::typing).collect();
    // Asked on every read, not just on save: a workflow can be re-imported or
    // its contract corrected long after the line was drawn, and the editor
    // should say so before somebody starts a four-hour run.
    let error = line::validate_chain(&typings).err();
    serde_json::json!({
        "id": id,
        "name": name,
        "description": description,
        "created_at": created_at,
        "updated_at": updated_at,
        "stage_count": stages.len(),
        "stages": stages_json(stages),
        "valid": error.is_none(),
        "error": error.as_ref().map(|e| e.message.clone()),
        "error_stage_idx": error.as_ref().map(|e| e.stage_idx),
        // Editing is refused while a run of this line is walking it. Said on
        // every read rather than only in the 409, so the editor can lock
        // itself and offer Duplicate instead of letting somebody type for ten
        // minutes and then be told no.
        "live_runs": live_runs,
        "editable": live_runs == 0,
    })
}

#[utoipa::path(
    get,
    path = "/api/comfyui/lines",
    tag = "comfyui",
    summary = "List production lines",
    description = "Every line in this library, with its stages and whether the chain still \
                   type-checks against the workflows' current contracts.",
    responses(
        (status = 200, description = "List of lines"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn list_lines(UState(state): UState) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let rows: Vec<LineRow> = production_lines::table
        .order(production_lines::created_at.asc())
        .select((
            production_lines::id,
            production_lines::name,
            production_lines::description,
            production_lines::created_at,
            production_lines::updated_at,
        ))
        .load(&mut conn)
        .map_err(|_| ApiError::internal())?;

    let mut items = Vec::with_capacity(rows.len());
    for (id, name, description, created_at, updated_at) in rows {
        let stages = crate::comfyui::runs::stages_of_line(&mut conn, &id)
            .map_err(|_| ApiError::internal())?;
        let live = live_run_count(&mut conn, &id);
        items.push(line_json(
            &id,
            &name,
            description.as_deref(),
            created_at.as_deref(),
            updated_at.as_deref(),
            &stages,
            live,
        ));
    }
    Ok(Json(serde_json::json!({ "items": items })))
}

#[utoipa::path(
    get,
    path = "/api/comfyui/lines/{id}",
    tag = "comfyui",
    summary = "Get a production line",
    params(("id" = String, Path, description = "Line ID")),
    responses(
        (status = 200, description = "The line and its stages"),
        (status = 404, description = "Line not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn get_line(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let row: Option<LineDetail> = production_lines::table
        .filter(production_lines::id.eq(&id))
        .select((
            production_lines::name,
            production_lines::description,
            production_lines::created_at,
            production_lines::updated_at,
        ))
        .first(&mut conn)
        .optional()
        .map_err(|_| ApiError::internal())?;
    let Some((name, description, created_at, updated_at)) = row else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    let stages =
        crate::comfyui::runs::stages_of_line(&mut conn, &id).map_err(|_| ApiError::internal())?;
    let live = live_run_count(&mut conn, &id);
    Ok(Json(line_json(
        &id,
        &name,
        description.as_deref(),
        created_at.as_deref(),
        updated_at.as_deref(),
        &stages,
        live,
    )))
}

// ===== Lines: write =========================================================

/// Everything that has to be true before a line is worth storing.
///
/// The order matters to the message a person gets: a typo in a workflow id is
/// reported as a missing workflow, not as an unfittable chain.
pub(super) fn check_payload(
    conn: &mut SqliteConnection,
    payload: &LinePayload,
) -> Result<Vec<StageTyping>, ApiError> {
    if payload.name.trim().is_empty() {
        return Err(ApiError::bad_request("A line needs a name."));
    }
    if payload.stages.is_empty() {
        return Err(ApiError::bad_request("A line needs at least one stage."));
    }

    let mut typings = Vec::with_capacity(payload.stages.len());
    // Fan-out multiplies down a line: every take at stage k expands stage k+1's
    // sweep again. Each stage's own sweep is capped by `expand`, but it is the
    // running product that says how many tasks one request can become.
    let mut branches: usize = 1;
    for (idx, stage) in payload.stages.iter().enumerate() {
        let row: Option<(String, Option<String>, String)> = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(&stage.workflow_id))
            .select((
                comfyui_workflows::name,
                comfyui_workflows::contract_json,
                comfyui_workflows::workflow_json,
            ))
            .first(conn)
            .optional()
            .map_err(|_| ApiError::internal())?;
        let Some((name, contract_json, workflow_json)) = row else {
            return Err(ApiError::bad_request(format!(
                "Stage {} names workflow {}, which is not in this library.",
                idx + 1,
                stage.workflow_id
            )));
        };

        if let Some(mode) = stage.source_mode.as_deref() {
            if mode.parse::<crate::comfyui::SourceMode>().is_err() {
                return Err(ApiError::bad_request(format!(
                    "Stage {}: '{}' is not a source mode. Try first_frame, last_frame, \
                     at_time:<ms>, keyframe:<n> or whole_video.",
                    idx + 1,
                    mode
                )));
            }
        }

        // A sweep that cannot be read is the caller's mistake, and finding out
        // at the moment the stage is queued would be finding out too late.
        let takes = crate::comfyui::expand(&stage.parameters, &stage.vary)
            .map_err(|e| ApiError::bad_request(format!("Stage {}: {}", idx + 1, e)))?
            .len();
        branches = branches.saturating_mul(takes);
        if branches > crate::comfyui::params::MAX_FANOUT {
            return Err(ApiError::bad_request(format!(
                "Stage {}: the line's sweeps multiply to {} takes by this stage, more than \
                 the {} one run may become.",
                idx + 1,
                branches,
                crate::comfyui::params::MAX_FANOUT
            )));
        }

        // A key is pinned, swept or asked for — one of the three, never two.
        // Sweeping a value the line also asks for is two people deciding it.
        if let Some(key) = stage.exposed.iter().find(|k| stage.vary.contains_key(*k)) {
            return Err(ApiError::bad_request(format!(
                "Stage {}: {} is both swept and asked for. A setting is pinned, \
                 varied or exposed — one of the three.",
                idx + 1,
                key
            )));
        }

        // A hold point exists to stop the line before the next stage spends an
        // hour on takes nobody chose. On the last stage there is no next stage:
        // its output is the product, so a verdict there could only ever mean
        // "delete some of what you just made", which is what the library is
        // for. Refused when the line is drawn rather than discovered by a run
        // that parks and can never be released.
        if stage.hold_for_review && idx + 1 == payload.stages.len() {
            return Err(ApiError::bad_request(format!(
                "Stage {} is the last one, and its output is the product. A hold \
                 for review goes on a stage that has something after it.",
                idx + 1
            )));
        }

        let contract = contract_of(contract_json.as_deref(), &workflow_json);
        typings.push(StageTyping {
            stage_idx: idx as i32,
            name,
            accepts: contract.accepts,
            produces: contract.produces,
            source_mode: stage.source_mode.clone(),
            takes_video: graph_takes_video(&workflow_json),
        });
    }

    line::validate_chain(&typings).map_err(|e| ApiError::bad_request(e.message))?;
    Ok(typings)
}

/// Write a line's stages. The caller has already emptied the old ones.
pub(super) fn insert_stages(
    conn: &mut SqliteConnection,
    line_id: &str,
    payload: &LinePayload,
) -> Result<(), diesel::result::Error> {
    for (idx, stage) in payload.stages.iter().enumerate() {
        let text_overrides = serde_json::to_string(&stage.text_overrides).unwrap_or_default();
        let parameters = serde_json::to_string(&stage.parameters).unwrap_or_default();
        let vary = serde_json::to_string(&stage.vary).unwrap_or_default();
        let exposed = serde_json::to_string(&stage.exposed).unwrap_or_default();
        diesel::insert_into(line_stages::table)
            .values(NewLineStage {
                id: &uuid::Uuid::new_v4().to_string(),
                line_id,
                stage_idx: idx as i32,
                workflow_id: &stage.workflow_id,
                text_overrides: Some(&text_overrides),
                parameters: Some(&parameters),
                vary: Some(&vary),
                source_mode: stage.source_mode.as_deref(),
                keep_output: stage.keep_output,
                exposed: Some(&exposed),
                hold_for_review: stage.hold_for_review,
            })
            .execute(conn)?;
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/comfyui/lines",
    tag = "comfyui",
    summary = "Create a production line",
    description = "Store a chain of workflows. Rejected with a message naming the stage if any \
                   stage cannot consume what the one before it produces.",
    request_body = LinePayload,
    responses(
        (status = 200, description = "Line created"),
        (status = 400, description = "The chain does not fit together, or a stage is unusable"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn create_line(
    UState(state): UState,
    Json(payload): Json<LinePayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    check_payload(&mut conn, &payload)?;

    let id = uuid::Uuid::new_v4().to_string();
    conn.transaction::<_, diesel::result::Error, _>(|conn| {
        diesel::insert_into(production_lines::table)
            .values(NewProductionLine {
                id: &id,
                name: payload.name.trim(),
                description: payload.description.as_deref(),
            })
            .execute(conn)?;
        insert_stages(conn, &id, &payload)
    })
    .map_err(|e| {
        tracing::error!("Failed to create line: {}", e);
        ApiError::internal()
    })?;

    let stages =
        crate::comfyui::runs::stages_of_line(&mut conn, &id).map_err(|_| ApiError::internal())?;
    Ok(Json(line_json(
        &id,
        payload.name.trim(),
        payload.description.as_deref(),
        None,
        None,
        &stages,
        0,
    )))
}

/// Is anything still walking this line?
///
/// A live run reads its stages as it goes, so editing or deleting one under it
/// would change what the rest of the run does — or leave it with a stage that
/// no longer exists. Refused rather than versioned: v1 lines are small and
/// remaking one is cheap, and FR5b can do better once there is an editor to do
/// it in.
///
/// A **held** run counts. It is stopped, but only until somebody says which
/// takes go on — and the moment they do, it reads the stages after the hold. A
/// line edited under it is exactly the change that was refused for a running
/// one, arriving with a delay.
pub(super) fn live_run_count(conn: &mut SqliteConnection, line_id: &str) -> i64 {
    runs::table
        .filter(
            runs::line_id
                .eq(line_id)
                .and(runs::status.eq_any(RunState::live())),
        )
        .count()
        .get_result::<i64>(conn)
        .unwrap_or(0)
}

fn has_live_runs(conn: &mut SqliteConnection, line_id: &str) -> bool {
    live_run_count(conn, line_id) > 0
}

/// Why a line mutation's transaction rolled back: the database said no, or the
/// live-run check did.
///
/// The check runs *inside* the same immediate transaction as the write — asked
/// before it, a run could start in the gap and find its line rewritten under
/// its feet mid-walk.
enum MutateError {
    Db(diesel::result::Error),
    LiveRuns,
}

impl From<diesel::result::Error> for MutateError {
    fn from(e: diesel::result::Error) -> Self {
        MutateError::Db(e)
    }
}

fn live_runs_conflict(action: &str) -> ApiError {
    ApiError::conflict(format!(
        "A run of this line is still in flight. Wait for it, or cancel it, before {}.",
        action
    ))
}

#[utoipa::path(
    put,
    path = "/api/comfyui/lines/{id}",
    tag = "comfyui",
    summary = "Replace a production line",
    description = "Replace a line's name, description and whole stage list. Refused while a run \
                   of this line is still in flight.",
    params(("id" = String, Path, description = "Line ID")),
    request_body = LinePayload,
    responses(
        (status = 200, description = "Line replaced"),
        (status = 400, description = "The chain does not fit together"),
        (status = 404, description = "Line not found"),
        (status = 409, description = "A run of this line is still in flight"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn update_line(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<LinePayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let exists: i64 = production_lines::table
        .filter(production_lines::id.eq(&id))
        .count()
        .get_result(&mut conn)
        .map_err(|_| ApiError::internal())?;
    if exists == 0 {
        return Err(StatusCode::NOT_FOUND.into());
    }
    check_payload(&mut conn, &payload)?;

    let now = chrono::Utc::now()
        .naive_utc()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    conn.immediate_transaction::<_, MutateError, _>(|conn| {
        if has_live_runs(conn, &id) {
            return Err(MutateError::LiveRuns);
        }
        diesel::update(production_lines::table.filter(production_lines::id.eq(&id)))
            .set((
                production_lines::name.eq(payload.name.trim()),
                production_lines::description.eq(payload.description.as_deref()),
                production_lines::updated_at.eq(&now),
            ))
            .execute(conn)?;
        diesel::delete(line_stages::table.filter(line_stages::line_id.eq(&id))).execute(conn)?;
        insert_stages(conn, &id, &payload)?;
        Ok(())
    })
    .map_err(|e| match e {
        MutateError::LiveRuns => live_runs_conflict("editing"),
        MutateError::Db(e) => {
            tracing::error!("Failed to update line: {}", e);
            ApiError::internal()
        }
    })?;

    let stages =
        crate::comfyui::runs::stages_of_line(&mut conn, &id).map_err(|_| ApiError::internal())?;
    Ok(Json(line_json(
        &id,
        payload.name.trim(),
        payload.description.as_deref(),
        None,
        Some(&now),
        &stages,
        0,
    )))
}

#[utoipa::path(
    delete,
    path = "/api/comfyui/lines/{id}",
    tag = "comfyui",
    summary = "Delete a production line",
    params(("id" = String, Path, description = "Line ID")),
    responses(
        (status = 200, description = "Line deleted"),
        (status = 404, description = "Line not found"),
        (status = 409, description = "A run of this line is still in flight"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn delete_line(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let deleted = conn
        .immediate_transaction::<_, MutateError, _>(|conn| {
            if has_live_runs(conn, &id) {
                return Err(MutateError::LiveRuns);
            }
            diesel::delete(line_stages::table.filter(line_stages::line_id.eq(&id)))
                .execute(conn)?;
            // Finished runs keep their snapshotted label and stage count, so the
            // board still reads correctly after the line they walked is gone.
            diesel::update(runs::table.filter(runs::line_id.eq(&id)))
                .set(runs::line_id.eq(None::<String>))
                .execute(conn)?;
            Ok(
                diesel::delete(production_lines::table.filter(production_lines::id.eq(&id)))
                    .execute(conn)?,
            )
        })
        .map_err(|e| match e {
            MutateError::LiveRuns => live_runs_conflict("deleting"),
            MutateError::Db(_) => ApiError::internal(),
        })?;

    if deleted == 0 {
        return Err(StatusCode::NOT_FOUND.into());
    }
    Ok(Json(serde_json::json!({"status": "ok"})))
}

// ===== Runs =================================================================

#[utoipa::path(
    post,
    path = "/api/comfyui/runs",
    tag = "comfyui",
    summary = "Start a production line against a shot",
    description = "Open a run and queue its first stage. Each stage's output becomes the next \
                   stage's input as it completes. Refused if the line does not fit together, or \
                   if its first stage cannot read this shot.",
    request_body = StartRunPayload,
    responses(
        (status = 200, description = "Run started"),
        (status = 400, description = "The line does not fit, or does not fit this shot"),
        (status = 404, description = "Line or shot not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn start_run(
    UState(state): UState,
    Json(payload): Json<StartRunPayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    // FR7 replaces this line with a query and a cursor. Everything after it
    // already works on a set.
    let shot_ids = vec![payload.shot_id.clone()];

    let mut started = Vec::with_capacity(shot_ids.len());
    for shot_id in &shot_ids {
        match crate::comfyui::runs::start_line_run(
            &mut conn,
            &payload.line_id,
            shot_id,
            &payload.stage_values,
        ) {
            Ok(run) => started.push(serde_json::json!({
                "run_id": run.run_id,
                "shot_id": shot_id,
                "stage_count": run.stage_count,
                "tasks": run.task_ids,
            })),
            Err(crate::comfyui::runs::StartError::NotFound(_)) => {
                return Err(StatusCode::NOT_FOUND.into())
            }
            Err(crate::comfyui::runs::StartError::Rejected(e)) => {
                return Err(ApiError::bad_request(e.message))
            }
            Err(crate::comfyui::runs::StartError::Db(e)) => {
                tracing::error!("Failed to start run: {}", e);
                return Err(ApiError::internal());
            }
        }
    }

    Ok(Json(serde_json::json!({
        "runs": started,
        "count": started.len(),
        // What a single-shot caller reads. FR7's batch caller reads `runs`.
        "run_id": started.first().and_then(|r| r["run_id"].as_str()),
    })))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct RunsQuery {
    shot_id: Option<String>,
    /// Max items to return (default 50)
    limit: Option<i64>,
    /// Cursor: `next_cursor` from the previous page — `created_at|id` of its
    /// last run. The id is the tie-breaker: a migrated backlog can put dozens
    /// of runs on the same second, and a timestamp alone would skip the rest
    /// of them.
    cursor: Option<String>,
}

/// Everything one board row needs, gathered in batches rather than per row.
struct BoardRow {
    id: String,
    line_id: Option<String>,
    shot_id: String,
    label: String,
    status: String,
    stage_count: i32,
    error_message: Option<String>,
    created_at: Option<String>,
    finished_at: Option<String>,
    /// Where a held run is parked. `None` for every other status.
    held_at_stage: Option<i32>,
}

type RunTuple = (
    String,
    Option<String>,
    String,
    String,
    String,
    i32,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i32>,
);

/// The columns a board row is built out of, in [`BoardRow`]'s own order.
fn run_columns() -> (
    runs::id,
    runs::line_id,
    runs::shot_id,
    runs::label,
    runs::status,
    runs::stage_count,
    runs::error_message,
    runs::created_at,
    runs::finished_at,
    runs::held_at_stage,
) {
    (
        runs::id,
        runs::line_id,
        runs::shot_id,
        runs::label,
        runs::status,
        runs::stage_count,
        runs::error_message,
        runs::created_at,
        runs::finished_at,
        runs::held_at_stage,
    )
}

impl From<RunTuple> for BoardRow {
    fn from(t: RunTuple) -> Self {
        BoardRow {
            id: t.0,
            line_id: t.1,
            shot_id: t.2,
            label: t.3,
            status: t.4,
            stage_count: t.5,
            error_message: t.6,
            created_at: t.7,
            finished_at: t.8,
            held_at_stage: t.9,
        }
    }
}

/// How long the run has been going, or how long it took. The board reads it as
/// `HH:MM:SS`, and computing it here keeps it off a browser clock that may not
/// agree with the server's.
fn elapsed_seconds(created_at: Option<&str>, finished_at: Option<&str>) -> Option<i64> {
    let parse = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok();
    let start = created_at.and_then(parse)?;
    let end = finished_at
        .and_then(parse)
        .unwrap_or_else(|| chrono::Utc::now().naive_utc());
    Some((end - start).num_seconds().max(0))
}

#[utoipa::path(
    get,
    path = "/api/comfyui/runs",
    tag = "comfyui",
    summary = "List runs",
    description = "The queue board: one row per run, newest first, with the stage it is on and \
                   how far along its line that is. The tasks underneath are reachable through \
                   GET /api/comfyui/runs/{id}.",
    params(RunsQuery),
    responses(
        (status = 200, description = "List of runs"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn list_runs(
    UState(state): UState,
    Query(query): Query<RunsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let limit = query.limit.unwrap_or(50).min(200);
    let mut q = runs::table
        .select(run_columns())
        .order((runs::created_at.desc(), runs::id.desc()))
        .limit(limit + 1)
        .into_boxed();
    if let Some(shot_id) = &query.shot_id {
        q = q.filter(runs::shot_id.eq(shot_id));
    }
    if let Some(cursor) = &query.cursor {
        // `created_at|id`, matching the ordering above. A bare timestamp (an
        // old client, or a hand-typed cursor) still works — it just re-reads
        // the runs that share that second.
        match cursor.split_once('|') {
            Some((ts, id)) => {
                q = q.filter(
                    runs::created_at.lt(ts.to_string()).or(runs::created_at
                        .eq(ts.to_string())
                        .and(runs::id.lt(id.to_string()))),
                );
            }
            None => q = q.filter(runs::created_at.le(cursor)),
        }
    }

    let mut tuples: Vec<RunTuple> = q.load(&mut conn).map_err(|e| {
        tracing::error!("Failed to query runs: {}", e);
        ApiError::internal()
    })?;
    let has_more = tuples.len() as i64 > limit;
    if has_more {
        tuples.truncate(limit as usize);
    }

    let rows: Vec<BoardRow> = tuples.into_iter().map(BoardRow::from).collect();

    let items = decorate_runs(&mut conn, &rows);
    let next_cursor = if has_more {
        rows.last()
            .and_then(|r| r.created_at.as_ref().map(|ts| format!("{}|{}", ts, r.id)))
    } else {
        None
    };
    Ok(Json(
        serde_json::json!({ "items": items, "next_cursor": next_cursor }),
    ))
}

/// The tasks of a page of runs, folded into what a schedule board shows: which
/// stage, of how many, running what.
fn decorate_runs(conn: &mut SqliteConnection, rows: &[BoardRow]) -> Vec<serde_json::Value> {
    let run_ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let task_rows: Vec<RunTaskRow> = if run_ids.is_empty() {
        Vec::new()
    } else {
        enhancement_tasks::table
            .filter(enhancement_tasks::run_id.eq_any(&run_ids))
            .select((
                enhancement_tasks::run_id.assume_not_null(),
                enhancement_tasks::stage_idx,
                enhancement_tasks::status,
                enhancement_tasks::workflow_id,
                enhancement_tasks::source_file_id,
            ))
            .load(conn)
            .unwrap_or_default()
    };

    let workflow_names: HashMap<String, String> = comfyui_workflows::table
        .select((comfyui_workflows::id, comfyui_workflows::name))
        .load::<(String, String)>(conn)
        .unwrap_or_default()
        .into_iter()
        .collect();

    // How many takes each held run is waiting on — `HELD · 4 TAKES` on the
    // board. Asked only of the rows that are actually holding, so an ordinary
    // page costs nothing.
    let held: Vec<(&str, i32)> = rows
        .iter()
        .filter(|r| r.status == RunState::Held.as_str())
        .filter_map(|r| r.held_at_stage.map(|at| (r.id.as_str(), at)))
        .collect();
    let held_takes = crate::comfyui::holds::held_take_counts(conn, &held);

    // Who the shot belongs to and what it looks like — the same three batched
    // lookups the task queue makes, for the same reason: a page is fifty rows.
    let shot_ids: Vec<&str> = rows.iter().map(|r| r.shot_id.as_str()).collect();
    let shot_rows: Vec<(String, Option<String>, Option<String>)> = if shot_ids.is_empty() {
        Vec::new()
    } else {
        shots::table
            .filter(shots::id.eq_any(&shot_ids))
            .select((shots::id, shots::main_file_id, shots::primary_person_id))
            .load(conn)
            .unwrap_or_default()
    };
    let main_files: HashMap<&str, Option<&str>> = shot_rows
        .iter()
        .map(|(id, fid, _)| (id.as_str(), fid.as_deref()))
        .collect();
    let person_ids: Vec<String> = shot_rows
        .iter()
        .filter_map(|(_, _, pid)| pid.clone())
        .collect();
    let person_names: HashMap<String, Option<String>> = if person_ids.is_empty() {
        HashMap::new()
    } else {
        people::table
            .filter(people::id.eq_any(&person_ids))
            .select((people::id, people::name))
            .load(conn)
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let shot_people: HashMap<&str, Option<&str>> = shot_rows
        .iter()
        .map(|(id, _, pid)| (id.as_str(), pid.as_deref()))
        .collect();

    let file_ids: Vec<&str> = task_rows
        .iter()
        .filter_map(|t| t.4.as_deref())
        .chain(main_files.values().copied().flatten())
        .collect();
    let file_paths: HashMap<String, String> = if file_ids.is_empty() {
        HashMap::new()
    } else {
        files::table
            .filter(files::id.eq_any(&file_ids))
            .select((files::id, files::path))
            .load::<(String, String)>(conn)
            .unwrap_or_default()
            .into_iter()
            .collect()
    };

    rows.iter()
        .map(|run| {
            let tasks: Vec<&RunTaskRow> = task_rows.iter().filter(|t| t.0 == run.id).collect();
            let phases: Vec<(i32, line::TaskPhase)> = tasks
                .iter()
                .map(|t| (t.1.unwrap_or(0), line::phase_of(&t.2)))
                .collect();
            let tally = line::tally(&phases, run.stage_count);

            // The workflow the run is on right now: whichever task sits at the
            // stage the tally picked out. Works for a line and for a lone
            // enhance alike, because both are runs of tasks.
            let stage_label = tasks
                .iter()
                .find(|t| t.1.unwrap_or(0) == tally.current_stage)
                .and_then(|t| workflow_names.get(&t.3).cloned());

            let thumb_source = tasks
                .iter()
                .filter(|t| t.1.unwrap_or(0) == 0)
                .find_map(|t| t.4.as_deref())
                .or_else(|| main_files.get(run.shot_id.as_str()).copied().flatten());

            serde_json::json!({
                "id": run.id,
                "line_id": run.line_id,
                "shot_id": run.shot_id,
                "label": run.label,
                // `runs.status` is what the worker wrote and is the authority;
                // the tally is how far along that run got.
                "status": run.status,
                "stage_count": run.stage_count,
                // A held run is *at* the stage it is holding, not past it: the
                // tally counts its takes as finished, and the board should say
                // which stage a person is being asked about.
                "current_stage": run.held_at_stage
                    .unwrap_or_else(|| tally.current_stage.min(run.stage_count)),
                "stage_label": run.held_at_stage
                    .and_then(|at| tasks.iter().find(|t| t.1.unwrap_or(0) == at))
                    .and_then(|t| workflow_names.get(&t.3).cloned())
                    .or(stage_label),
                "held_at_stage": run.held_at_stage,
                "held_takes": held_takes.get(run.id.as_str()).copied().unwrap_or(0),
                "task_count": tasks.len(),
                "in_flight": tally.in_flight,
                "completed": tally.completed,
                "failed": tally.failed,
                "error_message": run.error_message,
                "created_at": run.created_at,
                "finished_at": run.finished_at,
                "elapsed_seconds": elapsed_seconds(
                    run.created_at.as_deref(),
                    run.finished_at.as_deref(),
                ),
                "thumbnail_url": thumb_source
                    .map(|fid| format!("/api/files/{}/thumbnail", fid)),
                "person_name": shot_people
                    .get(run.shot_id.as_str())
                    .copied()
                    .flatten()
                    .and_then(|pid| person_names.get(pid).cloned().flatten()),
                "source_name": thumb_source
                    .and_then(|fid| file_paths.get(fid))
                    .and_then(|p| p.rsplit('/').next())
                    .map(str::to_string),
            })
        })
        .collect()
}

#[utoipa::path(
    get,
    path = "/api/comfyui/runs/{id}",
    tag = "comfyui",
    summary = "Get a run and its tasks",
    description = "One run with every task under it, in stage order — the drill-down behind a \
                   board row.",
    params(("id" = String, Path, description = "Run ID")),
    responses(
        (status = 200, description = "The run and its tasks"),
        (status = 404, description = "Run not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn get_run(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let row: Option<RunTuple> = runs::table
        .filter(runs::id.eq(&id))
        .select(run_columns())
        .first(&mut conn)
        .optional()
        .map_err(|_| ApiError::internal())?;
    let Some(t) = row else {
        return Err(StatusCode::NOT_FOUND.into());
    };
    let board = BoardRow::from(t);

    let tasks: Vec<serde_json::Value> = enhancement_tasks::table
        .inner_join(comfyui_workflows::table)
        .filter(enhancement_tasks::run_id.eq(&id))
        .order((
            enhancement_tasks::stage_idx.asc(),
            enhancement_tasks::created_at.asc(),
        ))
        .select((
            enhancement_tasks::id,
            enhancement_tasks::stage_idx,
            enhancement_tasks::parent_task_id,
            enhancement_tasks::workflow_id,
            comfyui_workflows::name,
            enhancement_tasks::status,
            enhancement_tasks::error_message,
            enhancement_tasks::source_file_id,
            enhancement_tasks::output_file_id,
            enhancement_tasks::retry_count,
            enhancement_tasks::created_at,
            enhancement_tasks::completed_at,
        ))
        .load::<(
            String,
            Option<i32>,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
        )>(&mut conn)
        .unwrap_or_default()
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "id": t.0,
                "stage_idx": t.1,
                "parent_task_id": t.2,
                "workflow_id": t.3,
                "workflow_name": t.4,
                "status": t.5,
                "error_message": t.6,
                "source_file_id": t.7,
                "output_file_id": t.8,
                "retry_count": t.9.unwrap_or(0),
                "created_at": t.10,
                "completed_at": t.11,
            })
        })
        .collect();

    let mut json = decorate_runs(&mut conn, std::slice::from_ref(&board))
        .into_iter()
        .next()
        .unwrap_or_else(|| serde_json::json!({}));
    json["tasks"] = serde_json::Value::Array(tasks);
    Ok(Json(json))
}

#[utoipa::path(
    post,
    path = "/api/comfyui/runs/{id}/retry",
    tag = "comfyui",
    summary = "Resume a run from the stage that failed",
    description = "Re-queue every failed or cancelled task of a run, from the source file it was \
                   already given. Stages that already succeeded are not re-run.",
    params(("id" = String, Path, description = "Run ID")),
    responses(
        (status = 200, description = "Run resumed"),
        (status = 400, description = "Nothing in this run is waiting to be retried"),
        (status = 404, description = "Run not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn retry_run(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let exists: i64 = runs::table
        .filter(runs::id.eq(&id))
        .count()
        .get_result(&mut conn)
        .map_err(|_| ApiError::internal())?;
    if exists == 0 {
        return Err(StatusCode::NOT_FOUND.into());
    }

    let requeued = crate::comfyui::retry_run(&mut conn, &id).map_err(|e| {
        tracing::error!("Failed to retry run {}: {}", id, e);
        ApiError::internal()
    })?;
    if requeued == 0 {
        return Err(ApiError::bad_request(
            "Nothing in this run is waiting to be retried.",
        ));
    }
    Ok(Json(
        serde_json::json!({"status": "running", "resumed": requeued}),
    ))
}

#[utoipa::path(
    post,
    path = "/api/comfyui/runs/{id}/cancel",
    tag = "comfyui",
    summary = "Cancel a run",
    description = "Stop every task of a run that has not landed yet, interrupting on ComfyUI the \
                   one that is actually executing and dropping the rest from its queue.",
    params(("id" = String, Path, description = "Run ID")),
    responses(
        (status = 200, description = "Run cancelled"),
        (status = 404, description = "Run not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn cancel_run(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let status: Option<String> = runs::table
        .filter(runs::id.eq(&id))
        .select(runs::status)
        .first(&mut conn)
        .optional()
        .map_err(|_| ApiError::internal())?;
    let Some(status) = status else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    // Cancelling a *held* run is the hold's own Cancel verdict, and goes
    // through it: the decision is recorded in `run_holds` like every other, and
    // the intermediates go, which is what abandoning a run means. There is one
    // behaviour per state rather than two spellings of cancel that differ by
    // which button was pressed.
    //
    // If it turns out not to be a hold after all — a `held` row with no stage
    // on it, which nothing writes but a hand could — this falls through to the
    // ordinary cancel below rather than refusing. Stopping a run is the one
    // thing that must always work.
    if status == RunState::Held.as_str() {
        match crate::comfyui::holds::give_verdict(
            &mut conn,
            &state.library_root,
            &id,
            crate::comfyui::Verdict::Cancel,
            &[],
            None,
        ) {
            Ok(outcome) => {
                return Ok(Json(serde_json::json!({
                    "status": "cancelled",
                    "stopped": 0,
                    "abandoned_takes": outcome.reviewed.len(),
                })))
            }
            Err(e) => tracing::warn!(
                "Run {} is marked held but is not holding anything ({}); \
                 cancelling it the ordinary way",
                id,
                e
            ),
        }
    }

    // The prompts to stop on ComfyUI's side, read before the local rows change.
    let prompt_ids: Vec<String> = enhancement_tasks::table
        .filter(
            enhancement_tasks::run_id
                .eq(&id)
                .and(enhancement_tasks::comfyui_prompt_id.is_not_null())
                .and(enhancement_tasks::status.ne_all(&[
                    "completed",
                    "failed",
                    crate::comfyui::STATUS_CANCELLED,
                ])),
        )
        .select(enhancement_tasks::comfyui_prompt_id.assume_not_null())
        .load(&mut conn)
        .unwrap_or_default();

    let stopped = crate::comfyui::cancel_run(&mut conn, &id).map_err(|e| {
        tracing::error!("Failed to cancel run {}: {}", id, e);
        ApiError::internal()
    })?;

    if !prompt_ids.is_empty() {
        let _ = tokio::task::spawn_blocking(move || {
            let client = crate::comfyui::ComfyUiClient::new(&url);
            for prompt_id in prompt_ids {
                // Only the running prompt is worth interrupting: /interrupt
                // stops whatever is executing, so firing it for a merely-queued
                // prompt would kill somebody else's job.
                if let Ok(true) = client.is_prompt_running(&prompt_id) {
                    if let Err(e) = client.interrupt() {
                        tracing::warn!("Interrupt for prompt {} failed: {}", prompt_id, e);
                    }
                }
                if let Err(e) = client.delete_queued(&prompt_id) {
                    tracing::warn!("Dropping prompt {} from the queue failed: {}", prompt_id, e);
                }
            }
        })
        .await;
    }

    Ok(Json(serde_json::json!({
        "status": RunState::Cancelled.as_str(),
        "stopped": stopped,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::{Accepts, MediaType};
    use diesel::connection::SimpleConnection;

    const IMAGE_GRAPH: &str = r#"{
        "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
        "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out", "images": ["4", 0]}}
    }"#;

    /// A workflow whose contract is stated rather than guessed, so a test can
    /// build a join that does or does not fit without a graph to match.
    fn stated(id: &str, name: &str, accepts: Accepts, produces: MediaType) -> String {
        let contract = serde_json::json!({
            "version": 1,
            "accepts": accepts,
            "produces": produces,
        });
        format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json, contract_json) \
             VALUES ('{}', '{}', '{}', '{}');",
            id,
            name,
            IMAGE_GRAPH.replace('\'', "''"),
            serde_json::to_string(&contract)
                .unwrap()
                .replace('\'', "''")
        )
    }

    fn library(sql: &str) -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        conn.batch_execute(sql).unwrap();
        (dir, conn)
    }

    fn stage(workflow_id: &str) -> LineStagePayload {
        LineStagePayload {
            workflow_id: workflow_id.to_string(),
            text_overrides: HashMap::new(),
            parameters: ParameterMap::new(),
            vary: VaryMap::new(),
            source_mode: None,
            keep_output: false,
            exposed: Vec::new(),
            hold_for_review: false,
        }
    }

    fn line(stages: Vec<LineStagePayload>) -> LinePayload {
        LinePayload {
            name: "4K Restore".to_string(),
            description: None,
            stages,
        }
    }

    #[test]
    fn a_line_whose_stages_fit_together_is_accepted() {
        let (_dir, mut conn) = library(&format!(
            "{}{}{}",
            stated("wf-i2v", "Image to Video", Accepts::Image, MediaType::Video),
            stated("wf-interp", "Interpolate", Accepts::Video, MediaType::Video),
            stated("wf-4k", "Upscale 4K", Accepts::Video, MediaType::Video),
        ));
        let payload = line(vec![stage("wf-i2v"), stage("wf-interp"), stage("wf-4k")]);
        assert!(check_payload(&mut conn, &payload).is_ok());
    }

    #[test]
    fn a_line_whose_third_stage_cannot_eat_the_second_is_refused_when_it_is_drawn() {
        let (_dir, mut conn) = library(&format!(
            "{}{}{}",
            stated("wf-i2v", "Image to Video", Accepts::Image, MediaType::Video),
            // A still restorer, which happily reads a frame of the clip above
            // it — and then leaves the clip upscaler below it nothing to eat.
            stated("wf-restore", "Restore", Accepts::Image, MediaType::Image),
            stated("wf-4k", "Upscale 4K", Accepts::Video, MediaType::Video),
        ));
        let payload = line(vec![stage("wf-i2v"), stage("wf-restore"), stage("wf-4k")]);
        let ApiError(status, message) = check_payload(&mut conn, &payload).unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            message.unwrap(),
            "Stage 3 (Upscale 4K) takes video, but stage 2 (Restore) produces image.",
            "the refusal names the stage, in the words the person used"
        );
        // And nothing was stored: validation happens before the write.
        let stored: i64 = production_lines::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(stored, 0);
    }

    #[test]
    fn a_hold_goes_on_a_stage_that_has_something_after_it() {
        let (_dir, mut conn) = library(&format!(
            "{}{}",
            stated("wf-i2v", "Image to Video", Accepts::Image, MediaType::Video),
            stated("wf-4k", "Upscale 4K", Accepts::Video, MediaType::Video),
        ));

        // Held on stage 1, which stage 2 pays for: the whole point.
        let mut payload = line(vec![stage("wf-i2v"), stage("wf-4k")]);
        payload.stages[0].hold_for_review = true;
        assert!(check_payload(&mut conn, &payload).is_ok());

        // Held on the last stage, whose output is the product. Refused when the
        // line is drawn rather than found out by a run that parks and can never
        // be released.
        let mut payload = line(vec![stage("wf-i2v"), stage("wf-4k")]);
        payload.stages[1].hold_for_review = true;
        let ApiError(status, message) = check_payload(&mut conn, &payload).unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let message = message.unwrap();
        assert!(message.contains("Stage 2 is the last one"), "{}", message);
        assert!(message.contains("hold for review"), "{}", message);
    }

    #[test]
    fn the_board_says_how_many_takes_a_held_run_is_waiting_on() {
        let (_dir, mut conn) = library(&format!(
            "{}{}",
            stated("wf-i2v", "Extend Clip", Accepts::Image, MediaType::Video),
            stated("wf-4k", "Upscale 4K", Accepts::Video, MediaType::Video),
        ));
        conn.batch_execute(
            "INSERT INTO production_lines (id, name) VALUES ('line-1', 'Extend');
             INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, hold_for_review) \
               VALUES ('st-0', 'line-1', 0, 'wf-i2v', 1), ('st-1', 'line-1', 1, 'wf-4k', 0);
             INSERT INTO shots (id) VALUES ('shot-1');
             INSERT INTO runs (id, line_id, shot_id, label, status, stage_count, held_at_stage) \
               VALUES ('run-1', 'line-1', 'shot-1', 'Extend', 'held', 2, 0);
             INSERT INTO enhancement_tasks \
               (id, shot_id, workflow_id, status, run_id, stage_idx) VALUES \
               ('t-a', 'shot-1', 'wf-i2v', 'completed', 'run-1', 0),
               ('t-b', 'shot-1', 'wf-i2v', 'completed', 'run-1', 0),
               ('t-c', 'shot-1', 'wf-i2v', 'completed', 'run-1', 0),
               ('t-d', 'shot-1', 'wf-i2v', 'completed', 'run-1', 0);",
        )
        .unwrap();

        let board = BoardRow {
            id: "run-1".to_string(),
            line_id: Some("line-1".to_string()),
            shot_id: "shot-1".to_string(),
            label: "Extend".to_string(),
            status: "held".to_string(),
            stage_count: 2,
            error_message: None,
            created_at: None,
            finished_at: None,
            held_at_stage: Some(0),
        };
        let row = decorate_runs(&mut conn, std::slice::from_ref(&board))
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(row["held_takes"], serde_json::json!(4), "HELD · 4 TAKES");
        assert_eq!(
            row["current_stage"],
            serde_json::json!(0),
            "a held run is at the stage it is holding, not past it"
        );
        assert_eq!(row["stage_label"], serde_json::json!("Extend Clip"));

        // A verdict has been given over two of them. They are history: the
        // board counts what is still waiting, not what was ever made.
        conn.batch_execute(
            "INSERT INTO run_holds \
               (id, run_id, stage_idx, verdict, reviewed_task_ids, kept_task_ids) \
             VALUES ('h-1', 'run-1', 0, 'continue', '[\"t-a\",\"t-b\"]', '[\"t-a\"]');",
        )
        .unwrap();
        let row = decorate_runs(&mut conn, std::slice::from_ref(&board))
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(row["held_takes"], serde_json::json!(2));
    }

    #[test]
    fn a_held_run_locks_its_line_exactly_as_a_running_one_does() {
        let (_dir, mut conn) = library(&format!(
            "{}{}",
            stated("wf-i2v", "Image to Video", Accepts::Image, MediaType::Video),
            stated("wf-4k", "Upscale 4K", Accepts::Video, MediaType::Video),
        ));
        conn.batch_execute(
            "INSERT INTO production_lines (id, name) VALUES ('line-1', 'Extend');
             INSERT INTO shots (id) VALUES ('shot-1');
             INSERT INTO runs (id, line_id, shot_id, label, status, stage_count) \
               VALUES ('run-1', 'line-1', 'shot-1', 'Extend', 'held', 2);",
        )
        .unwrap();
        assert_eq!(
            live_run_count(&mut conn, "line-1"),
            1,
            "a verdict puts it straight back to reading the stages after the hold"
        );

        conn.batch_execute("UPDATE runs SET status = 'completed';")
            .unwrap();
        assert_eq!(live_run_count(&mut conn, "line-1"), 0);
    }

    #[test]
    fn a_stage_naming_a_workflow_this_library_does_not_have_says_so() {
        let (_dir, mut conn) = library(&stated(
            "wf-i2v",
            "Image to Video",
            Accepts::Image,
            MediaType::Video,
        ));
        let payload = line(vec![stage("wf-i2v"), stage("wf-missing")]);
        let ApiError(status, message) = check_payload(&mut conn, &payload).unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            message
                .unwrap()
                .contains("Stage 2 names workflow wf-missing"),
            "a typo should not read as a type error"
        );
    }

    #[test]
    fn a_source_mode_nobody_can_read_is_refused_at_the_stage_that_set_it() {
        let (_dir, mut conn) = library(&stated(
            "wf-4k",
            "Upscale 4K",
            Accepts::Image,
            MediaType::Image,
        ));
        let mut s = stage("wf-4k");
        s.source_mode = Some("middle_frame".to_string());
        let ApiError(_, message) = check_payload(&mut conn, &line(vec![s])).unwrap_err();
        assert!(message
            .unwrap()
            .contains("Stage 1: 'middle_frame' is not a source mode"));
    }

    #[test]
    fn a_sweep_that_says_neither_how_many_nor_which_is_refused_here_not_at_dispatch() {
        let (_dir, mut conn) = library(&stated(
            "wf-4k",
            "Upscale 4K",
            Accepts::Image,
            MediaType::Image,
        ));
        let mut s = stage("wf-4k");
        s.vary = serde_json::from_value(serde_json::json!({ "3.seed": {} })).unwrap();
        let ApiError(_, message) = check_payload(&mut conn, &line(vec![s])).unwrap_err();
        assert!(message.unwrap().starts_with("Stage 1:"));
    }

    #[test]
    fn a_line_with_no_stages_is_not_a_line() {
        let (_dir, mut conn) = library("");
        let ApiError(status, _) = check_payload(&mut conn, &line(vec![])).unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_runs_clock_reads_from_the_server_not_the_browser() {
        assert_eq!(
            elapsed_seconds(Some("2026-08-30 12:00:00"), Some("2026-08-30 12:03:12")),
            Some(192),
            "three minutes twelve"
        );
        // Still going: measured against now, which is after any fixed past.
        assert!(elapsed_seconds(Some("2020-01-01 00:00:00"), None).unwrap() > 0);
        assert_eq!(elapsed_seconds(None, None), None);
    }

    // ----- The editor -------------------------------------------------------

    /// The library the worked example is built out of.
    fn shop() -> (tempfile::TempDir, SqliteConnection) {
        library(&format!(
            "{}{}{}{}",
            stated("wf-i2v", "Image to Video", Accepts::Image, MediaType::Video),
            stated("wf-interp", "Interpolate", Accepts::Video, MediaType::Video),
            stated("wf-4k", "Upscale 4K", Accepts::Video, MediaType::Video),
            stated("wf-restore", "Restore", Accepts::Image, MediaType::Image),
        ))
    }

    #[test]
    fn reordering_a_line_is_accepted_or_refused_by_name() {
        let (_dir, mut conn) = shop();
        // The order that works. A PUT replaces the whole list, so this is what
        // a reorder actually sends.
        let ok = line(vec![stage("wf-i2v"), stage("wf-interp"), stage("wf-4k")]);
        assert!(check_payload(&mut conn, &ok).is_ok());

        // Swapping the last two is still valid — both eat and make video.
        let swapped = line(vec![stage("wf-i2v"), stage("wf-4k"), stage("wf-interp")]);
        assert!(check_payload(&mut conn, &swapped).is_ok());

        // Dragging the still-maker into the middle is not — and the refusal
        // names the stage it broke rather than the one that was moved. Not the
        // still-maker itself: it reads a frame of the clip above it quite
        // happily. It is the upscaler *below* it that is left with a still.
        let broken = line(vec![stage("wf-i2v"), stage("wf-restore"), stage("wf-4k")]);
        let ApiError(status, message) = check_payload(&mut conn, &broken).unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            message.unwrap(),
            "Stage 3 (Upscale 4K) takes video, but stage 2 (Restore) produces image."
        );

        // And dragging it to the top strands both clip stages under it.
        let upended = line(vec![
            stage("wf-restore"),
            stage("wf-interp"),
            stage("wf-4k"),
        ]);
        let ApiError(_, message) = check_payload(&mut conn, &upended).unwrap_err();
        assert_eq!(
            message.unwrap(),
            "Stage 2 (Interpolate) takes video, but stage 1 (Restore) produces image."
        );
    }

    #[test]
    fn a_setting_is_pinned_varied_or_exposed_but_never_two_of_them() {
        let (_dir, mut conn) = shop();
        let mut s = stage("wf-restore");
        s.vary = serde_json::from_value(serde_json::json!({ "3.seed": { "count": 4 } })).unwrap();
        s.exposed = vec!["3.seed".to_string()];
        let ApiError(status, message) = check_payload(&mut conn, &line(vec![s])).unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            message.unwrap().contains("both swept and asked for"),
            "two people deciding one value is a refusal, not a merge"
        );

        // Exposing something that is merely pinned is fine: the answer wins.
        let mut s = stage("wf-restore");
        s.parameters = serde_json::from_value(serde_json::json!({ "3.seed": 7 })).unwrap();
        s.exposed = vec!["3.seed".to_string()];
        assert!(check_payload(&mut conn, &line(vec![s])).is_ok());
    }

    #[test]
    fn duplicating_a_line_copies_everything_its_stages_carry() {
        let (_dir, mut conn) = shop();

        let mut first = stage("wf-i2v");
        first.text_overrides =
            HashMap::from([("6.text".to_string(), "a winter street".to_string())]);
        first.parameters = serde_json::from_value(serde_json::json!({ "3.steps": 28 })).unwrap();
        first.vary =
            serde_json::from_value(serde_json::json!({ "3.seed": { "count": 3 } })).unwrap();
        first.exposed = vec!["6.text".to_string()];
        let mut second = stage("wf-interp");
        second.source_mode = Some("whole_video".to_string());
        second.keep_output = true;

        let original = line(vec![first, second]);
        check_payload(&mut conn, &original).unwrap();
        diesel::insert_into(production_lines::table)
            .values(NewProductionLine {
                id: "line-1",
                name: "4K Restore",
                description: Some("the original"),
            })
            .execute(&mut conn)
            .unwrap();
        insert_stages(&mut conn, "line-1", &original).unwrap();

        // The fork: read the stages back, turn them into a payload, write them
        // under a new id. Exactly what the endpoint does, without the router.
        let read = crate::comfyui::runs::stages_of_line(&mut conn, "line-1").unwrap();
        let fork = LinePayload::new(
            "4K Restore (2)".to_string(),
            Some("the original".to_string()),
            read.iter().map(LineStagePayload::from_row).collect(),
        );
        diesel::insert_into(production_lines::table)
            .values(NewProductionLine {
                id: "line-2",
                name: fork.name(),
                description: fork.description(),
            })
            .execute(&mut conn)
            .unwrap();
        insert_stages(&mut conn, "line-2", &fork).unwrap();

        let copied = crate::comfyui::runs::stages_of_line(&mut conn, "line-2").unwrap();
        assert_eq!(copied.len(), 2);
        for (a, b) in read.iter().zip(&copied) {
            assert_eq!(a.workflow_id, b.workflow_id);
            assert_eq!(a.text_overrides, b.text_overrides);
            assert_eq!(a.parameters, b.parameters);
            assert_eq!(a.vary, b.vary);
            assert_eq!(a.source_mode, b.source_mode);
            assert_eq!(a.keep_output, b.keep_output);
            assert_eq!(a.exposed, b.exposed);
        }
        assert_eq!(copied[0].exposed, ["6.text"]);
        assert_eq!(copied[1].source_mode.as_deref(), Some("whole_video"));
        assert!(copied[1].keep_output);

        // And the fork stands on its own: editing it leaves the original be.
        let json = line_json("line-2", fork.name(), None, None, None, &copied, 0);
        assert_eq!(json["valid"], serde_json::json!(true));
        assert_eq!(json["editable"], serde_json::json!(true));
    }

    #[test]
    fn a_line_says_on_every_read_whether_it_can_be_edited() {
        let (_dir, mut conn) = shop();
        let payload = line(vec![stage("wf-i2v"), stage("wf-interp")]);
        diesel::insert_into(production_lines::table)
            .values(NewProductionLine {
                id: "line-1",
                name: "4K Restore",
                description: None,
            })
            .execute(&mut conn)
            .unwrap();
        insert_stages(&mut conn, "line-1", &payload).unwrap();
        conn.batch_execute(
            "INSERT INTO shots (id) VALUES ('shot-1');
             INSERT INTO runs (id, line_id, shot_id, label, status, stage_count) \
             VALUES ('run-1', 'line-1', 'shot-1', '4K Restore', 'running', 2);",
        )
        .unwrap();

        assert_eq!(live_run_count(&mut conn, "line-1"), 1);
        let stages = crate::comfyui::runs::stages_of_line(&mut conn, "line-1").unwrap();
        let json = line_json("line-1", "4K Restore", None, None, None, &stages, 1);
        assert_eq!(json["editable"], serde_json::json!(false));
        assert_eq!(json["live_runs"], serde_json::json!(1));
        // The guard the editor is reflecting is the one PUT enforces.
        assert!(has_live_runs(&mut conn, "line-1"));

        // Once it lands, the line is its own again.
        diesel::update(runs::table.filter(runs::id.eq("run-1")))
            .set(runs::status.eq("completed"))
            .execute(&mut conn)
            .unwrap();
        assert_eq!(live_run_count(&mut conn, "line-1"), 0);
    }

    #[test]
    fn a_join_that_can_be_read_two_ways_comes_back_as_a_question() {
        // A graph with a video loader *and* an image loader: the clip itself,
        // or a frame of it. Derived, not stated, because the loaders are what
        // decide it.
        let graph = r#"{
            "4": {"class_type": "LoadImage", "inputs": {"image": "ref.png"}},
            "7": {"class_type": "VHS_LoadVideo", "inputs": {"video": "clip.mp4"}},
            "9": {"class_type": "VHS_VideoCombine", "inputs": {"filename_prefix": "out"}}
        }"#;
        let (_dir, mut conn) = library(&format!(
            "{}INSERT INTO comfyui_workflows (id, name, workflow_json) \
             VALUES ('wf-extend', 'Extend Clip', '{}');",
            stated("wf-i2v", "Image to Video", Accepts::Image, MediaType::Video),
            graph.replace('\'', "''")
        ));
        let payload = line(vec![stage("wf-i2v"), stage("wf-extend")]);
        check_payload(&mut conn, &payload).unwrap();
        diesel::insert_into(production_lines::table)
            .values(NewProductionLine {
                id: "line-1",
                name: "Extend",
                description: None,
            })
            .execute(&mut conn)
            .unwrap();
        insert_stages(&mut conn, "line-1", &payload).unwrap();

        let stages = crate::comfyui::runs::stages_of_line(&mut conn, "line-1").unwrap();
        let drawn = stages_json(&stages);
        // Nothing above the first stage, so no join above it.
        assert_eq!(drawn[0]["handoff"], serde_json::Value::Null);
        let join = &drawn[1]["handoff"];
        assert_eq!(join["carries"], serde_json::json!("video"));
        assert_eq!(join["resolved"], serde_json::json!("whole_video"));
        assert_eq!(join["is_a_question"], serde_json::json!(true));
        assert_eq!(
            join["modes"],
            serde_json::json!(["whole_video", "first_frame", "last_frame", "at_time"])
        );
    }
}
