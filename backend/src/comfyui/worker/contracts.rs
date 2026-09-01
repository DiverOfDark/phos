//! Filling in the contracts of workflows imported before contracts existed.
//!
//! Two reasons a row needs one:
//!
//! * it was imported before this column did, so it has none at all;
//! * it has one that was derived while ComfyUI was unreachable, so its
//!   parameters carry no ranges and only the prompt boxes the old heuristics
//!   recognise were found. That contract is worth deriving again — once — when
//!   the catalogue can finally be read.
//!
//! Which is why this runs on the worker rather than at boot: the worker is the
//! one thing in Phos that already has both a database connection and a ComfyUI
//! client, wakes up repeatedly, and exists once per library. A server that was
//! down when Phos started is picked up a few minutes later instead of at the
//! next restart.
//!
//! Corrections are never lost. A row is re-derived *with* whatever the person
//! typed, because [`StageContract::derive_with`] folds them into the derivation
//! rather than patching the result.

use crate::comfyui::contract::StageContract;
use crate::schema::comfyui_workflows;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use serde_json::Value;
use tracing::{debug, info, warn};

/// Derive and store the contracts that are missing or under-typed.
///
/// Returns `true` when there is nothing left to do, so the worker can stop
/// asking. `false` means a later pass could still improve something — almost
/// always because ComfyUI could not be reached this time.
pub(super) fn backfill_contracts(
    conn: &mut SqliteConnection,
    client: &crate::comfyui::ComfyUiClient,
) -> bool {
    let rows: Vec<(String, String, Option<String>)> = match comfyui_workflows::table
        .select((
            comfyui_workflows::id,
            comfyui_workflows::workflow_json,
            comfyui_workflows::contract_json,
        ))
        .load(conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!("Could not read workflows to derive contracts: {}", e);
            return false;
        }
    };

    // What the row said when it was read is kept verbatim: the update below is
    // conditional on it, so a correction a person saves while this pass is off
    // fetching the catalogue is never overwritten with a stale derivation.
    let pending: Vec<(String, String, Option<String>, Option<StageContract>)> = rows
        .into_iter()
        .filter_map(|(id, graph, stored)| match stored.as_deref() {
            // Never derived.
            None => Some((id, graph, None, None)),
            Some(json) => match serde_json::from_str::<StageContract>(json) {
                // Derived blind, or over a catalogue missing some of this
                // graph's classes; worth another go now.
                Ok(c) if c.under_informed() => Some((id, graph, stored.clone(), Some(c))),
                Ok(_) => None,
                // Written by something that is not this type any more. Redo it
                // rather than leaving a row nothing can read.
                Err(e) => {
                    debug!("Workflow {} has an unreadable contract: {}", id, e);
                    Some((id, graph, stored.clone(), None))
                }
            },
        })
        .collect();

    if pending.is_empty() {
        return true;
    }

    let catalog = crate::comfyui::node_catalog(client);

    let mut written = 0usize;
    let mut unsettled = 0usize;
    for (id, graph_json, stored_raw, existing) in &pending {
        let graph: Value = serde_json::from_str(graph_json).unwrap_or(Value::Null);
        let corrections = existing
            .as_ref()
            .map(|c| c.corrections.clone())
            .unwrap_or_default();
        let contract = StageContract::derive_with(&graph, catalog.as_deref(), corrections);
        // A graph whose classes the server still does not have stays on the
        // list: installing the pack changes the answer, and this pass is the
        // only thing that would notice.
        if contract.under_informed() {
            unsettled += 1;
        }
        if existing.as_ref() == Some(&contract) {
            // Nothing new to say — a blind derivation over a blind one, or the
            // same missing pack as last time. Do not churn the row.
            continue;
        }
        let Ok(encoded) = serde_json::to_string(&contract) else {
            continue;
        };
        // Guarded by what was read at the top: zero rows means someone (the
        // correction endpoint) wrote meanwhile, and their write — made against
        // fresher input — stands. The row shows up again next pass if it still
        // wants anything.
        let result = match stored_raw {
            Some(raw) => diesel::update(
                comfyui_workflows::table.filter(
                    comfyui_workflows::id
                        .eq(id)
                        .and(comfyui_workflows::contract_json.eq(raw)),
                ),
            )
            .set(comfyui_workflows::contract_json.eq(&encoded))
            .execute(conn),
            None => diesel::update(
                comfyui_workflows::table.filter(
                    comfyui_workflows::id
                        .eq(id)
                        .and(comfyui_workflows::contract_json.is_null()),
                ),
            )
            .set(comfyui_workflows::contract_json.eq(&encoded))
            .execute(conn),
        };
        match result {
            Ok(0) => debug!(
                "Workflow {} was corrected while this pass ran; leaving it",
                id
            ),
            Ok(_) => written += 1,
            Err(e) => warn!("Could not store the contract for workflow {}: {}", id, e),
        }
    }

    if written > 0 {
        info!(
            "Derived stage contracts for {} workflow(s){}",
            written,
            if catalog.is_none() {
                " without ComfyUI's node catalogue"
            } else {
                ""
            }
        );
    }
    unsettled == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::contract::{Accepts, ContractCorrections, MediaType, ParamName};
    use crate::comfyui::nodes::{fixtures::object_info, parse_object_info, remember_for_test};
    use crate::models::NewComfyuiWorkflow;

    /// An img2img graph with a seed and a prompt: enough that a typed contract
    /// and an untyped one are visibly different.
    fn graph() -> &'static str {
        r#"{
            "4": { "class_type": "CheckpointLoaderSimple",
                   "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" } },
            "10": { "class_type": "LoadImage", "inputs": { "image": "photo.png" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] } },
            "3": { "class_type": "KSampler", "inputs": {
                     "model": ["4", 0], "positive": ["6", 0],
                     "latent_image": ["10", 0], "seed": 42, "steps": 20, "cfg": 8.0,
                     "sampler_name": "euler", "scheduler": "normal", "denoise": 0.45 } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["3", 0], "filename_prefix": "ComfyUI" } }
        }"#
    }

    /// A library with one imported workflow, and whatever contract it was given.
    fn library(contract: Option<&str>) -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        diesel::insert_into(comfyui_workflows::table)
            .values(NewComfyuiWorkflow {
                id: "wf-1",
                name: "refine",
                description: None,
                workflow_json: graph(),
                inputs_json: None,
                outputs_json: None,
                contract_json: contract,
            })
            .execute(&mut conn)
            .unwrap();
        (dir, conn)
    }

    fn stored(conn: &mut SqliteConnection) -> Option<StageContract> {
        let raw: Option<String> = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq("wf-1"))
            .select(comfyui_workflows::contract_json)
            .first(conn)
            .unwrap();
        raw.map(|s| serde_json::from_str(&s).unwrap())
    }

    /// A client pointed at a base URL whose catalogue is already in the cache,
    /// so nothing here goes near a network.
    fn client_with_a_catalogue(name: &str) -> crate::comfyui::ComfyUiClient {
        let url = format!("http://{}.test", name);
        remember_for_test(&url, parse_object_info(&object_info()));
        crate::comfyui::ComfyUiClient::new(&url)
    }

    /// Nothing is listening on port 1, so this refuses at once rather than
    /// spending the client's timeout budget.
    fn client_with_nothing_behind_it() -> crate::comfyui::ComfyUiClient {
        crate::comfyui::ComfyUiClient::new("http://127.0.0.1:1")
    }

    #[test]
    fn a_workflow_imported_before_contracts_existed_gets_one() {
        let (_dir, mut conn) = library(None);
        assert!(stored(&mut conn).is_none());

        let settled = backfill_contracts(&mut conn, &client_with_a_catalogue("backfill-new"));
        assert!(settled, "nothing should be left to do");

        let c = stored(&mut conn).expect("a contract was written");
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Image);
        assert!(!c.wants_catalog());
        // Typed, because the catalogue could be read.
        assert_eq!(c.params_named(ParamName::Seed).count(), 1);
    }

    #[test]
    fn a_contract_derived_blind_is_done_again_and_keeps_what_a_person_said() {
        // Imported while ComfyUI was down, and corrected by hand afterwards.
        let graph_value: serde_json::Value = serde_json::from_str(graph()).unwrap();
        let blind = StageContract::derive_with(
            &graph_value,
            None,
            ContractCorrections {
                produces: Some(MediaType::Video),
                ..Default::default()
            },
        );
        assert!(blind.wants_catalog());
        assert!(blind.params.is_empty(), "nothing could be typed");

        let (_dir, mut conn) = library(Some(&serde_json::to_string(&blind).unwrap()));
        let settled = backfill_contracts(&mut conn, &client_with_a_catalogue("backfill-blind"));
        assert!(settled);

        let c = stored(&mut conn).unwrap();
        // The catalogue arrived: the settings are typed now.
        assert!(!c.wants_catalog());
        assert_eq!(c.params_named(ParamName::Steps).count(), 1);
        // And the correction was not quietly undone.
        assert_eq!(c.produces, MediaType::Video);
        assert_eq!(c.corrections.produces, Some(MediaType::Video));
    }

    #[test]
    fn a_workflow_that_already_has_a_typed_contract_is_left_exactly_as_it_was() {
        let graph_value: serde_json::Value = serde_json::from_str(graph()).unwrap();
        let good = StageContract::derive(&graph_value, Some(&parse_object_info(&object_info())));
        let encoded = serde_json::to_string(&good).unwrap();
        let (_dir, mut conn) = library(Some(&encoded));

        // No client is needed at all: a settled library never asks ComfyUI.
        let settled = backfill_contracts(&mut conn, &client_with_nothing_behind_it());
        assert!(settled);

        let raw: Option<String> = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq("wf-1"))
            .select(comfyui_workflows::contract_json)
            .first(&mut conn)
            .unwrap();
        assert_eq!(raw.as_deref(), Some(encoded.as_str()));
    }

    #[test]
    fn with_comfyui_down_a_missing_contract_is_still_written_and_the_pass_asks_again() {
        let (_dir, mut conn) = library(None);
        let settled = backfill_contracts(&mut conn, &client_with_nothing_behind_it());
        assert!(!settled, "the worker should try again once ComfyUI is back");

        // A degraded contract beats none: the console can still say what this
        // workflow takes and gives.
        let c = stored(&mut conn).expect("something was written anyway");
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Image);
        assert!(c.wants_catalog());
        // And a second pass with a catalogue finishes the job.
        assert!(backfill_contracts(
            &mut conn,
            &client_with_a_catalogue("backfill-recovered")
        ));
        assert!(!stored(&mut conn).unwrap().wants_catalog());
    }

    #[test]
    fn a_second_pass_with_comfyui_still_down_writes_nothing_new() {
        let (_dir, mut conn) = library(None);
        assert!(!backfill_contracts(
            &mut conn,
            &client_with_nothing_behind_it()
        ));
        let first = stored(&mut conn).unwrap();
        assert!(!backfill_contracts(
            &mut conn,
            &client_with_nothing_behind_it()
        ));
        assert_eq!(stored(&mut conn).unwrap(), first);
    }

    #[test]
    fn a_graph_using_an_uninstalled_pack_is_rederived_once_the_pack_arrives() {
        // Imported while the server was up but missing a pack: the catalogue
        // was read, some classes were not in it. Not the same as derived blind
        // — and not settled either, because installing the pack changes what
        // the catalogue answers.
        let graph_value: Value = serde_json::from_str(graph()).unwrap();
        let mut doc = object_info();
        doc.as_object_mut().unwrap().remove("KSampler");
        let missing_pack = parse_object_info(&doc);
        let partial = StageContract::derive(&graph_value, Some(&missing_pack));
        assert!(partial.under_informed());
        assert!(!partial.wants_catalog());

        let (_dir, mut conn) = library(Some(&serde_json::to_string(&partial).unwrap()));

        // While the pack is still missing, the pass keeps asking — and does
        // not churn the row with an identical rewrite.
        let url = "http://backfill-still-missing.test";
        remember_for_test(url, parse_object_info(&doc));
        assert!(!backfill_contracts(
            &mut conn,
            &crate::comfyui::ComfyUiClient::new(url)
        ));
        assert_eq!(stored(&mut conn).unwrap(), partial);

        // The pack is installed: the next pass finishes the job.
        assert!(backfill_contracts(
            &mut conn,
            &client_with_a_catalogue("backfill-pack-installed")
        ));
        let c = stored(&mut conn).unwrap();
        assert!(!c.under_informed());
        assert_eq!(c.params_named(ParamName::Seed).count(), 1);
    }

    #[test]
    fn a_contract_written_by_something_this_build_cannot_read_is_replaced() {
        let (_dir, mut conn) = library(Some("{\"accepts\": 17}"));
        assert!(backfill_contracts(
            &mut conn,
            &client_with_a_catalogue("backfill-garbage")
        ));
        assert_eq!(stored(&mut conn).unwrap().accepts, Accepts::Image);
    }
}
