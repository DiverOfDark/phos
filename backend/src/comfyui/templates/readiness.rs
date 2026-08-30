//! Can this template run on *this* ComfyUI, and if not, what has to be
//! installed?
//!
//! Asked before anything is dispatched, because the failure this exists to
//! prevent is the slow one: a chain that gets minutes in and comes back with a
//! ComfyUI validation error naming a node the user has never heard of. A
//! template that cannot run must say exactly what to install, on the screen
//! where it is offered.
//!
//! # Two halves, and only one of them is FR6's
//!
//! **What is missing** is FR5d's question, already answered by
//! `comfyui::portable::Requirements::derive(graphs).check(catalog)`, and this
//! module must not answer it a second way — two readiness checks that disagree
//! is precisely the failure the coordination exists to prevent. FR5d landed in
//! parallel on the same base, so [`check`] below is a stand-in written to that
//! exact rule and returning that exact shape.
//!
//! **At integration: delete [`derive_requirements`] and [`check`], and call
//! FR5d's.** [`assess`] then reads
//! `portable::Requirements::derive(bundle.graphs()).check(catalog)`, and
//! everything below the `===== Rendering` line stays as it is.
//!
//! Rendering is FR6's half and does not exist in FR5d: the status vocabulary
//! the console reads (`READY`, `MISSING NODE RIFE VFI`,
//! `MISSING MODEL wan2.1_i2v_720p.safetensors`, `UNKNOWN`), and one extra
//! check FR5d has no reason to make.
//!
//! # The extra check: does the graph fit the node?
//!
//! Node classes and model files are not the only way a template fails at
//! dispatch. A graph written against one release of a node pack can set a field
//! the installed release does not have, or miss one it requires, and ComfyUI
//! refuses the whole prompt for either. The shipped graphs were built from
//! published node definitions rather than exported from a live server, so that
//! is the *likeliest* way they are wrong — and `/object_info` can be asked
//! about it. It is a **degraded** answer, never a refusal: a node with dynamic
//! inputs can disagree with its own catalogue entry and run perfectly.
//!
//! # Not knowing is its own answer
//!
//! An unreachable server, one too old for `/object_info`, one whose answer
//! nothing parsed: all of those give [`ReadinessState::Unchecked`] — FR5d's
//! `unchecked`, rendered as `UNKNOWN` — never "missing". Nothing is refused on
//! it either: the template installs, and the run finds out.

use super::bundle::{LineBundle, ModelRef, Requirements};
use crate::comfyui::nodes::{NodeCatalog, WidgetSpec};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

// ===== FR5d's half, stood in for ===========================================

/// Suffixes that mean "this string names a weights file".
///
/// A property of the *value*, not of the field name or of what the catalogue
/// says the field is, so the same rule gives the same answer where a line is
/// exported (ComfyUI possibly down) and where it is imported (a different
/// ComfyUI entirely). `.png` and `.mp4` are not here: a `LoadImage` default is
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

fn names_a_model(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    MODEL_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

/// Read the requirements straight off the graphs.
///
/// Stand-in for `portable::Requirements::derive`. Derived rather than read out
/// of the document's own `requirements` block on purpose: the graphs are what
/// will be sent to ComfyUI, and a hand-written manifest must not be able to
/// talk a template into claiming something untrue.
pub fn derive_requirements<'a>(graphs: impl Iterator<Item = &'a Value>) -> Requirements {
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

/// What the catalogue said, and what is missing.
///
/// Stand-in for `portable::RequirementsReport`, field for field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, utoipa::ToSchema)]
pub struct RequirementsReport {
    /// False when there was no catalogue to check against.
    pub checked: bool,
    pub missing_nodes: Vec<String>,
    pub missing_models: Vec<ModelRef>,
    /// Weights files that could be neither confirmed nor denied.
    pub unverified_models: Vec<ModelRef>,
}

/// Does this box have what the template needs?
///
/// Stand-in for `portable::Requirements::check`.
pub fn check(requirements: &Requirements, catalog: Option<&NodeCatalog>) -> RequirementsReport {
    let Some(catalog) = catalog.filter(|c| !c.is_empty()) else {
        return RequirementsReport {
            checked: false,
            unverified_models: requirements.models.clone(),
            ..Default::default()
        };
    };

    let missing_nodes: Vec<String> = requirements
        .node_classes
        .iter()
        .filter(|c| catalog.get(c).is_none())
        .cloned()
        .collect();

    let mut missing_models = Vec::new();
    let mut unverified_models = Vec::new();
    for model in &requirements.models {
        // A model on a node that is not installed is already reported by the
        // node being missing; saying it twice would make one problem look like
        // two.
        let Some(class) = catalog.get(&model.class_type) else {
            continue;
        };
        match class.input(&model.field).map(|i| &i.widget) {
            // ComfyUI fills an installed-assets combo with exactly the files it
            // found, so this is the one case that can be decided.
            Some(WidgetSpec::Combo {
                choices, truncated, ..
            }) => {
                if choices.iter().any(|c| c == &model.name) {
                    continue;
                }
                if *truncated {
                    unverified_models.push(model.clone());
                } else {
                    missing_models.push(model.clone());
                }
            }
            _ => unverified_models.push(model.clone()),
        }
    }

    RequirementsReport {
        checked: true,
        missing_nodes,
        missing_models,
        unverified_models,
    }
}

// ===== Rendering — FR6's half ==============================================

/// How ready, in one word. FR5d's three, plus one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReadinessState {
    /// Every node and model is there, and the graphs fit the nodes.
    Ready,
    /// Something has to be installed first.
    Missing,
    /// Everything is installed, but a graph and a node disagree about a field.
    /// FR6's; FR5d has no reason to ask.
    Degraded,
    /// The catalogue could not be read. Not a verdict.
    Unchecked,
}

