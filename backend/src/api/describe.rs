//! The prompt a shot's own contents compile to, and the describe run behind it.
//!
//! Two endpoints, both about one thing: **the compiled prompt is visible and
//! editable before the costly stage is queued.** A prompt you cannot see or
//! correct is worse than one you typed, so for a single interactive shot the
//! describe stage runs first — it takes seconds — and the Enhance dialog shows
//! what it said.
//!
//! * `POST /api/comfyui/describe` starts that, or answers from the shot's cache
//!   without touching a GPU at all.
//! * `GET /api/comfyui/describe/{shot_id}` is the poll, and re-compiles the
//!   prompt for whatever intent and style the dialog currently has typed in.
//!   Compiling is a pure function of the description, so changing the style
//!   costs nothing and never re-describes the photograph.
//!
//! At batch scale the same control is a hold point on the describe stage, which
//! is FR5c's and not built here.
//!
//! Everything that *decides* is in [`crate::comfyui::prompt`]; this file reads
//! rows and writes runs.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

use crate::comfyui::prompt::{self, Analysis, CompiledPrompt, Intent, ShotFacts};
use crate::comfyui::{runs as run_store, MediaType, StageContract};
use crate::models::NewEnhancementTask;
use crate::schema::{comfyui_workflows, enhancement_tasks, shots};

use super::comfyui::{require_comfyui, ApiError};
use super::UState;

/// Where the description for one shot has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum DescribeState {
    /// Nothing has described this shot, and nothing is trying to.
    None,
    /// A describe task is in flight.
    Running,
    /// There is a description, and a prompt compiled from it.
    Ready,
    /// The describe task failed. `error` says why.
    Failed,
}

/// The description, the prompt it compiles to, and everything that went into
/// both — which is what makes the prompt correctable rather than magic.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct DescribeResponse {
    shot_id: String,
    state: DescribeState,
    /// Answered from `shots.analysis_json` rather than from a run.
    cached: bool,
    /// What Phos knew and the model could not see.
    facts: ShotFacts,
    /// The instruction that was, or would be, sent into the describe workflow.
    #[serde(skip_serializing_if = "Option::is_none")]
    instruction: Option<String>,
    /// Exactly what the describe stage published, unedited.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// Its structured form, when the model produced one.
    #[serde(skip_serializing_if = "Option::is_none")]
    analysis: Option<Analysis>,
    /// The two strings a generation stage takes. Editable: the dialog sends
    /// whatever the person left in the box as an ordinary `text_overrides`
    /// entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<CompiledPrompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// What the caller wants described, and what for.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub(super) struct DescribePayload {
    shot_id: String,
    /// Which describe workflow to run. Omit it and the first workflow whose
    /// contract says `produces: text` is used.
    #[serde(default)]
    workflow_id: Option<String>,
    /// The user's one line about what they are after.
    #[serde(default)]
    intent: Option<String>,
    /// A style preset, in the preset's own words.
    #[serde(default)]
    style: Option<String>,
    /// What the run must not do. Reaches the model as a rule and the generation
    /// stage as a negative prompt.
    #[serde(default)]
    do_not: Vec<String>,
    /// Describe it again even though the shot already carries a description.
    #[serde(default)]
    refresh: bool,
}

impl DescribePayload {
    fn intent(&self) -> Intent {
        Intent {
            intent: self.intent.clone(),
            style: self.style.clone(),
            do_not: self.do_not.clone(),
        }
    }
}

/// The same three knobs, as query parameters, for the poll.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub(super) struct DescribeQuery {
    intent: Option<String>,
    style: Option<String>,
    /// Constraints, one per line or separated by `;`.
    do_not: Option<String>,
}

