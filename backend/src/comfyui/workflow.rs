//! Reading and rewriting the workflow graph.
//!
//! Two jobs: work out what a graph takes and produces (so the UI can offer the
//! overridable inputs, and so the poller knows whether to expect a video), and
//! rewrite a copy of it for one run — the uploaded image, the user's text
//! overrides, and the pinned `filename_prefix` that makes the output findable
//! by name afterwards.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The subfolder every Phos-queued workflow is made to write into.
pub const OUTPUT_SUBFOLDER: &str = "phos";

/// The `filename_prefix` a task's output nodes are rewritten to. Knowing this
/// before the run starts is what lets Phos find a file when history is empty,
/// unhelpful, or gone with a ComfyUI restart.
///
/// `attempt` makes the prefix unique per dispatch. ComfyUI never overwrites:
/// a second run with the same prefix keeps `_00001` and writes `_00002`, so a
/// by-name probe that only knows the prefix would find the *first* attempt's
/// file and call the retry done. A fresh prefix per attempt means the file we
/// look for can only have been written by the run we are following.
pub(crate) fn output_prefix_for_task(task_id: &str, attempt: &str) -> String {
    format!("{}/{}-{}", OUTPUT_SUBFOLDER, task_id, attempt)
}

/// A short token that differs on every dispatch of the same task.
pub(crate) fn fresh_attempt_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub node_id: String,
    pub node_type: String,
    pub field_name: String,
    pub current_value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutput {
    pub node_id: String,
    pub node_type: String,
}

