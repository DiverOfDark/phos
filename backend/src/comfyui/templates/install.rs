//! Putting a template into a library, and keeping it there across upgrades.
//!
//! The only part of this module that touches the database. Everything it
//! decides it asks [`super::marker`] first, which is pure and tested without
//! one.
//!
//! # Install is an import, but it is not the importer
//!
//! A template is a `phos.line` document, so installing one is importing one:
//! resolve each stage's workflow key to a row, build a `LinePayload`, check it,
//! insert the stages. [`install`] is those four steps, and FR5d's importer is
//! the same four in `api::line_io` — `resolve_workflows` + `check_payload` +
//! `insert_stages`.
//!
//! This module was written expecting [`install`] to call that
//! `resolve_workflows` once the two branches met. It does not, and the three
//! reasons are worth writing down so nobody re-litigates them:
//!
//! * **Deduplication is wrong here.** `resolve_workflows` reuses any row whose
//!   canonical graph matches. `POST /templates/{key}/install` promises the
//!   opposite — *a fresh copy, leaving the existing rows alone, because they
//!   may have been edited and they are the user's* — and reusing the previous
//!   install's rows would quietly make two lines share one workflow.
//! * **The marker cuts both ways.** What is stored must carry the `_phos`
//!   block; what the contract, inputs and outputs are derived from must not
//!   (see [`derive_columns`]), so that what is analysed is exactly what will be
//!   dispatched. `resolve_workflows` derives from the graph it stores, and
//!   giving it two graphs would make it a shared shell around two behaviours.
//! * **The layering only runs one way.** `api` depends on `comfyui`, never the
//!   reverse; calling into `api::line_io` from here would be the first
//!   exception, and moving the function down instead would contradict
//!   `portable`'s own "nothing in this module touches a database".
//!
//! What the two paths actually share is the format itself — one
//! [`LineBundle`], one requirements derivation, one readiness check — and that
//! is shared.
//!
//! # Sync
//!
//! On every start, for every template this library has a row for, each seeded
//! workflow is rehashed and one of three things happens:
//!
//! * unedited and behind — the shipped graph is written over it;
//! * unedited and current — nothing;
//! * edited — the `_phos` block is dropped, **the user's content is left
//!   exactly as it is**, and no later upgrade will look at it again.
//!
//! Silently overwriting somebody's edited workflow is the worst thing this file
//! could do, so the abandon path writes precisely one thing: the graph with its
//! marker removed.
//!
//! A template whose row exists is never installed a second time on its own,
//! whatever happened to its rows afterwards. Deleting a workflow, or the line,
//! is a decision; re-seeding over it on the next restart would be Phos
//! overruling it. Putting it back is what the console's Install action is for.

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use serde::Serialize;
use std::collections::BTreeMap;

use super::marker::{self, Verdict};
use crate::comfyui::nodes::NodeCatalog;
use crate::comfyui::portable::LineBundle;
use crate::comfyui::runs::contract_of;
use crate::comfyui::{detect_inputs, detect_outputs, StageContract, StageTyping};
use crate::models::{NewBundledTemplate, NewComfyuiWorkflow, NewLineStage, NewProductionLine};
use crate::schema::{bundled_templates, comfyui_workflows, line_stages, production_lines};

/// What one seeded workflow looks like right now.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct InstalledWorkflow {
    /// The bundle-local key, so a reader can line it up with the template.
    pub key: String,
    pub workflow_id: String,
    /// `None` when the row has been deleted.
    pub name: Option<String>,
    /// Still carries this template's marker, so an upgrade may still update it.
    pub managed: bool,
    /// The template version the marker records.
    pub version: Option<u32>,
}

/// What a library has of one template.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct InstalledState {
    /// The version last seeded.
    pub version: u32,
    pub line_id: Option<String>,
    /// The line row is still there. `false` once the user deletes it.
    pub line_exists: bool,
    pub workflows: Vec<InstalledWorkflow>,
    /// At least one workflow is present and no longer managed: somebody edited
    /// it, so this template will not be updated over their work.
    pub customised: bool,
}

/// What syncing one template did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub installed: bool,
    pub updated: usize,
    pub abandoned: usize,
    pub unchanged: usize,
}

impl SyncOutcome {
    pub fn touched(&self) -> bool {
        self.installed || self.updated > 0 || self.abandoned > 0
    }
}

