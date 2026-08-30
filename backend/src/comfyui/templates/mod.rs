//! Five lines that work on a fresh install.
//!
//! Phos can chain workflows, but until now a new library had nothing in it: you
//! had to find or write ComfyUI graphs, import them, and wire a chain by hand
//! before anything at all could run. These five ship with the binary and are
//! seeded into every library the first time it is opened.
//!
//! | Template | In → out |
//! |---|---|
//! | Restore & Upscale | image → image, 4x |
//! | Photo to 5s clip | image → 720p video |
//! | Extend clip +5s | video → video, continued from its last frame |
//! | Video to 4K | video → video, upscaled |
//! | Interpolate to 60fps | video → video, frame-interpolated |
//!
//! # The two things that are hard
//!
//! **Honest readiness.** A template only runs if that ComfyUI box has the
//! custom nodes and the model files. [`readiness`] asks `/object_info` — the
//! only description of a ComfyUI that comes from the ComfyUI — and answers
//! `READY`, `MISSING 2 NODES`, `MISSING MODEL wan2.1_i2v_720p.safetensors`, or
//! `UNKNOWN` when the catalogue could not be read. A template that cannot run
//! names exactly what to install, on the screen where it is offered, rather
//! than failing at dispatch minutes later with a ComfyUI validation error.
//!
//! **Staying updatable until the user edits.** Every seeded workflow carries a
//! `_phos` block holding a hash of the graph exactly as shipped. On upgrade it
//! is rehashed: still matching means untouched, and the new version is written
//! over it; differing means somebody edited it, so the block is dropped and
//! nothing ever touches it again. See [`marker`]. Bundled templates improve
//! across releases for people who never customised them, and are never
//! clobbered for people who did.
//!
//! # These graphs have not been run
//!
//! They were built from published node definitions, not exported from a live
//! server — Phos's build has no ComfyUI to export from. Each bundle says so in
//! its `confidence` field, and the console shows it. The readiness check does
//! more than list requirements for that reason: it compares each graph's fields
//! against what this server's copy of each node actually declares, so a graph
//! written against a different release of a node pack is reported *before* a
//! run, not by one.
//!
//! # One format, not two
//!
//! A template *is* an exported line. The five files in `bundles/` are FR5d's
//! `phos.line` documents — line, workflow graphs, contracts, requirements —
//! with one additive `template` block carrying the key and version an upgrade
//! needs. Paste one into the Lines tab's import box and it imports. See
//! [`crate::comfyui::portable::LineBundle`] — the same type the Lines tab's
//! export and import use, with the one additive `template` block [`bundle`]
//! defines.
//!
//! # Shape
//!
//! * [`bundle`] — the `template` block FR5d's document does not define, and
//!   the build-time check on the five shipped files.
//! * [`marker`] — the `_phos` block, the hash, and the update decision. Pure.
//! * [`readiness`] — what this ComfyUI is missing, and how it reads. Pure.
//! * [`install`] — the only part that writes rows.

pub mod bundle;
pub mod install;
pub mod marker;
pub mod readiness;

use crate::comfyui::portable::LineBundle;
use diesel::sqlite::SqliteConnection;
use std::sync::OnceLock;

pub use bundle::Confidence;
pub use readiness::{Readiness, ReadinessState};

/// The bundles, in the order the console lists them: what you would do to a
/// photograph first, then what you would do to the clip that came out.
const BUNDLED: &[&str] = &[
    include_str!("bundles/restore-upscale.json"),
    include_str!("bundles/photo-to-clip.json"),
    include_str!("bundles/extend-clip.json"),
    include_str!("bundles/video-to-4k.json"),
    include_str!("bundles/interpolate-60fps.json"),
];

/// Every template this build ships.
///
/// Parsed once. A bundle that will not parse is dropped with a log line rather
/// than panicking: the four that do parse are still worth having, and a test
/// over the shipped set means this can only happen to a build nobody tested.
pub fn bundled() -> &'static [LineBundle] {
    static PARSED: OnceLock<Vec<LineBundle>> = OnceLock::new();
    PARSED.get_or_init(|| {
        BUNDLED
            .iter()
            .filter_map(|raw| match serde_json::from_str::<LineBundle>(raw) {
                Ok(bundle) => Some(bundle),
                Err(e) => {
                    tracing::error!("A bundled template does not parse: {}", e);
                    None
                }
            })
            .collect()
    })
}

/// One template by key.
pub fn bundled_by_key(key: &str) -> Option<&'static LineBundle> {
    bundled().iter().find(|b| b.key() == key)
}

