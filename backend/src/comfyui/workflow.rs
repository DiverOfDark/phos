//! Reading and rewriting the workflow graph.
//!
//! Two jobs: work out what a graph produces (so the poller knows whether to
//! expect a video, and which filenames are worth probing), and rewrite a copy
//! of it for one run — the uploaded image, the user's text overrides, and the
//! pinned `filename_prefix` that makes the output findable by name afterwards.
//!
//! What a graph *takes* is [`super::overrides`]' question, because answering it
//! well needs what ComfyUI says about its own node classes.

use super::loaders::{bind_targets, SourceBinding};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The subfolder every Phos-queued workflow is made to write into.
pub const OUTPUT_SUBFOLDER: &str = "phos";

/// The `filename_prefix` a task's output nodes are rewritten to. Knowing this
/// before the run starts is what lets Phos find a file when history is empty,
/// unhelpful, or gone with a ComfyUI restart.
pub(crate) fn output_prefix_for_task(task_id: &str) -> String {
    format!("{}/{}", OUTPUT_SUBFOLDER, task_id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutput {
    pub node_id: String,
    pub node_type: String,
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
pub(super) enum SaverKind {
    Image,
    Animated,
    Video,
    Audio,
}

pub(super) fn saver_kind(node_type: &str) -> SaverKind {
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
        for suffix in suffixes_for(saver_kind(&output.node_type)) {
            if !suffixes.contains(suffix) {
                suffixes.push(suffix);
            }
        }
    }
    if suffixes.is_empty() {
        // No recognisable saver — an unparseable graph, or one whose output
        // node we could not classify. Guess the common few rather than nothing.
        suffixes.extend_from_slice(&["_00001_.png", "_00001_.webp", "_00001.mp4"]);
    }
    suffixes
}

fn suffixes_for(kind: SaverKind) -> &'static [&'static str] {
    match kind {
        SaverKind::Image => &["_00001_.png", "_00001_.webp", "_00001_.jpg"],
        SaverKind::Animated => &["_00001_.webp", "_00001_.png", "_00001_.gif"],
        // VHS_VideoCombine drops the trailing underscore, and can be configured
        // to emit a gif or an animated webp instead of a video.
        SaverKind::Video => &["_00001.mp4", "_00001.webm", "_00001.gif", "_00001_.webp"],
        SaverKind::Audio => &["_00001_.flac", "_00001_.mp3"],
    }
}

/// Substitute inputs into a workflow copy: point the loader nodes that the
/// binding selects at the uploaded filename, apply any text overrides, and pin
/// every saver's `filename_prefix` to `output_prefix`.
///
/// Pinning the prefix is what turns a lost history entry from a dead end into a
/// lookup: Phos knows the filename before the run starts, so it can ask `/view`
/// directly instead of depending on ComfyUI to tell it what it wrote.
///
/// Which loaders get the file is [`bind_targets`]' decision, not this
/// function's — writing it into *every* `LoadImage`, which is what happened
/// before, made a start-frame/end-frame workflow impossible to run.
pub(crate) fn prepare_workflow(
    workflow: &Value,
    binding: &SourceBinding,
    text_overrides: &std::collections::HashMap<String, String>,
    output_prefix: Option<&str>,
) -> Value {
    let targets = bind_targets(workflow, binding);
    let mut wf = workflow.clone();
    if let Some(nodes) = wf.as_object_mut() {
        for (node_id, node) in nodes.iter_mut() {
            if let Some(bound) = targets.iter().find(|b| &b.node_id == node_id) {
                if let Some(inputs) = node.get_mut("inputs") {
                    inputs[bound.field.as_str()] = Value::String(bound.value.clone());
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
    use crate::comfyui::loaders::{LoaderKind, SourceRole};
    use serde_json::json;

    /// The everyday binding: one uploaded image, into whatever image loader the
    /// graph has, with nothing configured.
    fn plain_image<'a>(
        filename: &'a str,
        no_roles: &'a std::collections::HashMap<String, SourceRole>,
    ) -> SourceBinding<'a> {
        SourceBinding {
            uploaded_filename: filename,
            kind: LoaderKind::Image,
            role: SourceRole::Start,
            role_overrides: no_roles,
        }
    }

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

        // The video combiners drop the trailing underscore.
        let video = json!({
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff" } }
        });
        assert!(expected_output_suffixes(&video).contains(&"_00001.mp4"));
        assert!(!expected_output_suffixes(&video).contains(&"_00001_.jpg"));

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
        assert!(suffixes.contains(&"_00001.mp4"));
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
        let prefix = output_prefix_for_task("task-1234");
        assert_eq!(prefix, "phos/task-1234");

        let no_roles = std::collections::HashMap::new();
        let prepared = prepare_workflow(
            &wf,
            &plain_image("uploaded.png", &no_roles),
            &std::collections::HashMap::new(),
            Some(&prefix),
        );
        for node in ["9", "12", "20"] {
            assert_eq!(
                prepared[node]["inputs"]["filename_prefix"].as_str(),
                Some("phos/task-1234"),
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
    fn scope_a_leaves_a_linked_prefix_alone() {
        // A prefix wired from another node is a link, not a literal; rewriting it
        // would break the graph.
        let wf = json!({
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": ["8", 0] } }
        });
        let no_roles = std::collections::HashMap::new();
        let prepared = prepare_workflow(
            &wf,
            &plain_image("uploaded.png", &no_roles),
            &std::collections::HashMap::new(),
            Some("phos/task-1234"),
        );
        assert_eq!(prepared["9"]["inputs"]["filename_prefix"], json!(["8", 0]));
    }

    // === FR2 — video in, video out ==========================================

    #[test]
    fn a_video_workflow_gets_the_clip_written_into_its_video_loader() {
        let wf = json!({
            "1": { "class_type": "VHS_LoadVideo",
                   "inputs": { "video": "author_clip.mp4", "frame_load_cap": 0 } },
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff", "images": ["1", 0] } },
        });
        let no_roles = std::collections::HashMap::new();
        let prepared = prepare_workflow(
            &wf,
            &SourceBinding {
                uploaded_filename: "phos_ab_cd_video.mp4",
                kind: LoaderKind::Video,
                role: SourceRole::Start,
                role_overrides: &no_roles,
            },
            &std::collections::HashMap::new(),
            Some("phos/task-1"),
        );
        assert_eq!(
            prepared["1"]["inputs"]["video"].as_str(),
            Some("phos_ab_cd_video.mp4")
        );
        // Its other widgets are left exactly as the author set them.
        assert_eq!(prepared["1"]["inputs"]["frame_load_cap"].as_i64(), Some(0));
        assert_eq!(
            prepared["12"]["inputs"]["filename_prefix"].as_str(),
            Some("phos/task-1")
        );
    }

    #[test]
    fn the_end_frame_loader_keeps_the_authors_file_when_the_start_frame_is_bound() {
        // The bug this replaces: every LoadImage got the same filename, so a
        // two-frame workflow could only ever interpolate an image with itself.
        let wf = json!({
            "4": { "class_type": "LoadImage", "inputs": { "image": "author_start.png" },
                   "_meta": { "title": "Start Frame" } },
            "5": { "class_type": "LoadImage", "inputs": { "image": "author_end.png" },
                   "_meta": { "title": "End Frame" } },
        });
        let no_roles = std::collections::HashMap::new();
        let prepared = prepare_workflow(
            &wf,
            &plain_image("phos_upload.png", &no_roles),
            &std::collections::HashMap::new(),
            None,
        );
        assert_eq!(
            prepared["4"]["inputs"]["image"].as_str(),
            Some("phos_upload.png")
        );
        assert_eq!(
            prepared["5"]["inputs"]["image"].as_str(),
            Some("author_end.png"),
            "the end frame was overwritten with the start frame"
        );
    }

    #[test]
    fn a_text_override_still_beats_the_binding_on_the_same_field() {
        // The override loop runs after the binding, deliberately: a user who
        // names a file explicitly means it.
        let wf = json!({
            "4": { "class_type": "LoadImage", "inputs": { "image": "author.png" } }
        });
        let overrides: std::collections::HashMap<String, String> =
            [("4.image".to_string(), "chosen.png".to_string())]
                .into_iter()
                .collect();
        let no_roles = std::collections::HashMap::new();
        let prepared = prepare_workflow(
            &wf,
            &plain_image("phos_upload.png", &no_roles),
            &overrides,
            None,
        );
        assert_eq!(
            prepared["4"]["inputs"]["image"].as_str(),
            Some("chosen.png")
        );
    }
}