/// Detect input nodes that the user can override.
pub fn detect_inputs(workflow: &Value) -> Vec<WorkflowInput> {
    let mut inputs = Vec::new();
    if let Some(nodes) = workflow.as_object() {
        for (node_id, node) in nodes {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let node_inputs = node.get("inputs");

            match class_type {
                "LoadImage" => {
                    if let Some(inp) = node_inputs {
                        if let Some(val) = inp.get("image") {
                            inputs.push(WorkflowInput {
                                node_id: node_id.clone(),
                                node_type: class_type.to_string(),
                                field_name: "image".to_string(),
                                current_value: val.clone(),
                            });
                        }
                    }
                }
                "CLIPTextEncode" => {
                    if let Some(inp) = node_inputs {
                        if let Some(val) = inp.get("text") {
                            // Only include if text is a string (not a link to another node)
                            if val.is_string() {
                                inputs.push(WorkflowInput {
                                    node_id: node_id.clone(),
                                    node_type: class_type.to_string(),
                                    field_name: "text".to_string(),
                                    current_value: val.clone(),
                                });
                            }
                        }
                    }
                }
                _ => {
                    // Check for String (Multiline) widget pattern
                    if let Some(inp) = node_inputs {
                        if let Some(obj) = inp.as_object() {
                            for (field, val) in obj {
                                if val.is_string()
                                    && (class_type.contains("String")
                                        || class_type.contains("Text"))
                                {
                                    inputs.push(WorkflowInput {
                                        node_id: node_id.clone(),
                                        node_type: class_type.to_string(),
                                        field_name: field.clone(),
                                        current_value: val.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    inputs
}

/// Detect output nodes (SaveImage, SaveVideo, VHS_VideoCombine, etc.).
///
/// Recognised by shape rather than by a fixed list: anything named `Save*` or
/// `Preview*`, plus anything carrying a `filename_prefix` input, which is how
/// every saver in the ecosystem is told where to write. A hardcoded list missed
/// core `SaveVideo` and every custom saver.
pub fn detect_outputs(workflow: &Value) -> Vec<WorkflowOutput> {
    let mut outputs = Vec::new();
    if let Some(nodes) = workflow.as_object() {
        for (node_id, node) in nodes {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if is_output_node(class_type, node) {
                outputs.push(WorkflowOutput {
                    node_id: node_id.clone(),
                    node_type: class_type.to_string(),
                });
            }
        }
    }
    outputs.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    outputs
}

fn is_output_node(class_type: &str, node: &Value) -> bool {
    if class_type.starts_with("Save")
        || class_type.starts_with("Preview")
        || class_type.contains("VideoCombine")
    {
        return true;
    }
    node.get("inputs")
        .and_then(|i| i.get("filename_prefix"))
        .is_some_and(|v| v.is_string())
}

/// What a saver node writes. The distinction earns its keep twice: everything
/// but a plain image is muxed or encoded after the graph finishes (so the
/// settle budget is long), and the counter suffix differs — the video
/// combiners append `_00001`, everything else `_00001_`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaverKind {
    Image,
    Animated,
    Video,
    Audio,
}

fn saver_kind(node_type: &str) -> SaverKind {
    let t = node_type.to_ascii_lowercase();
    if t.contains("video") || t.contains("webm") {
        SaverKind::Video
    } else if t.contains("animated") {
        SaverKind::Animated
    } else if t.contains("audio") {
        SaverKind::Audio
    } else {
        SaverKind::Image
    }
}

/// Does this workflow save something that is muxed or encoded after the graph
/// finishes? Those are the runs worth waiting a quarter of an hour for.
pub(crate) fn has_slow_output(workflow: &Value) -> bool {
    detect_outputs(workflow)
        .iter()
        .any(|o| saver_kind(&o.node_type) != SaverKind::Image)
}

/// Filename suffixes this workflow's savers could produce, most likely first.
///
/// The graph already says which handful of names are possible, so probing
/// `/view` for the full cross-product of counters and extensions was sixteen
/// requests per re-check, at least twelve of which could never hit. This is
/// only a backstop in any case — the primary path reads the filenames out of
/// history, whatever key they are published under.
pub(crate) fn expected_output_suffixes(workflow: &Value) -> Vec<&'static str> {
    let mut suffixes: Vec<&'static str> = Vec::new();
    for output in detect_outputs(workflow) {
        for suffix in suffixes_for(saver_kind(&output.node_type), &output.node_type) {
            if !suffixes.contains(suffix) {
                suffixes.push(suffix);
            }
        }
    }
    if suffixes.is_empty() {
        // No recognisable saver — an unparseable graph, or one whose output
        // node we could not classify. Guess the common few rather than nothing.
        suffixes.extend_from_slice(&["_00001_.png", "_00001_.webp", "_00001_.mp4"]);
    }
    suffixes
}

/// The counter spelling is per node family, not per media type. Every core
/// saver — `SaveImage`, `SaveVideo`, `SaveAudio`, `SaveAnimatedWEBP` — goes
/// through `get_save_image_path` and writes `<prefix>_00001_.<ext>` with a
/// trailing underscore. `VHS_VideoCombine` formats its own name and drops it:
/// `<prefix>_00001.mp4`. Getting this wrong is silent — the by-name probe just
/// never hits — which is why `tests/comfyui_contract_test.rs` checks it
/// against a real server.
fn suffixes_for(kind: SaverKind, node_type: &str) -> &'static [&'static str] {
    match kind {
        SaverKind::Image => &["_00001_.png", "_00001_.webp", "_00001_.jpg"],
        SaverKind::Animated => &["_00001_.webp", "_00001_.png", "_00001_.gif"],
        SaverKind::Video if node_type.contains("VideoCombine") => {
            // Can also be configured to emit a gif or an animated webp.
            &["_00001.mp4", "_00001.webm", "_00001.gif", "_00001_.webp"]
        }
        SaverKind::Video => &["_00001_.mp4", "_00001_.webm", "_00001_.mkv"],
        SaverKind::Audio => &["_00001_.flac", "_00001_.mp3"],
    }
}

/// Substitute inputs into a workflow copy: set LoadImage.image to the uploaded
/// filename, apply any text overrides, and pin every saver's `filename_prefix`
/// to `output_prefix`.
///
/// Pinning the prefix is what turns a lost history entry from a dead end into a
/// lookup: Phos knows the filename before the run starts, so it can ask `/view`
/// directly instead of depending on ComfyUI to tell it what it wrote.
pub(crate) fn prepare_workflow(
    workflow: &Value,
    uploaded_filename: &str,
    text_overrides: &std::collections::HashMap<String, String>,
    output_prefix: Option<&str>,
) -> Value {
    let mut wf = workflow.clone();
    if let Some(nodes) = wf.as_object_mut() {
        for (node_id, node) in nodes.iter_mut() {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if class_type == "LoadImage" {
                if let Some(inputs) = node.get_mut("inputs") {
                    inputs["image"] = Value::String(uploaded_filename.to_string());
                }
            }

            // Apply text overrides keyed by "node_id.field_name"
            if let Some(inputs) = node.get_mut("inputs") {
                if let Some(obj) = inputs.as_object_mut() {
                    for (field, val) in obj.iter_mut() {
                        let key = format!("{}.{}", node_id, field);
                        if let Some(override_val) = text_overrides.get(&key) {
                            if val.is_string() {
                                *val = Value::String(override_val.clone());
                            }
                        }
                    }
                }
            }

            // Every saver in the ecosystem takes `filename_prefix`; overwrite it
            // wherever it is a literal. A prefix wired from another node is left
            // alone — rewriting it would break the link.
            if let Some(prefix) = output_prefix {
                if let Some(existing) = node
                    .get_mut("inputs")
                    .and_then(|i| i.get_mut("filename_prefix"))
                {
                    if existing.is_string() {
                        *existing = Value::String(prefix.to_string());
                    }
                }
            }
        }
    }
    wf
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defect_2_core_save_video_is_an_output_node() {
        let wf = json!({
            "12": { "class_type": "SaveVideo", "inputs": { "filename_prefix": "video/ComfyUI" } }
        });
        assert_eq!(detect_outputs(&wf).len(), 1);
        assert!(has_slow_output(&wf));
    }

    #[test]
    fn defect_2_custom_saver_is_an_output_node() {
        // Recognised by its `filename_prefix` input, not by a name we knew.
        let wf = json!({
            "20": { "class_type": "ImageWriterXL",
                    "inputs": { "filename_prefix": "out", "images": ["19", 0] } }
        });
        assert_eq!(detect_outputs(&wf).len(), 1);
    }

    // === Scope A — deterministic filenames ===================================
    #[test]
    fn the_graph_says_which_filenames_are_worth_probing() {
        let images = json!({
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "ComfyUI" } }
        });
        // An image workflow can only have written an image.
        let suffixes = expected_output_suffixes(&images);
        assert_eq!(suffixes, ["_00001_.png", "_00001_.webp", "_00001_.jpg"]);
        assert!(!suffixes.iter().any(|s| s.ends_with(".mp4")));

        // VHS_VideoCombine drops the trailing underscore.
        let video = json!({
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff" } }
        });
        assert!(expected_output_suffixes(&video).contains(&"_00001.mp4"));
        assert!(!expected_output_suffixes(&video).contains(&"_00001_.jpg"));

        // Core SaveVideo keeps it, like every other core saver. Verified
        // against a real ComfyUI by `a_lost_video_prompt_is_recovered_by_name`
        // in tests/comfyui_contract_test.rs — the probe silently missed the
        // file until this was told apart from VHS.
        let core_video = json!({
            "12": { "class_type": "SaveVideo",
                    "inputs": { "filename_prefix": "video/ComfyUI", "format": "mp4" } }
        });
        let suffixes = expected_output_suffixes(&core_video);
        assert_eq!(suffixes[0], "_00001_.mp4", "{:?}", suffixes);
        assert!(!suffixes.contains(&"_00001.mp4"), "{:?}", suffixes);

        // Core SaveVideo and SaveAudio are classified by name too.
        let audio = json!({
            "3": { "class_type": "SaveAudio", "inputs": { "filename_prefix": "audio/x" } }
        });
        assert_eq!(
            expected_output_suffixes(&audio),
            ["_00001_.flac", "_00001_.mp3"]
        );
    }

    #[test]
    fn a_graph_with_two_savers_probes_for_both_without_repeating() {
        let mixed = json!({
            "9":  { "class_type": "SaveImage", "inputs": { "filename_prefix": "a" } },
            "10": { "class_type": "PreviewImage", "inputs": {} },
            "12": { "class_type": "SaveVideo", "inputs": { "filename_prefix": "b" } },
        });
        let suffixes = expected_output_suffixes(&mixed);
        assert!(suffixes.contains(&"_00001_.png"));
        assert!(suffixes.contains(&"_00001_.mp4"));
        let mut deduped = suffixes.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            suffixes.len(),
            "{:?} repeats itself",
            suffixes
        );
    }

    #[test]
    fn a_graph_with_no_recognisable_saver_still_guesses_something() {
        assert!(!expected_output_suffixes(&Value::Null).is_empty());
    }

    #[test]
    fn scope_a_pins_every_savers_filename_prefix() {
        let wf = json!({
            "9":  { "class_type": "SaveImage", "inputs": { "filename_prefix": "ComfyUI" } },
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff", "frame_rate": 8 } },
            "20": { "class_type": "MysterySaver", "inputs": { "filename_prefix": "whatever" } },
            "4":  { "class_type": "LoadImage", "inputs": { "image": "old.png" } },
        });
        let prefix = output_prefix_for_task("task-1234", "a1b2c3d4");
        assert_eq!(prefix, "phos/task-1234-a1b2c3d4");

        let prepared = prepare_workflow(
            &wf,
            "uploaded.png",
            &std::collections::HashMap::new(),
            Some(&prefix),
        );
        for node in ["9", "12", "20"] {
            assert_eq!(
                prepared[node]["inputs"]["filename_prefix"].as_str(),
                Some("phos/task-1234-a1b2c3d4"),
                "node {} kept its own prefix",
                node
            );
        }
        // The rest of prepare_workflow still does its job.
        assert_eq!(
            prepared["4"]["inputs"]["image"].as_str(),
            Some("uploaded.png")
        );
        assert_eq!(prepared["12"]["inputs"]["frame_rate"].as_i64(), Some(8));
    }

    #[test]
    fn each_dispatch_of_a_task_writes_under_its_own_prefix() {
        // Same task, two attempts (a manual retry, or a /prompt that timed out
        // after ComfyUI had accepted it): ComfyUI keeps the first file and
        // advances the counter for the second, so probing `<prefix>_00001` under
        // a shared prefix would import the stale result. Distinct prefixes mean
        // the counter is always `_00001` and always ours.
        let first = output_prefix_for_task("task-1234", &fresh_attempt_id());
        let second = output_prefix_for_task("task-1234", &fresh_attempt_id());
        assert_ne!(first, second);
        assert!(first.starts_with("phos/task-1234-"));
        assert!(second.starts_with("phos/task-1234-"));
    }

    #[test]
    fn scope_a_leaves_a_linked_prefix_alone() {
        // A prefix wired from another node is a link, not a literal; rewriting it
        // would break the graph.
        let wf = json!({
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": ["8", 0] } }
        });
        let prepared = prepare_workflow(
            &wf,
            "uploaded.png",
            &std::collections::HashMap::new(),
            Some("phos/task-1234"),
        );
        assert_eq!(prepared["9"]["inputs"]["filename_prefix"], json!(["8", 0]));
    }
}