/// Seed, or bring up to date, every bundled template in one library.
///
/// Called once per library at startup, after the schema migrations and before
/// anything can ask for a line. Cheap on the ordinary path — five key lookups
/// and, when nothing has changed, five hashes.
///
/// Deliberately not gated on ComfyUI being configured: a library that gains a
/// ComfyUI later should already have the templates, not wait for the next
/// restart. The rows are invisible until the console can show them.
///
/// The catalogue is *not* read here. Seeding runs before the network is worth
/// waiting on, and a contract derived without one is re-derived by the
/// contract-backfill pass as soon as ComfyUI answers.
pub fn seed_library(conn: &mut SqliteConnection) {
    install::sync_all(conn, bundled(), None);
}

/// [`seed_library`] for a library that is described by its pool.
///
/// Never fatal: a library that could not be seeded still opens, and its
/// templates appear at the next start. Refusing to serve somebody's
/// photographs because five example lines could not be written would be an
/// absurd trade.
pub fn seed_pool(pool: &crate::db::DbPool, label: &str) {
    match pool.get() {
        Ok(mut conn) => seed_library(&mut conn),
        Err(e) => tracing::error!(
            "Bundled templates: cannot open {} to seed them: {}",
            label,
            e
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::{Accepts, MediaType};
    use diesel::prelude::*;
    use serde_json::{json, Value};

    fn library() -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let conn = crate::db::open_diesel_connection(&db_path).unwrap();
        (dir, conn)
    }

    /// A small bundle we can bump the version of, standing in for a release.
    fn fixture(version: u32, prompt: &str) -> LineBundle {
        serde_json::from_value(json!({
            "format": "phos.line",
            "format_version": 1,
            "template": { "key": "fixture", "version": version, "confidence": "high" },
            "line": {
                "name": "Fixture",
                "stages": [{ "workflow": "only", "keep_output": true }]
            },
            "workflows": {
                "only": {
                    "name": format!("Fixture v{}", version),
                    "description": "a test graph",
                    "graph": {
                        "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
                        "2": { "class_type": "CLIPTextEncode", "inputs": { "text": prompt } },
                        "3": { "class_type": "SaveImage",
                               "inputs": { "images": ["1", 0], "filename_prefix": "phos/x" } }
                    }
                }
            }
        }))
        .unwrap()
    }

    fn stored_graph(conn: &mut SqliteConnection, workflow_id: &str) -> Value {
        use crate::schema::comfyui_workflows;
        let json: String = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(workflow_id))
            .select(comfyui_workflows::workflow_json)
            .first(conn)
            .unwrap();
        serde_json::from_str(&json).unwrap()
    }

    fn only_workflow_id(conn: &mut SqliteConnection, bundle: &LineBundle) -> String {
        install::state_of(conn, bundle)
            .unwrap()
            .expect("installed")
            .workflows[0]
            .workflow_id
            .clone()
    }

    // ===== The shipped five =================================================

    #[test]
    fn all_five_templates_parse() {
        assert_eq!(bundled().len(), 5, "one bundle failed to parse");
        let keys: Vec<&str> = bundled().iter().map(|b| b.key()).collect();
        assert_eq!(
            keys,
            vec![
                "restore-upscale",
                "photo-to-clip",
                "extend-clip",
                "video-to-4k",
                "interpolate-60fps"
            ]
        );
    }

    /// Everything `POST /api/comfyui/workflows` and `POST /api/comfyui/lines`
    /// would refuse, asked of the shipped set at build time instead.
    #[test]
    fn every_shipped_template_would_survive_its_own_import() {
        for bundle in bundled() {
            assert_eq!(
                bundle::problems(bundle),
                Vec::<String>::new(),
                "template {}",
                bundle.key()
            );
        }
    }

    /// A bundled template has to be readable as an ordinary line export, or
    /// "one format, not two" is not true.
    ///
    /// FR5d owns `phos.line` and landed in parallel, so this reads each file
    /// through a struct that has FR5d's fields and *not* FR6's `template`
    /// block. It passing is what says the block is additive: a reader that has
    /// never heard of templates still gets a line, its graphs and its
    /// requirements.
    #[test]
    fn every_template_reads_as_a_plain_line_export() {
        #[derive(serde::Deserialize)]
        struct AsFr5dReadsIt {
            format: String,
            format_version: u32,
            line: crate::comfyui::portable::BundleLine,
            workflows: std::collections::BTreeMap<String, crate::comfyui::portable::BundleWorkflow>,
            requirements: crate::comfyui::portable::Requirements,
        }

        for raw in BUNDLED {
            let doc: AsFr5dReadsIt = serde_json::from_str(raw).expect("reads as a line export");
            assert_eq!(doc.format, "phos.line");
            assert_eq!(doc.format_version, 1);
            assert!(!doc.line.stages.is_empty());
            assert!(!doc.requirements.node_classes.is_empty());
            for stage in &doc.line.stages {
                assert!(
                    doc.workflows.contains_key(&stage.workflow),
                    "stage names {:?}, which the file does not carry",
                    stage.workflow
                );
            }
        }
    }

    /// `requirements` in the file is documentation — what is checked comes off
    /// the graphs. Documentation that disagrees with the thing it documents is
    /// worse than none, so the two are pinned equal here.
    #[test]
    fn the_written_requirements_match_the_derived_ones() {
        for bundle in bundled() {
            assert_eq!(
                bundle.requirements,
                crate::comfyui::portable::Requirements::derive(bundle.graphs()),
                "template {}",
                bundle.key()
            );
        }
    }

    /// The table in the module docs, asserted: what each template eats and
    /// hands back, derived the way the line validator will derive it.
    #[test]
    fn the_five_type_the_way_the_table_says() {
        let expected = [
            ("restore-upscale", Accepts::Image, MediaType::Image),
            ("photo-to-clip", Accepts::Image, MediaType::Video),
            ("extend-clip", Accepts::Video, MediaType::Video),
            ("video-to-4k", Accepts::Video, MediaType::Video),
            ("interpolate-60fps", Accepts::Video, MediaType::Video),
        ];
        for (key, accepts, produces) in expected {
            let bundle = bundled_by_key(key).unwrap();
            let typings = install::typings(bundle, None);
            assert_eq!(typings.len(), bundle.line.stages.len());
            assert_eq!(typings[0].accepts, accepts, "{} accepts", key);
            assert_eq!(typings[0].produces, produces, "{} produces", key);
            crate::comfyui::validate_chain(&typings)
                .unwrap_or_else(|e| panic!("{} does not validate: {}", key, e.message));
        }
    }

    /// They are meant to compose: clip → extend → 4K → 60fps has to type-check
    /// as one line, or the five are five dead ends.
    #[test]
    fn the_video_templates_chain_together() {
        let mut chain = Vec::new();
        for key in [
            "photo-to-clip",
            "extend-clip",
            "video-to-4k",
            "interpolate-60fps",
        ] {
            let bundle = bundled_by_key(key).unwrap();
            let mut stage = install::typings(bundle, None).remove(0);
            stage.stage_idx = chain.len() as i32;
            chain.push(stage);
        }
        crate::comfyui::validate_chain(&chain).expect("the four should chain");
    }

    // ===== Readiness, asked of the templates that actually ship =============

    /// A stand-in ComfyUI that agrees with the shipped graphs: every class they
    /// use, declaring exactly the fields they set, with each model picker
    /// holding the file its template names.
    ///
    /// This is *not* evidence that a real ComfyUI answers like this — the
    /// graphs are unverified, which is the whole reason the check exists. It is
    /// the baseline the interesting tests perturb: with it, every template must
    /// read READY, and each thing taken away from it must be named.
    fn a_server_with_everything() -> crate::comfyui::NodeCatalog {
        use crate::comfyui::nodes::{NodeClass, NodeInput, WidgetSpec};
        use std::collections::{BTreeMap, BTreeSet};

        // class -> the fields the graphs set on it.
        let mut fields: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        // class -> input -> the filenames its picker must offer.
        let mut models: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        for bundle in bundled() {
            for graph in bundle.graphs() {
                for node in graph.as_object().unwrap().values() {
                    let class = node["class_type"].as_str().unwrap().to_string();
                    let entry = fields.entry(class).or_default();
                    for field in node["inputs"].as_object().unwrap().keys() {
                        entry.insert(field.clone());
                    }
                }
            }
            for m in &crate::comfyui::portable::Requirements::derive(bundle.graphs()).models {
                models
                    .entry(m.class_type.clone())
                    .or_default()
                    .entry(m.field.clone())
                    .or_default()
                    .push(m.name.clone());
            }
        }

        let classes = fields
            .into_iter()
            .map(|(class, names)| {
                let inputs = names
                    .into_iter()
                    .map(|name| {
                        let choices = models
                            .get(&class)
                            .and_then(|by_input| by_input.get(&name))
                            .cloned();
                        NodeInput {
                            name,
                            required: true,
                            widget: match choices {
                                Some(choices) => WidgetSpec::Combo {
                                    default: choices.first().cloned(),
                                    choices,
                                    truncated: false,
                                },
                                None => WidgetSpec::Link {
                                    data_type: "*".to_string(),
                                },
                            },
                            tooltip: None,
                        }
                    })
                    .collect();
                (
                    class.clone(),
                    NodeClass {
                        name: class,
                        display_name: None,
                        category: None,
                        output_node: false,
                        inputs,
                    },
                )
            })
            .collect();
        crate::comfyui::NodeCatalog { classes }
    }

    #[test]
    fn a_server_that_has_everything_reads_ready_for_all_five() {
        let catalog = a_server_with_everything();
        for bundle in bundled() {
            let r = readiness::assess(bundle, Some(&catalog));
            assert_eq!(r.label, "READY", "{}: {}", bundle.key(), r.detail);
        }
    }

    /// The custom node pack nobody installed, named.
    #[test]
    fn a_missing_custom_node_is_named_on_the_template_that_needs_it() {
        let mut catalog = a_server_with_everything();
        catalog.classes.remove("RIFE VFI");
        let r = readiness::assess(bundled_by_key("interpolate-60fps").unwrap(), Some(&catalog));
        assert_eq!(r.state, ReadinessState::Missing);
        assert_eq!(r.label, "MISSING NODE RIFE VFI");
        // And only that template: the other four do not use it.
        assert_eq!(
            readiness::assess(bundled_by_key("video-to-4k").unwrap(), Some(&catalog)).label,
            "READY"
        );
    }

    /// The weights nobody downloaded, named — read out of the loader's own
    /// picker, because its contents *are* the files on that box.
    #[test]
    fn a_missing_model_is_named_by_filename() {
        use crate::comfyui::nodes::WidgetSpec;
        let mut catalog = a_server_with_everything();
        catalog
            .classes
            .get_mut("UNETLoader")
            .unwrap()
            .inputs
            .iter_mut()
            .for_each(|i| {
                i.widget = WidgetSpec::Combo {
                    choices: vec!["flux1-dev.safetensors".to_string()],
                    default: None,
                    truncated: false,
                }
            });
        let r = readiness::assess(bundled_by_key("photo-to-clip").unwrap(), Some(&catalog));
        assert_eq!(r.state, ReadinessState::Missing);
        assert_eq!(
            r.label,
            "MISSING MODEL wan2.1_i2v_720p_14B_fp8_scaled.safetensors"
        );
        // And it says which picker to put it in, which is which directory.
        assert!(r.detail.contains("UNETLoader.unet_name"), "{}", r.detail);
    }

    /// A ComfyUI that is asleep is not a ComfyUI that is missing things.
    #[test]
    fn an_unavailable_catalogue_gives_unknown_for_every_template() {
        for bundle in bundled() {
            let r = readiness::assess(bundle, None);
            assert_eq!(r.state, ReadinessState::Unchecked, "{}", bundle.key());
            assert_eq!(r.label, "UNKNOWN");
            assert!(r.requirements.missing_nodes.is_empty());
            assert!(r.requirements.missing_models.is_empty());
        }
    }

    // ===== Seeding ==========================================================

    #[test]
    fn a_fresh_library_gets_all_five_lines_and_their_workflows() {
        use crate::schema::{comfyui_workflows, line_stages, production_lines};
        let (_dir, mut conn) = library();
        seed_library(&mut conn);

        let lines: i64 = production_lines::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(lines, 5);
        let workflows: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(workflows, 5);
        let stages: i64 = line_stages::table.count().get_result(&mut conn).unwrap();
        assert_eq!(stages, 5);

        // Every stage's `source_mode` came across: extend-clip reads the last
        // frame of what it is handed, and the others let the graph decide.
        let modes: Vec<Option<String>> = line_stages::table
            .select(line_stages::source_mode)
            .load(&mut conn)
            .unwrap();
        assert_eq!(
            modes
                .iter()
                .filter(|m| m.as_deref() == Some("last_frame"))
                .count(),
            1
        );
    }

    #[test]
    fn seeding_twice_changes_nothing() {
        use crate::schema::production_lines;
        let (_dir, mut conn) = library();
        seed_library(&mut conn);
        seed_library(&mut conn);
        seed_library(&mut conn);
        let lines: i64 = production_lines::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(lines, 5, "a restart must not seed a second copy");
    }

    // ===== The upgrade, which is the whole point ============================

    #[test]
    fn an_untouched_template_is_updated_in_place_across_a_version_bump() {
        let (_dir, mut conn) = library();
        let v1 = fixture(1, "old prompt");
        install::sync_one(&mut conn, &v1, None).unwrap();
        let workflow_id = only_workflow_id(&mut conn, &v1);

        let v2 = fixture(2, "new prompt");
        let outcome = install::sync_one(&mut conn, &v2, None).unwrap();
        assert_eq!(outcome.updated, 1);
        assert!(!outcome.installed, "in place, not a second copy");

        let graph = stored_graph(&mut conn, &workflow_id);
        assert_eq!(graph["2"]["inputs"]["text"], json!("new prompt"));
        let marker = marker::read_marker(&graph).expect("still managed");
        assert_eq!(marker.template_version, 2);
        assert_eq!(
            marker.hash,
            marker::content_hash(&v2.workflow("only").unwrap().graph)
        );

        // The name follows too, and the line still points at the same row.
        let state = install::state_of(&mut conn, &v2).unwrap().unwrap();
        assert_eq!(state.version, 2);
        assert_eq!(state.workflows[0].workflow_id, workflow_id);
        assert_eq!(state.workflows[0].name.as_deref(), Some("Fixture v2"));
        assert!(!state.customised);
    }

    /// The worst thing this feature could do is overwrite somebody's edit.
    #[test]
    fn an_edited_template_survives_an_upgrade_with_its_edits_and_loses_its_marker() {
        use crate::schema::comfyui_workflows;
        let (_dir, mut conn) = library();
        let v1 = fixture(1, "old prompt");
        install::sync_one(&mut conn, &v1, None).unwrap();
        let workflow_id = only_workflow_id(&mut conn, &v1);

        // The user edits the graph, the way the console would.
        let mut edited = stored_graph(&mut conn, &workflow_id);
        edited["2"]["inputs"]["text"] = json!("MY prompt");
        edited["4"] = json!({ "class_type": "PreviewImage", "inputs": { "images": ["1", 0] } });
        diesel::update(comfyui_workflows::table.filter(comfyui_workflows::id.eq(&workflow_id)))
            .set(comfyui_workflows::workflow_json.eq(edited.to_string()))
            .execute(&mut conn)
            .unwrap();

        let v2 = fixture(2, "new prompt");
        let outcome = install::sync_one(&mut conn, &v2, None).unwrap();
        assert_eq!(outcome.abandoned, 1);
        assert_eq!(outcome.updated, 0);

        let graph = stored_graph(&mut conn, &workflow_id);
        assert_eq!(
            graph["2"]["inputs"]["text"],
            json!("MY prompt"),
            "their edit stands"
        );
        assert!(
            graph.get("4").is_some(),
            "and the node they added is still there"
        );
        assert_eq!(marker::read_marker(&graph), None, "and it is theirs now");
        assert!(graph.get(marker::MARKER_KEY).is_none());

        // The name is theirs too: an abandoned workflow is not renamed.
        let name: String = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(&workflow_id))
            .select(comfyui_workflows::name)
            .first(&mut conn)
            .unwrap();
        assert_eq!(name, "Fixture v1");

        let state = install::state_of(&mut conn, &v2).unwrap().unwrap();
        assert!(state.customised);
        assert!(!state.workflows[0].managed);
    }

    #[test]
    fn a_re_upgrade_after_an_edit_leaves_it_alone() {
        use crate::schema::comfyui_workflows;
        let (_dir, mut conn) = library();
        install::sync_one(&mut conn, &fixture(1, "old prompt"), None).unwrap();
        let workflow_id = only_workflow_id(&mut conn, &fixture(1, "old prompt"));

        let mut edited = stored_graph(&mut conn, &workflow_id);
        edited["2"]["inputs"]["text"] = json!("MY prompt");
        diesel::update(comfyui_workflows::table.filter(comfyui_workflows::id.eq(&workflow_id)))
            .set(comfyui_workflows::workflow_json.eq(edited.to_string()))
            .execute(&mut conn)
            .unwrap();

        install::sync_one(&mut conn, &fixture(2, "new prompt"), None).unwrap();
        let after_first = stored_graph(&mut conn, &workflow_id);

        // Two more releases go by.
        let third = install::sync_one(&mut conn, &fixture(3, "newer prompt"), None).unwrap();
        let fourth = install::sync_one(&mut conn, &fixture(4, "newest prompt"), None).unwrap();
        assert_eq!((third.updated, third.abandoned), (0, 0));
        assert_eq!((fourth.updated, fourth.abandoned), (0, 0));
        assert_eq!(
            stored_graph(&mut conn, &workflow_id),
            after_first,
            "nothing may touch it again"
        );
    }

    #[test]
    fn a_template_whose_version_did_not_move_is_not_rewritten() {
        let (_dir, mut conn) = library();
        let v1 = fixture(1, "prompt");
        install::sync_one(&mut conn, &v1, None).unwrap();
        let outcome = install::sync_one(&mut conn, &v1, None).unwrap();
        assert_eq!(
            outcome,
            install::SyncOutcome {
                unchanged: 1,
                ..Default::default()
            }
        );
    }

    /// Deleting a seeded workflow is a decision, not damage to repair.
    #[test]
    fn a_workflow_the_user_deleted_is_not_resurrected() {
        use crate::schema::{comfyui_workflows, line_stages};
        let (_dir, mut conn) = library();
        let v1 = fixture(1, "prompt");
        install::sync_one(&mut conn, &v1, None).unwrap();
        let workflow_id = only_workflow_id(&mut conn, &v1);
        diesel::delete(line_stages::table.filter(line_stages::workflow_id.eq(&workflow_id)))
            .execute(&mut conn)
            .unwrap();
        diesel::delete(comfyui_workflows::table.filter(comfyui_workflows::id.eq(&workflow_id)))
            .execute(&mut conn)
            .unwrap();

        install::sync_one(&mut conn, &fixture(2, "prompt"), None).unwrap();
        let count: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 0);
        let state = install::state_of(&mut conn, &v1).unwrap().unwrap();
        assert_eq!(state.workflows[0].name, None);
    }

    /// Releasing the claim is what makes the console's Install offer it again —
    /// and the reinstall is a fresh copy, leaving the old rows alone.
    #[test]
    fn releasing_a_template_lets_it_be_installed_again_beside_the_old_one() {
        use crate::schema::comfyui_workflows;
        let (_dir, mut conn) = library();
        let v1 = fixture(1, "prompt");
        install::sync_one(&mut conn, &v1, None).unwrap();
        let first = only_workflow_id(&mut conn, &v1);

        install::release(&mut conn, "fixture").unwrap();
        assert!(install::state_of(&mut conn, &v1).unwrap().is_none());

        install::sync_one(&mut conn, &v1, None).unwrap();
        let second = only_workflow_id(&mut conn, &v1);
        assert_ne!(first, second);
        let count: i64 = comfyui_workflows::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 2, "the old one is still the user's");
    }

    /// A graph the console cannot even parse is never overwritten: whatever is
    /// in there, guessing loses it.
    #[test]
    fn an_unparseable_stored_graph_is_left_where_it_is() {
        use crate::schema::comfyui_workflows;
        let (_dir, mut conn) = library();
        let v1 = fixture(1, "prompt");
        install::sync_one(&mut conn, &v1, None).unwrap();
        let workflow_id = only_workflow_id(&mut conn, &v1);
        diesel::update(comfyui_workflows::table.filter(comfyui_workflows::id.eq(&workflow_id)))
            .set(comfyui_workflows::workflow_json.eq("not json at all"))
            .execute(&mut conn)
            .unwrap();

        install::sync_one(&mut conn, &fixture(2, "prompt"), None).unwrap();
        let raw: String = comfyui_workflows::table
            .filter(comfyui_workflows::id.eq(&workflow_id))
            .select(comfyui_workflows::workflow_json)
            .first(&mut conn)
            .unwrap();
        assert_eq!(raw, "not json at all");
    }

    /// The seeded graph is a graph the dispatcher can use: the marker is not a
    /// node, and nothing that reads the graph may trip over it.
    #[test]
    fn a_seeded_graph_still_reads_as_the_graph_it_is() {
        let (_dir, mut conn) = library();
        seed_library(&mut conn);
        let bundle = bundled_by_key("restore-upscale").unwrap();
        let id = only_workflow_id(&mut conn, bundle);
        let graph = stored_graph(&mut conn, &id);

        assert!(graph.get(marker::MARKER_KEY).is_some(), "marked");
        crate::comfyui::importable(&graph).expect("still importable");
        assert_eq!(crate::comfyui::detect_outputs(&graph).len(), 1);
        assert_eq!(
            marker::strip_marker(&graph),
            bundle.workflow("upscale").unwrap().graph
        );
    }
}
