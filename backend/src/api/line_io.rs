//! Lines travel: exporting one to a file, and reading one back in.
//!
//! [`crate::comfyui::portable`] decides what the file *is* and what it needs;
//! this puts a line into one and takes one out of the database on the other
//! side. It is a sibling of [`super::lines`] rather than more of it because
//! everything here is about a document, and because import reaches straight
//! back into line CRUD — the same [`super::lines::check_payload`] a `POST` runs
//! — rather than re-deciding anything.
//!
//! # Import is honest before it is useful
//!
//! The report comes first. `?dry_run=true` writes nothing and answers with the
//! whole requirements check, so the import dialog can say "this needs
//! WanImageToVideo, which this server does not have" while the person is still
//! deciding — never at dispatch, four stages into a run.
//!
//! Then it imports anyway. Missing requirements are not a refusal: a perfectly
//! ordinary setup is a NAS holding the library and a workstation holding the
//! GPU, and a line filed on the box that cannot run it today is still a line.
//! The two things import *does* refuse are a file it cannot read and a chain
//! that does not type-check here, and the second one carries the report with
//! it, because "stage 2 takes video but stage 1 produces image" and "the node
//! that would have made the video is not installed" are the same sentence said
//! twice.
//!
//! # Three things that would otherwise go quietly wrong
//!
//! * **A name already in use.** The import is renamed, never merged over the
//!   top of what is there, and the response says what it ended up called.
//! * **A workflow already present.** Matched on the graph plus the contract
//!   corrections it travels with ([`workflow_identity`]), so re-importing a
//!   line you exported reuses the workflows it came from instead of leaving a
//!   second copy of each — while a bundle whose corrections disagree with the
//!   row already here gets its own row rather than someone else's contract.
//! * **A contract from another box.** Re-derived here, against this ComfyUI's
//!   catalogue — a contract is a fact about the machine that will run the graph.
//!   The `corrections` inside it are kept verbatim, because a correction is the
//!   one part nobody can work out again.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::comfyui::portable::{
    available_name, workflow_identity, BundleLine, BundleStage, BundleWorkflow, LineBundle,
};
use crate::comfyui::runs::contract_of;
use crate::comfyui::{ContractCorrections, NodeCatalog, StageContract};
use crate::models::{NewComfyuiWorkflow, NewProductionLine};
use crate::schema::{comfyui_workflows, production_lines};

use super::comfyui::{node_catalog, require_comfyui, ApiError};
use super::lines::{check_payload, insert_stages, line_json, LinePayload, LineStagePayload};
use super::UState;

// ===== Export ===============================================================