impl DescribeQuery {
    fn intent(&self) -> Intent {
        let mut overrides = HashMap::new();
        if let Some(v) = &self.do_not {
            overrides.insert(prompt::DO_NOT_KEY.to_string(), v.clone());
        }
        Intent {
            intent: self.intent.clone(),
            style: self.style.clone(),
            do_not: Intent::from_overrides(&overrides).do_not,
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/comfyui/describe",
    tag = "comfyui",
    summary = "Describe a shot, and compile a prompt from it",
    description = "Run a describe workflow against one shot and compile a prompt from what it \
                   says, together with the person names, EXIF date and place, and caption the \
                   library already holds. Answers immediately from the shot's cached description \
                   unless `refresh` is set. The compiled prompt is meant to be shown and edited \
                   before the costly stage is queued.",
    request_body = DescribePayload,
    responses(
        (status = 200, description = "The description, or the run that will produce one",
         body = DescribeResponse),
        (status = 400, description = "No workflow that produces text"),
        (status = 404, description = "Shot or workflow not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn describe_shot(
    UState(state): UState,
    Json(payload): Json<DescribePayload>,
) -> Result<Json<DescribeResponse>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    if !shot_exists(&mut conn, &payload.shot_id) {
        return Err(StatusCode::NOT_FOUND.into());
    }

    let facts = prompt::shot_facts(&mut conn, &payload.shot_id);
    let intent = payload.intent();

    // The cheap answer first: a description this shot already carries is a
    // description, and compiling a prompt from it costs nothing.
    if !payload.refresh {
        if let Some(cached) = prompt::cached_analysis(&mut conn, &payload.shot_id) {
            return Ok(Json(ready(
                payload.shot_id,
                facts,
                &intent,
                &cached.text,
                true,
            )));
        }
    }

    let (workflow_id, contract) = describe_workflow(&mut conn, payload.workflow_id.as_deref())?;

    // The instruction, and the directives that made it, both go on the task's
    // row: a run should be readable afterwards without re-deriving anything.
    let mut overrides: HashMap<String, String> = HashMap::new();
    intent.to_overrides(&mut overrides);
    if payload.refresh {
        overrides.insert(prompt::REFRESH_KEY.to_string(), "1".to_string());
    }
    contract.apply_role_corrections(&mut overrides);
    let instruction = prompt::describe_instruction(&facts, &intent);
    if let Err(e) = prompt::bind_instruction(&contract, &mut overrides, &instruction) {
        return Err(ApiError::bad_request(e.message));
    }

    let overrides_json = serde_json::to_string(&overrides).unwrap_or_else(|_| "{}".to_string());
    let label: String = comfyui_workflows::table
        .filter(comfyui_workflows::id.eq(&workflow_id))
        .select(comfyui_workflows::name)
        .first(&mut conn)
        .unwrap_or_else(|_| "Describe".to_string());

    // A describe run is a one-stage run like any other, so it shows on the same
    // board, retries the same way and is cancelled the same way.
    let (run_id, task_id) = conn
        .transaction::<_, diesel::result::Error, _>(|conn| {
            let run_id = run_store::open_run(conn, None, &payload.shot_id, &label, 1)?;
            let task_id = uuid::Uuid::new_v4().to_string();
            diesel::insert_into(enhancement_tasks::table)
                .values(NewEnhancementTask {
                    id: &task_id,
                    shot_id: &payload.shot_id,
                    workflow_id: &workflow_id,
                    text_overrides: Some(&overrides_json),
                    source_file_id: None,
                    source_mode: None,
                    parameters: None,
                    run_id: Some(&run_id),
                    stage_idx: Some(0),
                    parent_task_id: None,
                })
                .execute(conn)?;
            Ok((run_id, task_id))
        })
        .map_err(|e| {
            tracing::error!("Failed to queue a describe task: {}", e);
            ApiError::internal()
        })?;

    Ok(Json(DescribeResponse {
        shot_id: payload.shot_id,
        state: DescribeState::Running,
        cached: false,
        facts,
        instruction: Some(instruction),
        text: None,
        analysis: None,
        prompt: None,
        workflow_id: Some(workflow_id),
        task_id: Some(task_id),
        run_id: Some(run_id),
        error: None,
    }))
}

#[utoipa::path(
    get,
    path = "/api/comfyui/describe/{shot_id}",
    tag = "comfyui",
    summary = "The description a shot carries, and the prompt it compiles to",
    description = "Poll for a describe run, or read the description a shot already carries. \
                   `intent`, `style` and `do_not` re-compile the prompt without describing the \
                   photograph again.",
    params(("shot_id" = String, Path, description = "Shot ID"), DescribeQuery),
    responses(
        (status = 200, description = "Where the description has got to", body = DescribeResponse),
        (status = 404, description = "Shot not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn get_description(
    UState(state): UState,
    Path(shot_id): Path<String>,
    Query(query): Query<DescribeQuery>,
) -> Result<Json<DescribeResponse>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    if !shot_exists(&mut conn, &shot_id) {
        return Err(StatusCode::NOT_FOUND.into());
    }

    let facts = prompt::shot_facts(&mut conn, &shot_id);
    let intent = query.intent();

    // The task first, because a run in flight is the newer truth: a refresh is
    // running precisely when the cached answer is the one being replaced.
    if let Some(task) = latest_describe_task(&mut conn, &shot_id) {
        match task.status.as_str() {
            "completed" => {
                let text = task.text_output.unwrap_or_default();
                if !text.trim().is_empty() {
                    return Ok(Json(ready(shot_id, facts, &intent, &text, false)));
                }
            }
            "failed" | "cancelled" => {
                return Ok(Json(DescribeResponse {
                    shot_id,
                    state: DescribeState::Failed,
                    cached: false,
                    facts,
                    instruction: None,
                    text: None,
                    analysis: None,
                    prompt: None,
                    workflow_id: Some(task.workflow_id),
                    task_id: Some(task.id),
                    run_id: task.run_id,
                    error: task.error_message,
                }));
            }
            _ => {
                return Ok(Json(DescribeResponse {
                    shot_id,
                    state: DescribeState::Running,
                    cached: false,
                    facts,
                    instruction: None,
                    text: None,
                    analysis: None,
                    prompt: None,
                    workflow_id: Some(task.workflow_id),
                    task_id: Some(task.id),
                    run_id: task.run_id,
                    error: None,
                }));
            }
        }
    }

    match prompt::cached_analysis(&mut conn, &shot_id) {
        Some(cached) => Ok(Json(ready(shot_id, facts, &intent, &cached.text, true))),
        None => Ok(Json(DescribeResponse {
            shot_id,
            state: DescribeState::None,
            cached: false,
            facts,
            instruction: None,
            text: None,
            analysis: None,
            prompt: None,
            workflow_id: None,
            task_id: None,
            run_id: None,
            error: None,
        })),
    }
}

/// A description in hand, compiled for this intent.
fn ready(
    shot_id: String,
    facts: ShotFacts,
    intent: &Intent,
    text: &str,
    cached: bool,
) -> DescribeResponse {
    DescribeResponse {
        shot_id,
        state: DescribeState::Ready,
        cached,
        facts,
        instruction: None,
        text: Some(text.to_string()),
        analysis: prompt::parse_analysis(text),
        prompt: Some(prompt::compile_from_text(text, intent)),
        workflow_id: None,
        task_id: None,
        run_id: None,
        error: None,
    }
}

fn shot_exists(conn: &mut diesel::SqliteConnection, shot_id: &str) -> bool {
    shots::table
        .filter(shots::id.eq(shot_id))
        .count()
        .get_result::<i64>(conn)
        .map(|c| c > 0)
        .unwrap_or(false)
}

/// The workflow to describe with: the one asked for, or the first one whose
/// contract says it hands on text.
///
/// Refused with a message rather than a bare 400: "no describe workflow" is a
/// thing the user fixes by importing one, and they can only do that if they are
/// told.
fn describe_workflow(
    conn: &mut diesel::SqliteConnection,
    wanted: Option<&str>,
) -> Result<(String, StageContract), ApiError> {
    let rows: Vec<(String, Option<String>, String)> = comfyui_workflows::table
        .order(comfyui_workflows::created_at.asc())
        .select((
            comfyui_workflows::id,
            comfyui_workflows::contract_json,
            comfyui_workflows::workflow_json,
        ))
        .load(conn)
        .map_err(|_| ApiError::internal())?;

    if let Some(wanted) = wanted {
        let Some((id, contract_json, workflow_json)) =
            rows.into_iter().find(|(id, _, _)| id == wanted)
        else {
            return Err(StatusCode::NOT_FOUND.into());
        };
        let contract = run_store::contract_of(contract_json.as_deref(), &workflow_json);
        if contract.produces != MediaType::Text {
            return Err(ApiError::bad_request(
                "That workflow produces a picture, not a description. A describe stage is a \
                 workflow whose contract says it hands on text.",
            ));
        }
        return Ok((id, contract));
    }

    rows.into_iter()
        .map(|(id, contract_json, workflow_json)| {
            let contract = run_store::contract_of(contract_json.as_deref(), &workflow_json);
            (id, contract)
        })
        .find(|(_, contract)| contract.produces == MediaType::Text)
        .ok_or_else(|| {
            ApiError::bad_request(
                "No imported workflow produces text. Import a describe workflow — one that reads \
                 an image and shows a sentence — and Phos will write its instruction for you.",
            )
        })
}

/// The newest describe task for a shot: a task on a workflow that produces
/// text.
struct DescribeTask {
    id: String,
    workflow_id: String,
    run_id: Option<String>,
    status: String,
    text_output: Option<String>,
    error_message: Option<String>,
}

fn latest_describe_task(
    conn: &mut diesel::SqliteConnection,
    shot_id: &str,
) -> Option<DescribeTask> {
    type Row = (
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );
    let rows: Vec<Row> = enhancement_tasks::table
        .inner_join(
            comfyui_workflows::table.on(comfyui_workflows::id.eq(enhancement_tasks::workflow_id)),
        )
        .filter(enhancement_tasks::shot_id.eq(shot_id))
        .order(enhancement_tasks::created_at.desc())
        .limit(20)
        .select((
            enhancement_tasks::id,
            enhancement_tasks::workflow_id,
            enhancement_tasks::run_id,
            enhancement_tasks::status,
            enhancement_tasks::text_output,
            enhancement_tasks::error_message,
            comfyui_workflows::contract_json,
            comfyui_workflows::workflow_json,
        ))
        .load(conn)
        .unwrap_or_default();

    rows.into_iter()
        .find(|r| run_store::contract_of(r.6.as_deref(), &r.7).produces == MediaType::Text)
        .map(|r| DescribeTask {
            id: r.0,
            workflow_id: r.1,
            run_id: r.2,
            status: r.3,
            text_output: r.4,
            error_message: r.5,
        })
}
