//! Which node in a graph reads the file we uploaded.
//!
//! Two questions, both answered from the graph alone:
//!
//! * **What can this workflow load?** An image, a video, or both. It decides
//!   the default source mode — a graph with a video loader wants the clip, not
//!   a frame of it — and whether the graph is importable at all.
//! * **Which loader gets the upload?** A start-frame/end-frame workflow has two
//!   `LoadImage` nodes and they are not interchangeable, so writing the same
//!   filename into both (which is what Phos used to do) makes that workflow
//!   impossible to run. The nodes are told apart by the title their author
//!   typed, which the API-format JSON preserves under `_meta.title`.
//!
//! Both are matched by *shape* rather than against a fixed list of class names.
//! The same module learned that lesson once already: `detect_outputs` missed
//! core `SaveVideo` because it enumerated the savers it had heard of.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// What a loader node reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Image,
    Video,
}

/// Which slot of a multi-input workflow a source fills.
///
/// Three is enough for every graph worth running: the frame a clip starts on,
/// the frame it ends on, and a picture that is neither (a style or pose
/// reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceRole {
    Start,
    End,
    Reference,
}

impl SourceRole {
    /// The role a node's title implies, or `None` when it says nothing.
    ///
    /// Matched on whole words, not substrings: "blended" and "extended" both
    /// contain "end", and a graph called "Extend Clip" is not an end-frame
    /// slot.
    pub fn from_title(title: &str) -> Option<Self> {
        let mut role = None;
        for word in title
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let word = word.to_ascii_lowercase();
            let found = match word.as_str() {
                "end" | "last" | "final" | "ending" | "tail" => Some(SourceRole::End),
                "start" | "first" | "begin" | "beginning" | "initial" | "init" | "source"
                | "input" => Some(SourceRole::Start),
                "reference" | "ref" | "style" | "control" | "pose" | "depth" | "mask"
                | "ipadapter" => Some(SourceRole::Reference),
                _ => None,
            };
            // First word wins: "start frame reference" is a start frame.
            if role.is_none() {
                role = found;
            }
        }
        role
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "start" => Some(SourceRole::Start),
            "end" => Some(SourceRole::End),
            "reference" | "ref" => Some(SourceRole::Reference),
            _ => None,
        }
    }
}

/// A node that reads a file named by one of its own widget values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderNode {
    pub node_id: String,
    pub node_type: String,
    /// The input field holding the filename.
    pub field: String,
    pub kind: LoaderKind,
    /// The role its title implies, defaulting to [`SourceRole::Start`] so a
    /// workflow with one untitled loader needs no configuration at all.
    pub role: SourceRole,
    pub title: Option<String>,
    /// True when the field holds a *filesystem path* rather than a name in
    /// ComfyUI's input directory — `VHS_LoadVideoPath` is the one that matters.
    /// An upload only ever yields a name, so such a node is the last one worth
    /// binding, and gets the name qualified with the directory it landed in.
    pub path_style: bool,
}

/// Every loader node in the graph, ordered by node id.
pub fn detect_loaders(workflow: &Value) -> Vec<LoaderNode> {
    let mut loaders = Vec::new();
    let Some(nodes) = workflow.as_object() else {
        return loaders;
    };
    for (node_id, node) in nodes {
        let class_type = node
            .get("class_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some((kind, field)) = loader_shape(class_type, node) else {
            continue;
        };
        let title = node
            .get("_meta")
            .and_then(|m| m.get("title"))
            .and_then(|t| t.as_str())
            .map(str::to_string);
        let role = title
            .as_deref()
            .and_then(SourceRole::from_title)
            .unwrap_or(SourceRole::Start);
        let path_style = class_type.to_ascii_lowercase().contains("path")
            || field.ends_with("path")
            || field == "path";
        loaders.push(LoaderNode {
            node_id: node_id.clone(),
            node_type: class_type.to_string(),
            field,
            kind,
            role,
            title,
            path_style,
        });
    }
    loaders.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    loaders
}

/// Field names a loader might hold its filename in, most specific first.
const VIDEO_FIELDS: &[&str] = &["video", "video_path", "file", "path", "filename"];
const IMAGE_FIELDS: &[&str] = &["image", "image_path", "file", "path", "filename"];