/// Sync every bundled template into one library.
///
/// Never fails the caller: a template that cannot be written is logged and the
/// rest are still seeded, because a library with four of the five is better
/// than a server that will not start.
pub fn sync_all(
    conn: &mut SqliteConnection,
    bundles: &[LineBundle],
    catalog: Option<&NodeCatalog>,
) -> BTreeMap<String, SyncOutcome> {
    let mut outcomes = BTreeMap::new();
    for bundle in bundles {
        match sync_one(conn, bundle, catalog) {
            Ok(outcome) => {
                if outcome.touched() {
                    tracing::info!(
                        "Bundled template {} v{}: {}{}{}",
                        bundle.key(),
                        bundle.version(),
                        if outcome.installed { "installed " } else { "" },
                        if outcome.updated > 0 {
                            format!("{} workflow(s) updated ", outcome.updated)
                        } else {
                            String::new()
                        },
                        if outcome.abandoned > 0 {
                            format!("{} left to the user (edited)", outcome.abandoned)
                        } else {
                            String::new()
                        }
                    );
                }
                outcomes.insert(bundle.key().to_string(), outcome);
            }
            Err(e) => tracing::error!(
                "Bundled template {} could not be seeded: {}",
                bundle.key(),
                e
            ),
        }
    }
    outcomes
}

/// Install this template if the library has never seen it, otherwise bring it
/// up to date.
pub fn sync_one(
    conn: &mut SqliteConnection,
    bundle: &LineBundle,
    catalog: Option<&NodeCatalog>,
) -> QueryResult<SyncOutcome> {
    match read_row(conn, bundle.key())? {
        None => {
            install(conn, bundle, catalog)?;
            Ok(SyncOutcome {
                installed: true,
                ..Default::default()
            })
        }
        Some(row) => update_in_place(conn, bundle, &row, catalog),
    }
}

/// Write this template's workflows and line, and claim its key.
///
/// The caller has decided this is wanted: `sync_one` only reaches here when the
/// library has no row for the key, and the console's Install action removes the
/// row first. In one transaction, so a half-written line cannot be left behind.
pub fn install(
    conn: &mut SqliteConnection,
    bundle: &LineBundle,
    catalog: Option<&NodeCatalog>,
) -> QueryResult<String> {
    conn.transaction(|conn| {
        let mut ids: BTreeMap<String, String> = BTreeMap::new();
        for (key, wf) in &bundle.workflows {
            let id = uuid::Uuid::new_v4().to_string();
            let marked = marker::with_marker(&wf.graph, bundle.key(), key, bundle.version());
            let derived = derive_columns(&wf.graph, wf.contract.as_ref(), catalog);
            diesel::insert_into(comfyui_workflows::table)
                .values(NewComfyuiWorkflow {
                    id: &id,
                    name: &wf.name,
                    description: wf.description.as_deref(),
                    workflow_json: &serde_json::to_string(&marked).unwrap_or_default(),
                    inputs_json: Some(&derived.inputs),
                    outputs_json: Some(&derived.outputs),
                    contract_json: Some(&derived.contract),
                })
                .execute(conn)?;
            ids.insert(key.clone(), id);
        }

        let line_id = uuid::Uuid::new_v4().to_string();
        diesel::insert_into(production_lines::table)
            .values(NewProductionLine {
                id: &line_id,
                name: &bundle.line.name,
                description: bundle.line.description.as_deref(),
            })
            .execute(conn)?;

        // Stage order is array order: the file has no `stage_idx`, and the
        // column is filled from the position rather than from a second source.
        for (idx, stage) in bundle.line.stages.iter().enumerate() {
            let Some(workflow_id) = ids.get(&stage.workflow) else {
                // `LineBundle::problems` refuses this at test time; if one ever
                // got through, a line missing a stage is worse than no line.
                return Err(diesel::result::Error::RollbackTransaction);
            };
            let text_overrides = serde_json::to_string(&stage.text_overrides).unwrap_or_default();
            let parameters = serde_json::to_string(&stage.parameters).unwrap_or_default();
            let vary = serde_json::to_string(&stage.vary).unwrap_or_default();
            diesel::insert_into(line_stages::table)
                .values(NewLineStage {
                    id: &uuid::Uuid::new_v4().to_string(),
                    line_id: &line_id,
                    stage_idx: idx as i32,
                    workflow_id,
                    text_overrides: Some(&text_overrides),
                    parameters: Some(&parameters),
                    vary: Some(&vary),
                    source_mode: stage.source_mode.as_deref(),
                    keep_output: stage.keep_output,
                    exposed: None,
                })
                .execute(conn)?;
        }

        diesel::insert_into(bundled_templates::table)
            .values(NewBundledTemplate {
                template_key: bundle.key(),
                template_version: bundle.version() as i32,
                line_id: Some(&line_id),
                workflow_ids: &serde_json::to_string(&ids).unwrap_or_else(|_| "{}".to_string()),
            })
            .execute(conn)?;

        Ok(line_id)
    })
}

