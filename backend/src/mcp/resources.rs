//! Shots, people and files as MCP resources.
//!
//! The URI space is three flat templates — `phos://shot/{id}`,
//! `phos://person/{id}`, `phos://file/{id}` — because those are the three ids
//! that appear in tool results, and a model that has one should be able to read
//! it without first learning a path grammar.
//!
//! `resources/list` is not the library. It returns the most recent
//! [`RECENT_LIMIT`] shots so a client has something to show before the model
//! asks for anything; everything else is reached through the templates or
//! through a `resource_link` returned by a search. Listing a real library here
//! would be tens of thousands of rows nobody asked for.

use axum::extract::Path;
use axum::http::StatusCode;
use base64::Engine;
use diesel::prelude::*;
use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate,
};
use rmcp::ErrorData as McpError;

use crate::api::{AppState, UState};
use crate::schema::shots;

/// How many shots `resources/list` offers without being asked for anything.
const RECENT_LIMIT: i64 = 100;

/// The width `phos://file/{id}` renders at. Large enough for a model to read
/// detail out of, small enough that a base64 JPEG is not most of a context
/// window; `get_image` lets a caller ask for more when it matters.
pub(crate) const RESOURCE_IMAGE_WIDTH: u32 = 1024;

pub fn shot_uri(id: &str) -> String {
    format!("phos://shot/{id}")
}

pub fn file_uri(id: &str) -> String {
    format!("phos://file/{id}")
}

pub fn person_uri(id: &str) -> String {
    format!("phos://person/{id}")
}

/// The `Resource` a people listing returns as a link.
pub fn person_resource(id: &str, name: Option<&str>) -> Resource {
    Resource::new(person_uri(id), format!("person {id}"))
        .with_mime_type("application/json")
        .with_title(name.unwrap_or("unnamed cluster").to_string())
}

/// The `Resource` a search returns as a link. Carries the caption as the
/// description so the model can decide whether to read it without reading it.
pub fn shot_resource(id: &str, caption: Option<&str>, timestamp: Option<&str>) -> Resource {
    let mut resource = Resource::new(shot_uri(id), format!("shot {id}"))
        .with_mime_type("application/json")
        .with_title(match (caption, timestamp) {
            (Some(c), _) if !c.is_empty() => c.to_string(),
            (_, Some(t)) => t.to_string(),
            _ => format!("shot {id}"),
        });
    if let (Some(caption), Some(timestamp)) = (caption, timestamp) {
        resource = resource.with_description(format!("{timestamp} — {caption}"));
    }
    resource
}

pub fn templates() -> ListResourceTemplatesResult {
    ListResourceTemplatesResult::with_all_items(vec![
        ResourceTemplate::new("phos://shot/{id}", "shot")
            .with_title("Shot record")
            .with_mime_type("application/json")
            .with_description(
                "One moment in the library as JSON: timestamp, AI caption, review status, \
                 the people in it, and every file it holds with that file's id.",
            ),
        ResourceTemplate::new("phos://file/{id}", "file")
            .with_title("File image")
            .with_mime_type("image/jpeg")
            .with_description(
                "The image itself, rendered to a JPEG about 1024px wide. Video files render \
                 their first frame. The id is a *file* id, which shot records list.",
            ),
        ResourceTemplate::new("phos://person/{id}", "person")
            .with_title("Person record")
            .with_mime_type("application/json")
            .with_description(
                "A detected person as JSON: their name and the shots filed under them — the \
                 ones where they are the primary (largest) face — newest first, capped at the \
                 most recent 200. The reserved id `unsorted` returns the shots that belong to \
                 nobody yet.",
            ),
    ])
}

/// The recent slice offered by `resources/list`.
pub async fn list_recent(state: &AppState) -> Result<ListResourcesResult, McpError> {
    let mut conn = state
        .pool
        .get()
        .map_err(|e| McpError::internal_error(format!("database unavailable: {e}"), None))?;

    let rows: Vec<(String, Option<String>, Option<String>)> = shots::table
        .select((shots::id, shots::description, shots::timestamp))
        .order(shots::timestamp.desc())
        .limit(RECENT_LIMIT)
        .load(&mut conn)
        .map_err(|e| McpError::internal_error(format!("failed to list shots: {e}"), None))?;

    Ok(ListResourcesResult::with_all_items(
        rows.iter()
            .map(|(id, description, timestamp)| {
                shot_resource(id, description.as_deref(), timestamp.as_deref())
            })
            .collect(),
    ))
}

/// Read one resource by URI.
pub async fn read(state: &AppState, uri: &str) -> Result<ReadResourceResult, McpError> {
    let not_found = || McpError::resource_not_found(format!("no resource at {uri}"), None);

    let rest = uri.strip_prefix("phos://").ok_or_else(|| {
        McpError::resource_not_found(
            format!("{uri} is not a phos:// URI; see resources/templates/list"),
            None,
        )
    })?;
    let (kind, id) = rest.split_once('/').ok_or_else(not_found)?;
    if id.is_empty() {
        return Err(not_found());
    }

    match kind {
        "shot" => {
            let detail =
                crate::api::shots::get_shot_detail(Path(id.to_string()), UState(state.clone()))
                    .await
                    .map_err(|status| map_status(status, uri))?;
            Ok(ReadResourceResult::new(vec![json_contents(
                uri, &detail.0,
            )?]))
        }
        "person" => {
            let browse =
                crate::api::people::get_person_browse(Path(id.to_string()), UState(state.clone()))
                    .await
                    .map_err(|status| map_status(status, uri))?;
            // Same cap as `person_timeline`: a link the model follows must not
            // be a way around the safeguard the tool applies.
            let mut value = serde_json::to_value(&browse.0)
                .map_err(|e| McpError::internal_error(format!("failed to serialize: {e}"), None))?;
            super::tools::truncate_timeline(&mut value, id);
            Ok(ReadResourceResult::new(vec![json_contents(uri, &value)?]))
        }
        "file" => {
            let jpeg = crate::api::files::thumbnail_bytes(state, id, RESOURCE_IMAGE_WIDTH)
                .await
                .map_err(|status| map_status(status, uri))?;
            Ok(ReadResourceResult::new(vec![ResourceContents::blob(
                base64::engine::general_purpose::STANDARD.encode(jpeg),
                uri,
            )
            .with_mime_type("image/jpeg")]))
        }
        _ => Err(not_found()),
    }
}

fn json_contents<T: serde::Serialize>(uri: &str, value: &T) -> Result<ResourceContents, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("failed to serialize: {e}"), None))?;
    Ok(ResourceContents::text(json, uri).with_mime_type("application/json"))
}

/// Translate a REST handler's status into the MCP error the model should see.
/// A 404 has to stay a "not found" rather than becoming a generic failure, or a
/// model retries a guessed id forever.
fn map_status(status: StatusCode, uri: &str) -> McpError {
    if status == StatusCode::NOT_FOUND {
        McpError::resource_not_found(format!("no resource at {uri}"), None)
    } else {
        McpError::internal_error(format!("reading {uri} failed with {status}"), None)
    }
}
