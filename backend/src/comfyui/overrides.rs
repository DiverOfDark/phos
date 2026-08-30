//! Which of a graph's fields a person can override, and what kind of control
//! each one wants.
//!
//! Phos used to answer this by sniffing substrings out of class names — a class
//! *containing* `String` or `Text`, plus `LoadImage` and `CLIPTextEncode` by
//! name — which is why text was the only thing ever overridable. With
//! [`super::nodes`] holding what ComfyUI says about its own node classes, the
//! answer comes from the server instead: seeds, steps, cfg, frame counts,
//! resolutions and the LoRAs actually installed on that box.
//!
//! # The old answer is still the fallback
//!
//! ComfyUI may be unreachable, may be too old to have `/object_info`, or may
//! answer something nothing can be read out of. It may also simply not have the
//! custom node this graph uses. Each of those degrades to the heuristics, per
//! node — a workflow that could be run before this module existed is still
//! runnable when nothing can be learned about it.

use super::nodes::{NodeCatalog, NodeClass, WidgetSpec, MAX_STORED_CHOICES};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The field Phos pins for itself on every saver, so it can find the output by
/// name afterwards. Never offered as a control: an override would be discarded.
const PINNED_FIELD: &str = "filename_prefix";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub node_id: String,
    pub node_type: String,
    /// The name the workflow's author typed on the node (`_meta.title`), when
    /// there is one. "Positive Prompt" tells a person far more than
    /// "6 · CLIPTextEncode · text" does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_title: Option<String>,
    pub field_name: String,
    pub current_value: Value,
    /// What ComfyUI says this field is, when `/object_info` could be read.
    /// `None` means the heuristics found it and nothing is known beyond "it
    /// holds a string" — which is every input Phos surfaced before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WidgetSpec>,
}

/// Detect input fields the user can override, resolving real widget types from
/// the node catalogue where it has something to say.
///
/// `catalog` is `None` whenever nothing could be learned about the server, and
/// then this is the pre-`/object_info` behaviour, kept exactly.
pub fn detect_inputs(workflow: &Value, catalog: Option<&NodeCatalog>) -> Vec<WorkflowInput> {
    let mut inputs = Vec::new();
    let Some(nodes) = workflow.as_object() else {
        return inputs;
    };
    for (node_id, node) in nodes {
        let ctx = Node {
            id: node_id,
            class_type: node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            title: node
                .get("_meta")
                .and_then(|m| m.get("title"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            node,
        };
        match catalog.and_then(|c| c.get(ctx.class_type)) {
            Some(class) => typed_inputs(&ctx, class, &mut inputs),
            // A class this server has never heard of — an uninstalled custom
            // node, or no catalogue at all. Guess the way Phos always did.
            None => heuristic_inputs(&ctx, &mut inputs),
        }
    }
    inputs
}

/// One node of the graph, with the bits every path needs.
struct Node<'a> {
    id: &'a str,
    class_type: &'a str,
    title: Option<String>,
    node: &'a Value,
}

impl Node<'_> {
    fn graph_inputs(&self) -> Option<&serde_json::Map<String, Value>> {
        self.node.get("inputs").and_then(|v| v.as_object())
    }

    fn input(
        &self,
        field: &str,
        current_value: Value,
        widget: Option<WidgetSpec>,
    ) -> WorkflowInput {
        WorkflowInput {
            node_id: self.id.to_string(),
            node_type: self.class_type.to_string(),
            node_title: self.title.clone(),
            field_name: field.to_string(),
            current_value,
            widget,
        }
    }
}

/// A value ComfyUI wired in from another node, written `["6", 0]`. Widget
/// values are always scalars, so an array is always a link — and a link must
/// not be offered as something to type into.
fn is_link(value: &Value) -> bool {
    value.is_array()
}

fn typed_inputs(ctx: &Node, class: &NodeClass, out: &mut Vec<WorkflowInput>) {
    // The shot *is* the image input, so the console never asks for one. Naming
    // every file in ComfyUI's input directory as a choice would also be tens of
    // thousands of strings in a record stored beside the workflow.
    if ctx.class_type == "LoadImage" {
        heuristic_inputs(ctx, out);
        return;
    }

    let Some(graph_inputs) = ctx.graph_inputs() else {
        return;
    };

    let mut emitted: Vec<&str> = Vec::new();
    for declared in &class.inputs {
        if !declared.widget.is_widget() || declared.name == PINNED_FIELD {
            continue;
        }
        // Only fields the graph actually carries a literal for: an override is
        // applied by rewriting the value that is already there.
        let Some(current) = graph_inputs.get(&declared.name) else {
            continue;
        };
        if is_link(current) {
            continue;
        }
        emitted.push(&declared.name);
        out.push(ctx.input(
            &declared.name,
            current.clone(),
            Some(declared.widget.capped(MAX_STORED_CHOICES)),
        ));
    }

    // A graph saved against an older build of this node can carry a field the
    // server no longer declares. Losing it would take a prompt box away from a
    // workflow that has one, so the heuristics still get a look at anything the
    // catalogue says nothing about.
    let mut extra = Vec::new();
    heuristic_inputs(ctx, &mut extra);
    out.extend(extra.into_iter().filter(|i| {
        !emitted.contains(&i.field_name.as_str()) && class.input(&i.field_name).is_none()
    }));
}