/// Forget this library's claim on a template, leaving every row it wrote alone.
///
/// The workflows and the line stay — they are the user's now, markers and all —
/// and the key becomes installable again.
pub fn release(conn: &mut SqliteConnection, key: &str) -> QueryResult<usize> {
    diesel::delete(bundled_templates::table.filter(bundled_templates::template_key.eq(key)))
        .execute(conn)
}

/// What a library holds of this template, for the console to render.
pub fn state_of(
    conn: &mut SqliteConnection,
    bundle: &LineBundle,
) -> QueryResult<Option<InstalledState>> {
    let Some(row) = read_row(conn, bundle.key())? else {
        return Ok(None);
    };

    let line_exists = match row.line_id.as_deref() {
        Some(id) => {
            production_lines::table
                .filter(production_lines::id.eq(id))
                .count()
                .get_result::<i64>(conn)?
                > 0
        }
        None => false,
    };

    let mut workflows = Vec::new();
    let mut customised = false;
    for key in bundle.workflows.keys() {
        let Some(workflow_id) = row.workflow_ids.get(key) else {
            continue;
        };
        let stored: Option<(String, String)> = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(workflow_id))
            .select((comfyui_workflows::name, comfyui_workflows::workflow_json))
            .first(conn)
            .optional()?;
        let (name, marker) = match stored {
            Some((name, json)) => {
                let graph: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                (Some(name), marker::read_marker(&graph))
            }
            None => (None, None),
        };
        if name.is_some() && marker.is_none() {
            customised = true;
        }
        workflows.push(InstalledWorkflow {
            key: key.clone(),
            workflow_id: workflow_id.clone(),
            name,
            managed: marker.is_some(),
            version: marker.map(|m| m.template_version),
        });
    }

    Ok(Some(InstalledState {
        version: row.template_version,
        line_id: row.line_id,
        line_exists,
        workflows,
        customised,
    }))
}

// ===== The upgrade ==========================================================

struct Row {
    template_version: u32,
    line_id: Option<String>,
    workflow_ids: BTreeMap<String, String>,
}

fn read_row(conn: &mut SqliteConnection, key: &str) -> QueryResult<Option<Row>> {
    let row: Option<(i32, Option<String>, String)> = bundled_templates::table
        .filter(bundled_templates::template_key.eq(key))
        .select((
            bundled_templates::template_version,
            bundled_templates::line_id,
            bundled_templates::workflow_ids,
        ))
        .first(conn)
        .optional()?;
    Ok(row.map(|(version, line_id, ids)| Row {
        template_version: version.max(0) as u32,
        line_id,
        workflow_ids: serde_json::from_str(&ids).unwrap_or_default(),
    }))
}

