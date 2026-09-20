//! The tools the model actually calls.
//!
//! Every one of these is a thin shim over the handler behind the matching REST
//! route: the shim owns the *shape* (what a model should be asked for, and what
//! it gets back), never the logic. That is deliberate — face reassignment
//! recalculating a shot's primary person, a rename moving a folder on disk, the
//! unsorted-shots definition — all of it is subtle, all of it lives in
//! `crate::api`, and a second copy here would be a second copy that drifts.
//!
//! Payloads are built with `serde_json::from_value` rather than struct
//! literals so the REST payload types keep their private fields; if a payload
//! gains a required field, this file fails to build rather than silently
//! sending a default.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::Json;
use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars::JsonSchema;
use rmcp::ErrorData as McpError;
use serde::Deserialize;
use serde_json::json;

use super::resources::{file_uri, person_resource, shot_resource};
use super::PhosMcp;
use crate::api::UState;

/// Default and ceiling for `search_shots`. A model that asks for "all of them"
/// gets a page, not a library: the ceiling is what keeps one careless call from
/// spending the whole context window.
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

/// How many shots a person's timeline inlines before it starts counting instead.
const TIMELINE_LIMIT: usize = 200;

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchShotsParams {
    /// Free text matched against each shot's AI-written caption and its file
    /// paths. Omit to browse by the other filters alone.
    pub query: Option<String>,
    /// Only shots whose primary person is this person id (from `list_people`).
    pub person_id: Option<String>,
    /// Earliest shot timestamp to include, ISO-8601, e.g. `2024-06-01`.
    pub from: Option<String>,
    /// Latest shot timestamp to include, ISO-8601.
    pub to: Option<String>,
    /// Review status: `pending`, `confirmed`, or `unsorted` for shots that
    /// belong to no person yet.
    pub status: Option<String>,
    /// Maximum shots to return. Defaults to 50, capped at 200.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShotIdParams {
    /// The shot id, as returned by `search_shots`.
    pub shot_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPeopleParams {
    /// Case-insensitive substring of the person's name. Omit to list everyone.
    pub name_contains: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PersonTimelineParams {
    /// The person id, or the reserved id `unsorted` for shots nobody owns yet.
    pub person_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetImageParams {
    /// A **file** id — not a shot id. Shot records list the files they hold.
    pub file_id: String,
    /// Width in pixels to render at, 64–1920. Defaults to 1024.
    pub width: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenamePersonParams {
    /// The person to rename.
    pub person_id: String,
    /// The new name. This also renames the person's folder on disk.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MergePeopleParams {
    /// The person to merge away. Every face of theirs moves to the target.
    pub source_id: String,
    /// The person to keep.
    pub target_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssignFaceParams {
    /// The face to reassign, from a shot record's face list.
    pub face_id: String,
    /// The person the face belongs to.
    pub person_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfirmShotsParams {
    /// The shots whose people are correct as they stand.
    pub shot_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScanParams {
    /// Directory to scan. Must be inside this library; defaults to the whole
    /// library.
    pub path: Option<String>,
}

// ---------------------------------------------------------------------------
// Read tools
// ---------------------------------------------------------------------------

#[rmcp::tool_router(router = read_tool_router, vis = "pub(crate)")]
impl PhosMcp {
    /// Find shots, and answer with links rather than records.
    #[rmcp::tool(
        name = "search_shots",
        description = "Search the library for shots (one shot = one moment, holding one or \
                       more files). Filter by free text over the AI caption and file path, by \
                       person, by date range, or by review status. Returns links to \
                       phos://shot/{id} resources — read the ones that look relevant.",
        annotations(title = "Search shots", read_only_hint = true)
    )]
    pub async fn search_shots(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<SearchShotsParams>,
    ) -> Result<CallToolResult, McpError> {
        let params = params.0;
        let limit = params
            .limit
            .map(|l| (l as usize).min(MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);

        let query: crate::api::shots::ShotsQuery = from_json(json!({
            "q": params.query,
            "person_id": params.person_id,
            "status": params.status,
            "from": params.from,
            "to": params.to,
        }))?;

        let Json(shots) =
            crate::api::shots::get_shots(UState(self.state.clone()), Query(query)).await;

        let total = shots.len();
        let mut content = vec![ContentBlock::text(if total == 0 {
            "No shots matched. Free-text search reads the AI caption and the file path, not \
             the image itself, so try a broader phrase or drop a filter."
                .to_string()
        } else if total > limit {
            format!("{total} shots matched; the first {limit} are linked below.")
        } else {
            format!("{total} shot(s) matched.")
        })];

        content.extend(shots.iter().take(limit).map(|shot| {
            ContentBlock::resource_link(shot_resource(
                &shot.id,
                shot.description.as_deref(),
                shot.timestamp.as_deref(),
            ))
        }));

        Ok(CallToolResult::success(content))
    }

    /// One shot in full, including the file ids `get_image` needs.
    #[rmcp::tool(
        name = "get_shot",
        description = "Read one shot in full: timestamp, AI caption, review status, the people \
                       detected in it, and every file it holds with that file's id. Pass a file \
                       id to get_image to actually look at the photo.",
        annotations(title = "Get shot", read_only_hint = true)
    )]
    pub async fn get_shot(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<ShotIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let detail =
            crate::api::shots::get_shot_detail(Path(params.0.shot_id), UState(self.state.clone()))
                .await
                .map_err(status_error)?;
        json_result(&detail.0)
    }

    /// The people the library knows about.
    #[rmcp::tool(
        name = "list_people",
        description = "List the people detected in the library, with how many faces and shots \
                       each has. Unnamed clusters come back with a null name — they are still \
                       usable ids.",
        annotations(title = "List people", read_only_hint = true)
    )]
    pub async fn list_people(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<ListPeopleParams>,
    ) -> Result<CallToolResult, McpError> {
        let people = crate::api::people::get_people(UState(self.state.clone()))
            .await
            .map_err(status_error)?;

        let mut value = serde_json::to_value(&people.0)
            .map_err(|e| McpError::internal_error(format!("failed to serialize: {e}"), None))?;

        if let Some(filter) = params.0.name_contains {
            let needle = filter.to_lowercase();
            if let Some(array) = value.as_array_mut() {
                array.retain(|person| {
                    person["name"]
                        .as_str()
                        .is_some_and(|n| n.to_lowercase().contains(&needle))
                });
            }
        }

        // The counts are the answer; the links are what to do about it, so both
        // go back. A model that wants one person's shots can read the resource
        // rather than guessing at `person_timeline`'s argument.
        let mut content = vec![ContentBlock::text(
            serde_json::to_string_pretty(&value)
                .map_err(|e| McpError::internal_error(format!("failed to serialize: {e}"), None))?,
        )];
        if let Some(array) = value.as_array() {
            content.extend(array.iter().filter_map(|person| {
                let id = person["id"].as_str()?;
                Some(ContentBlock::resource_link(person_resource(
                    id,
                    person["name"].as_str(),
                )))
            }));
        }

        Ok(CallToolResult::success(content))
    }

    /// The shots filed under one person.
    #[rmcp::tool(
        name = "person_timeline",
        description = "The shots filed under one person — the ones where they are the primary \
                       (largest) face — newest first, with each shot's files. A group photo \
                       where they are not the primary face is not listed here; use \
                       search_shots to look wider. Pass the reserved id `unsorted` for the \
                       shots that belong to nobody yet.",
        annotations(title = "Person timeline", read_only_hint = true)
    )]
    pub async fn person_timeline(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<PersonTimelineParams>,
    ) -> Result<CallToolResult, McpError> {
        let browse = crate::api::people::get_person_browse(
            Path(params.0.person_id.clone()),
            UState(self.state.clone()),
        )
        .await
        .map_err(status_error)?;

        let mut value = serde_json::to_value(&browse.0)
            .map_err(|e| McpError::internal_error(format!("failed to serialize: {e}"), None))?;

        truncate_timeline(&mut value, &params.0.person_id);

        json_result(&value)
    }

    /// The photo itself.
    #[rmcp::tool(
        name = "get_image",
        description = "Look at a file: returns the image as a JPEG, resized. Videos return \
                       their first frame. The id must be a FILE id (shot records list them), \
                       not a shot id.",
        annotations(title = "Get image", read_only_hint = true)
    )]
    pub async fn get_image(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<GetImageParams>,
    ) -> Result<CallToolResult, McpError> {
        let params = params.0;
        let width = params
            .width
            .unwrap_or(super::resources::RESOURCE_IMAGE_WIDTH);
        let jpeg = crate::api::files::thumbnail_bytes(&self.state, &params.file_id, width)
            .await
            .map_err(status_error)?;

        Ok(CallToolResult::success(vec![
            ContentBlock::text(format!(
                "{} ({} bytes)",
                file_uri(&params.file_id),
                jpeg.len()
            )),
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(jpeg),
                "image/jpeg",
            ),
        ]))
    }

    /// How big the library is, and how much of it is still unreviewed.
    #[rmcp::tool(
        name = "library_stats",
        description = "Counts for the whole library: shots, files, people, faces, and how many \
                       shots are still waiting to be reviewed.",
        annotations(title = "Library stats", read_only_hint = true)
    )]
    pub async fn library_stats(&self) -> Result<CallToolResult, McpError> {
        let stats = crate::api::stats::get_stats(UState(self.state.clone())).await;
        json_result(&stats.0)
    }
}

