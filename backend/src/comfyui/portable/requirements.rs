//! What a line needs installed, and whether this box has it.
//!
//! A line that arrived from somewhere else references node classes and model
//! files that were on the machine it was drawn on. This works out which, and
//! asks [`NodeCatalog`] — the same `/object_info` catalogue the contract
//! deriver and the parameter typer read — whether they are here.
//!
//! # Told at import, not at dispatch
//!
//! The point is the timing. A line naming `WanImageToVideo` on a box without it
//! is a line that cannot run, and finding that out four stages into a run is
//! the failure mode this whole subsystem is arranged to avoid. So the answer is
//! computed while the person is still looking at the import dialog, and it
//! names the classes rather than saying "something is missing".
//!
//! # An answer nobody could give is not "no"
//!
//! ComfyUI can be down, too old for `/object_info`, or answering something that
//! parses to nothing — [`super::super::nodes`] returns `None` for all three,
//! and every other caller in this module treats that as ordinary and degrades.
//! So does this one: with no catalogue the import still happens, and the report
//! says the requirements could not be checked. A confident wrong answer would
//! be worse than an admitted unknown, and a refusal would make a dead ComfyUI
//! into a reason a line cannot be filed.
//!
//! # Requirements are derived, never trusted
//!
//! A bundle carries a `requirements` block, and it is documentation for whoever
//! opens the file in an editor. The importer ignores it and recomputes from the
//! graphs in the same file, because the graphs are what will actually be sent
//! to ComfyUI and a hand-edited manifest must not be able to talk an import
//! into believing something untrue.

use super::super::nodes::{NodeCatalog, WidgetSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// Suffixes that mean "this string names a weights file".
///
/// Deliberately a property of the *value* rather than of the field name or of
/// what the catalogue says the field is: the same rule has to give the same
/// answer where the line is exported (where ComfyUI may be down) and where it
/// is imported (where it is a different ComfyUI entirely). Image and video
/// filenames — `.png`, `.mp4` — are not here, because a `LoadImage` default is
/// not something the target box has to have.
const MODEL_SUFFIXES: &[&str] = &[
    ".safetensors",
    ".ckpt",
    ".pt",
    ".pth",
    ".bin",
    ".gguf",
    ".sft",
    ".onnx",
];

/// One weights file a graph names, and where it names it.
///
/// The node class and field are carried because "sd_xl.safetensors is missing"
/// is not actionable and "CheckpointLoaderSimple wants sd_xl.safetensors in
/// ckpt_name" is: it says which of ComfyUI's model directories to put it in.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
pub struct ModelRef {
    pub class_type: String,
    pub field: String,
    pub name: String,
}

/// What a line needs installed on the ComfyUI that will run it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Requirements {
    /// Every distinct `class_type` in the line's graphs, sorted.
    #[serde(default)]
    pub node_classes: Vec<String>,
    /// Every weights file the graphs name, sorted.
    #[serde(default)]
    pub models: Vec<ModelRef>,
}