fn update_in_place(
    conn: &mut SqliteConnection,
    bundle: &LineBundle,
    row: &Row,
    catalog: Option<&NodeCatalog>,
) -> QueryResult<SyncOutcome> {
    let mut outcome = SyncOutcome::default();

    for (key, wf) in &bundle.workflows {
        let Some(workflow_id) = row.workflow_ids.get(key) else {
            // A workflow the bundle gained after this library was seeded. It
            // has no row to update and no line stage pointing at it, so adding
            // one here would be half a migration; the console's Install action
            // is how a person asks for the new shape.
            continue;
        };
        let stored_json: Option<String> = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(workflow_id))
            .select(comfyui_workflows::workflow_json)
            .first(conn)
            .optional()?;
        let Some(stored_json) = stored_json else {
            continue; // deleted by the user
        };
        let stored: serde_json::Value = match serde_json::from_str(&stored_json) {
            Ok(v) => v,
            // A graph nothing can parse is not one to overwrite: it is either
            // corrupt or from a Phos that stores something else, and either way
            // guessing loses whatever is in there.
            Err(_) => continue,
        };

        let shipped_hash = marker::content_hash(&wf.graph);
        match marker::decide(&stored, bundle.version(), &shipped_hash) {
            Verdict::UpToDate | Verdict::Unmanaged => outcome.unchanged += 1,
            Verdict::Update => {
                let marked = marker::with_marker(&wf.graph, bundle.key(), key, bundle.version());
                let derived = derive_columns(&wf.graph, wf.contract.as_ref(), catalog);
                diesel::update(
                    comfyui_workflows::table.filter(comfyui_workflows::id.eq(workflow_id)),
                )
                .set((
                    comfyui_workflows::name.eq(&wf.name),
                    comfyui_workflows::description.eq(wf.description.as_deref()),
                    comfyui_workflows::workflow_json
                        .eq(serde_json::to_string(&marked).unwrap_or_default()),
                    comfyui_workflows::inputs_json.eq(Some(&derived.inputs)),
                    comfyui_workflows::outputs_json.eq(Some(&derived.outputs)),
                    comfyui_workflows::contract_json.eq(Some(&derived.contract)),
                ))
                .execute(conn)?;
                outcome.updated += 1;
            }
            Verdict::Abandon => {
                // Everything the user has stays. The one thing that changes is
                // that the marker is gone, so no upgrade ever looks again.
                let released = marker::strip_marker(&stored);
                diesel::update(
                    comfyui_workflows::table.filter(comfyui_workflows::id.eq(workflow_id)),
                )
                .set(
                    comfyui_workflows::workflow_json
                        .eq(serde_json::to_string(&released).unwrap_or(stored_json)),
                )
                .execute(conn)?;
                outcome.abandoned += 1;
            }
        }
    }

    if row.template_version != bundle.version() {
        diesel::update(
            bundled_templates::table.filter(bundled_templates::template_key.eq(bundle.key())),
        )
        .set((
            bundled_templates::template_version.eq(bundle.version() as i32),
            bundled_templates::updated_at.eq(diesel::dsl::now),
        ))
        .execute(conn)?;
    }

    Ok(outcome)
}

// ===== The three derived columns ============================================

struct Derived {
    inputs: String,
    outputs: String,
    contract: String,
}

/// The same three columns an imported workflow gets, from the same functions.
///
/// The contract is *re-derived* against this box's catalogue; only the
/// corrections the document carries survive verbatim, which is FR5d's rule for
/// an imported line and is right for the same reason — a contract is a fact
/// about the machine that runs the graph, a correction is a person's judgement.
///
/// Derived from the graph *without* its marker, so what is analysed is exactly
/// what will be dispatched.
fn derive_columns(
    graph: &serde_json::Value,
    contract: Option<&StageContract>,
    catalog: Option<&NodeCatalog>,
) -> Derived {
    let bare = marker::strip_marker(graph);
    let corrections = contract.map(|c| c.corrections.clone()).unwrap_or_default();
    let contract = StageContract::derive_with(&bare, catalog, corrections);
    Derived {
        inputs: serde_json::to_string(&detect_inputs(&bare, catalog)).unwrap_or_default(),
        outputs: serde_json::to_string(&detect_outputs(&bare)).unwrap_or_default(),
        contract: serde_json::to_string(&contract).unwrap_or_default(),
    }
}

/// The typing of the line a bundle installs, for `validate_chain`.
///
/// Read off the same contracts the API would, so a bundled line is refused by
/// exactly the rule a hand-drawn one is.
pub fn typings(bundle: &LineBundle, catalog: Option<&NodeCatalog>) -> Vec<StageTyping> {
    bundle
        .line
        .stages
        .iter()
        .enumerate()
        .filter_map(|(idx, stage)| {
            let wf = bundle.workflow(&stage.workflow)?;
            let derived = derive_columns(&wf.graph, wf.contract.as_ref(), catalog);
            let contract = contract_of(
                Some(&derived.contract),
                &serde_json::to_string(&wf.graph).unwrap_or_default(),
            );
            Some(StageTyping {
                stage_idx: idx as i32,
                name: wf.name.clone(),
                accepts: contract.accepts,
                produces: contract.produces,
            })
        })
        .collect()
}