// ---------------------------------------------------------------------------
// Write tools — only present when the server was built writable
// ---------------------------------------------------------------------------

#[rmcp::tool_router(router = write_tool_router, vis = "pub(crate)")]
impl PhosMcp {
    #[rmcp::tool(
        name = "rename_person",
        description = "Rename a person. This also renames their folder on disk, so the files \
                       move.",
        annotations(
            title = "Rename person",
            read_only_hint = false,
            destructive_hint = true
        )
    )]
    pub async fn rename_person(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<RenamePersonParams>,
    ) -> Result<CallToolResult, McpError> {
        let params = params.0;
        let payload: crate::api::people::RenamePersonPayload =
            from_json(json!({ "name": params.name }))?;
        let result = crate::api::people::rename_person(
            Path(params.person_id),
            UState(self.state.clone()),
            Json(payload),
        )
        .await
        .map_err(status_error)?;
        json_result(&result.0)
    }

    #[rmcp::tool(
        name = "merge_people",
        description = "Merge one person into another. Every face of the source moves to the \
                       target and the source person is deleted. Not reversible.",
        annotations(
            title = "Merge people",
            read_only_hint = false,
            destructive_hint = true
        )
    )]
    pub async fn merge_people(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<MergePeopleParams>,
    ) -> Result<CallToolResult, McpError> {
        let params = params.0;
        let payload: crate::api::people::MergePeoplePayload = from_json(json!({
            "source_id": params.source_id,
            "target_id": params.target_id,
        }))?;
        let result = crate::api::people::merge_people(UState(self.state.clone()), Json(payload))
            .await
            .map_err(status_error)?;
        json_result(&result.0)
    }

    #[rmcp::tool(
        name = "assign_face",
        description = "Assign one detected face to a person. On a shot that has not been \
                       confirmed yet, this also updates the shot's primary person when that \
                       face is the largest one on it; a confirmed shot keeps the primary \
                       person it was confirmed with.",
        annotations(title = "Assign face", read_only_hint = false, destructive_hint = true)
    )]
    pub async fn assign_face(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<AssignFaceParams>,
    ) -> Result<CallToolResult, McpError> {
        let params = params.0;
        let payload: crate::api::faces::ReassignFacePayload =
            from_json(json!({ "person_id": params.person_id }))?;
        let result = crate::api::faces::reassign_face(
            Path(params.face_id),
            UState(self.state.clone()),
            Json(payload),
        )
        .await
        .map_err(status_error)?;
        json_result(&result.0)
    }

    #[rmcp::tool(
        name = "confirm_shots",
        description = "Mark shots as reviewed: their people are correct and automatic \
                       clustering should stop second-guessing them.",
        annotations(
            title = "Confirm shots",
            read_only_hint = false,
            destructive_hint = false
        )
    )]
    pub async fn confirm_shots(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<ConfirmShotsParams>,
    ) -> Result<CallToolResult, McpError> {
        let payload: crate::api::shots::BatchConfirmPayload =
            from_json(json!({ "shot_ids": params.0.shot_ids }))?;
        let result = crate::api::shots::batch_confirm(UState(self.state.clone()), Json(payload))
            .await
            .map_err(status_error)?;
        json_result(&result.0)
    }

    #[rmcp::tool(
        name = "trigger_scan",
        description = "Scan the library for new or changed files. Runs in the background and \
                       returns immediately; poll library_stats to see it land.",
        annotations(
            title = "Trigger scan",
            read_only_hint = false,
            destructive_hint = false
        )
    )]
    pub async fn trigger_scan(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<ScanParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = match params.0.path {
            Some(path) => confine_to_library(&self.state.library_root, &path)?,
            None => self.state.library_root.to_string_lossy().to_string(),
        };
        let payload: crate::api::stats::ScanParams = from_json(json!({ "path": path }))?;
        let result =
            crate::api::stats::trigger_scan(UState(self.state.clone()), Json(payload)).await;
        json_result(&result.0)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Cap a person browse graph at [`TIMELINE_LIMIT`] shots, in place.
///
/// A prolific person's whole graph is far more than a model needs in one
/// answer, so it is truncated here rather than at the handler, which the
/// offline-first Android client relies on returning everything. The kept shots
/// are the newest ones, which is the order the handler returns them in and the
/// order the tool advertises. The `phos://person/{id}` resource applies the
/// same cap, so following the link cannot smuggle the whole graph back in.
pub(super) fn truncate_timeline(value: &mut serde_json::Value, person_id: &str) {
    let mut truncated = 0usize;
    if let Some(shots) = value.get_mut("shots").and_then(|s| s.as_array_mut()) {
        if shots.len() > TIMELINE_LIMIT {
            truncated = shots.len() - TIMELINE_LIMIT;
            shots.truncate(TIMELINE_LIMIT);
        }
    }
    if truncated > 0 {
        value["truncated_shots"] = json!(truncated);
        value["truncation_note"] = json!(format!(
            "{truncated} older shot(s) omitted; narrow with search_shots person_id={person_id} \
             plus a date range."
        ));
    }
}

/// Resolve a caller-supplied scan path against the library it is allowed to
/// touch, rejecting anything outside it.
///
/// A token names one library, so a scan has to stay inside that library's root.
/// Without this, a token holder could point the scanner at any directory the
/// server can see — another user's library, or the filesystem at large — and
/// `Scanner` would index those files into *this* database (as absolute paths,
/// which `get_image` then happily serves) and may delete an outside file it
/// decides is a duplicate of one it already holds. Both components are
/// canonicalized first so `..` and symlinks cannot walk out of the root.
fn confine_to_library(root: &std::path::Path, requested: &str) -> Result<String, McpError> {
    let outside = || {
        McpError::invalid_params(
            "path must be inside this library; omit it to scan the whole library",
            None,
        )
    };

    let root = root
        .canonicalize()
        .map_err(|e| McpError::internal_error(format!("library root is unreadable: {e}"), None))?;
    let path = std::path::Path::new(requested)
        .canonicalize()
        .map_err(|_| outside())?;

    if !path.starts_with(&root) {
        return Err(outside());
    }
    Ok(path.to_string_lossy().to_string())
}

/// Build a REST payload from JSON, so payload structs keep their private fields.
fn from_json<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, McpError> {
    serde_json::from_value(value)
        .map_err(|e| McpError::invalid_params(format!("invalid arguments: {e}"), None))
}

fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("failed to serialize: {e}"), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

/// Turn a handler's status code into an MCP error. A 404 stays recognisable as
/// "that id does not exist" so a model corrects the id instead of retrying.
fn status_error(status: StatusCode) -> McpError {
    match status {
        StatusCode::NOT_FOUND => McpError::invalid_params("no such id in this library", None),
        StatusCode::BAD_REQUEST => McpError::invalid_params("the request was rejected", None),
        other => McpError::internal_error(format!("phos returned {other}"), None),
    }
}