/// What Phos guessed before it could ask. Kept verbatim, because this is the
/// answer any workflow gets when `/object_info` cannot be read.
fn heuristic_inputs(ctx: &Node, out: &mut Vec<WorkflowInput>) {
    let node_inputs = ctx.node.get("inputs");
    match ctx.class_type {
        "LoadImage" => {
            if let Some(val) = node_inputs.and_then(|i| i.get("image")) {
                out.push(ctx.input("image", val.clone(), None));
            }
        }
        "CLIPTextEncode" => {
            if let Some(val) = node_inputs.and_then(|i| i.get("text")) {
                // Only include if text is a string (not a link to another node)
                if val.is_string() {
                    out.push(ctx.input("text", val.clone(), None));
                }
            }
        }
        class_type => {
            // Check for String (Multiline) widget pattern
            if !class_type.contains("String") && !class_type.contains("Text") {
                return;
            }
            if let Some(obj) = node_inputs.and_then(|i| i.as_object()) {
                for (field, val) in obj {
                    if val.is_string() {
                        out.push(ctx.input(field, val.clone(), None));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::nodes::{fixtures::object_info, parse_object_info};
    use super::*;
    use serde_json::json;

    /// A realistic API-format graph: text-to-image with a LoRA, an image input,
    /// a titled prompt node, and one node from a pack this server does not have.
    fn graph() -> Value {
        json!({
            "3": { "class_type": "KSampler", "inputs": {
                     "model": ["10", 0], "positive": ["6", 0], "negative": ["7", 0],
                     "latent_image": ["5", 0],
                     "seed": 156680208700286i64, "steps": 20, "cfg": 8.0,
                     "sampler_name": "euler", "scheduler": "normal", "denoise": 1.0 } },
            "4": { "class_type": "CheckpointLoaderSimple",
                   "inputs": { "ckpt_name": "v1-5-pruned-emaonly.ckpt" } },
            "5": { "class_type": "EmptyLatentImage",
                   "inputs": { "width": 512, "height": 512, "batch_size": 1 } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] },
                   "_meta": { "title": "Positive Prompt" } },
            "7": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "blurry", "clip": ["4", 1] },
                   "_meta": { "title": "Negative Prompt" } },
            "8": { "class_type": "LoadImage", "inputs": { "image": "example.png" } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["3", 0], "filename_prefix": "ComfyUI" } },
            "10": { "class_type": "LoraLoaderModelOnly", "inputs": {
                      "model": ["4", 0], "lora_name": "add_detail.safetensors",
                      "strength_model": 0.8 } },
            "11": { "class_type": "SomeoneElsesStringNode",
                    "inputs": { "value": "custom pack, not installed here" } }
        })
    }

    fn keys(inputs: &[WorkflowInput]) -> Vec<String> {
        inputs
            .iter()
            .map(|i| format!("{}.{}", i.node_id, i.field_name))
            .collect()
    }

    fn find<'a>(inputs: &'a [WorkflowInput], key: &str) -> &'a WorkflowInput {
        inputs
            .iter()
            .find(|i| format!("{}.{}", i.node_id, i.field_name) == key)
            .unwrap_or_else(|| panic!("no input {} in {:?}", key, keys(inputs)))
    }

    // === Without a catalogue: exactly what Phos did before ===================

    #[test]
    fn with_no_catalogue_the_answer_is_the_one_phos_always_gave() {
        // The pre-change implementation on this graph: LoadImage.image, both
        // CLIPTextEncode.text, and the string field of the class whose *name*
        // contains "String". Nothing else — no seed, no steps, no cfg.
        let inputs = detect_inputs(&graph(), None);
        assert_eq!(keys(&inputs), ["11.value", "6.text", "7.text", "8.image"]);
        assert!(
            inputs.iter().all(|i| i.widget.is_none()),
            "the fallback claims to know nothing about types"
        );
        // Both spellings of "unavailable" agree.
        assert_eq!(detect_inputs(&graph(), None), inputs);
        // And the empty catalogue an unparseable answer produces agrees too.
        assert_eq!(
            detect_inputs(&graph(), Some(&NodeCatalog::default())),
            inputs
        );
    }

    #[test]
    fn a_workflow_that_could_be_run_before_is_still_importable_with_no_catalogue() {
        // The import gate is "has a LoadImage input"; the enhance path needs a
        // text override key per prompt box. Both survive with no server.
        let inputs = detect_inputs(&graph(), None);
        assert!(inputs.iter().any(|i| i.node_type == "LoadImage"));
        assert!(keys(&inputs).contains(&"6.text".to_string()));
        assert_eq!(find(&inputs, "6.text").current_value, json!("a photograph"));
    }

    #[test]
    fn a_graph_of_the_wrong_shape_is_no_inputs_rather_than_a_panic() {
        for wrong in [json!([]), json!("nope"), json!(null)] {
            assert!(detect_inputs(&wrong, None).is_empty());
            assert!(detect_inputs(&wrong, Some(&catalog())).is_empty());
        }
        // A node with no class_type and no inputs is simply skipped.
        assert!(detect_inputs(&json!({ "1": {} }), Some(&catalog())).is_empty());
    }

    // === With a catalogue: real types =======================================

    fn catalog() -> NodeCatalog {
        parse_object_info(&object_info())
    }

    #[test]
    fn the_catalogue_turns_a_graph_into_typed_controls() {
        let inputs = detect_inputs(&graph(), Some(&catalog()));
        let found = keys(&inputs);

        // Everything the old sniffing could never see.
        for expected in [
            "3.seed",
            "3.steps",
            "3.cfg",
            "3.sampler_name",
            "3.scheduler",
            "3.denoise",
            "4.ckpt_name",
            "5.width",
            "5.height",
            "5.batch_size",
            "10.lora_name",
            "10.strength_model",
        ] {
            assert!(
                found.contains(&expected.to_string()),
                "missing {}",
                expected
            );
        }

        assert_eq!(
            find(&inputs, "3.seed").widget,
            Some(WidgetSpec::Seed {
                default: Some(0),
                min: Some(0),
                max: Some(i64::MAX)
            })
        );
        assert_eq!(
            find(&inputs, "3.seed").current_value,
            json!(156680208700286i64)
        );
        assert_eq!(
            find(&inputs, "3.steps").widget,
            Some(WidgetSpec::Int {
                default: Some(20),
                min: Some(1),
                max: Some(10000),
                step: None
            })
        );
        assert_eq!(
            find(&inputs, "3.cfg").widget,
            Some(WidgetSpec::Float {
                default: Some(8.0),
                min: Some(0.0),
                max: Some(100.0),
                step: Some(0.1)
            })
        );
        assert_eq!(
            find(&inputs, "5.width").widget,
            Some(WidgetSpec::Int {
                default: Some(512),
                min: Some(16),
                max: Some(16384),
                step: Some(8)
            })
        );
        assert_eq!(
            find(&inputs, "6.text").widget,
            Some(WidgetSpec::Text {
                multiline: true,
                default: None
            })
        );

        // The enums carry what is installed on this box.
        let Some(WidgetSpec::Combo { choices, .. }) = &find(&inputs, "4.ckpt_name").widget else {
            panic!("ckpt_name should be an enum");
        };
        assert!(choices.contains(&"sd_xl_base_1.0.safetensors".to_string()));
        let Some(WidgetSpec::Combo { choices, .. }) = &find(&inputs, "10.lora_name").widget else {
            panic!("lora_name should be an enum");
        };
        assert!(choices.contains(&"film_grain.safetensors".to_string()));
        let Some(WidgetSpec::Combo { choices, .. }) = &find(&inputs, "3.sampler_name").widget
        else {
            panic!("sampler_name should be an enum");
        };
        assert!(choices.contains(&"dpmpp_2m".to_string()));
    }

    #[test]
    fn wired_sockets_and_the_pinned_prefix_are_not_offered() {
        let inputs = detect_inputs(&graph(), Some(&catalog()));
        let found = keys(&inputs);
        // `model`, `positive`, `clip`, `images` are links, not controls.
        for socket in ["3.model", "3.positive", "3.negative", "6.clip", "9.images"] {
            assert!(!found.contains(&socket.to_string()), "offered {}", socket);
        }
        // Phos rewrites `filename_prefix` for every run; an override there
        // would be silently discarded, so it is never shown.
        assert!(!found.contains(&"9.filename_prefix".to_string()));
    }

    #[test]
    fn load_image_stays_the_one_field_it_always_was() {
        let inputs = detect_inputs(&graph(), Some(&catalog()));
        let load: Vec<&WorkflowInput> = inputs
            .iter()
            .filter(|i| i.node_type == "LoadImage")
            .collect();
        // The catalogue would list every file on the server as a choice, which
        // is both useless (the shot is the input) and enormous.
        assert_eq!(load.len(), 1);
        assert_eq!(load[0].field_name, "image");
        assert_eq!(load[0].widget, None);
    }

    #[test]
    fn a_class_the_server_does_not_have_still_falls_back_to_the_heuristics() {
        let inputs = detect_inputs(&graph(), Some(&catalog()));
        // `SomeoneElsesStringNode` is in no catalogue; its string field is
        // found the old way rather than lost.
        assert_eq!(find(&inputs, "11.value").widget, None);
        assert_eq!(
            find(&inputs, "11.value").current_value,
            json!("custom pack, not installed here")
        );
    }

    #[test]
    fn a_field_the_server_no_longer_declares_is_not_lost() {
        // A graph saved against an older build of a node: `value` is still
        // declared, `suffix` is not. Where the heuristics would have found it
        // before, they still do — knowing the class must not cost the user a
        // box the old sniffing gave them.
        let doc = json!({
            "StringConcatenate": { "input": { "required": {
                "value": ["STRING", { "multiline": true }]
            } } }
        });
        let stale = json!({
            "1": { "class_type": "StringConcatenate",
                   "inputs": { "value": "current", "suffix": ", older build" } }
        });
        let inputs = detect_inputs(&stale, Some(&parse_object_info(&doc)));
        assert_eq!(
            find(&inputs, "1.value").widget,
            Some(WidgetSpec::Text {
                multiline: true,
                default: None
            })
        );
        // Found by the fallback, typed as nothing, and not listed twice.
        assert_eq!(find(&inputs, "1.suffix").widget, None);
        assert_eq!(keys(&inputs), ["1.value", "1.suffix"]);
    }

    // === _meta.title ========================================================

    #[test]
    fn the_title_the_author_typed_on_the_node_survives_import() {
        for catalog in [None, Some(catalog())] {
            let inputs = detect_inputs(&graph(), catalog.as_ref());
            assert_eq!(
                find(&inputs, "6.text").node_title.as_deref(),
                Some("Positive Prompt")
            );
            assert_eq!(
                find(&inputs, "7.text").node_title.as_deref(),
                Some("Negative Prompt")
            );
            // A node without one says nothing rather than inventing a name.
            assert_eq!(find(&inputs, "8.image").node_title, None);
        }
    }

    #[test]
    fn the_stored_shape_stays_compatible_with_what_is_already_in_the_database() {
        let inputs = detect_inputs(&graph(), None);
        let json = serde_json::to_value(&inputs[0]).unwrap();
        // The four fields every stored workflow already has.
        assert!(json.get("node_id").is_some());
        assert!(json.get("node_type").is_some());
        assert!(json.get("field_name").is_some());
        assert!(json.get("current_value").is_some());
        // The new ones are omitted when there is nothing to say, so a fallback
        // import reads back exactly as it used to.
        assert!(json.get("widget").is_none());
        assert!(json.get("node_title").is_none());

        // And a record written before this change still parses.
        let old: WorkflowInput = serde_json::from_value(json!({
            "node_id": "6", "node_type": "CLIPTextEncode",
            "field_name": "text", "current_value": "a photograph"
        }))
        .unwrap();
        assert_eq!(old.widget, None);
        assert_eq!(old.node_title, None);
    }

    #[test]
    fn an_enormous_enum_is_cut_down_before_it_is_stored_beside_the_workflow() {
        let many: Vec<String> = (0..2000)
            .map(|i| format!("lora_{}.safetensors", i))
            .collect();
        let doc = json!({
            "LoraLoaderModelOnly": { "input": { "required": {
                "lora_name": [many, {}]
            } } }
        });
        let wf = json!({
            "1": { "class_type": "LoraLoaderModelOnly",
                   "inputs": { "lora_name": "lora_7.safetensors" } }
        });
        let inputs = detect_inputs(&wf, Some(&parse_object_info(&doc)));
        let Some(WidgetSpec::Combo {
            choices, truncated, ..
        }) = &find(&inputs, "1.lora_name").widget
        else {
            panic!("lora_name should be an enum");
        };
        assert_eq!(choices.len(), MAX_STORED_CHOICES);
        assert!(truncated, "the console must know to ask for the full list");
    }
}
