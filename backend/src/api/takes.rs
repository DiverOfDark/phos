//! The Takes curation lane, in JSON.
//!
//! One request draws the whole screen: every held run, its takes, the original
//! each one is a variation of, and what continuing will cost. The lane is
//! keyboard-first and the number it exists to hit is two hundred takes in ten
//! minutes, so the shape here is chosen for a screen that never waits — a page
//! arrives whole rather than a card at a time, and `1`–`5`, `X` and `P` all act
//! on facts that are already in the browser.
//!
//! `GET /api/comfyui/runs/{id}/hold` still answers for one run; this is the
//! same hold, decorated, for all of them.

use axum::{extract::Path, extract::Query, Json};
use serde::Deserialize;

use crate::comfyui::takes::{self, Sheet, TakeDetail};

use super::comfyui::{require_comfyui, ApiError};
use super::UState;

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct TakesQuery {
    /// Held runs per page. Default 24, capped at 100 — each row prices its own
    /// continuation, so a page is real work.
    limit: Option<i64>,
    /// Cursor: `created_at` of the last run from the previous page.
    cursor: Option<String>,
}

/// One take as a card, which is a task plus everything a picture needs.
fn take_json(
    take: &crate::comfyui::holds::Take,
    detail: Option<&TakeDetail>,
    is_source: bool,
) -> serde_json::Value {
    let file_id = take.output_file_id.as_deref();
    serde_json::json!({
        "task_id": take.task_id,
        "output_file_id": file_id,
        // The picture, and the bytes behind it. A take with no file is a
        // describe stage's sentence; `text_output` is what there is to read.
        "thumbnail_url": file_id.map(|id| format!("/api/files/{}/thumbnail", id)),
        "file_url": file_id.map(|id| format!("/api/files/{}", id)),
        "text_output": take.text_output,
        "mime_type": detail.and_then(|d| d.mime_type.clone()),
        // What rejecting this one frees. The lane totals these and puts the
        // number on screen before the key that deletes them is pressed.
        "file_size": detail.and_then(|d| d.file_size),
        "rating": detail.and_then(|d| d.rating),
        // Already this shot's main file, so `P` has nothing left to do.
        "is_main_file": detail.map(|d| d.is_main_file).unwrap_or(false),
        // The resolved values this take ran with. Four takes of one fan-out
        // differ only here, and telling them apart is the whole reason a person
        // is looking at four cards.
        "parameters": take.parameters
            .as_deref()
            .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
            .unwrap_or_else(|| serde_json::json!({})),
        "completed_at": take.completed_at,
        // A take that is also the file the stage read: a stage that wrote back
        // over its input. Marked so compare mode does not show it twice.
        "is_source": is_source,
    })
}

/// One held run as the lane draws it.
pub(super) fn sheet_json(sheet: &Sheet) -> serde_json::Value {
    let hold = &sheet.hold;
    let per_take = crate::comfyui::line::continuation_tasks(1, &hold.fanouts);
    let takes: Vec<serde_json::Value> = hold
        .takes
        .iter()
        .map(|t| {
            let detail = t
                .output_file_id
                .as_deref()
                .and_then(|id| sheet.details.get(id));
            let is_source = t.output_file_id.is_some() && t.output_file_id == sheet.source_file_id;
            take_json(t, detail, is_source)
        })
        .collect();

    serde_json::json!({
        "run_id": hold.run_id,
        "shot_id": hold.shot_id,
        "label": hold.label,
        "stage_idx": hold.stage_idx,
        "stage_count": hold.stage_count,
        "stage_label": hold.stage_label,
        "created_at": sheet.created_at,
        // FR7's batch, when the run came from one. The board groups by the same
        // field, so the two screens cannot disagree about what a batch is.
        "batch_id": sheet.batch_id,
        // The left-hand side of compare mode: what the held stage was given to
        // work from, which every take of a fan-out shares.
        "source_file_id": sheet.source_file_id,
        "source_thumbnail_url": sheet.source_file_id
            .as_ref()
            .map(|id| format!("/api/files/{}/thumbnail", id)),
        // What `P` would replace.
        "main_file_id": sheet.main_file_id,
        "takes": takes,
        "fanouts": hold.fanouts,
        "tasks_per_take": per_take.iter().sum::<usize>(),
    })
}