/// A field a graph sets that its node does not take, or one the node requires
/// and the graph does not set.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct InputMismatch {
    pub workflow_key: String,
    pub node_id: String,
    pub node_class: String,
    pub field: String,
    /// `unexpected` — the graph sets it and the node has no such input.
    /// `absent` — the node requires it and the graph does not set it.
    pub kind: String,
    /// The same thing in a sentence, so a UI never has to compose one.
    pub message: String,
}

/// What a template's readiness comes to.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct Readiness {
    pub state: ReadinessState,
    /// The one-line verdict, in the status vocabulary the rest of the console
    /// uses: `READY`, `MISSING NODE RIFE VFI`, `MISSING 2 MODELS`, `UNKNOWN`.
    pub label: String,
    /// The whole story, for the panel under the label.
    pub detail: String,
    /// FR5d's report, verbatim, so a client can render either.
    pub requirements: RequirementsReport,
    pub input_mismatches: Vec<InputMismatch>,
}

/// Ask the catalogue everything, and say what came back.
pub fn assess(bundle: &LineBundle, catalog: Option<&NodeCatalog>) -> Readiness {
    // At integration these two lines become
    // `portable::Requirements::derive(bundle.graphs()).check(catalog)`.
    let report = check(&derive_requirements(bundle.graphs()), catalog);

    let mut input_mismatches = Vec::new();
    if let Some(catalog) = catalog.filter(|c| !c.is_empty()) {
        for (key, wf) in &bundle.workflows {
            input_mismatches.extend(graph_mismatches(key, &wf.graph, catalog));
        }
    }

    let state = if !report.checked {
        ReadinessState::Unchecked
    } else if !report.missing_nodes.is_empty() || !report.missing_models.is_empty() {
        ReadinessState::Missing
    } else if !input_mismatches.is_empty() {
        ReadinessState::Degraded
    } else {
        ReadinessState::Ready
    };

    Readiness {
        label: label_for(state, &report, &input_mismatches),
        detail: detail_for(state, &report, &input_mismatches),
        state,
        requirements: report,
        input_mismatches,
    }
}

