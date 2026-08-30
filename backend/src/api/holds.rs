//! A held run, and the verdict somebody gives on it.
//!
//! Two endpoints and one idea. `GET` is the contact sheet in JSON — the takes a
//! run is holding, and what continuing with them will cost. `POST` is the three
//! buttons: continue with these, run it again, or abandon it.
//!
//! # Enough surface, not the surface
//!
//! The takes curation lane — a keyboard-driven contact sheet over every held
//! run at once — is FR10b. What is here is the runtime made reachable: a person
//! can see the takes of one run and give a verdict, from the queue board. FR10b
//! replaces the *screen*, not these endpoints: it reads the same `GET` per run
//! (or a list endpoint built beside it) and posts the same `POST`.

use axum::{extract::Path, Json};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::comfyui::holds::{self, Hold, HoldError};
use crate::comfyui::line::{self, Verdict};
use crate::comfyui::takes::bulk::Scope;
use crate::comfyui::takes::verdicts;

use super::comfyui::{require_comfyui, ApiError};
use super::UState;

/// What a person said about a held run.
#[derive(Deserialize, ToSchema)]
pub(super) struct VerdictPayload {
    /// `continue`, `regenerate` or `cancel`.
    pub(super) verdict: String,
    /// The takes that go on, by task id. Required for `continue` and ignored by
    /// the other two, which are about the hold rather than about a selection.
    #[serde(default)]
    pub(super) keep: Vec<String>,
    /// The takes whose **bytes go**, by task id — the curation lane's `X`.
    ///
    /// Deleting, not passing over: a take nobody named is disposed of by its
    /// stage's `keep_output` policy, which is the line author's decision, while
    /// a rejected one goes regardless, which is the reviewer's. Generated video
    /// is enormous and a farm fills a disk in days, so this is the key that
    /// keeps the library habitable.
    ///
    /// Never carried to another run by `scope`, whatever it says. Deleting
    /// bytes is something you do to pictures you have looked at.
    #[serde(default)]
    pub(super) reject: Vec<String>,
    /// Why, in the reviewer's own words. Kept with the verdict.
    #[serde(default)]
    pub(super) note: Option<String>,
    /// How far this verdict reaches: `run` (the default) or `batch`.
    ///
    /// `batch` applies the same verdict to every other run of the same FR7
    /// batch held at the same stage of the same line. `continue` resolves there
    /// to *all* of that run's waiting takes, because task ids are per run and
    /// the selection made here does not exist there.
    #[serde(default)]
    pub(super) scope: Option<String>,
}

/// The hold as the board draws it.
fn hold_json(hold: &Hold) -> serde_json::Value {
    let takes: Vec<serde_json::Value> = hold
        .takes
        .iter()
        .map(|t| {
            serde_json::json!({
                "task_id": t.task_id,
                "output_file_id": t.output_file_id,
                // The picture. A take with no file is a describe stage's
                // sentence, and `text_output` is what there is to read.
                "thumbnail_url": t.output_file_id
                    .as_ref()
                    .map(|id| format!("/api/files/{}/thumbnail", id)),
                "text_output": t.text_output,
                "parameters": t.parameters
                    .as_deref()
                    .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                    .unwrap_or_else(|| serde_json::json!({})),
                "completed_at": t.completed_at,
            })
        })
        .collect();

    // What one kept take costs from here on. The board multiplies it by the
    // selection, so the number moves as boxes are ticked — which is the whole
    // reason a hold is worth stopping at.
    let per_take = line::continuation_tasks(1, &hold.fanouts);
    serde_json::json!({
        "run_id": hold.run_id,
        "shot_id": hold.shot_id,
        "label": hold.label,
        "stage_idx": hold.stage_idx,
        "stage_count": hold.stage_count,
        "stage_label": hold.stage_label,
        "takes": takes,
        // How wide each stage after the hold fans out, in order.
        "fanouts": hold.fanouts,
        // And what that comes to for one take: the number to multiply.
        "tasks_per_take": per_take.iter().sum::<usize>(),
    })
}