/// Is this a loader, and if so what does it read and from which field?
///
/// `VHS_LoadVideo`, `VHS_LoadVideoPath` and core `LoadVideo` all match on the
/// `loadvideo` in their name; the field is then found by looking for one that
/// actually holds a string. A field wired from another node is a link, and
/// overwriting a link would break the graph, so such a node is not a loader we
/// can bind to.
fn loader_shape(class_type: &str, node: &Value) -> Option<(LoaderKind, String)> {
    let inputs = node.get("inputs")?.as_object()?;
    let literal = |name: &str| inputs.get(name).is_some_and(|v| v.is_string());
    let first_literal =
        |names: &[&str]| names.iter().find(|n| literal(n)).map(|n| (*n).to_string());

    let t = class_type.to_ascii_lowercase();
    if t.contains("loadvideo") || t.contains("videoload") {
        return first_literal(VIDEO_FIELDS).map(|f| (LoaderKind::Video, f));
    }
    if t.contains("loadimage") || t.contains("imageload") {
        return first_literal(IMAGE_FIELDS).map(|f| (LoaderKind::Image, f));
    }
    // Anything else named like a loader: classify by the field it carries, so a
    // custom node nobody here has heard of still works.
    if t.contains("load") {
        if literal("video") {
            return Some((LoaderKind::Video, "video".to_string()));
        }
        if literal("image") {
            return Some((LoaderKind::Image, "image".to_string()));
        }
    }
    None
}

/// Does this graph read a video file, as opposed to a still?
pub fn takes_video(workflow: &Value) -> bool {
    detect_loaders(workflow)
        .iter()
        .any(|l| l.kind == LoaderKind::Video)
}

/// Can Phos feed this graph anything at all? The import gate.
///
/// It used to be "does it have a `LoadImage`", which 400'd every video workflow
/// in existence — they load through `VHS_LoadVideo` or core `LoadVideo` — and
/// so made video→video unreachable no matter what the worker could do. What
/// actually matters is that *some* loader exists to receive the shot; a graph
/// with none has nowhere to put it and nothing to run.
pub fn importable(workflow: &Value) -> Result<(), &'static str> {
    if detect_loaders(workflow).is_empty() {
        return Err(
            "This workflow has no node Phos can feed the shot into. It needs a source \
             loader — LoadImage, VHS_LoadVideo, LoadVideo or similar — whose filename \
             is a plain value rather than wired in from another node.",
        );
    }
    Ok(())
}

/// What the run uploaded, and where it should land.
#[derive(Debug, Clone)]
pub(crate) struct SourceBinding<'a> {
    /// The name ComfyUI stored the upload under.
    pub uploaded_filename: &'a str,
    /// What the upload is, so it lands in a node that can read it.
    pub kind: LoaderKind,
    /// Which slot it fills.
    pub role: SourceRole,
    /// `node_id` → role, overriding what the title implied.
    pub role_overrides: &'a HashMap<String, SourceRole>,
}

/// One input field, and the value to write into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundInput {
    pub node_id: String,
    pub field: String,
    pub value: String,
}

/// Where ComfyUI's `/upload/image` puts what it is given, relative to the
/// server's own working directory. It is the only thing an upload tells us
/// about *where* the file is, which is why a path-style loader is a last
/// resort rather than a first choice.
const COMFY_INPUT_DIR: &str = "input";

/// Which loader inputs to write the uploaded filename into, and what to write.
///
/// Three tiers, narrowest first, so a graph this module failed to read is never
/// worse off than it was before roles existed:
///
/// 1. loaders of the right kind in the right slot — the point of the exercise;
/// 2. failing that, every loader of the right kind — a lone loader titled "end
///    frame" still has to run when nobody asked for an end frame;
/// 3. failing that, every loader at all — what Phos did before.
///
/// Within a tier, a loader that takes a name beats one that takes a path: an
/// upload yields a name, and the path a `VHS_LoadVideoPath` wants can only be
/// guessed at.
pub(crate) fn bind_targets(workflow: &Value, binding: &SourceBinding) -> Vec<BoundInput> {
    let loaders = detect_loaders(workflow);
    let effective = |l: &LoaderNode| {
        binding
            .role_overrides
            .get(&l.node_id)
            .copied()
            .unwrap_or(l.role)
    };

    let pick = |f: &dyn Fn(&LoaderNode) -> bool| -> Vec<&LoaderNode> {
        let matched: Vec<&LoaderNode> = loaders.iter().filter(|l| f(l)).collect();
        // Prefer the ones that take a plain name.
        let by_name: Vec<&LoaderNode> = matched.iter().copied().filter(|l| !l.path_style).collect();
        if by_name.is_empty() {
            matched
        } else {
            by_name
        }
    };

    let mut chosen = pick(&|l| l.kind == binding.kind && effective(l) == binding.role);
    if chosen.is_empty() {
        chosen = pick(&|l| l.kind == binding.kind);
    }
    if chosen.is_empty() {
        chosen = pick(&|_| true);
    }

    chosen
        .into_iter()
        .map(|l| BoundInput {
            node_id: l.node_id.clone(),
            field: l.field.clone(),
            value: if l.path_style {
                format!("{}/{}", COMFY_INPUT_DIR, binding.uploaded_filename)
            } else {
                binding.uploaded_filename.to_string()
            },
        })
        .collect()
}

