//! The four questions the line editor asks, and nothing else.
//!
//! A line is CRUD — [`super::lines`] has that. Building one is four different
//! questions, and every one of them is a question the *server* has to answer,
//! because the rules live here:
//!
//! * **What may go in this slot?** `GET /lines/stage-options`. The picker calls
//!   it and shows what comes back. It could not decide for itself without a
//!   second copy of [`crate::comfyui::Accepts::admits`] in JavaScript, and a
//!   picker that disagreed with the dispatcher would offer stages that then
//!   fail four hours into a run.
//! * **Would this hold together?** `POST /lines/validate`. The same check
//!   `POST` and `PUT` make, without writing anything — so a reorder can be
//!   shown as valid or refused *while* it is being dragged, rather than after
//!   Save.
//! * **Give me this one to change.** `POST /lines/{id}/duplicate`. The default
//!   way to get a line: open one, fork it, swap a stage. Also the answer to a
//!   line that is locked because a run is walking it.
//! * **What have I been doing by hand?** `GET /lines/suggestions`. Phos already
//!   recorded every chain anybody ran a workflow at a time;
//!   [`crate::comfyui::promote`] reads them back and offers the ones that
//!   repeated.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::Deserialize;

use crate::comfyui::editor::{stage_options, Candidate, Slot};
use crate::comfyui::promote::{self, Limits};
use crate::comfyui::runs::contract_of;
use crate::comfyui::{Accepts, MediaType};
use crate::schema::{comfyui_workflows, enhancement_tasks, line_stages, production_lines, runs};

use super::comfyui::{require_comfyui, ApiError};
use super::lines::{check_payload, insert_stages, line_json, LinePayload, LineStagePayload};
use super::UState;

// ===== What may go in this slot? ============================================

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct StageOptionsQuery {
    /// What the stage above this slot produces: `image`, `video` or `text`.
    /// Omit it for the first stage of a line, which has nothing above it.
    after: Option<String>,
    /// What the stage below this slot accepts: `image`, `video`, `text` or
    /// `none`. Omit it for the last stage, which has nothing to satisfy.
    before: Option<String>,
}

fn parse_media(value: &str, field: &str) -> Result<MediaType, ApiError> {
    serde_json::from_value(serde_json::Value::String(value.to_ascii_lowercase())).map_err(|_| {
        ApiError::bad_request(format!("{:?} is not a media type for {}.", value, field))
    })
}

fn parse_accepts(value: &str) -> Result<Accepts, ApiError> {
    serde_json::from_value(serde_json::Value::String(value.to_ascii_lowercase())).map_err(|_| {
        ApiError::bad_request(format!("{:?} is not something a stage accepts.", value))
    })
}

/// Every workflow in the library, as the picker needs it.
fn candidates(conn: &mut SqliteConnection) -> Result<Vec<Candidate>, ApiError> {
    let rows: Vec<(String, String, Option<String>, String)> = comfyui_workflows::table
        .order(comfyui_workflows::name.asc())
        .select((
            comfyui_workflows::id,
            comfyui_workflows::name,
            comfyui_workflows::contract_json,
            comfyui_workflows::workflow_json,
        ))
        .load(conn)
        .map_err(|_| ApiError::internal())?;
    Ok(rows
        .into_iter()
        .map(|(id, name, contract_json, workflow_json)| {
            let contract = contract_of(contract_json.as_deref(), &workflow_json);
            Candidate {
                workflow_id: id,
                name,
                accepts: contract.accepts,
                produces: contract.produces,
            }
        })
        .collect())
}

