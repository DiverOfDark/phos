//! Bundled templates: the five lines a fresh install already has.
//!
//! Two endpoints. Listing them is mostly [`crate::comfyui::templates`]'s work —
//! what ships, what this library already has of it, and what this ComfyUI is
//! missing before it could run. Installing one is a single call into the same
//! seeding path startup uses.
//!
//! # Readiness is read on every list
//!
//! Not cached beside the template, because the answer is about the *server*,
//! not the template: someone drops a checkpoint into `models/` and the answer
//! changes with no request to Phos in between. The catalogue underneath is
//! cached until the health check sees ComfyUI come back, so this costs one HTTP
//! call the first time and nothing after.
//!
//! When it cannot be read the templates still list, marked `UNKNOWN`, and
//! Install still works. Refusing to show somebody their templates because their
//! GPU box is asleep would be the wrong trade every time.

use axum::{extract::Path, http::StatusCode, Json};

use crate::comfyui::portable::LineBundle;
use crate::comfyui::templates::{self, install, readiness};

use super::comfyui::{node_catalog, require_comfyui, ApiError};
use super::UState;

/// One template, as the console draws it.
fn template_json(
    bundle: &LineBundle,
    catalog: Option<&crate::comfyui::NodeCatalog>,
    installed: Option<install::InstalledState>,
) -> serde_json::Value {
    let readiness = readiness::assess(bundle, catalog);
    // Derived, not stored: it is what the line editor will show when somebody
    // drops this template into a chain, and it must agree with the validator.
    let typings = install::typings(bundle, catalog);
    let template = bundle.template();
    serde_json::json!({
        "key": bundle.key(),
        "name": bundle.line.name,
        "summary": template.and_then(|t| t.summary.clone()),
        "version": bundle.version(),
        "confidence": template.map(|t| t.confidence),
        "notes": template.and_then(|t| t.notes.clone()),
        "stage_count": bundle.line.stages.len(),
        "accepts": typings.first().map(|t| t.accepts),
        "produces": typings.last().map(|t| t.produces),
        // Read off the graphs, not out of the document's own `requirements`
        // block: what will run is what the graphs say.
        "requirements": crate::comfyui::portable::Requirements::derive(bundle.graphs()),
        "readiness": readiness,
        "installed": installed,
    })
}

#[utoipa::path(
    get,
    path = "/api/comfyui/templates",
    tag = "comfyui",
    summary = "List bundled templates",
    description = "The templates this build ships, what this library already has of each, and \
                   whether this ComfyUI has the nodes and models to run it. Readiness is \
                   `unknown` — never `missing` — when the node catalogue cannot be read.",
    responses(
        (status = 200, description = "Bundled templates with readiness"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn list_templates(
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = require_comfyui(&state)?;
    let catalog = node_catalog(&url).await;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let mut items = Vec::new();
    for bundle in templates::bundled() {
        let installed = install::state_of(&mut conn, bundle).map_err(|e| {
            tracing::error!("Reading template state failed: {}", e);
            ApiError::internal()
        })?;
        items.push(template_json(bundle, catalog.as_deref(), installed));
    }

    Ok(Json(serde_json::json!({
        "items": items,
        // So the console can say "Phos could not ask" once, rather than five
        // times in five status pills.
        "catalog_available": catalog.is_some(),
    })))
}

#[utoipa::path(
    post,
    path = "/api/comfyui/templates/{key}/install",
    tag = "comfyui",
    summary = "Install a bundled template",
    description = "Write this template's workflows and its line into the library. Installing one \
                   that is already installed gives a fresh copy and leaves the existing rows \
                   alone — they may have been edited, and they are the user's.",
    params(("key" = String, Path, description = "Template key")),
    responses(
        (status = 200, description = "Installed"),
        (status = 404, description = "No such template"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn install_template(
    Path(key): Path<String>,
    UState(state): UState,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = require_comfyui(&state)?;
    let Some(bundle) = templates::bundled_by_key(&key) else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    // Worth the round trip here even though seeding does without one: this is
    // the moment a person is watching, and a contract derived with the
    // catalogue types the stage's parameters properly.
    let catalog = node_catalog(&url).await;
    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    // Installing again is a *fresh copy*: the claim on the key moves to the new
    // rows, and whatever was there stays exactly as it is. Overwriting would
    // undo an edit, which is the one thing this feature must never do.
    install::release(&mut conn, &key).map_err(|_| ApiError::internal())?;
    let line_id = install::install(&mut conn, bundle, catalog.as_deref()).map_err(|e| {
        tracing::error!("Installing template {} failed: {}", key, e);
        ApiError::internal()
    })?;

    let installed = install::state_of(&mut conn, bundle).map_err(|_| ApiError::internal())?;
    Ok(Json(serde_json::json!({
        "line_id": line_id,
        "template": template_json(bundle, catalog.as_deref(), installed),
    })))
}