#[utoipa::path(
    get,
    path = "/api/comfyui/lines/{id}/export",
    tag = "comfyui",
    summary = "Export a production line",
    description = "The line, its stages, the workflow graph behind every stage, and a manifest \
                   of the node classes and model files it needs — as one JSON document that can \
                   be imported into another library or another Phos install. This is the same \
                   format bundled templates ship in.",
    params(("id" = String, Path, description = "Line ID")),
    responses(
        (status = 200, description = "The line as a portable bundle", body = LineBundle),
        (status = 404, description = "Line not found"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn export_line(
    Path(id): Path<String>,
    UState(state): UState,
) -> Result<Json<LineBundle>, ApiError> {
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

    let stages =
        crate::comfyui::runs::stages_of_line(&mut conn, &id).map_err(|_| ApiError::internal())?;
    if stages.is_empty() {
        return Err(ApiError::bad_request("This line has no stages to export."));
    }

    // The graphs. Without them the export is a list of ids that mean nothing
    // anywhere else, so this is the part that makes the file portable.
    let mut workflows: BTreeMap<String, BundleWorkflow> = BTreeMap::new();
    for stage in &stages {
        if workflows.contains_key(&stage.workflow_id) {
            continue;
        }
        let row: Option<(String, Option<String>, String, Option<String>)> =
            comfyui_workflows::table
                .filter(comfyui_workflows::id.eq(&stage.workflow_id))
                .select((
                    comfyui_workflows::name,
                    comfyui_workflows::description,
                    comfyui_workflows::workflow_json,
                    comfyui_workflows::contract_json,
                ))
                .first(&mut conn)
                .optional()
                .map_err(|_| ApiError::internal())?;
        let Some((wf_name, wf_description, graph_json, contract_json)) = row else {
            // A stage whose workflow is gone. The foreign key should prevent
            // it; if it happened anyway, an export that silently dropped the
            // stage would be worse than one that says so.
            return Err(ApiError::bad_request(format!(
                "Stage {} names workflow {}, which is no longer in this library.",
                stage.stage_idx + 1,
                stage.workflow_id
            )));
        };
        let graph: serde_json::Value =
            serde_json::from_str(&graph_json).map_err(|_| ApiError::internal())?;
        workflows.insert(
            stage.workflow_id.clone(),
            BundleWorkflow {
                name: wf_name,
                description: wf_description,
                // Stored, or derived on the spot for a workflow imported before
                // contracts existed — exactly what a line read does.
                contract: Some(contract_of(contract_json.as_deref(), &graph_json)),
                graph,
            },
        );
    }

    let line = BundleLine {
        name,
        description,
        stages: stages
            .iter()
            .map(|s| BundleStage {
                workflow: s.workflow_id.clone(),
                text_overrides: serde_json::from_str(&s.text_overrides).unwrap_or_default(),
                parameters: serde_json::from_str(&s.parameters).unwrap_or_default(),
                vary: serde_json::from_str(&s.vary).unwrap_or_default(),
                source_mode: s.source_mode.clone(),
                keep_output: s.keep_output,
                exposed: s.exposed.clone(),
            })
            .collect(),
    };

    let mut bundle = LineBundle::build(line, workflows);
    bundle.exported_at = Some(
        chrono::Utc::now()
            .naive_utc()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    );
    Ok(Json(bundle))
}

// ===== Import ===============================================================

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct ImportQuery {
    /// Check the bundle and report what it needs, writing nothing. Defaults to
    /// `false`.
    #[serde(default)]
    dry_run: bool,
    /// Name the imported line this rather than what the file calls it. A name
    /// already in use is still suffixed rather than overwritten.
    name: Option<String>,
}

/// What one of the bundle's workflows turned into here.
struct ResolvedWorkflow {
    id: String,
    name: String,
    /// The name of the workflow it matched, when it matched one. `None` means
    /// this import created it.
    reused_as: Option<String>,
}

/// A transaction that can fail for two unrelated reasons.
enum ImportFailure {
    Refused(ApiError),
    Db(diesel::result::Error),
}

impl From<diesel::result::Error> for ImportFailure {
    fn from(e: diesel::result::Error) -> Self {
        ImportFailure::Db(e)
    }
}

#[utoipa::path(
    post,
    path = "/api/comfyui/lines/import",
    tag = "comfyui",
    summary = "Import a production line",
    description = "Read a line out of an exported bundle — the same format bundled templates \
                   ship in. Its workflows are created, or reused where an identical graph is \
                   already here, and its requirements are checked against what this ComfyUI has \
                   installed. A line whose nodes or models are missing is still imported; the \
                   response says what is missing. A name already in use is suffixed, never \
                   overwritten. Pass `dry_run=true` for the report alone.",
    params(ImportQuery),
    request_body = LineBundle,
    responses(
        (status = 200, description = "The imported line, and what it needs"),
        (status = 400, description = "The file is not a readable line export, or its chain does \
                                      not fit together here"),
        (status = 503, description = "ComfyUI not configured"),
    )
)]
pub(super) async fn import_line(
    UState(state): UState,
    Query(query): Query<ImportQuery>,
    Json(doc): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = require_comfyui(&state)?;
    let bundle = LineBundle::parse(&doc).map_err(ApiError::bad_request)?;

    // Recomputed from the graphs in the file rather than read out of its
    // `requirements` block: the graphs are what will actually be run.
    let requirements = bundle.derived_requirements();
    let catalog = node_catalog(&url).await;
    let report = requirements.check(catalog.as_deref());

    let mut conn = state.pool.get().map_err(|_| ApiError::internal())?;

    let desired = query
        .name
        .as_deref()
        .unwrap_or(&bundle.line.name)
        .trim()
        .to_string();
    if desired.is_empty() {
        return Err(ApiError::bad_request("A line needs a name."));
    }

    if query.dry_run {
        // Nothing is written, so the name and the workflow plan are what
        // *would* happen.
        let taken: Vec<String> = production_lines::table
            .select(production_lines::name)
            .load(&mut conn)
            .map_err(|_| ApiError::internal())?;
        let name = available_name(&desired, &taken);
        let renamed_from = (name != desired).then(|| desired.clone());
        let existing = existing_workflows(&mut conn).map_err(|_| ApiError::internal())?;
        let preview: Vec<serde_json::Value> = bundle
            .workflows
            .iter()
            .map(|(key, wf)| {
                let reused = existing.get(&workflow_identity(&wf.graph, &bundle_corrections(wf)));
                serde_json::json!({
                    "key": key,
                    "name": wf.name,
                    "reused": reused.is_some(),
                    "reused_as": reused.map(|(_, n)| n.clone()),
                })
            })
            .collect();
        return Ok(Json(serde_json::json!({
            "dry_run": true,
            "name": name,
            "renamed_from": renamed_from,
            "stage_count": bundle.line.stages.len(),
            "workflows": preview,
            "requirements": requirements,
            "report": report,
            "report_status": report.status(),
            "report_headline": report.headline(),
        })));
    }

    let line_id = uuid::Uuid::new_v4().to_string();
    // One transaction: a line whose workflows landed but whose stages did not
    // would leave graphs nobody asked for lying about the library. Immediate,
    // and the name is picked inside it: `production_lines.name` has no unique
    // index, so two overlapping imports that both read the taken names before
    // either wrote would both pick the same suffix. The write lock an
    // immediate transaction takes up front is what serialises that read.
    let (resolved, name) = conn
        .immediate_transaction::<_, ImportFailure, _>(|conn| {
            let taken: Vec<String> = production_lines::table
                .select(production_lines::name)
                .load(conn)?;
            let name = available_name(&desired, &taken);
            let resolved = resolve_workflows(conn, &bundle, catalog.as_deref())?;

            let payload = LinePayload {
                name: name.clone(),
                description: bundle.line.description.clone(),
                stages: bundle
                    .line
                    .stages
                    .iter()
                    .map(|s| LineStagePayload {
                        workflow_id: resolved[&s.workflow].id.clone(),
                        text_overrides: s.text_overrides.clone(),
                        parameters: s.parameters.clone(),
                        vary: s.vary.clone(),
                        source_mode: s.source_mode.clone(),
                        keep_output: s.keep_output,
                        exposed: s.exposed.clone(),
                    })
                    .collect(),
            };

            // The same check a `POST /lines` runs — not a second one. A bundle
            // whose chain does not fit *here* is refused here, and the reason
            // is usually the reason the report already gave.
            check_payload(conn, &payload).map_err(|e| {
                ImportFailure::Refused(match (e.1, report.is_ready()) {
                    (Some(message), false) => {
                        ApiError::bad_request(format!("{} {}", message, report.headline()))
                    }
                    (Some(message), true) => ApiError::bad_request(message),
                    (None, _) => ApiError(e.0, None),
                })
            })?;

            diesel::insert_into(production_lines::table)
                .values(NewProductionLine {
                    id: &line_id,
                    name: &name,
                    description: payload.description.as_deref(),
                })
                .execute(conn)?;
            insert_stages(conn, &line_id, &payload)?;
            Ok((resolved, name))
        })
        .map_err(|e| match e {
            ImportFailure::Refused(api) => api,
            ImportFailure::Db(e) => {
                tracing::error!("Failed to import line: {}", e);
                ApiError::internal()
            }
        })?;
    let renamed_from = (name != desired).then(|| desired.clone());

    let stages = crate::comfyui::runs::stages_of_line(&mut conn, &line_id)
        .map_err(|_| ApiError::internal())?;
    let created: Vec<&str> = resolved
        .values()
        .filter(|w| w.reused_as.is_none())
        .map(|w| w.name.as_str())
        .collect();
    let reused: Vec<&str> = resolved
        .values()
        .filter_map(|w| w.reused_as.as_deref())
        .collect();

    Ok(Json(serde_json::json!({
        "line": line_json(
            &line_id,
            &name,
            bundle.line.description.as_deref(),
            None,
            None,
            &stages,
            0,
        ),
        "renamed_from": renamed_from,
        "workflows_created": created,
        "workflows_reused": reused,
        "requirements": requirements,
        "report": report,
        "report_status": report.status(),
        "report_headline": report.headline(),
    })))
}