#[utoipa::path(
    get,
    path = "/api/comfyui/lines/stage-options",
    tag = "comfyui",
    summary = "Which workflows may go in one slot of a line",
    description = "Every workflow, marked with whether it may sit between what `after` produces \
                   and what `before` accepts. The refused ones come back too, each with the \
                   reason, so the editor can say what it is not offering and why. Both bounds \
                   are optional: no `after` is the first stage of a line, no `before` is the \
                   last.",
    params(StageOptionsQuery),
    responses(
        (status = 200, description = "The workflows, offered or refused"),
        (status = 400, description = "`after` or `before` is not a type"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn list_stage_options(
    UState(state): UState,
    Query(query): Query<StageOptionsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let slot = Slot {
        after: query
            .after
            .as_deref()
            .map(|v| parse_media(v, "after"))
            .transpose()?,
        before: query.before.as_deref().map(parse_accepts).transpose()?,
    };

    let options = stage_options(&candidates(&mut conn)?, slot);
    let offered = options.iter().filter(|o| o.offered()).count();
    Ok(Json(serde_json::json!({
        "after": slot.after,
        "before": slot.before,
        "offered": offered,
        "refused": options.len() - offered,
        "items": options
            .iter()
            .map(|o| serde_json::json!({
                "workflow_id": o.candidate.workflow_id,
                "name": o.candidate.name,
                "accepts": o.candidate.accepts,
                "produces": o.candidate.produces,
                "offered": o.offered(),
                "reason": o.refused,
            }))
            .collect::<Vec<_>>(),
    })))
}

// ===== Would this hold together? ============================================

#[utoipa::path(
    post,
    path = "/api/comfyui/lines/validate",
    tag = "comfyui",
    summary = "Check a line without saving it",
    description = "The same check POST and PUT make, with nothing written. What the editor asks \
                   after a reorder, so a chain that no longer fits says so where it was dragged \
                   rather than after Save.",
    request_body = LinePayload,
    responses(
        (status = 200, description = "Whether it holds together, and why not"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn validate_line(
    UState(state): UState,
    Json(payload): Json<LinePayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    // A refusal is the answer here, not an error: asking "is this valid" and
    // being told "no, because…" is a successful question.
    Ok(Json(match check_payload(&mut conn, &payload) {
        Ok(_) => serde_json::json!({ "valid": true, "error": null }),
        Err(ApiError(_, message)) => serde_json::json!({
            "valid": false,
            "error": message,
        }),
    }))
}

// ===== Give me this one to change ===========================================

/// The name a fork gets: `4K Restore`, `4K Restore (2)`, `4K Restore (3)`.
///
/// Not `(copy)`, `(copy) (copy)`: forking is the *normal* way to make a line,
/// so the fifth fork of a template should read as a name somebody could live
/// with rather than as an accident.
fn fork_name(existing: &[String], base: &str) -> String {
    let stem = base
        .rsplit_once(" (")
        .and_then(|(stem, tail)| {
            tail.strip_suffix(')')
                .filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
                .map(|_| stem)
        })
        .unwrap_or(base);
    (2..)
        .map(|n| format!("{} ({})", stem, n))
        .find(|candidate| !existing.iter().any(|e| e == candidate))
        .unwrap_or_else(|| base.to_string())
}

#[utoipa::path(
    post,
    path = "/api/comfyui/lines/{id}/duplicate",
    tag = "comfyui",
    summary = "Fork a production line",
    description = "Copy a line and everything its stages carry — workflows, prompts, parameters, \
                   sweeps, source modes, keep flags and exposed keys — under a new name. The \
                   default way to get a line, and the way to change one that a run is currently \
                   walking.",
    params(("id" = String, Path, description = "Line ID")),
    responses(
        (status = 200, description = "The fork"),
        (status = 404, description = "Line not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn duplicate_line(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let row: Option<(String, Option<String>)> = production_lines::table
        .filter(production_lines::id.eq(&id))
        .select((production_lines::name, production_lines::description))
        .first(&mut conn)
        .optional()
        .map_err(|_| ApiError::internal())?;
    let Some((name, description)) = row else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    let names: Vec<String> = production_lines::table
        .select(production_lines::name)
        .load(&mut conn)
        .unwrap_or_default();
    let stages =
        crate::comfyui::runs::stages_of_line(&mut conn, &id).map_err(|_| ApiError::internal())?;

    // A fork of a line whose workflows have since been re-imported may no
    // longer fit together. It is copied anyway — refusing to fork a line the
    // editor is about to repair would be the wrong end of the stick — and the
    // read that follows says so.
    let payload = LinePayload::new(
        fork_name(&names, &name),
        description,
        stages.iter().map(LineStagePayload::from_row).collect(),
    );

    let new_id = uuid::Uuid::new_v4().to_string();
    conn.transaction::<_, diesel::result::Error, _>(|conn| {
        diesel::insert_into(production_lines::table)
            .values(crate::models::NewProductionLine {
                id: &new_id,
                name: payload.name(),
                description: payload.description(),
            })
            .execute(conn)?;
        insert_stages(conn, &new_id, &payload)
    })
    .map_err(|e| {
        tracing::error!("Failed to duplicate line {}: {}", id, e);
        ApiError::internal()
    })?;

    let copied = crate::comfyui::runs::stages_of_line(&mut conn, &new_id)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(line_json(
        &new_id,
        payload.name(),
        payload.description(),
        None,
        None,
        &copied,
        0,
    )))
}

// ===== What have I been doing by hand? ======================================

/// One completed, hand-run enhancement, as the detector reads it: the file it
/// ate, the file it made, and what it was set to.
type HistoryRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[utoipa::path(
    get,
    path = "/api/comfyui/lines/suggestions",
    tag = "comfyui",
    summary = "Lines somebody has already been running by hand",
    description = "Sequences of workflows a person ran one at a time, over and over, on enough \
                   different shots to be a habit rather than an afternoon. Each comes back as a \
                   line ready to POST, with the settings that never changed pinned and the ones \
                   that did marked as asked-for.",
    responses(
        (status = 200, description = "The sequences worth offering"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn list_suggestions(
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    // Only what was run *by hand*: a task that was a stage of a saved line is
    // that line already, and offering it back would be a loop.
    let rows: Vec<HistoryRow> = enhancement_tasks::table
        .left_join(runs::table.on(runs::id.nullable().eq(enhancement_tasks::run_id)))
        .filter(
            enhancement_tasks::status
                .eq("completed")
                .and(enhancement_tasks::parent_task_id.is_null())
                .and(runs::line_id.is_null()),
        )
        .order(enhancement_tasks::created_at.asc())
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            enhancement_tasks::workflow_id,
            enhancement_tasks::source_file_id,
            enhancement_tasks::output_file_id,
            enhancement_tasks::source_mode,
            enhancement_tasks::parameters,
            enhancement_tasks::text_overrides,
        ))
        .load(&mut conn)
        .map_err(|e| {
            tracing::error!("Failed to read enhancement history: {}", e);
            ApiError::internal()
        })?;

    let tasks: Vec<promote::TaskRow> = rows
        .into_iter()
        .map(|r| promote::TaskRow {
            task_id: r.0,
            shot_id: r.1,
            workflow_id: r.2,
            source_file_id: r.3,
            output_file_id: r.4,
            source_mode: r.5,
            parameters: promote::parameters_of(r.6.as_deref()),
            text_overrides: promote::text_overrides_of(r.7.as_deref()),
        })
        .collect();

    let names: std::collections::HashMap<String, String> = comfyui_workflows::table
        .select((comfyui_workflows::id, comfyui_workflows::name))
        .load::<(String, String)>(&mut conn)
        .unwrap_or_default()
        .into_iter()
        .collect();

    // Sequences already saved as a line are not news.
    let saved: Vec<Vec<String>> = production_lines::table
        .select(production_lines::id)
        .load::<String>(&mut conn)
        .unwrap_or_default()
        .into_iter()
        .map(|line_id| {
            line_stages::table
                .filter(line_stages::line_id.eq(&line_id))
                .order(line_stages::stage_idx.asc())
                .select(line_stages::workflow_id)
                .load::<String>(&mut conn)
                .unwrap_or_default()
        })
        .collect();

    let mut items = Vec::new();
    for found in promote::suggest(&tasks, Limits::default()) {
        let ids: Vec<String> = found.stages.iter().map(|s| s.workflow_id.clone()).collect();
        if saved.contains(&ids) {
            continue;
        }
        // A sequence somebody ran by hand can still be one the type system
        // refuses — a contract corrected since, most likely. Not offered: an
        // offer that 400s on Save is worse than no offer.
        let payload = LinePayload::new(
            suggested_name(&found, &names),
            Some("Promoted from what you have been running by hand.".to_string()),
            found
                .stages
                .iter()
                .map(|s| {
                    LineStagePayload::new(
                        s.workflow_id.clone(),
                        s.pinned_text.clone().into_iter().collect(),
                        s.pinned.clone(),
                        s.source_mode.clone(),
                        s.exposed.clone(),
                    )
                })
                .collect(),
        );
        if check_payload(&mut conn, &payload).is_err() {
            continue;
        }
        items.push(serde_json::json!({
            "name": payload.name(),
            "shot_count": found.shots.len(),
            "shots": found.shots,
            "stages": found
                .stages
                .iter()
                .map(|s| serde_json::json!({
                    "workflow_id": s.workflow_id,
                    "workflow_name": names.get(&s.workflow_id),
                    "source_mode": s.source_mode,
                    "parameters": s.pinned,
                    "text_overrides": s.pinned_text,
                    "exposed": s.exposed,
                }))
                .collect::<Vec<_>>(),
        }));
    }

    Ok(Json(serde_json::json!({ "items": items })))
}

/// `Restore → Upscale`, in the names the person gave their workflows.
fn suggested_name(
    found: &promote::Suggestion,
    names: &std::collections::HashMap<String, String>,
) -> String {
    found
        .stages
        .iter()
        .map(|s| {
            names
                .get(&s.workflow_id)
                .cloned()
                .unwrap_or_else(|| s.workflow_id.clone())
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn shop() -> (tempfile::TempDir, SqliteConnection) {
        library(&format!(
            "{}{}{}{}",
            stated(
                "wf-i2v",
                "Photo to 5s Clip",
                Accepts::Image,
                MediaType::Video
            ),
            stated(
                "wf-interp",
                "Interpolate 60fps",
                Accepts::Video,
                MediaType::Video
            ),
            stated("wf-4k", "Upscale 4K", Accepts::Video, MediaType::Video),
            stated(
                "wf-restore",
                "Restore Portrait",
                Accepts::Image,
                MediaType::Image
            ),
        ))
    }

    fn offered(options: &[crate::comfyui::editor::StageOption]) -> Vec<&str> {
        options
            .iter()
            .filter(|o| o.offered())
            .map(|o| o.candidate.name.as_str())
            .collect()
    }

    #[test]
    fn the_picker_reads_each_workflows_own_contract_out_of_the_library() {
        let (_dir, mut conn) = shop();
        let all = candidates(&mut conn).unwrap();
        assert_eq!(all.len(), 4);

        // After a stage that makes a clip, only the video-eating ones.
        let after_clip = stage_options(
            &all,
            Slot {
                after: Some(MediaType::Video),
                before: None,
            },
        );
        assert_eq!(offered(&after_clip), ["Interpolate 60fps", "Upscale 4K"]);

        // At the top of a line, everything: a first stage is checked against
        // the shot it is run on, which no editor can know in advance.
        assert_eq!(offered(&stage_options(&all, Slot::default())).len(), 4);

        // And swapping the middle of image → video → video has one answer.
        assert_eq!(
            offered(&stage_options(
                &all,
                Slot {
                    after: Some(MediaType::Image),
                    before: Some(Accepts::Video),
                },
            )),
            ["Photo to 5s Clip"]
        );
    }

    #[test]
    fn a_workflow_with_no_stored_contract_is_still_offered_on_a_derived_one() {
        // Imported before contracts existed: `contract_json` is NULL, and the
        // contract is worked out on the spot rather than the row being skipped.
        let (_dir, mut conn) = library(&format!(
            "INSERT INTO comfyui_workflows (id, name, workflow_json) \
             VALUES ('wf-old', 'Old Import', '{}');",
            IMAGE_GRAPH.replace('\'', "''")
        ));
        let all = candidates(&mut conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].accepts, Accepts::Image);
        assert_eq!(all[0].produces, MediaType::Image);
    }

    #[test]
    fn a_fork_is_named_so_the_fifth_one_still_reads_as_a_name() {
        assert_eq!(fork_name(&[], "4K Restore"), "4K Restore (2)");
        assert_eq!(
            fork_name(&["4K Restore (2)".to_string()], "4K Restore"),
            "4K Restore (3)"
        );
        // Forking a fork counts up rather than nesting brackets.
        assert_eq!(
            fork_name(
                &["4K Restore (2)".to_string(), "4K Restore (3)".to_string()],
                "4K Restore (2)"
            ),
            "4K Restore (4)"
        );
        // A name that merely ends in a bracket is not a numbered fork.
        assert_eq!(
            fork_name(&[], "Restore (portrait)"),
            "Restore (portrait) (2)"
        );
    }
}
