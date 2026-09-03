//! Reading and rewriting the workflow graph.
//!
//! Two jobs: work out what a graph produces (so the poller knows whether to
//! expect a video, and which filenames are worth probing), and rewrite a copy
//! of it for one run — the uploaded image, the user's text overrides, their
//! typed parameters, and the pinned `filename_prefix` that makes the output
//! findable by name afterwards.
//!
//! What a graph *takes* is [`super::overrides`]' question, because answering it
//! well needs what ComfyUI says about its own node classes.

use super::loaders::BindingPlan;
use super::params::ParameterMap;
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

/// Substitute inputs into a workflow copy: point the loader nodes that the
/// binding selects at the uploaded filename, apply any text overrides and typed
/// parameters, and pin every saver's `filename_prefix` to `output_prefix`.
///
/// Pinning the prefix is what turns a lost history entry from a dead end into a
/// lookup: Phos knows the filename before the run starts, so it can ask `/view`
/// directly instead of depending on ComfyUI to tell it what it wrote.
///
/// Which loaders get the file is [`super::loaders::bind_targets`]' decision,
/// not this function's — writing it into *every* `LoadImage`, which is what
/// happened before, made a start-frame/end-frame workflow impossible to run.
/// This applies the plan; it does not second-guess it.
///
/// The four passes run in that order on purpose: a text override beats the
/// binding (a user who names a file means it), a typed parameter beats a text
/// override (it is the channel that knows what the field is), and the pinned
/// prefix beats everything, because Phos has to be able to find what it made.
///
/// It also drops the `_phos` block a seeded workflow carries. ComfyUI reads
/// every top-level key of a prompt as a node and refuses one with no
/// `class_type`, so the marker has to come off somewhere; this is the single
/// funnel every dispatch goes through, and taking it off here means the
/// provenance stays in the stored graph — where an upgrade and an export can
/// both still read it — rather than in a column that would not travel.
pub(crate) fn prepare_workflow(
    workflow: &Value,
    plan: &BindingPlan,
    text_overrides: &std::collections::HashMap<String, String>,
    parameters: &ParameterMap,
    output_prefix: Option<&str>,
) -> Value {
    let targets = &plan.targets;
    let mut wf = super::templates::marker::strip_marker(workflow);
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
                    // Then the typed ones: seeds, steps, cfg, checkpoints,
                    // switches — everything the text channel cannot carry
                    // without changing its type.
                    super::params::apply_to_node(node_id, obj, parameters);
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
    use crate::comfyui::loaders::{bind_targets, LoaderKind, SourceBinding, SourceRole};
    use serde_json::json;

    /// The everyday plan: one uploaded image, into whatever image loader the
    /// graph has, with nothing configured.
    fn plain_image(workflow: &Value, filename: &str) -> BindingPlan {
        plan_for(workflow, filename, LoaderKind::Image)
    }

    fn plan_for(workflow: &Value, filename: &str, kind: LoaderKind) -> BindingPlan {
        bind_targets(
            workflow,
            &SourceBinding {
                uploaded_filename: filename,
                kind,
                role: SourceRole::Start,
                role_overrides: &std::collections::HashMap::new(),
            },
        )
        .unwrap_or_default()
    }

    /// A run that set no typed parameters — every path that existed before FR4.
    fn no_params() -> ParameterMap {
        ParameterMap::new()
    }

    fn params(pairs: &[(&str, Value)]) -> ParameterMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
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
            &plain_image(&wf, "uploaded.png"),
            &std::collections::HashMap::new(),
            &no_params(),
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
            &plain_image(&wf, "uploaded.png"),
            &std::collections::HashMap::new(),
            &no_params(),
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

        let prepared = prepare_workflow(
            &wf,
            &plan_for(&wf, "phos_ab_cd_video.mp4", LoaderKind::Video),
            &std::collections::HashMap::new(),
            &no_params(),
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

        let prepared = prepare_workflow(
            &wf,
            &plain_image(&wf, "phos_upload.png"),
            &std::collections::HashMap::new(),
            &no_params(),
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

        let prepared = prepare_workflow(
            &wf,
            &plain_image(&wf, "phos_upload.png"),
            &overrides,
            &no_params(),
            None,
        );
        assert_eq!(
            prepared["4"]["inputs"]["image"].as_str(),
            Some("chosen.png")
        );
    }

    // === FR4 — typed parameters =============================================

    /// A realistic text-to-image graph, the same one the override detector is
    /// tested against.
    fn sampler_graph() -> Value {
        json!({
            "3": { "class_type": "KSampler", "inputs": {
                     "model": ["4", 0], "positive": ["6", 0],
                     "seed": 156680208700286i64, "steps": 20, "cfg": 8.0,
                     "sampler_name": "euler", "denoise": 1.0 } },
            "4": { "class_type": "CheckpointLoaderSimple",
                   "inputs": { "ckpt_name": "v1-5-pruned-emaonly.ckpt" } },
            "5": { "class_type": "EmptyLatentImage",
                   "inputs": { "width": 512, "height": 512, "batch_size": 1 } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["3", 0], "filename_prefix": "ComfyUI" } },
        })
    }

    #[test]
    fn a_run_carries_its_typed_parameters_into_the_graph_it_submits() {
        let prepared = prepare_workflow(
            &sampler_graph(),
            &plain_image(&sampler_graph(), "uploaded.png"),
            &std::collections::HashMap::new(),
            &params(&[
                ("3.seed", json!(4242)),
                ("3.steps", json!(28)),
                ("3.cfg", json!(6.5)),
                ("3.sampler_name", json!("dpmpp_2m")),
                ("4.ckpt_name", json!("sd_xl_base_1.0.safetensors")),
                ("5.width", json!(1024)),
            ]),
            Some("phos/task-1"),
        );
        assert_eq!(prepared["3"]["inputs"]["seed"], json!(4242));
        assert_eq!(prepared["3"]["inputs"]["steps"], json!(28));
        assert_eq!(prepared["3"]["inputs"]["cfg"], json!(6.5));
        assert_eq!(prepared["3"]["inputs"]["sampler_name"], json!("dpmpp_2m"));
        assert_eq!(
            prepared["4"]["inputs"]["ckpt_name"],
            json!("sd_xl_base_1.0.safetensors")
        );
        assert_eq!(prepared["5"]["inputs"]["width"], json!(1024));
        // Untouched fields keep exactly what the author set.
        assert_eq!(prepared["5"]["inputs"]["height"], json!(512));
        assert_eq!(prepared["3"]["inputs"]["denoise"], json!(1.0));
        // And the wiring survives.
        assert_eq!(prepared["3"]["inputs"]["model"], json!(["4", 0]));
    }

    #[test]
    fn a_parameter_cannot_take_the_output_prefix_away_from_phos() {
        // The console never offers `filename_prefix`, but a hand-written request
        // must not be able to make a finished run unfindable either.
        let prepared = prepare_workflow(
            &sampler_graph(),
            &plain_image(&sampler_graph(), "uploaded.png"),
            &[("9.filename_prefix".to_string(), "mine".to_string())]
                .into_iter()
                .collect(),
            &params(&[("9.filename_prefix", json!("also mine"))]),
            Some("phos/task-1"),
        );
        assert_eq!(
            prepared["9"]["inputs"]["filename_prefix"],
            json!("phos/task-1")
        );
    }

    #[test]
    fn a_typed_parameter_beats_a_text_override_on_the_same_field() {
        // They should not collide — the console sends text one way and numbers
        // the other — but if they do, the channel that knows the field's type
        // is the one to believe.
        let prepared = prepare_workflow(
            &sampler_graph(),
            &plain_image(&sampler_graph(), "uploaded.png"),
            &[("6.text".to_string(), "from the text box".to_string())]
                .into_iter()
                .collect(),
            &params(&[("6.text", json!("from the typed map"))]),
            None,
        );
        assert_eq!(prepared["6"]["inputs"]["text"], json!("from the typed map"));
    }

    #[test]
    fn a_run_with_no_parameters_prepares_exactly_the_graph_it_did_before() {
        // FR3's fallback, and every task queued before this column existed.
        let overrides: std::collections::HashMap<String, String> =
            [("6.text".to_string(), "a lighthouse at dusk".to_string())]
                .into_iter()
                .collect();
        let prepared = prepare_workflow(
            &sampler_graph(),
            &plain_image(&sampler_graph(), "uploaded.png"),
            &overrides,
            &no_params(),
            Some("phos/task-1"),
        );
        assert_eq!(
            prepared["6"]["inputs"]["text"],
            json!("a lighthouse at dusk")
        );
        assert_eq!(prepared["3"]["inputs"]["seed"], json!(156680208700286i64));
        assert_eq!(prepared["3"]["inputs"]["steps"], json!(20));
        assert_eq!(prepared["3"]["inputs"]["cfg"], json!(8.0));
    }

    #[test]
    fn a_fanned_out_sweep_submits_a_different_graph_per_task() {
        // What the queue actually ends up running: four rows, four seeds, one
        // graph each — end to end from the request shape to the submitted JSON.
        use crate::comfyui::params::expand;
        let base: ParameterMap = params(&[("3.seed", json!(1000)), ("3.steps", json!(20))]);
        let vary: crate::comfyui::params::VaryMap = serde_json::from_value(json!({
            "3.seed": { "count": 4, "mode": "increment" }
        }))
        .unwrap();

        let seeds: Vec<i64> = expand(&base, &vary)
            .unwrap()
            .iter()
            .map(|task| {
                let prepared = prepare_workflow(
                    &sampler_graph(),
                    &plain_image(&sampler_graph(), "uploaded.png"),
                    &std::collections::HashMap::new(),
                    task,
                    Some("phos/task-1"),
                );
                assert_eq!(prepared["3"]["inputs"]["steps"], json!(20));
                prepared["3"]["inputs"]["seed"].as_i64().unwrap()
            })
            .collect();
        assert_eq!(seeds, [1000, 1001, 1002, 1003]);
    }

    /// A seeded workflow's provenance block is not a node, and ComfyUI reads
    /// every top-level key of a prompt as one. It has to come off here, because
    /// this is the only thing that runs before `/prompt`.
    #[test]
    fn the_template_marker_never_reaches_comfyui() {
        let marked = crate::comfyui::templates::marker::with_marker(
            &sampler_graph(),
            "restore-upscale",
            "upscale",
            1,
        );
        assert!(marked.get("_phos").is_some(), "the stored graph keeps it");

        let prepared = prepare_workflow(
            &marked,
            &plain_image(&marked, "uploaded.png"),
            &std::collections::HashMap::new(),
            &no_params(),
            Some("phos/task-1"),
        );
        assert!(prepared.get("_phos").is_none(), "the submitted one does not");
        // And every key that is left is a node ComfyUI can execute.
        for (key, node) in prepared.as_object().unwrap() {
            assert!(
                node.get("class_type").is_some(),
                "{} would be refused as a node with no class_type",
                key
            );
        }
    }
}