/// Key under which a task's override map carries the slot the upload fills.
pub(crate) const TARGET_ROLE_KEY: &str = "role";
/// Prefix under which it carries `role:<node_id>` reassignments.
pub(crate) const ROLE_OVERRIDE_PREFIX: &str = "role:";

/// Read the role directives out of a task's override map.
///
/// They ride along in `text_overrides` rather than in columns of their own: the
/// map is already per-task, already free-form, and every other key in it is
/// `<node_id>.<field>`, which cannot collide with `role` or `role:<node_id>`.
pub(crate) fn role_directives(
    text_overrides: &HashMap<String, String>,
) -> (Option<SourceRole>, HashMap<String, SourceRole>) {
    let target = text_overrides
        .get(TARGET_ROLE_KEY)
        .and_then(|v| SourceRole::parse(v));
    let mut per_node = HashMap::new();
    for (key, value) in text_overrides {
        if let Some(node_id) = key.strip_prefix(ROLE_OVERRIDE_PREFIX) {
            if let Some(role) = SourceRole::parse(value) {
                per_node.insert(node_id.to_string(), role);
            }
        }
    }
    (target, per_node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_overrides() -> HashMap<String, SourceRole> {
        HashMap::new()
    }

    /// `(node_id, field, value)` triples, which is what the assertions are about.
    fn bound(targets: &[BoundInput]) -> Vec<(&str, &str, &str)> {
        targets
            .iter()
            .map(|b| (b.node_id.as_str(), b.field.as_str(), b.value.as_str()))
            .collect()
    }

    #[test]
    fn the_three_video_loaders_are_all_recognised() {
        let wf = json!({
            "1": { "class_type": "VHS_LoadVideo", "inputs": { "video": "clip.mp4", "frame_load_cap": 0 } },
            "2": { "class_type": "VHS_LoadVideoPath", "inputs": { "video": "/tmp/clip.mp4" } },
            "3": { "class_type": "LoadVideo", "inputs": { "file": "clip.mp4" } },
        });
        let loaders = detect_loaders(&wf);
        assert_eq!(loaders.len(), 3, "{:?}", loaders);
        assert!(loaders.iter().all(|l| l.kind == LoaderKind::Video));
        assert_eq!(loaders[0].field, "video");
        assert_eq!(loaders[1].field, "video");
        assert_eq!(loaders[2].field, "file");
        assert!(takes_video(&wf));
    }

    #[test]
    fn a_video_loader_nobody_here_has_heard_of_is_recognised_by_its_shape() {
        let wf = json!({
            "7": { "class_type": "SomeoneElsesVideoLoaderXL", "inputs": { "video": "clip.mp4" } },
        });
        assert_eq!(detect_loaders(&wf)[0].kind, LoaderKind::Video);
    }

    #[test]
    fn a_field_wired_from_another_node_is_not_a_loader_we_can_bind() {
        // Overwriting a link would break the graph.
        let wf = json!({
            "1": { "class_type": "VHS_LoadVideo", "inputs": { "video": ["9", 0] } },
        });
        assert!(detect_loaders(&wf).is_empty());
        assert!(!takes_video(&wf));
    }

    #[test]
    fn model_loaders_are_not_source_loaders() {
        let wf = json!({
            "4": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "sd_xl.safetensors" } },
            "5": { "class_type": "LoraLoader", "inputs": { "lora_name": "x.safetensors", "strength_model": 1.0 } },
            "6": { "class_type": "VAELoader", "inputs": { "vae_name": "vae.pt" } },
        });
        assert!(detect_loaders(&wf).is_empty(), "{:?}", detect_loaders(&wf));
    }

    #[test]
    fn a_title_names_the_slot_and_a_whole_word_is_needed_to_do_it() {
        assert_eq!(SourceRole::from_title("End Frame"), Some(SourceRole::End));
        assert_eq!(SourceRole::from_title("Last frame"), Some(SourceRole::End));
        assert_eq!(
            SourceRole::from_title("Start image"),
            Some(SourceRole::Start)
        );
        assert_eq!(
            SourceRole::from_title("style reference"),
            Some(SourceRole::Reference)
        );
        // Substring matching would have called all three of these end frames.
        assert_eq!(SourceRole::from_title("Extend the clip"), None);
        assert_eq!(
            SourceRole::from_title("Blended input"),
            Some(SourceRole::Start)
        );
        assert_eq!(SourceRole::from_title("Load Image"), None);
    }

    #[test]
    fn an_untitled_loader_is_a_start_frame_so_nothing_needs_configuring() {
        let wf = json!({ "4": { "class_type": "LoadImage", "inputs": { "image": "x.png" } } });
        assert_eq!(detect_loaders(&wf)[0].role, SourceRole::Start);
    }

    #[test]
    fn a_single_image_workflow_binds_with_zero_configuration() {
        let wf = json!({ "4": { "class_type": "LoadImage", "inputs": { "image": "old.png" } } });
        let targets = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.png",
                kind: LoaderKind::Image,
                role: SourceRole::Start,
                role_overrides: &no_overrides(),
            },
        );
        assert_eq!(bound(&targets), [("4", "image", "new.png")]);
    }

    #[test]
    fn start_and_end_frames_are_not_interchangeable() {
        let wf = json!({
            "4": { "class_type": "LoadImage", "inputs": { "image": "a.png" },
                   "_meta": { "title": "Start Frame" } },
            "5": { "class_type": "LoadImage", "inputs": { "image": "b.png" },
                   "_meta": { "title": "End Frame" } },
        });
        let start = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.png",
                kind: LoaderKind::Image,
                role: SourceRole::Start,
                role_overrides: &no_overrides(),
            },
        );
        assert_eq!(bound(&start), [("4", "image", "new.png")]);

        let end = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.png",
                kind: LoaderKind::Image,
                role: SourceRole::End,
                role_overrides: &no_overrides(),
            },
        );
        assert_eq!(bound(&end), [("5", "image", "new.png")]);
    }

    #[test]
    fn an_explicit_override_beats_the_title() {
        let wf = json!({
            "4": { "class_type": "LoadImage", "inputs": { "image": "a.png" },
                   "_meta": { "title": "Start Frame" } },
            "5": { "class_type": "LoadImage", "inputs": { "image": "b.png" },
                   "_meta": { "title": "End Frame" } },
        });
        // The author's titles are backwards for what this graph actually does;
        // say so, and the reassignment sticks.
        let overrides: HashMap<String, SourceRole> = [
            ("4".to_string(), SourceRole::End),
            ("5".to_string(), SourceRole::Start),
        ]
        .into_iter()
        .collect();
        let targets = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.png",
                kind: LoaderKind::Image,
                role: SourceRole::Start,
                role_overrides: &overrides,
            },
        );
        assert_eq!(bound(&targets), [("5", "image", "new.png")]);
    }

    #[test]
    fn an_override_only_moves_the_node_it_names() {
        // Reassigning 5 does not demote 4, which the title already put in the
        // start slot — both are start frames, and both get the file.
        let wf = json!({
            "4": { "class_type": "LoadImage", "inputs": { "image": "a.png" },
                   "_meta": { "title": "Start Frame" } },
            "5": { "class_type": "LoadImage", "inputs": { "image": "b.png" },
                   "_meta": { "title": "End Frame" } },
        });
        let overrides: HashMap<String, SourceRole> =
            [("5".to_string(), SourceRole::Start)].into_iter().collect();
        let targets = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.png",
                kind: LoaderKind::Image,
                role: SourceRole::Start,
                role_overrides: &overrides,
            },
        );
        assert_eq!(
            bound(&targets),
            [("4", "image", "new.png"), ("5", "image", "new.png")]
        );
    }

    #[test]
    fn a_lone_end_frame_loader_still_runs_when_nobody_asked_for_one() {
        // Tier 2: right kind, wrong slot, and it is the only one there is.
        let wf = json!({
            "4": { "class_type": "LoadImage", "inputs": { "image": "a.png" },
                   "_meta": { "title": "End Frame" } },
        });
        let targets = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.png",
                kind: LoaderKind::Image,
                role: SourceRole::Start,
                role_overrides: &no_overrides(),
            },
        );
        assert_eq!(bound(&targets), [("4", "image", "new.png")]);
    }

    #[test]
    fn a_video_upload_goes_to_the_video_loader_and_leaves_the_reference_image_alone() {
        let wf = json!({
            "1": { "class_type": "VHS_LoadVideo", "inputs": { "video": "clip.mp4" } },
            "4": { "class_type": "LoadImage", "inputs": { "image": "style.png" },
                   "_meta": { "title": "Style reference" } },
        });
        let targets = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.mp4",
                kind: LoaderKind::Video,
                role: SourceRole::Start,
                role_overrides: &no_overrides(),
            },
        );
        assert_eq!(bound(&targets), [("1", "video", "new.mp4")]);
    }

    // === The import gate ====================================================

    #[test]
    fn a_video_workflow_is_importable() {
        // The whole point: before this, every one of these 400'd.
        for class_type in ["VHS_LoadVideo", "VHS_LoadVideoPath", "LoadVideo"] {
            let wf = json!({
                "1": { "class_type": class_type, "inputs": { "video": "clip.mp4" } },
                "2": { "class_type": "SaveVideo", "inputs": { "filename_prefix": "out" } },
            });
            assert!(importable(&wf).is_ok(), "{} was rejected", class_type);
        }
    }

    #[test]
    fn an_image_workflow_is_still_importable() {
        let wf = json!({
            "4": { "class_type": "LoadImage", "inputs": { "image": "x.png" } },
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "out" } },
        });
        assert!(importable(&wf).is_ok());
    }

    #[test]
    fn a_workflow_with_no_source_input_is_still_rejected_and_says_why() {
        // Text-to-image: nothing here takes the shot, so there is nothing to run.
        let wf = json!({
            "4": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "x.safetensors" } },
            "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "a cat" } },
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "out" } },
        });
        let err = importable(&wf).unwrap_err();
        assert!(
            err.contains("LoadImage") && err.contains("VHS_LoadVideo"),
            "{}",
            err
        );

        // And so is something that is not a graph at all.
        assert!(importable(&json!([])).is_err());
        assert!(importable(&Value::Null).is_err());
    }

    /// `VHS_LoadVideoPath` resolves its widget as a filesystem path, not as a
    /// name in the input directory, so a bare filename in it names nothing.
    #[test]
    fn a_loader_that_takes_a_name_beats_one_that_takes_a_path() {
        let wf = json!({
            "1": { "class_type": "VHS_LoadVideoPath", "inputs": { "video": "/data/a.mp4" } },
            "2": { "class_type": "VHS_LoadVideo", "inputs": { "video": "b.mp4" } },
        });
        let targets = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.mp4",
                kind: LoaderKind::Video,
                role: SourceRole::Start,
                role_overrides: &no_overrides(),
            },
        );
        assert_eq!(bound(&targets), [("2", "video", "new.mp4")]);
    }

    #[test]
    fn a_path_loader_with_no_alternative_gets_the_upload_directory_too() {
        let wf = json!({
            "1": { "class_type": "VHS_LoadVideoPath", "inputs": { "video": "/data/a.mp4" } },
        });
        let targets = bind_targets(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.mp4",
                kind: LoaderKind::Video,
                role: SourceRole::Start,
                role_overrides: &no_overrides(),
            },
        );
        assert_eq!(bound(&targets), [("1", "video", "input/new.mp4")]);
    }

    #[test]
    fn role_directives_are_read_out_of_the_override_map() {
        let overrides: HashMap<String, String> = [
            ("role".to_string(), "end".to_string()),
            ("role:5".to_string(), "start".to_string()),
            ("role:6".to_string(), "nonsense".to_string()),
            ("4.text".to_string(), "a prompt".to_string()),
        ]
        .into_iter()
        .collect();
        let (target, per_node) = role_directives(&overrides);
        assert_eq!(target, Some(SourceRole::End));
        assert_eq!(per_node.get("5"), Some(&SourceRole::Start));
        assert_eq!(per_node.get("6"), None, "unparseable roles are ignored");
        assert_eq!(per_node.len(), 1, "a text override is not a role");
    }

    #[test]
    fn an_empty_override_map_says_nothing() {
        let (target, per_node) = role_directives(&HashMap::new());
        assert_eq!(target, None);
        assert!(per_node.is_empty());
    }
}