impl Requirements {
    /// Read the requirements straight off the graphs.
    pub fn derive<'a>(graphs: impl Iterator<Item = &'a Value>) -> Self {
        let mut classes: BTreeSet<String> = BTreeSet::new();
        let mut models: BTreeSet<ModelRef> = BTreeSet::new();

        for graph in graphs {
            let Some(nodes) = graph.as_object() else {
                continue;
            };
            for node in nodes.values() {
                let Some(class_type) = node.get("class_type").and_then(|v| v.as_str()) else {
                    continue;
                };
                classes.insert(class_type.to_string());

                let Some(inputs) = node.get("inputs").and_then(|v| v.as_object()) else {
                    continue;
                };
                for (field, value) in inputs {
                    // A link is `["6", 0]`, never a filename.
                    let Some(text) = value.as_str() else { continue };
                    if names_a_model(text) {
                        models.insert(ModelRef {
                            class_type: class_type.to_string(),
                            field: field.clone(),
                            name: text.to_string(),
                        });
                    }
                }
            }
        }

        Requirements {
            node_classes: classes.into_iter().collect(),
            models: models.into_iter().collect(),
        }
    }

    /// Does this box have what the line needs?
    ///
    /// `catalog` is `None` whenever nothing could be learned about the server,
    /// and the report then says so rather than guessing either way.
    pub fn check(&self, catalog: Option<&NodeCatalog>) -> RequirementsReport {
        let Some(catalog) = catalog.filter(|c| !c.is_empty()) else {
            return RequirementsReport {
                checked: false,
                missing_nodes: Vec::new(),
                missing_models: Vec::new(),
                unverified_models: self.models.clone(),
                warnings: vec![
                    "ComfyUI could not be asked what it has installed, so this line's \
                     requirements were not checked. It may reference nodes or models this \
                     server does not have."
                        .to_string(),
                ],
            };
        };

        let missing_nodes: Vec<String> = self
            .node_classes
            .iter()
            .filter(|c| catalog.get(c).is_none())
            .cloned()
            .collect();

        let mut missing_models = Vec::new();
        let mut unverified_models = Vec::new();
        for model in &self.models {
            // A model on a node that is not installed is already reported by
            // the node being missing; saying it twice would make one problem
            // look like two.
            let Some(class) = catalog.get(&model.class_type) else {
                continue;
            };
            match class.input(&model.field).map(|i| &i.widget) {
                // ComfyUI fills an installed-assets combo with exactly the
                // files it found, so this is the one case that can be decided.
                Some(WidgetSpec::Combo {
                    choices, truncated, ..
                }) => {
                    if choices.iter().any(|c| c == &model.name) {
                        continue;
                    }
                    // A combo cut short beside a stored workflow is not
                    // evidence of absence.
                    if *truncated {
                        unverified_models.push(model.clone());
                    } else {
                        missing_models.push(model.clone());
                    }
                }
                // Present, but not an enum of installed files here: a text box
                // holding a path, or a field this ComfyUI version has retyped.
                _ => unverified_models.push(model.clone()),
            }
        }

        let mut warnings = Vec::new();
        if !unverified_models.is_empty() {
            warnings.push(format!(
                "{} model reference{} could not be checked against this server.",
                unverified_models.len(),
                if unverified_models.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }

        RequirementsReport {
            checked: true,
            missing_nodes,
            missing_models,
            unverified_models,
            warnings,
        }
    }
}

/// What the catalogue said, and what is missing.
///
/// Never a refusal on its own: an import goes through with things missing, so a
/// line can be filed on the box that stores the library and run on the box that
/// has the GPU. The report is what the person acts on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequirementsReport {
    /// False when there was no catalogue to check against.
    pub checked: bool,
    /// Node classes this ComfyUI does not have. Naming them is the whole point.
    pub missing_nodes: Vec<String>,
    /// Weights files this ComfyUI's own enum of installed files does not list.
    pub missing_models: Vec<ModelRef>,
    /// Weights files that could be neither confirmed nor denied.
    pub unverified_models: Vec<ModelRef>,
    pub warnings: Vec<String>,
}

impl RequirementsReport {
    /// Will this line run here, as far as anyone can tell?
    pub fn is_ready(&self) -> bool {
        self.checked && self.missing_nodes.is_empty() && self.missing_models.is_empty()
    }

    /// The three states the UI colours: `ready`, `missing`, `unchecked`.
    pub fn status(&self) -> &'static str {
        if !self.checked {
            "unchecked"
        } else if self.missing_nodes.is_empty() && self.missing_models.is_empty() {
            "ready"
        } else {
            "missing"
        }
    }

    /// One sentence naming what is missing.
    ///
    /// Names the classes rather than counting them: "install WanImageToVideo"
    /// is an instruction, "1 node missing" is a puzzle.
    pub fn headline(&self) -> String {
        if !self.checked {
            return "Requirements not checked — ComfyUI could not be reached.".to_string();
        }
        match (
            self.missing_nodes.is_empty(),
            self.missing_models.is_empty(),
        ) {
            (true, true) => "Everything this line needs is installed.".to_string(),
            (false, true) => format!("Missing node classes: {}.", self.missing_nodes.join(", ")),
            (true, false) => format!("Missing models: {}.", self.model_names().join(", ")),
            (false, false) => format!(
                "Missing node classes: {}. Missing models: {}.",
                self.missing_nodes.join(", "),
                self.model_names().join(", ")
            ),
        }
    }

    fn model_names(&self) -> Vec<&str> {
        self.missing_models
            .iter()
            .map(|m| m.name.as_str())
            .collect()
    }
}

