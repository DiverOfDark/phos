//! A line as a file: the one format a line travels in.
//!
//! Lines live in their library's own `.phos.db`, which is what makes a
//! library's metadata travel with its files — and also what stops a line ever
//! leaving. This module is the way out: one JSON document that carries a line,
//! its stages, *and the workflow graphs those stages point at*, so it can be
//! opened on another install, another machine, or by somebody else.
//!
//! # Why the graphs are in the file
//!
//! A line is a chain of workflow ids. Ids mean nothing anywhere but the library
//! that minted them, so a line exported as ids alone is a bundle of broken
//! pointers — it would import "successfully" and then fail at the first
//! dispatch. The graphs go in the file, and the importer either finds them
//! already present or creates them.
//!
//! # This is also the template format
//!
//! FR6 ships bundled templates. A template is a line somebody else drew, which
//! is exactly what this file is, so there is one format rather than two: a
//! bundled template is an exported line, seeded through the same importer, and
//! gets the same requirements report. Everything a template needs beyond a
//! hand-written export is optional here — `contract` is re-derived when absent,
//! `requirements` is recomputed rather than trusted, and workflow keys are
//! arbitrary strings so a template can say `"upscale"` where an export says a
//! uuid.
//!
//! # Nothing in this module touches a database or a network
//!
//! Parsing, canonicalising, deriving requirements, checking them against a
//! catalogue and resolving a name collision are all pure functions of their
//! arguments, and are tested without a library on disk. `api::line_io` holds
//! the parts that write.

mod requirements;

pub use requirements::{ModelRef, Requirements, RequirementsReport};

use super::contract::StageContract;
use super::params::{ParameterMap, VaryMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// The discriminator. A file without it is not a line export, and saying so is
/// more use than a serde error about a missing field called `line`.
pub const BUNDLE_FORMAT: &str = "phos.line";

/// Bumped only when a field changes meaning. Adding one does not need it:
/// everything optional is `#[serde(default)]`, so a v1 reader keeps reading.
pub const BUNDLE_VERSION: u32 = 1;

// ===== The document =========================================================

/// One step of a line, as it travels.
///
/// Field for field this is what line CRUD already takes, with one difference:
/// `workflow` is a key into this bundle's own `workflows` map rather than a
/// library id, because a library id is meaningless in the library it is being
/// carried to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BundleStage {
    /// Which entry of `workflows` this stage runs. Any string: an export uses
    /// the source library's workflow id, a hand-written template can use a
    /// readable name.
    pub workflow: String,
    /// Prompt bindings, plus the `role:<node_id>` directives the loader binder
    /// reads.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub text_overrides: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub parameters: ParameterMap,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub vary: VaryMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_mode: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_output: bool,
    /// The keys this stage deliberately left open — FR5b's *exposed*
    /// disposition. Travels with the stage for the same reason `parameters`
    /// does: a line that asks the sender for a seed is not the same line once
    /// it has been carried somewhere that pins one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposed: Vec<String>,
}

/// The line itself. Stage order is array order — there is no `stage_idx`,
/// because two sources of truth for one ordering is one too many.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BundleLine {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub stages: Vec<BundleStage>,
}

/// A workflow, carried whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BundleWorkflow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The ComfyUI API-format graph, exactly as the library stores it.
    #[schema(value_type = Object)]
    pub graph: Value,
    /// The contract this workflow carried where it came from.
    ///
    /// Optional, and re-derived on import against the *target* box's node
    /// catalogue — only the `corrections` inside it survive verbatim, because a
    /// correction is the one part of a contract that cannot be worked out
    /// again. A hand-written template can leave it out entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub contract: Option<StageContract>,
}

/// A line, its stages, its workflows and what it needs installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LineBundle {
    /// Always [`BUNDLE_FORMAT`].
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub format_version: u32,
    /// When it left, for a human reading the file. Never read by the importer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_at: Option<String>,
    /// Present on a **bundled template**, absent on somebody's export.
    ///
    /// The one block FR6 adds to this format, and the reason it is declared
    /// there rather than here: it says nothing about the line, only about
    /// Phos's claim on it — the key an upgrade matches and the version it
    /// compares. Carried on the document type all the same, so a template
    /// pasted into the import box round-trips as the file it is instead of
    /// quietly losing its identity on the way through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<super::templates::bundle::Template>,
    pub line: BundleLine,
    /// Every workflow the stages name, keyed by whatever they name it.
    #[serde(default)]
    pub workflows: BTreeMap<String, BundleWorkflow>,
    /// What this line needs installed, as computed where it was exported.
    ///
    /// Documentation, not input: the importer recomputes it from the graphs in
    /// this same file, so a stale or hand-edited manifest cannot make an import
    /// report something untrue.
    #[serde(default)]
    pub requirements: Requirements,
}