#[utoipa::path(
    get,
    path = "/api/comfyui/takes",
    tag = "comfyui",
    summary = "Every run waiting on a verdict",
    description = "The curation lane: one page of held runs, oldest first, each with the takes it \
                   is holding, the file those takes were made from, the shot's current main file, \
                   and what continuing with one take will cost. Oldest first because this is a \
                   backlog somebody is draining, not a feed they are browsing.",
    params(TakesQuery),
    responses(
        (status = 200, description = "A page of held runs"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn list_takes(
    Query(q): Query<TakesQuery>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = require_comfyui(&state)?;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let (sheets, next_cursor) = takes::list_sheets(
        &mut conn,
        q.limit.unwrap_or(takes::DEFAULT_PAGE),
        q.cursor.as_deref(),
    )
    .map_err(super::holds::to_api_error)?;

    let items: Vec<serde_json::Value> = sheets.iter().map(sheet_json).collect();
    let takes_waiting: usize = sheets.iter().map(|s| s.hold.takes.len()).sum();

    Ok(Json(serde_json::json!({
        "items": items,
        "next_cursor": next_cursor,
        // What the lane tab counts, for this page. The board's own count is the
        // whole backlog; this is what arrived.
        "takes_waiting": takes_waiting,
    })))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct RatingPayload {
    /// One to five, or `null` to clear it. Out of range is clamped, not
    /// refused: the lane cannot produce a six, so one is a caller's bug and
    /// losing somebody's keystroke over it helps nobody.
    rating: Option<i32>,
}

#[utoipa::path(
    put,
    path = "/api/files/{id}/rating",
    tag = "files",
    summary = "Rate a file",
    description = "The Takes lane's `1`–`5` keys. `null` clears the rating, which is a different \
                   answer from zero and is drawn differently. Any file may be rated, not only a \
                   generated one.",
    params(("id" = String, Path, description = "File ID")),
    request_body = RatingPayload,
    responses(
        (status = 200, description = "The rating as stored"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error"),
    )
)]
pub(super) async fn put_rating(
    Path(id): Path<String>,
    UState(state): UState,
    Json(payload): Json<RatingPayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;
    let stored = takes::rate_file(&mut conn, &id, payload.rating).map_err(|e| match e {
        diesel::result::Error::NotFound => axum::http::StatusCode::NOT_FOUND.into(),
        e => {
            tracing::error!("Rating {}: {}", id, e);
            ApiError::internal()
        }
    })?;
    Ok(Json(serde_json::json!({ "file_id": id, "rating": stored })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::holds::{Hold, Take};
    use std::collections::HashMap;

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
                    // A real workflow's sampler, deliberately not node 3.
                    parameters: Some(format!(r#"{{"17.noise_seed":{}}}"#, 1000 + i)),
                    completed_at: Some("2026-08-31 09:00:00".to_string()),
                })
                .collect(),
        }
    }

    fn a_sheet() -> Sheet {
        let hold = four_takes();
        let details: HashMap<String, TakeDetail> = (0..4)
            .map(|i| {
                (
                    format!("file-{}", i),
                    TakeDetail {
                        mime_type: Some("video/mp4".to_string()),
                        file_size: Some(140_000_000 + i),
                        rating: if i == 2 { Some(5) } else { None },
                        is_main_file: false,
                    },
                )
            })
            .collect();
        Sheet {
            hold,
            batch_id: Some("batch-a".to_string()),
            source_file_id: Some("file-original".to_string()),
            main_file_id: Some("file-original".to_string()),
            details,
            created_at: Some("2026-08-31 08:00:00".to_string()),
        }
    }

    #[test]
    fn a_card_carries_what_the_keyboard_acts_on() {
        // Every key in the lane's map has to work off what one request already
        // brought: `space` needs the mime type, `X` needs the byte count, `1`–`5`
        // need the rating that is already there, `P` needs to know whether it
        // has anything to do.
        let json = sheet_json(&a_sheet());
        let take = &json["takes"][2];
        assert_eq!(take["mime_type"], serde_json::json!("video/mp4"));
        assert_eq!(take["file_size"], serde_json::json!(140_000_002i64));
        assert_eq!(take["rating"], serde_json::json!(5));
        assert_eq!(take["is_main_file"], serde_json::json!(false));
        assert_eq!(
            take["file_url"],
            serde_json::json!("/api/files/file-2"),
            "space plays the file, not the thumbnail"
        );
    }

    #[test]
    fn four_takes_of_a_fan_out_are_told_apart_by_what_actually_differs() {
        // The bug FR5c's finisher fixed, pinned from the other end: the card
        // reads the seed out of `parameters`, and `parameters` has to arrive
        // whole and per take. Node 17 here, because node 3 is ComfyUI's example
        // graph and almost nobody else's.
        let json = sheet_json(&a_sheet());
        let seeds: Vec<i64> = json["takes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["parameters"]["17.noise_seed"].as_i64().unwrap())
            .collect();
        assert_eq!(seeds, vec![1000, 1001, 1002, 1003]);
        assert_eq!(
            seeds.iter().collect::<std::collections::HashSet<_>>().len(),
            4,
            "four indistinguishable cards is the problem this exists to solve"
        );
    }

    #[test]
    fn the_original_a_take_is_compared_with_comes_with_the_sheet() {
        // Compare mode is the case the whole hold mechanism exists for, and it
        // needs the left-hand side without a second request per run.
        let json = sheet_json(&a_sheet());
        assert_eq!(json["source_file_id"], serde_json::json!("file-original"));
        assert_eq!(
            json["source_thumbnail_url"],
            serde_json::json!("/api/files/file-original/thumbnail")
        );
        assert_eq!(json["main_file_id"], serde_json::json!("file-original"));
        assert!(
            json["takes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["is_source"] == serde_json::json!(false)),
            "none of these four is the file they were made from"
        );
    }

    #[test]
    fn a_batch_is_named_on_the_sheet_so_two_screens_cannot_disagree() {
        assert_eq!(
            sheet_json(&a_sheet())["batch_id"],
            serde_json::json!("batch-a")
        );
        let lonely = Sheet {
            batch_id: None,
            ..a_sheet()
        };
        assert_eq!(sheet_json(&lonely)["batch_id"], serde_json::Value::Null);
    }

    #[test]
    fn a_sentence_has_no_bytes_to_reject_and_says_so() {
        // A describe stage's take. Nothing to play, nothing to promote, nothing
        // to delete — and the card still has to draw.
        let mut sheet = a_sheet();
        sheet.hold.takes[0].output_file_id = None;
        sheet.hold.takes[0].text_output = Some("A jetty at dusk.".to_string());
        let json = sheet_json(&sheet);
        assert_eq!(json["takes"][0]["thumbnail_url"], serde_json::Value::Null);
        assert_eq!(json["takes"][0]["file_url"], serde_json::Value::Null);
        assert_eq!(json["takes"][0]["file_size"], serde_json::Value::Null);
        assert_eq!(json["takes"][0]["rating"], serde_json::Value::Null);
        assert_eq!(
            json["takes"][0]["text_output"],
            serde_json::json!("A jetty at dusk.")
        );
    }

    #[test]
    fn a_take_that_is_already_the_main_file_says_so_rather_than_offering_p() {
        let mut sheet = a_sheet();
        sheet.details.get_mut("file-1").unwrap().is_main_file = true;
        let json = sheet_json(&sheet);
        assert_eq!(json["takes"][1]["is_main_file"], serde_json::json!(true));
        assert_eq!(json["takes"][0]["is_main_file"], serde_json::json!(false));
    }

    #[test]
    fn the_price_of_continuing_rides_along_with_the_sheet() {
        // Two stages below, the first sweeping four ways: one kept take is four
        // upscales and four grades, not two tasks.
        let sheet = Sheet {
            hold: Hold {
                fanouts: vec![4, 1],
                ..four_takes()
            },
            ..a_sheet()
        };
        assert_eq!(sheet_json(&sheet)["tasks_per_take"], serde_json::json!(8));
    }
}