/// Fields this graph and this server's nodes disagree about.
fn graph_mismatches(
    workflow_key: &str,
    graph: &Value,
    catalog: &NodeCatalog,
) -> Vec<InputMismatch> {
    let mut out = Vec::new();
    let Some(nodes) = graph.as_object() else {
        return out;
    };
    for (node_id, node) in nodes {
        let Some(class_name) = node.get("class_type").and_then(|v| v.as_str()) else {
            continue;
        };
        // A class the server does not have is a missing node, not a field
        // problem, and its inputs are unknowable.
        let Some(class) = catalog.get(class_name) else {
            continue;
        };
        let set = node.get("inputs").and_then(|i| i.as_object());

        if let Some(set) = set {
            for field in set.keys() {
                if class.input(field).is_none() {
                    out.push(InputMismatch {
                        workflow_key: workflow_key.to_string(),
                        node_id: node_id.clone(),
                        node_class: class_name.to_string(),
                        field: field.clone(),
                        kind: "unexpected".to_string(),
                        message: format!(
                            "Node {} sets {}.{}, which this server's {} does not take.",
                            node_id, class_name, field, class_name
                        ),
                    });
                }
            }
        }
        for input in class.inputs.iter().filter(|i| i.required) {
            if !set.is_some_and(|s| s.contains_key(&input.name)) {
                out.push(InputMismatch {
                    workflow_key: workflow_key.to_string(),
                    node_id: node_id.clone(),
                    node_class: class_name.to_string(),
                    field: input.name.clone(),
                    kind: "absent".to_string(),
                    message: format!(
                        "This server's {} requires {}, which node {} does not set.",
                        class_name, input.name, node_id
                    ),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.workflow_key, &a.node_id, &a.field).cmp(&(&b.workflow_key, &b.node_id, &b.field))
    });
    out
}

/// The verdict, in the uppercase register the console reads statuses in.
///
/// Naming the one thing that is missing beats counting it, so a single missing
/// node or model is spelled out; past one, a count and the panel below.
fn label_for(
    state: ReadinessState,
    report: &RequirementsReport,
    mismatches: &[InputMismatch],
) -> String {
    if state == ReadinessState::Unchecked {
        return "UNKNOWN".to_string();
    }
    let (nodes, models) = (&report.missing_nodes, &report.missing_models);
    match (nodes.len(), models.len()) {
        (0, 0) => {}
        (1, 0) => return format!("MISSING NODE {}", nodes[0]),
        (0, 1) => return format!("MISSING MODEL {}", models[0].name),
        (n, 0) => return format!("MISSING {} NODES", n),
        (0, m) => return format!("MISSING {} MODELS", m),
        (n, m) => {
            return format!(
                "MISSING {} {}, {} {}",
                n,
                if n == 1 { "NODE" } else { "NODES" },
                m,
                if m == 1 { "MODEL" } else { "MODELS" }
            )
        }
    }
    match mismatches.len() {
        0 => "READY".to_string(),
        1 => "CHECK 1 INPUT".to_string(),
        n => format!("CHECK {} INPUTS", n),
    }
}

fn detail_for(
    state: ReadinessState,
    report: &RequirementsReport,
    mismatches: &[InputMismatch],
) -> String {
    if state == ReadinessState::Unchecked {
        return "ComfyUI could not be asked what it has installed, so Phos cannot say whether \
                this template will run."
            .to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if !report.missing_nodes.is_empty() {
        parts.push(format!(
            "This ComfyUI does not have {}. Install the custom node pack that provides {}.",
            join_names(&report.missing_nodes),
            if report.missing_nodes.len() == 1 {
                "it"
            } else {
                "them"
            }
        ));
    }
    for model in &report.missing_models {
        parts.push(format!(
            "{} is not in this ComfyUI's {}.{} picker.",
            model.name, model.class_type, model.field
        ));
    }
    for mismatch in mismatches {
        parts.push(mismatch.message.clone());
    }
    if !report.unverified_models.is_empty() {
        parts.push(format!(
            "{} model reference{} could not be checked against this server.",
            report.unverified_models.len(),
            if report.unverified_models.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if parts.is_empty() {
        return "Every node and model this template needs is installed.".to_string();
    }
    parts.join(" ")
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {}", head.join(", "), last),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::nodes::{NodeClass, NodeInput};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn class(name: &str, inputs: Vec<NodeInput>) -> NodeClass {
        NodeClass {
            name: name.to_string(),
            display_name: None,
            category: None,
            output_node: false,
            inputs,
        }
    }

    fn socket(name: &str) -> NodeInput {
        NodeInput {
            name: name.to_string(),
            required: true,
            widget: WidgetSpec::Link {
                data_type: "IMAGE".to_string(),
            },
            tooltip: None,
        }
    }

    fn combo(name: &str, choices: &[&str]) -> NodeInput {
        NodeInput {
            name: name.to_string(),
            required: true,
            widget: WidgetSpec::Combo {
                choices: choices.iter().map(|c| c.to_string()).collect(),
                default: choices.first().map(|c| c.to_string()),
                truncated: false,
            },
            tooltip: None,
        }
    }

    fn text(name: &str) -> NodeInput {
        NodeInput {
            name: name.to_string(),
            required: true,
            widget: WidgetSpec::Text {
                multiline: false,
                default: None,
            },
            tooltip: None,
        }
    }

    fn catalog(classes: Vec<NodeClass>) -> NodeCatalog {
        NodeCatalog {
            classes: classes
                .into_iter()
                .map(|c| (c.name.clone(), c))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    /// A bundle shaped like the shipped upscaler, small enough to reason about.
    fn bundle() -> LineBundle {
        serde_json::from_value(json!({
            "format": "phos.line",
            "format_version": 1,
            "template": { "key": "restore-upscale", "version": 1, "confidence": "high" },
            "line": {
                "name": "Restore & Upscale",
                "stages": [{ "workflow": "upscale", "keep_output": true }]
            },
            "workflows": {
                "upscale": {
                    "name": "Restore & Upscale 4x",
                    "graph": {
                        "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
                        "2": { "class_type": "UpscaleModelLoader",
                               "inputs": { "model_name": "4x-UltraSharp.pth" } },
                        "3": { "class_type": "ImageUpscaleWithModel",
                               "inputs": { "upscale_model": ["2", 0], "image": ["1", 0] } },
                        "4": { "class_type": "SaveImage",
                               "inputs": { "images": ["3", 0], "filename_prefix": "phos/x" } }
                    }
                }
            }
        }))
        .unwrap()
    }

    fn full_catalog() -> NodeCatalog {
        catalog(vec![
            class("LoadImage", vec![combo("image", &["a.png"])]),
            class(
                "UpscaleModelLoader",
                vec![combo(
                    "model_name",
                    &["4x-UltraSharp.pth", "RealESRGAN.pth"],
                )],
            ),
            class(
                "ImageUpscaleWithModel",
                vec![socket("upscale_model"), socket("image")],
            ),
            class("SaveImage", vec![socket("images"), text("filename_prefix")]),
        ])
    }

    /// The `requirements` block in the file is documentation; what is checked
    /// comes off the graphs. A `.png` default is not a weights file.
    #[test]
    fn requirements_are_read_off_the_graphs_not_the_manifest() {
        let r = derive_requirements(bundle().graphs());
        assert_eq!(
            r.node_classes,
            vec![
                "ImageUpscaleWithModel",
                "LoadImage",
                "SaveImage",
                "UpscaleModelLoader"
            ]
        );
        assert_eq!(r.models.len(), 1, "only the .pth, not the .png");
        assert_eq!(r.models[0].name, "4x-UltraSharp.pth");
        assert_eq!(r.models[0].class_type, "UpscaleModelLoader");
        assert_eq!(r.models[0].field, "model_name");
    }

    #[test]
    fn everything_installed_reads_ready() {
        let r = assess(&bundle(), Some(&full_catalog()));
        assert_eq!(r.state, ReadinessState::Ready);
        assert_eq!(r.label, "READY");
        assert!(r.requirements.missing_nodes.is_empty());
        assert!(r.requirements.missing_models.is_empty());
    }

    #[test]
    fn a_missing_node_is_named() {
        let mut c = full_catalog();
        c.classes.remove("ImageUpscaleWithModel");
        let r = assess(&bundle(), Some(&c));
        assert_eq!(r.state, ReadinessState::Missing);
        assert_eq!(r.label, "MISSING NODE ImageUpscaleWithModel");
        assert_eq!(
            r.requirements.missing_nodes,
            vec!["ImageUpscaleWithModel".to_string()]
        );
    }

    #[test]
    fn several_missing_nodes_are_counted() {
        let mut c = full_catalog();
        c.classes.remove("ImageUpscaleWithModel");
        c.classes.remove("UpscaleModelLoader");
        let r = assess(&bundle(), Some(&c));
        assert_eq!(r.label, "MISSING 2 NODES");
        assert!(
            r.detail
                .contains("ImageUpscaleWithModel and UpscaleModelLoader"),
            "the detail lists them: {}",
            r.detail
        );
    }

    /// The picker's contents *are* the list of files on that box.
    #[test]
    fn a_model_the_picker_does_not_offer_is_named() {
        let c = catalog(vec![
            class("LoadImage", vec![combo("image", &["a.png"])]),
            class(
                "UpscaleModelLoader",
                vec![combo("model_name", &["RealESRGAN.pth"])],
            ),
            class(
                "ImageUpscaleWithModel",
                vec![socket("upscale_model"), socket("image")],
            ),
            class("SaveImage", vec![socket("images"), text("filename_prefix")]),
        ]);
        let r = assess(&bundle(), Some(&c));
        assert_eq!(r.state, ReadinessState::Missing);
        assert_eq!(r.label, "MISSING MODEL 4x-UltraSharp.pth");
        assert!(
            r.detail.contains("UpscaleModelLoader.model_name"),
            "{}",
            r.detail
        );
    }

    /// One complaint per problem: a model whose loader is not installed is not
    /// also reported as missing, because nothing could have looked for it.
    #[test]
    fn a_missing_loaders_model_is_not_reported_twice() {
        let mut c = full_catalog();
        c.classes.remove("UpscaleModelLoader");
        let r = assess(&bundle(), Some(&c));
        assert_eq!(r.label, "MISSING NODE UpscaleModelLoader");
        assert!(r.requirements.missing_models.is_empty());
    }

    /// A picker cut short is not evidence of absence.
    #[test]
    fn a_truncated_picker_leaves_a_model_unverified_rather_than_missing() {
        let mut c = full_catalog();
        c.classes.get_mut("UpscaleModelLoader").unwrap().inputs[0].widget = WidgetSpec::Combo {
            choices: vec!["something-else.pth".to_string()],
            default: None,
            truncated: true,
        };
        let r = assess(&bundle(), Some(&c));
        assert_eq!(r.state, ReadinessState::Ready);
        assert_eq!(r.requirements.unverified_models.len(), 1);
        assert!(r.detail.contains("could not be checked"), "{}", r.detail);
    }

    #[test]
    fn no_catalogue_is_unknown_rather_than_missing() {
        let r = assess(&bundle(), None);
        assert_eq!(r.state, ReadinessState::Unchecked);
        assert_eq!(r.label, "UNKNOWN");
        assert!(!r.requirements.checked);
        assert!(
            r.requirements.missing_nodes.is_empty(),
            "unknown never accuses"
        );
        assert!(r.requirements.missing_models.is_empty());
        assert!(r.input_mismatches.is_empty());
    }

    /// A server that answered with nothing said nothing.
    #[test]
    fn an_empty_catalogue_is_unknown_too() {
        let r = assess(&bundle(), Some(&NodeCatalog::default()));
        assert_eq!(r.state, ReadinessState::Unchecked);
    }

    /// The check that catches a graph written against a different release of a
    /// node pack — before it 400s at dispatch.
    #[test]
    fn a_field_this_servers_node_does_not_have_is_flagged() {
        let mut b = bundle();
        b.workflows.get_mut("upscale").unwrap().graph["2"]["inputs"]["upscale_method"] =
            json!("lanczos");
        let r = assess(&b, Some(&full_catalog()));
        assert_eq!(r.state, ReadinessState::Degraded);
        assert_eq!(r.label, "CHECK 1 INPUT");
        assert_eq!(r.input_mismatches[0].kind, "unexpected");
        assert!(r.input_mismatches[0]
            .message
            .contains("UpscaleModelLoader does not take"));
    }

    #[test]
    fn a_required_field_the_graph_never_sets_is_flagged() {
        let mut b = bundle();
        b.workflows.get_mut("upscale").unwrap().graph["4"]["inputs"]
            .as_object_mut()
            .unwrap()
            .remove("filename_prefix");
        let r = assess(&b, Some(&full_catalog()));
        assert_eq!(r.state, ReadinessState::Degraded);
        assert_eq!(r.input_mismatches[0].kind, "absent");
        assert_eq!(r.input_mismatches[0].field, "filename_prefix");
    }

    /// A field problem is not a refusal: everything is installed, so it may
    /// well run. Missing outranks it when both are true.
    #[test]
    fn missing_outranks_a_field_disagreement() {
        let mut b = bundle();
        b.workflows.get_mut("upscale").unwrap().graph["2"]["inputs"]["upscale_method"] =
            json!("lanczos");
        let mut c = full_catalog();
        c.classes.remove("SaveImage");
        let r = assess(&b, Some(&c));
        assert_eq!(r.state, ReadinessState::Missing);
        assert_eq!(r.label, "MISSING NODE SaveImage");
    }

    #[test]
    fn nodes_and_models_missing_at_once_says_both() {
        let c = catalog(vec![
            class("LoadImage", vec![combo("image", &["a.png"])]),
            class(
                "UpscaleModelLoader",
                vec![combo("model_name", &["other.pth"])],
            ),
        ]);
        let r = assess(&bundle(), Some(&c));
        assert_eq!(r.label, "MISSING 2 NODES, 1 MODEL");
    }
}