/// The corrections a bundle's workflow travels with. A hand-written template
/// carries none.
fn bundle_corrections(wf: &BundleWorkflow) -> ContractCorrections {
    wf.contract
        .as_ref()
        .map(|c| c.corrections.clone())
        .unwrap_or_default()
}

/// Every workflow in this library, keyed by its [`workflow_identity`].
///
/// Loading every graph to compare them is affordable because a library holds
/// tens of workflows, not thousands, and because this happens once per import
/// rather than once per request.
fn existing_workflows(
    conn: &mut SqliteConnection,
) -> QueryResult<BTreeMap<String, (String, String)>> {
    let rows: Vec<(String, String, String, Option<String>)> = comfyui_workflows::table
        .select((
            comfyui_workflows::id,
            comfyui_workflows::name,
            comfyui_workflows::workflow_json,
            comfyui_workflows::contract_json,
        ))
        .load(conn)?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, name, json, contract)| {
            let graph: serde_json::Value = serde_json::from_str(&json).ok()?;
            let corrections = contract
                .and_then(|c| serde_json::from_str::<StageContract>(&c).ok())
                .map(|c| c.corrections)
                .unwrap_or_default();
            Some((workflow_identity(&graph, &corrections), (id, name)))
        })
        .collect())
}

/// Find each of the bundle's workflows here, or create it.
///
/// Deduplication is on [`workflow_identity`] — the graph plus its corrections:
/// the same nodes with the same values is the same workflow however it is
/// named, and one different value *or one different correction* is a different
/// workflow however familiar it looks. Reusing a row whose corrections differ
/// would validate and dispatch the imported line against a contract its author
/// never saw. A workflow created earlier in this same import counts as
/// present, so a bundle naming one graph under two keys makes one row.
fn resolve_workflows(
    conn: &mut SqliteConnection,
    bundle: &LineBundle,
    catalog: Option<&NodeCatalog>,
) -> Result<BTreeMap<String, ResolvedWorkflow>, ImportFailure> {
    let mut existing = existing_workflows(conn)?;
    let mut resolved = BTreeMap::new();
    for (key, wf) in &bundle.workflows {
        // A contract is a fact about the box that runs the graph, so it is
        // derived again here. Corrections travel — they are the part of a
        // contract that is a person's judgement rather than a derivation.
        let corrections = bundle_corrections(wf);
        let identity = workflow_identity(&wf.graph, &corrections);
        if let Some((id, name)) = existing.get(&identity) {
            resolved.insert(
                key.clone(),
                ResolvedWorkflow {
                    id: id.clone(),
                    name: wf.name.clone(),
                    reused_as: Some(name.clone()),
                },
            );
            continue;
        }

        let contract = StageContract::derive_with(&wf.graph, catalog, corrections);

        let id = uuid::Uuid::new_v4().to_string();
        let graph_json = serde_json::to_string(&wf.graph).map_err(|_| {
            ImportFailure::Refused(ApiError::bad_request(format!(
                "Workflow '{}' carries a graph that cannot be stored.",
                wf.name
            )))
        })?;
        let inputs = crate::comfyui::detect_inputs(&wf.graph, catalog);
        let outputs = crate::comfyui::detect_outputs(&wf.graph);
        let inputs_json = serde_json::to_string(&inputs).unwrap_or_else(|_| "[]".to_string());
        let outputs_json = serde_json::to_string(&outputs).unwrap_or_else(|_| "[]".to_string());
        let contract_json = serde_json::to_string(&contract).unwrap_or_else(|_| "{}".to_string());

        diesel::insert_into(comfyui_workflows::table)
            .values(NewComfyuiWorkflow {
                id: &id,
                name: &wf.name,
                description: wf.description.as_deref(),
                workflow_json: &graph_json,
                inputs_json: Some(&inputs_json),
                outputs_json: Some(&outputs_json),
                contract_json: Some(&contract_json),
            })
            .execute(conn)?;

        existing.insert(identity, (id.clone(), wf.name.clone()));
        resolved.insert(
            key.clone(),
            ResolvedWorkflow {
                id,
                name: wf.name.clone(),
                reused_as: None,
            },
        );
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::portable::{canonical_graph, Requirements};
    use crate::schema::line_stages;
    use diesel::connection::SimpleConnection;
    use serde_json::json;

    /// A graph that takes a still and saves a still.
    fn image_graph() -> serde_json::Value {
        json!({
            "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20, "cfg": 8.0}},
            "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
            "9": {"class_type": "SaveImage",
                  "inputs": {"filename_prefix": "out", "images": ["3", 0]}}
        })
    }

    fn library() -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let conn = crate::db::open_diesel_connection(&db_path).unwrap();
        (dir, conn)
    }

    /// The library the export tests read out of: one two-stage line over two
    /// workflows, with bindings, parameters, a sweep, a source mode and a keep
    /// flag — one of everything a stage can carry.
    fn seeded() -> (tempfile::TempDir, SqliteConnection) {
        let (dir, mut conn) = library();
        let mut second = image_graph();
        second["3"]["inputs"]["steps"] = json!(40);
        conn.batch_execute(&format!(
            "INSERT INTO comfyui_workflows (id, name, description, workflow_json) \
             VALUES ('wf-1', 'Portrait', 'first', '{a}'), ('wf-2', 'Upscale', NULL, '{b}');
             INSERT INTO production_lines (id, name, description) \
             VALUES ('line-1', '4K Restore', 'photo to 4K');
             INSERT INTO line_stages \
               (id, line_id, stage_idx, workflow_id, text_overrides, parameters, vary, \
                source_mode, keep_output) \
             VALUES \
               ('st-1', 'line-1', 0, 'wf-1', '{{\"6.text\":\"a golden retriever\"}}', \
                '{{\"3.seed\":1000}}', '{{\"3.seed\":{{\"count\":3,\"mode\":\"increment\"}}}}', \
                'first_frame', 1), \
               ('st-2', 'line-1', 1, 'wf-2', '{{}}', '{{\"3.cfg\":7.5}}', '{{}}', NULL, 0);",
            a = image_graph().to_string().replace('\'', "''"),
            b = second.to_string().replace('\'', "''"),
        ))
        .unwrap();
        (dir, conn)
    }

    /// What `export_line` builds, without the axum plumbing around it.
    fn export(conn: &mut SqliteConnection, line_id: &str) -> LineBundle {
        let (name, description): (String, Option<String>) = production_lines::table
            .filter(production_lines::id.eq(line_id))
            .select((production_lines::name, production_lines::description))
            .first(conn)
            .unwrap();
        let stages = crate::comfyui::runs::stages_of_line(conn, line_id).unwrap();

        let mut workflows: BTreeMap<String, BundleWorkflow> = BTreeMap::new();
        for stage in &stages {
            if workflows.contains_key(&stage.workflow_id) {
                continue;
            }
            let (wf_name, wf_description, graph_json, contract_json): (
                String,
                Option<String>,
                String,
                Option<String>,
            ) = comfyui_workflows::table
                .filter(comfyui_workflows::id.eq(&stage.workflow_id))
                .select((
                    comfyui_workflows::name,
                    comfyui_workflows::description,
                    comfyui_workflows::workflow_json,
                    comfyui_workflows::contract_json,
                ))
                .first(conn)
                .unwrap();
            workflows.insert(
                stage.workflow_id.clone(),
                BundleWorkflow {
                    name: wf_name,
                    description: wf_description,
                    contract: Some(contract_of(contract_json.as_deref(), &graph_json)),
                    graph: serde_json::from_str(&graph_json).unwrap(),
                },
            );
        }

        LineBundle::build(
            BundleLine {
                name,
                description,
                stages: stages
                    .iter()
                    .map(|s| BundleStage {
                        workflow: s.workflow_id.clone(),
                        text_overrides: serde_json::from_str(&s.text_overrides).unwrap_or_default(),
                        parameters: serde_json::from_str(&s.parameters).unwrap_or_default(),
                        vary: serde_json::from_str(&s.vary).unwrap_or_default(),
                        source_mode: s.source_mode.clone(),
                        keep_output: s.keep_output,
                        exposed: s.exposed.clone(),
                    })
                    .collect(),
            },
            workflows,
        )
    }

    /// What `import_line` writes, without the axum plumbing around it.
    fn import(
        conn: &mut SqliteConnection,
        bundle: &LineBundle,
        catalog: Option<&NodeCatalog>,
    ) -> Result<(String, String), ApiError> {
        let taken: Vec<String> = production_lines::table
            .select(production_lines::name)
            .load(conn)
            .unwrap();
        let name = available_name(&bundle.line.name, &taken);
        let line_id = uuid::Uuid::new_v4().to_string();
        conn.transaction::<_, ImportFailure, _>(|conn| {
            let resolved = resolve_workflows(conn, bundle, catalog)?;
            let payload = LinePayload {
                name: name.clone(),
                description: bundle.line.description.clone(),
                stages: bundle
                    .line
                    .stages
                    .iter()
                    .map(|s| LineStagePayload {
                        workflow_id: resolved[&s.workflow].id.clone(),
                        text_overrides: s.text_overrides.clone(),
                        parameters: s.parameters.clone(),
                        vary: s.vary.clone(),
                        source_mode: s.source_mode.clone(),
                        keep_output: s.keep_output,
                        exposed: s.exposed.clone(),
                    })
                    .collect(),
            };
            check_payload(conn, &payload).map_err(ImportFailure::Refused)?;
            diesel::insert_into(production_lines::table)
                .values(NewProductionLine {
                    id: &line_id,
                    name: &name,
                    description: payload.description.as_deref(),
                })
                .execute(conn)?;
            insert_stages(conn, &line_id, &payload)?;
            Ok(())
        })
        .map_err(|e| match e {
            ImportFailure::Refused(api) => api,
            ImportFailure::Db(e) => ApiError::bad_request(e.to_string()),
        })?;
        Ok((line_id, name))
    }

    /// Everything about a bundle that has to survive a trip, with the parts
    /// that are allowed to differ (workflow keys, timestamps) left out.
    fn comparable(bundle: &LineBundle) -> serde_json::Value {
        let stages: Vec<serde_json::Value> = bundle
            .line
            .stages
            .iter()
            .map(|s| {
                let wf = &bundle.workflows[&s.workflow];
                json!({
                    "workflow_name": wf.name,
                    "graph": canonical_graph(&wf.graph),
                    "text_overrides": s.text_overrides,
                    "parameters": s.parameters,
                    "vary": s.vary,
                    "source_mode": s.source_mode,
                    "keep_output": s.keep_output,
                })
            })
            .collect();
        json!({
            "name": bundle.line.name,
            "description": bundle.line.description,
            "stages": stages,
            "requirements": bundle.requirements,
        })
    }

    // === Export =============================================================

    #[test]
    fn an_export_carries_the_graph_behind_every_stage() {
        let (_dir, mut conn) = seeded();
        let bundle = export(&mut conn, "line-1");

        assert_eq!(bundle.format, "phos.line");
        assert_eq!(bundle.line.name, "4K Restore");
        assert_eq!(bundle.line.stages.len(), 2);
        assert_eq!(bundle.workflows.len(), 2);
        for stage in &bundle.line.stages {
            let wf = &bundle.workflows[&stage.workflow];
            assert!(wf.graph.is_object(), "a stage with no graph is a pointer");
            assert!(wf.contract.is_some());
        }
        // And what it needs, worked out from those graphs.
        assert_eq!(
            bundle.requirements.node_classes,
            vec!["KSampler", "LoadImage", "SaveImage"]
        );
    }

    #[test]
    fn an_export_reads_back_as_the_document_it_claims_to_be() {
        let (_dir, mut conn) = seeded();
        let doc = serde_json::to_value(export(&mut conn, "line-1")).unwrap();
        LineBundle::parse(&doc).expect("an export must be importable");
    }

    // === Round trip =========================================================

    #[test]
    fn export_import_export_gives_back_an_equivalent_line() {
        let (_dir, mut conn) = seeded();
        let first = export(&mut conn, "line-1");

        // Into a different library entirely — nothing of the original's ids,
        // workflows or names is there to be leaned on.
        let (_other_dir, mut other) = library();
        let (imported_id, name) = import(&mut other, &first, None).unwrap();
        assert_eq!(name, "4K Restore", "an empty library has no collision");

        let second = export(&mut other, &imported_id);
        assert_eq!(
            comparable(&first),
            comparable(&second),
            "a round trip must preserve the line"
        );
    }

    #[test]
    fn a_round_trip_preserves_stage_order_bindings_and_parameters() {
        let (_dir, mut conn) = seeded();
        let first = export(&mut conn, "line-1");
        let (_other_dir, mut other) = library();
        let (imported_id, _) = import(&mut other, &first, None).unwrap();
        let second = export(&mut other, &imported_id);

        // Order: the sweep is on stage 1, the cfg on stage 2, and swapping them
        // would still round-trip a set.
        let names: Vec<&str> = second
            .line
            .stages
            .iter()
            .map(|s| second.workflows[&s.workflow].name.as_str())
            .collect();
        assert_eq!(names, vec!["Portrait", "Upscale"]);

        let first_stage = &second.line.stages[0];
        assert_eq!(
            first_stage.text_overrides.get("6.text").map(String::as_str),
            Some("a golden retriever")
        );
        assert_eq!(first_stage.parameters["3.seed"], json!(1000));
        assert_eq!(
            serde_json::to_value(&first_stage.vary["3.seed"]).unwrap(),
            json!({"count": 3, "mode": "increment"})
        );
        assert_eq!(first_stage.source_mode.as_deref(), Some("first_frame"));
        assert!(first_stage.keep_output);

        let second_stage = &second.line.stages[1];
        assert_eq!(second_stage.parameters["3.cfg"], json!(7.5));
        assert!(second_stage.text_overrides.is_empty());
        assert!(!second_stage.keep_output);
        assert_eq!(second_stage.source_mode, None);
    }

    // === Workflow deduplication =============================================

    #[test]
    fn an_identical_graph_is_reused_rather_than_copied() {
        let (_dir, mut conn) = seeded();
        let bundle = export(&mut conn, "line-1");

        // Importing back into the library it came from.
        let before: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        let (imported_id, name) = import(&mut conn, &bundle, None).unwrap();
        let after: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();

        assert_eq!(before, 2);
        assert_eq!(after, 2, "both graphs were already here");
        assert_eq!(name, "4K Restore (imported)");

        // And the new line points at the original rows.
        let ids: Vec<String> = line_stages::table
            .filter(line_stages::line_id.eq(&imported_id))
            .order(line_stages::stage_idx.asc())
            .select(line_stages::workflow_id)
            .load(&mut conn)
            .unwrap();
        assert_eq!(ids, vec!["wf-1", "wf-2"]);
    }

    #[test]
    fn a_graph_that_differs_by_one_value_is_a_different_workflow() {
        let (_dir, mut conn) = seeded();
        let mut bundle = export(&mut conn, "line-1");
        // The same workflow, one seed apart.
        bundle.workflows.get_mut("wf-1").unwrap().graph["3"]["inputs"]["seed"] = json!(2);

        let (imported_id, _) = import(&mut conn, &bundle, None).unwrap();
        let count: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 3, "the changed graph is a third workflow");

        let ids: Vec<String> = line_stages::table
            .filter(line_stages::line_id.eq(&imported_id))
            .order(line_stages::stage_idx.asc())
            .select(line_stages::workflow_id)
            .load(&mut conn)
            .unwrap();
        assert_ne!(ids[0], "wf-1", "stage 1 got the new graph");
        assert_eq!(ids[1], "wf-2", "stage 2 reused the identical one");
    }

    #[test]
    fn a_graph_arriving_with_different_corrections_is_a_different_workflow() {
        let (_dir, mut conn) = seeded();
        let mut bundle = export(&mut conn, "line-1");
        // Same graph, but the file says node 6's text box is the negative
        // prompt — a judgement the row already here never heard. Reusing that
        // row would run the imported line against a contract without it.
        bundle
            .workflows
            .get_mut("wf-1")
            .unwrap()
            .contract
            .as_mut()
            .unwrap()
            .corrections
            .slots
            .insert("6.text".to_string(), Some("negative".to_string()));

        let (imported_id, _) = import(&mut conn, &bundle, None).unwrap();
        let count: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 3, "the corrected graph is its own workflow");

        // The new row carries the corrections it travelled with…
        let ids: Vec<String> = line_stages::table
            .filter(line_stages::line_id.eq(&imported_id))
            .order(line_stages::stage_idx.asc())
            .select(line_stages::workflow_id)
            .load(&mut conn)
            .unwrap();
        assert_ne!(ids[0], "wf-1", "stage 1 got its own row");
        assert_eq!(ids[1], "wf-2", "stage 2's corrections agree, so it reuses");
        let stored: Option<String> = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(&ids[0]))
            .select(comfyui_workflows::contract_json)
            .first(&mut conn)
            .unwrap();
        let contract: StageContract = serde_json::from_str(&stored.unwrap()).unwrap();
        assert_eq!(
            contract.corrections.slots.get("6.text"),
            Some(&Some("negative".to_string()))
        );

        // …and importing the same file again finds it rather than making a
        // fourth.
        import(&mut conn, &bundle, None).unwrap();
        let count: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn a_reordered_graph_is_still_the_same_workflow() {
        let (_dir, mut conn) = seeded();
        let mut bundle = export(&mut conn, "line-1");
        // Re-serialising through a library that sorts keys differently must not
        // be enough to make a near-duplicate.
        let reordered: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&bundle.workflows["wf-1"].graph)
                .unwrap()
                .replace(r#""seed":1,"steps":20"#, r#""steps":20,"seed":1"#),
        )
        .unwrap();
        bundle.workflows.get_mut("wf-1").unwrap().graph = reordered;

        import(&mut conn, &bundle, None).unwrap();
        let count: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn one_graph_under_two_keys_becomes_one_workflow() {
        let (_other_dir, mut conn) = library();
        let mut workflows = BTreeMap::new();
        for key in ["a", "b"] {
            workflows.insert(
                key.to_string(),
                BundleWorkflow {
                    name: format!("Copy {}", key),
                    description: None,
                    graph: image_graph(),
                    contract: None,
                },
            );
        }
        let bundle = LineBundle::build(
            BundleLine {
                name: "Twice".to_string(),
                description: None,
                stages: ["a", "b"]
                    .iter()
                    .map(|k| BundleStage {
                        workflow: k.to_string(),
                        text_overrides: Default::default(),
                        parameters: Default::default(),
                        vary: Default::default(),
                        source_mode: None,
                        keep_output: false,
                        exposed: Vec::new(),
                    })
                    .collect(),
            },
            workflows,
        );

        import(&mut conn, &bundle, None).unwrap();
        let count: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 1);
    }

    // === Name collisions ====================================================

    #[test]
    fn importing_over_an_existing_name_leaves_the_original_alone() {
        let (_dir, mut conn) = seeded();
        let bundle = export(&mut conn, "line-1");

        let (first_id, first_name) = import(&mut conn, &bundle, None).unwrap();
        let (second_id, second_name) = import(&mut conn, &bundle, None).unwrap();
        assert_eq!(first_name, "4K Restore (imported)");
        assert_eq!(second_name, "4K Restore (imported 2)");
        assert_ne!(first_id, second_id);

        // Three lines, and the one that was already there is untouched.
        let mut names: Vec<String> = production_lines::table
            .select(production_lines::name)
            .load(&mut conn)
            .unwrap();
        names.sort();
        assert_eq!(
            names,
            vec![
                "4K Restore",
                "4K Restore (imported 2)",
                "4K Restore (imported)"
            ]
        );
        let original_stages = crate::comfyui::runs::stages_of_line(&mut conn, "line-1").unwrap();
        assert_eq!(original_stages.len(), 2);
        assert_eq!(original_stages[0].workflow_id, "wf-1");
    }

    // === The requirements report ============================================

    #[test]
    fn a_bundle_naming_a_node_this_box_lacks_says_which_at_import() {
        let (_dir, mut conn) = library();
        let graph = json!({
            "1": {"class_type": "LoadImage", "inputs": {"image": "a.png"}},
            "2": {"class_type": "WanImageToVideo", "inputs": {"length": 81}},
            "3": {"class_type": "VHS_VideoCombine", "inputs": {"filename_prefix": "out"}}
        });
        let mut workflows = BTreeMap::new();
        workflows.insert(
            "clip".to_string(),
            BundleWorkflow {
                name: "Animate".to_string(),
                description: None,
                graph,
                contract: None,
            },
        );
        let bundle = LineBundle::build(
            BundleLine {
                name: "Animate".to_string(),
                description: None,
                stages: vec![BundleStage {
                    workflow: "clip".to_string(),
                    text_overrides: Default::default(),
                    parameters: Default::default(),
                    vary: Default::default(),
                    source_mode: None,
                    keep_output: false,
                    exposed: Vec::new(),
                }],
            },
            workflows,
        );

        // A ComfyUI with core nodes and nothing else.
        let catalog = crate::comfyui::nodes::parse_object_info(&json!({
            "LoadImage": {"input": {"required": {"image": ["IMAGE"]}}},
            "SaveImage": {"input": {"required": {}}, "output_node": true}
        }));
        let report = bundle.derived_requirements().check(Some(&catalog));
        assert_eq!(
            report.missing_nodes,
            vec!["VHS_VideoCombine", "WanImageToVideo"]
        );
        assert!(report.headline().contains("WanImageToVideo"));

        // …and the line is imported anyway, because the GPU box may have them.
        let (id, _) = import(&mut conn, &bundle, Some(&catalog)).unwrap();
        let stages = crate::comfyui::runs::stages_of_line(&mut conn, &id).unwrap();
        assert_eq!(stages.len(), 1);
    }

    #[test]
    fn an_import_with_no_catalogue_succeeds_with_a_warning() {
        let (_dir, mut conn) = seeded();
        let bundle = export(&mut conn, "line-1");
        let (_other_dir, mut other) = library();

        // ComfyUI down, too old, or answering nothing readable.
        let report = bundle.derived_requirements().check(None);
        assert!(!report.checked);
        assert!(report.missing_nodes.is_empty(), "no catalogue is not 'no'");
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("could not be asked"));

        let (id, _) = import(&mut other, &bundle, None).unwrap();
        let stages = crate::comfyui::runs::stages_of_line(&mut other, &id).unwrap();
        assert_eq!(stages.len(), 2, "the import went through regardless");
    }

    #[test]
    fn the_manifest_in_the_file_is_never_believed_over_the_graphs() {
        let (_dir, mut conn) = seeded();
        let mut bundle = export(&mut conn, "line-1");
        // Somebody edited the file to claim it needs nothing.
        bundle.requirements = Requirements::default();
        assert_eq!(
            bundle.derived_requirements().node_classes,
            vec!["KSampler", "LoadImage", "SaveImage"]
        );
    }

    // === Refusals ===========================================================

    #[test]
    fn a_bundle_whose_chain_does_not_fit_here_is_refused_and_writes_nothing() {
        let (_dir, mut conn) = library();
        // Two stages, both image → image, but the second one declares itself a
        // line-starter by having no loader at all.
        let starter = json!({
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a cat"}},
            "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out"}}
        });
        let mut workflows = BTreeMap::new();
        workflows.insert(
            "one".to_string(),
            BundleWorkflow {
                name: "Portrait".to_string(),
                description: None,
                graph: image_graph(),
                contract: None,
            },
        );
        workflows.insert(
            "two".to_string(),
            BundleWorkflow {
                name: "Dream".to_string(),
                description: None,
                graph: starter,
                contract: None,
            },
        );
        let bundle = LineBundle::build(
            BundleLine {
                name: "Impossible".to_string(),
                description: None,
                stages: ["one", "two"]
                    .iter()
                    .map(|k| BundleStage {
                        workflow: k.to_string(),
                        text_overrides: Default::default(),
                        parameters: Default::default(),
                        vary: Default::default(),
                        source_mode: None,
                        keep_output: false,
                        exposed: Vec::new(),
                    })
                    .collect(),
            },
            workflows,
        );

        let err = import(&mut conn, &bundle, None).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(
            err.1.as_deref().unwrap_or("").contains("Dream"),
            "the refusal names the stage: {:?}",
            err.1
        );

        // And the rollback took the workflows with it: a failed import leaves
        // no graphs nobody asked for.
        let workflows: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        let lines: i64 = production_lines::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!((workflows, lines), (0, 0));
    }
}