pub(super) fn to_api_error(e: HoldError) -> ApiError {
    match e {
        HoldError::NotFound => axum::http::StatusCode::NOT_FOUND.into(),
        HoldError::NotHeld => ApiError::conflict(e.to_string()),
        HoldError::Refused(m) => ApiError::bad_request(m),
        HoldError::Db(e) => {
            tracing::error!("Hold: {}", e);
            ApiError::internal()
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/comfyui/runs/{id}/hold",
    tag = "comfyui",
    summary = "The takes a held run is waiting on",
    description = "Every take of the stage this run is parked at, with what continuing from one \
                   of them will cost in tasks. Answers `null` for a run that is not holding \
                   anything.",
    params(("id" = String, Path, description = "Run ID")),
    responses(
        (status = 200, description = "The hold, or null"),
        (status = 404, description = "Run not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn get_hold(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    match holds::read_hold(&mut conn, &id).map_err(to_api_error)? {
        Some(hold) => Ok(Json(hold_json(&hold))),
        None => Ok(Json(serde_json::Value::Null)),
    }
}

#[utoipa::path(
    post,
    path = "/api/comfyui/runs/{id}/hold",
    tag = "comfyui",
    summary = "Give a verdict on a held run",
    description = "`continue` proceeds with the takes named — more than one is ordinary, and each \
                   walks the rest of the line for itself. `regenerate` runs the held stage again \
                   with fresh seeds and nothing else changed, and the run holds again on the new \
                   takes. `cancel` abandons the run and removes its intermediates except where a \
                   stage says keep. Takes named in `reject` have their files removed from the \
                   library outright — that is the curation lane's `X`, and it is how a farm's \
                   disk stays habitable. `scope: batch` applies the same verdict to the rest of \
                   the batch; a rejection never travels with it.",
    params(("id" = String, Path, description = "Run ID")),
    request_body = VerdictPayload,
    responses(
        (status = 200, description = "Verdict recorded"),
        (status = 400, description = "The verdict names no takes, names a take this hold is not offering, or is not a verdict"),
        (status = 404, description = "Run not found"),
        (status = 409, description = "This run is not holding anything for review"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn post_verdict(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<VerdictPayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let Some(verdict) = Verdict::parse(payload.verdict.trim()) else {
        return Err(ApiError::bad_request(format!(
            "{:?} is not a verdict. It is continue, regenerate or cancel.",
            payload.verdict
        )));
    };
    let scope = match payload.scope.as_deref().map(str::trim) {
        None | Some("") => Scope::Run,
        Some(s) => Scope::parse(s).ok_or_else(|| {
            ApiError::bad_request(format!("{:?} is not a scope. It is run or batch.", s))
        })?,
    };

    let applied = verdicts::apply(
        &mut conn,
        &state.library_root,
        &id,
        verdicts::Ask {
            verdict,
            keep: &payload.keep,
            reject: &payload.reject,
            note: payload.note.as_deref(),
            scope,
        },
    )
    .map_err(to_api_error)?;

    let outcome = &applied.outcome;
    Ok(Json(serde_json::json!({
        "verdict": outcome.verdict.as_str(),
        "status": outcome.status.as_str(),
        "kept": outcome.kept,
        "reviewed": outcome.reviewed,
        // A regenerate queued a fresh generation here and now; a continue
        // leaves that to the advance pass, which is the one place a stage is
        // queued from.
        "queued": outcome.queued,
        // The takes whose bytes are gone, and how much disk that was. Reported
        // because it is the one thing in this response nobody can go and check
        // afterwards.
        "rejected": applied.rejected,
        "freed_bytes": applied.freed_bytes,
        "scope": applied.scope.as_str(),
        // Sibling runs the same verdict reached, and the ones it could not. A
        // sibling that stopped holding in between is skipped with its reason,
        // not rolled back over the nine hundred that worked.
        "also_applied": applied.also_applied,
        "failed": applied.failed
            .iter()
            .map(|(run_id, error)| serde_json::json!({ "run_id": run_id, "error": error }))
            .collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::holds::Take;

    /// A hold as the runtime hands it over: four takes of an extend stage, one
    /// upscale each below.
    fn four_takes() -> Hold {
        Hold {
            run_id: "run-1".to_string(),
            shot_id: "shot-1".to_string(),
            label: "Extend then upscale".to_string(),
            stage_idx: 1,
            stage_count: 3,
            stage_label: Some("Extend Clip".to_string()),
            fanouts: vec![1],
            takes: (0..4)
                .map(|i| Take {
                    task_id: format!("take-{}", i),
                    output_file_id: Some(format!("file-{}", i)),
                    text_output: None,
                    parameters: Some(format!(r#"{{"3.seed":{}}}"#, 1000 + i)),
                    completed_at: Some("2026-08-31 09:00:00".to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn a_hold_reads_as_a_contact_sheet_with_a_price_on_it() {
        // The shape FR10b's curation lane will read. Pinned here because that
        // lane replaces the screen, not these endpoints.
        let json = hold_json(&four_takes());
        assert_eq!(json["stage_idx"], serde_json::json!(1));
        assert_eq!(json["stage_label"], serde_json::json!("Extend Clip"));
        assert_eq!(json["takes"].as_array().unwrap().len(), 4);
        assert_eq!(
            json["takes"][0]["thumbnail_url"],
            serde_json::json!("/api/files/file-0/thumbnail"),
            "a take is a picture, and the picture is where the library keeps it"
        );
        assert_eq!(
            json["takes"][2]["parameters"]["3.seed"],
            serde_json::json!(1002),
            "the seed is what tells four otherwise identical cards apart"
        );
        assert_eq!(
            json["tasks_per_take"],
            serde_json::json!(1),
            "one upscale per take kept"
        );
    }

    #[test]
    fn a_take_with_no_file_is_a_sentence_and_says_so() {
        let mut hold = four_takes();
        hold.takes[0].output_file_id = None;
        hold.takes[0].text_output = Some("A jetty at dusk.".to_string());
        let json = hold_json(&hold);
        assert_eq!(json["takes"][0]["thumbnail_url"], serde_json::Value::Null);
        assert_eq!(
            json["takes"][0]["text_output"],
            serde_json::json!("A jetty at dusk.")
        );
    }

    #[test]
    fn the_price_is_per_take_because_a_hold_is_also_a_fan_out() {
        // Two stages below, the first of which sweeps four ways: one kept take
        // is four upscales and four grades, not two tasks.
        let hold = Hold {
            fanouts: vec![4, 1],
            ..four_takes()
        };
        assert_eq!(hold_json(&hold)["tasks_per_take"], serde_json::json!(8));
    }
}