impl LineBundle {
    /// The template block, on a document that has one.
    pub fn template(&self) -> Option<&super::templates::bundle::Template> {
        self.template.as_ref()
    }

    /// The key an upgrade matches on. Empty for an ordinary export, which is
    /// nothing a template sync ever looks at.
    pub fn key(&self) -> &str {
        self.template.as_ref().map_or("", |t| t.key.as_str())
    }

    pub fn version(&self) -> u32 {
        self.template.as_ref().map_or(0, |t| t.version)
    }

    pub fn workflow(&self, key: &str) -> Option<&BundleWorkflow> {
        self.workflows.get(key)
    }

    /// The graphs, for anything that derives from all of them at once.
    pub fn graphs(&self) -> impl Iterator<Item = &Value> {
        self.workflows.values().map(|w| &w.graph)
    }

    /// Read a bundle out of arbitrary JSON, with the refusals worth wording.
    ///
    /// Checks the discriminator before anything else, so pasting a ComfyUI
    /// workflow into the import box says what happened rather than complaining
    /// about a missing field.
    pub fn parse(doc: &Value) -> Result<Self, String> {
        match doc.get("format").and_then(|v| v.as_str()) {
            Some(BUNDLE_FORMAT) => {}
            Some(other) => {
                return Err(format!(
                    "This file says it is '{}', not a Phos line export.",
                    other
                ))
            }
            None => {
                return Err(
                    "This is not a Phos line export — it has no 'format' field. Export a \
                     line from the Lines tab to get one."
                        .to_string(),
                )
            }
        }

        let version = doc
            .get("format_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if version > BUNDLE_VERSION as u64 {
            return Err(format!(
                "This line was exported by a newer Phos (format {}, this one reads {}). \
                 Update Phos and try again.",
                version, BUNDLE_VERSION
            ));
        }

        let bundle: LineBundle = serde_json::from_value(doc.clone())
            .map_err(|e| format!("This line export could not be read: {}", e))?;
        bundle.validate()?;
        Ok(bundle)
    }

    /// Everything that has to be true before the bundle is worth acting on.
    ///
    /// Not a type check on the chain — that is [`super::line::validate_chain`],
    /// which an import reaches through the very same path a person drawing a
    /// line in the editor goes through. This is only "does the document hold
    /// together with itself".
    fn validate(&self) -> Result<(), String> {
        if self.line.name.trim().is_empty() {
            return Err("This export has no line name.".to_string());
        }
        if self.line.stages.is_empty() {
            return Err("This export has no stages.".to_string());
        }
        for (idx, stage) in self.line.stages.iter().enumerate() {
            let Some(wf) = self.workflows.get(&stage.workflow) else {
                return Err(format!(
                    "Stage {} names workflow '{}', which this file does not contain.",
                    idx + 1,
                    stage.workflow
                ));
            };
            if !wf.graph.is_object() {
                return Err(format!(
                    "Stage {} ({}) carries no ComfyUI API-format graph.",
                    idx + 1,
                    wf.name
                ));
            }
        }
        Ok(())
    }

    /// What this line needs, computed from the graphs in this file.
    ///
    /// Used in preference to `self.requirements` everywhere it matters. The two
    /// agree for anything Phos exported; where they do not, the graphs are what
    /// will actually be run.
    pub fn derived_requirements(&self) -> Requirements {
        Requirements::derive(self.workflows.values().map(|w| &w.graph))
    }

    /// The bundle a line becomes on the way out. `exported_at` is the caller's
    /// to fill in — a clock is not something this module has.
    pub fn build(line: BundleLine, workflows: BTreeMap<String, BundleWorkflow>) -> Self {
        let requirements = Requirements::derive(workflows.values().map(|w| &w.graph));
        LineBundle {
            format: BUNDLE_FORMAT.to_string(),
            format_version: BUNDLE_VERSION,
            exported_at: None,
            // A line somebody exports is theirs, not one Phos tracks.
            template: None,
            line,
            workflows,
            requirements,
        }
    }
}