/// Does this string name a weights file?
fn names_a_model(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    MODEL_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::super::super::nodes::{parse_object_info, NodeClass, NodeInput};
    use super::*;
    use serde_json::json;

    fn graph() -> Value {
        json!({
            "3": {"class_type": "KSampler", "inputs": {"seed": 1, "model": ["4", 0]}},
            "4": {"class_type": "CheckpointLoaderSimple",
                  "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}},
            "5": {"class_type": "LoraLoader",
                  "inputs": {"lora_name": "add_detail.safetensors", "strength_model": 1.0}},
            "6": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
            "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out"}}
        })
    }

    /// A catalogue with the classes `graph()` uses, and the two combos filled
    /// with what this imaginary box has installed.
    fn catalog(installed_checkpoints: &[&str], installed_loras: &[&str]) -> NodeCatalog {
        let mut catalog = parse_object_info(&json!({
            "KSampler": {"input": {"required": {"seed": ["INT", {}]}}},
            "LoadImage": {"input": {"required": {"image": ["IMAGE"]}}},
            "SaveImage": {"input": {"required": {}}, "output_node": true}
        }));
        catalog.classes.insert(
            "CheckpointLoaderSimple".to_string(),
            combo_class(
                "CheckpointLoaderSimple",
                "ckpt_name",
                installed_checkpoints,
                false,
            ),
        );
        catalog.classes.insert(
            "LoraLoader".to_string(),
            combo_class("LoraLoader", "lora_name", installed_loras, false),
        );
        catalog
    }

    fn combo_class(name: &str, field: &str, choices: &[&str], truncated: bool) -> NodeClass {
        NodeClass {
            name: name.to_string(),
            display_name: None,
            category: None,
            output_node: false,
            inputs: vec![NodeInput {
                name: field.to_string(),
                required: true,
                widget: WidgetSpec::Combo {
                    choices: choices.iter().map(|c| json!(c)).collect(),
                    default: None,
                    truncated,
                },
                tooltip: None,
            }],
        }
    }

    // === Deriving ===========================================================

    #[test]
    fn every_class_the_graphs_use_is_a_requirement() {
        let r = Requirements::derive(std::iter::once(&graph()));
        assert_eq!(
            r.node_classes,
            vec![
                "CheckpointLoaderSimple",
                "KSampler",
                "LoadImage",
                "LoraLoader",
                "SaveImage"
            ]
        );
    }

    #[test]
    fn weights_files_are_requirements_and_ordinary_filenames_are_not() {
        let r = Requirements::derive(std::iter::once(&graph()));
        assert_eq!(
            r.models,
            vec![
                ModelRef {
                    class_type: "CheckpointLoaderSimple".into(),
                    field: "ckpt_name".into(),
                    name: "sd_xl_base_1.0.safetensors".into()
                },
                ModelRef {
                    class_type: "LoraLoader".into(),
                    field: "lora_name".into(),
                    name: "add_detail.safetensors".into()
                },
            ],
            "example.png is a LoadImage default, not something the target box must have"
        );
    }

    #[test]
    fn a_line_of_several_stages_needs_the_union_of_their_graphs() {
        let second = json!({
            "1": {"class_type": "UpscaleModelLoader",
                  "inputs": {"model_name": "4x-UltraSharp.pth"}}
        });
        let r = Requirements::derive([&graph(), &second].into_iter());
        assert!(r.node_classes.contains(&"UpscaleModelLoader".to_string()));
        assert!(r.node_classes.contains(&"KSampler".to_string()));
        assert_eq!(r.models.len(), 3);
    }

    #[test]
    fn a_link_is_never_mistaken_for_a_filename() {
        let r = Requirements::derive(std::iter::once(&json!({
            "3": {"class_type": "KSampler", "inputs": {"model": ["4", 0]}}
        })));
        assert!(r.models.is_empty());
    }

    #[test]
    fn the_same_model_named_by_two_stages_is_one_requirement() {
        let r = Requirements::derive([&graph(), &graph()].into_iter());
        assert_eq!(r.models.len(), 2);
    }

    // === Checking ===========================================================

    #[test]
    fn a_box_with_everything_installed_reads_ready() {
        let r = Requirements::derive(std::iter::once(&graph()));
        let report = r.check(Some(&catalog(
            &["sd_xl_base_1.0.safetensors"],
            &["add_detail.safetensors"],
        )));
        assert!(report.is_ready());
        assert_eq!(report.status(), "ready");
        assert_eq!(
            report.headline(),
            "Everything this line needs is installed."
        );
    }

    #[test]
    fn a_box_without_the_node_class_is_told_exactly_which_one() {
        // The reported case: a line built around WanImageToVideo, opened on a
        // box that has never had it.
        let wan = json!({
            "1": {"class_type": "LoadImage", "inputs": {"image": "a.png"}},
            "2": {"class_type": "WanImageToVideo", "inputs": {"length": 81}},
            "3": {"class_type": "VHS_VideoCombine", "inputs": {"filename_prefix": "out"}}
        });
        let r = Requirements::derive(std::iter::once(&wan));
        let report = r.check(Some(&catalog(&[], &[])));

        assert_eq!(report.status(), "missing");
        assert!(!report.is_ready());
        assert_eq!(
            report.missing_nodes,
            vec!["VHS_VideoCombine", "WanImageToVideo"]
        );
        assert!(
            report.headline().contains("WanImageToVideo"),
            "{}",
            report.headline()
        );
        assert!(report.missing_models.is_empty());
    }

    #[test]
    fn a_model_this_box_does_not_have_is_named_with_the_field_to_put_it_in() {
        let r = Requirements::derive(std::iter::once(&graph()));
        let report = r.check(Some(&catalog(
            &["v1-5-pruned.ckpt"],
            &["other.safetensors"],
        )));

        assert_eq!(report.status(), "missing");
        assert_eq!(
            report.missing_models,
            vec![
                ModelRef {
                    class_type: "CheckpointLoaderSimple".into(),
                    field: "ckpt_name".into(),
                    name: "sd_xl_base_1.0.safetensors".into()
                },
                ModelRef {
                    class_type: "LoraLoader".into(),
                    field: "lora_name".into(),
                    name: "add_detail.safetensors".into()
                },
            ]
        );
        assert!(report.missing_nodes.is_empty());
    }

    #[test]
    fn a_model_on_a_node_that_is_itself_missing_is_reported_once() {
        let r = Requirements::derive(std::iter::once(&json!({
            "1": {"class_type": "SomeCustomLoader", "inputs": {"weights": "x.safetensors"}}
        })));
        let report = r.check(Some(&catalog(&[], &[])));
        assert_eq!(report.missing_nodes, vec!["SomeCustomLoader"]);
        assert!(
            report.missing_models.is_empty(),
            "one problem, not two: the node is what is missing"
        );
    }

    #[test]
    fn a_truncated_enum_is_unverified_rather_than_missing() {
        let mut catalog = catalog(&["a.safetensors"], &[]);
        catalog.classes.insert(
            "CheckpointLoaderSimple".to_string(),
            combo_class(
                "CheckpointLoaderSimple",
                "ckpt_name",
                &["a.safetensors"],
                true,
            ),
        );
        let r = Requirements::derive(std::iter::once(&json!({
            "4": {"class_type": "CheckpointLoaderSimple",
                  "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}}
        })));
        let report = r.check(Some(&catalog));
        assert!(report.missing_models.is_empty());
        assert_eq!(report.unverified_models.len(), 1);
        assert_eq!(report.status(), "ready", "an unknown is not a refusal");
    }

    #[test]
    fn a_field_that_is_not_an_enum_of_installed_files_is_unverified() {
        let mut catalog = catalog(&[], &[]);
        catalog.classes.insert(
            "CheckpointLoaderSimple".to_string(),
            NodeClass {
                name: "CheckpointLoaderSimple".to_string(),
                display_name: None,
                category: None,
                output_node: false,
                inputs: vec![NodeInput {
                    name: "ckpt_name".to_string(),
                    required: true,
                    widget: WidgetSpec::Text {
                        multiline: false,
                        default: None,
                    },
                    tooltip: None,
                }],
            },
        );
        let r = Requirements::derive(std::iter::once(&json!({
            "4": {"class_type": "CheckpointLoaderSimple",
                  "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}}
        })));
        let report = r.check(Some(&catalog));
        assert!(report.missing_models.is_empty());
        assert_eq!(report.unverified_models.len(), 1);
        assert!(report.warnings[0].contains("could not be checked"));
    }

    // === No catalogue =======================================================

    #[test]
    fn with_no_catalogue_nothing_is_declared_missing() {
        let r = Requirements::derive(std::iter::once(&graph()));
        let report = r.check(None);

        assert!(!report.checked);
        assert_eq!(report.status(), "unchecked");
        assert!(report.missing_nodes.is_empty());
        assert!(report.missing_models.is_empty());
        assert_eq!(report.unverified_models.len(), 2);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("could not be asked"));
        assert!(report.headline().contains("could not be reached"));
    }

    #[test]
    fn an_empty_catalogue_is_the_same_as_no_catalogue() {
        // `/object_info` answered something nothing parsed out of. Believing it
        // would report every class in the line as missing.
        let r = Requirements::derive(std::iter::once(&graph()));
        let report = r.check(Some(&NodeCatalog::default()));
        assert!(!report.checked);
        assert!(report.missing_nodes.is_empty());
    }
}