// ===== Is this graph one we already have? ===================================

/// A graph reduced to a form two copies of it always share.
///
/// "The same workflow" has to mean something exact, or an import either makes a
/// near-duplicate of everything it touches or silently reuses a graph that is
/// not the one it was given. It means: the same JSON, with object keys in
/// sorted order and no insignificant whitespace. That is byte-equality plus the
/// key reorderings a round trip through a JSON library can introduce, and
/// nothing else — a changed seed, a swapped checkpoint or an added node all
/// make a different workflow, because they all make a different run.
pub fn canonical_graph(graph: &Value) -> String {
    let mut out = String::new();
    write_canonical(graph, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

// ===== Two lines cannot have the same name ==================================

/// A name for the imported line that is not already in use.
///
/// Importing must not overwrite the line somebody already has, and must not
/// leave two rows called the same thing that only an id tells apart. So the
/// import is renamed and the caller is told it was: the imported line is the
/// one that moves, because it is the one the person is looking at.
///
/// Terminates: `taken` is finite, so at most `taken.len() + 1` candidates are
/// tried before one is free.
pub fn available_name(desired: &str, taken: &[String]) -> String {
    let desired = desired.trim();
    if !taken.iter().any(|t| t == desired) {
        return desired.to_string();
    }
    let first = format!("{} (imported)", desired);
    if !taken.contains(&first) {
        return first;
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{} (imported {})", desired, n);
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph() -> Value {
        json!({
            "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20}},
            "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
            "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out"}}
        })
    }

    fn bundle_doc() -> Value {
        json!({
            "format": "phos.line",
            "format_version": 1,
            "line": {
                "name": "4K Restore",
                "stages": [{ "workflow": "wf-a", "keep_output": true }]
            },
            "workflows": {
                "wf-a": { "name": "Portrait", "graph": graph() }
            }
        })
    }

    // === The discriminator ==================================================

    #[test]
    fn a_comfyui_workflow_pasted_into_the_import_box_says_what_it_is() {
        let err = LineBundle::parse(&graph()).unwrap_err();
        assert!(err.contains("not a Phos line export"), "{}", err);
        assert!(err.contains("'format'"), "{}", err);
    }

    #[test]
    fn a_bundle_from_a_newer_phos_is_refused_by_name() {
        let mut doc = bundle_doc();
        doc["format_version"] = json!(99);
        let err = LineBundle::parse(&doc).unwrap_err();
        assert!(err.contains("newer Phos"), "{}", err);
    }

    #[test]
    fn a_stage_pointing_at_a_workflow_the_file_does_not_carry_is_refused() {
        let mut doc = bundle_doc();
        doc["line"]["stages"][0]["workflow"] = json!("wf-missing");
        let err = LineBundle::parse(&doc).unwrap_err();
        assert!(err.contains("wf-missing"), "{}", err);
        assert!(err.contains("does not contain"), "{}", err);
    }

    #[test]
    fn a_workflow_carrying_no_graph_is_refused_naming_the_stage() {
        let mut doc = bundle_doc();
        doc["workflows"]["wf-a"]["graph"] = json!("nope");
        let err = LineBundle::parse(&doc).unwrap_err();
        assert!(err.contains("Stage 1"), "{}", err);
        assert!(err.contains("Portrait"), "{}", err);
    }

    #[test]
    fn a_line_with_no_stages_is_refused() {
        let mut doc = bundle_doc();
        doc["line"]["stages"] = json!([]);
        assert!(LineBundle::parse(&doc).unwrap_err().contains("no stages"));
    }

    // === Defaults, so a template can be written by hand =====================

    #[test]
    fn everything_a_hand_written_template_can_omit_is_omittable() {
        // No description, no contract, no requirements, no overrides, no
        // parameters, no vary, no source mode.
        let bundle = LineBundle::parse(&bundle_doc()).unwrap();
        assert_eq!(bundle.line.description, None);
        assert_eq!(bundle.workflows["wf-a"].contract, None);
        assert!(bundle.requirements.node_classes.is_empty());
        let stage = &bundle.line.stages[0];
        assert!(stage.text_overrides.is_empty());
        assert!(stage.parameters.is_empty());
        assert!(stage.vary.is_empty());
        assert_eq!(stage.source_mode, None);
        assert!(stage.keep_output);
    }

    #[test]
    fn an_omitted_requirements_block_is_recomputed_from_the_graphs() {
        let bundle = LineBundle::parse(&bundle_doc()).unwrap();
        assert_eq!(
            bundle.derived_requirements().node_classes,
            vec!["KSampler", "LoadImage", "SaveImage"]
        );
    }

    #[test]
    fn build_fills_in_the_discriminator_and_the_requirements() {
        let mut workflows = BTreeMap::new();
        workflows.insert(
            "wf-a".to_string(),
            BundleWorkflow {
                name: "Portrait".to_string(),
                description: None,
                graph: graph(),
                contract: None,
            },
        );
        let bundle = LineBundle::build(
            BundleLine {
                name: "L".to_string(),
                description: None,
                stages: vec![BundleStage {
                    workflow: "wf-a".to_string(),
                    text_overrides: HashMap::new(),
                    parameters: ParameterMap::new(),
                    vary: VaryMap::new(),
                    source_mode: None,
                    keep_output: false,
                    exposed: Vec::new(),
                }],
            },
            workflows,
        );
        assert_eq!(bundle.format, BUNDLE_FORMAT);
        assert_eq!(bundle.format_version, BUNDLE_VERSION);
        assert!(bundle
            .requirements
            .node_classes
            .contains(&"KSampler".to_string()));
        // And what it built reads back as itself.
        let doc = serde_json::to_value(&bundle).unwrap();
        assert_eq!(LineBundle::parse(&doc).unwrap(), bundle);
    }

    // === Canonicalisation ===================================================

    #[test]
    fn the_same_graph_written_in_a_different_key_order_is_the_same_graph() {
        let a: Value =
            serde_json::from_str(r#"{"b": 1, "a": {"y": [1, 2], "x": "s"}, "c": null}"#).unwrap();
        let b: Value =
            serde_json::from_str(r#"{"c": null, "a": {"x": "s", "y": [1, 2]}, "b": 1}"#).unwrap();
        assert_eq!(canonical_graph(&a), canonical_graph(&b));
        assert_eq!(
            canonical_graph(&a),
            r#"{"a":{"x":"s","y":[1,2]},"b":1,"c":null}"#
        );
    }

    #[test]
    fn a_changed_value_or_an_added_node_is_a_different_graph() {
        let base = graph();
        let mut seed = base.clone();
        seed["3"]["inputs"]["seed"] = json!(2);
        let mut extra = base.clone();
        extra["10"] = json!({"class_type": "PreviewImage", "inputs": {}});
        assert_ne!(canonical_graph(&base), canonical_graph(&seed));
        assert_ne!(canonical_graph(&base), canonical_graph(&extra));
    }

    #[test]
    fn array_order_is_not_canonicalised_away() {
        // Node links are arrays — `["6", 0]` and `[0, "6"]` are different wires.
        let a = json!({"inputs": {"latent": ["6", 0]}});
        let b = json!({"inputs": {"latent": [0, "6"]}});
        assert_ne!(canonical_graph(&a), canonical_graph(&b));
    }

    // === Name collisions ====================================================

    #[test]
    fn a_free_name_is_used_as_it_is() {
        assert_eq!(available_name("4K Restore", &[]), "4K Restore");
        assert_eq!(
            available_name("  4K Restore  ", &["Other".to_string()]),
            "4K Restore"
        );
    }

    #[test]
    fn a_taken_name_is_suffixed_rather_than_overwritten() {
        let taken = vec!["4K Restore".to_string()];
        assert_eq!(
            available_name("4K Restore", &taken),
            "4K Restore (imported)"
        );
    }

    #[test]
    fn importing_the_same_line_repeatedly_keeps_numbering() {
        let mut taken: Vec<String> = vec!["Restore".to_string()];
        for expected in [
            "Restore (imported)",
            "Restore (imported 2)",
            "Restore (imported 3)",
        ] {
            let name = available_name("Restore", &taken);
            assert_eq!(name, expected);
            taken.push(name);
        }
    }
}
