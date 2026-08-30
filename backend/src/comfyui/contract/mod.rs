//! What a workflow takes and what it hands on — the stage contract.
//!
//! A workflow on its own is a graph. A *stage* is a graph Phos knows the shape
//! of: that it wants an image and gives back a clip, that its second loader is
//! the end frame rather than another start frame, that node 6 is where a prompt
//! goes and node 3 is where the seed lives. Everything that chains stages into a
//! line — validating the chain at design time, filtering the picker to what
//! fits, binding a description into the next stage's prompt — is that one fact
//! asked in different ways.
//!
//! # Derived, then corrected
//!
//! Derivation is a pure function of the graph and [`NodeCatalog`], and it is
//! made of heuristics: titles the author typed, class names, and what ComfyUI
//! says its nodes take. On an ordinary graph it is right. On an unusual one it
//! will be wrong, and a wrong contract must be a two-click fix rather than a
//! re-import.
//!
//! So a stored contract is *derived output plus the corrections a person made*,
//! and [`StageContract::derive_with`] re-runs the derivation with those
//! corrections folded in. That ordering matters: a correction is not a patch
//! applied to a finished contract, it takes part in building one. Saying "node 7
//! is the negative prompt" has to be able to add a slot the heuristics never
//! found, not just rename one they did.
//!
//! # Read by things not built yet
//!
//! Some of what a contract offers has no caller in this commit: [`Accepts::admits`]
//! is how a chain of stages is validated, [`StageContract::slot`] is how a
//! prompt is bound into one. They carry `#[allow(dead_code)]` rather than being
//! left out and rebuilt — the shape is the deliverable, and `main.rs` compiles
//! this tree a second time as a private module where "nothing calls it yet"
//! reads as "nothing will".
//!
//! # `text` is a real type
//!
//! Nothing in ComfyUI's core produces text, and no bundled workflow does yet.
//! It is modelled anyway, because a describe stage — Qwen reading a photograph,
//! its sentence bound into the next stage's prompt slot — produces no file at
//! all. A type system that learns about text later has to be reshaped later.

mod derive;

use super::loaders::{detect_loaders, LoaderKind, SourceRole};
use super::nodes::{NodeCatalog, WidgetSpec};
use super::overrides::detect_inputs;
use derive::{derive_params, derive_produces, derive_slots, node_order, unknown_classes};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Bumped when a field changes meaning. Adding one does not need it: every
/// field is `#[serde(default)]`, so an older stored contract still reads.
pub const CONTRACT_VERSION: u32 = 1;

// ===== The type system ======================================================

/// What flows from one stage to the next.
///
/// Three, and `text` is one of them: a text-producing stage writes no file at
/// all, so it is not a degenerate image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Image,
    Video,
    Text,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Image => "image",
            MediaType::Video => "video",
            MediaType::Text => "text",
        }
    }
}

/// What a stage consumes.
///
/// Four values rather than three, because "nothing" is a real answer: a
/// text-to-image graph begins a line instead of continuing one, and a line that
/// starts with it needs no upstream at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Accepts {
    Image,
    Video,
    Text,
    None,
}

#[allow(dead_code)] // Read by the line editor and its validation, not yet built.
impl Accepts {
    /// Can a stage producing `upstream` feed this one? The whole point of the
    /// exercise, and the reason it lives here rather than in the line editor:
    /// FR5's validation and FR5b's picker must agree, so they ask the same
    /// function.
    pub fn admits(self, upstream: MediaType) -> bool {
        match self {
            Accepts::Image => upstream == MediaType::Image,
            Accepts::Video => upstream == MediaType::Video,
            Accepts::Text => upstream == MediaType::Text,
            // A stage that consumes nothing cannot be handed anything.
            Accepts::None => false,
        }
    }

    /// A stage that takes nothing can only be the first in a line.
    pub fn starts_a_line(self) -> bool {
        self == Accepts::None
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Accepts::Image => "image",
            Accepts::Video => "video",
            Accepts::Text => "text",
            Accepts::None => "none",
        }
    }
}

impl From<MediaType> for Accepts {
    fn from(t: MediaType) -> Self {
        match t {
            MediaType::Image => Accepts::Image,
            MediaType::Video => Accepts::Video,
            MediaType::Text => Accepts::Text,
        }
    }
}

/// A knob a line can set on any stage without knowing which node holds it.
///
/// A closed vocabulary on purpose. The exhaustive per-node control surface is
/// [`detect_inputs`]' job; a contract names only the handful of settings that
/// mean the same thing on every stage, so a line can say "seed 42" once.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ParamName {
    Seed,
    Steps,
    Cfg,
    Denoise,
    Frames,
    Fps,
    Width,
    Height,
}

#[allow(dead_code)] // `parse` and `as_str` are how a line names a setting.
impl ParamName {
    /// The field names that mean this parameter, across the packs in the wild.
    fn aliases(self) -> &'static [&'static str] {
        match self {
            ParamName::Seed => &["seed", "noise_seed", "rand_seed"],
            ParamName::Steps => &["steps", "num_steps", "sampling_steps"],
            ParamName::Cfg => &["cfg", "cfg_scale", "guidance_scale", "guidance"],
            ParamName::Denoise => &["denoise", "denoising_strength"],
            // `length` is only a frame count on a graph that makes something
            // that moves; on a latent it is a batch dimension.
            ParamName::Frames => &["num_frames", "frame_count", "video_frames", "frames"],
            ParamName::Fps => &["fps", "frame_rate"],
            ParamName::Width => &["width"],
            ParamName::Height => &["height"],
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ParamName::Seed => "seed",
            ParamName::Steps => "steps",
            ParamName::Cfg => "cfg",
            ParamName::Denoise => "denoise",
            ParamName::Frames => "frames",
            ParamName::Fps => "fps",
            ParamName::Width => "width",
            ParamName::Height => "height",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        const ALL: &[ParamName] = &[
            ParamName::Seed,
            ParamName::Steps,
            ParamName::Cfg,
            ParamName::Denoise,
            ParamName::Frames,
            ParamName::Fps,
            ParamName::Width,
            ParamName::Height,
        ];
        let s = s.trim().to_ascii_lowercase();
        ALL.iter().copied().find(|p| p.as_str() == s)
    }
}

// ===== The parts of a contract ==============================================

/// One of the graph's source loaders, and which slot of the stage it fills.
///
/// Straight out of [`detect_loaders`] — FR2 already worked out that a
/// start-frame and an end-frame loader are not interchangeable and how to tell
/// them apart. A contract only records the answer where a line can read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSlot {
    pub role: SourceRole,
    pub node_id: String,
    pub node_type: String,
    pub kind: LoaderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A text input a describe stage or a person fills.
///
/// `positive` and `negative` by convention, but the name is a free string: a
/// graph can carry a third prompt box, and a compiler binding text into one
/// needs to name it rather than pick from a closed list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSlot {
    pub name: String,
    pub node_id: String,
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_title: Option<String>,
    /// ComfyUI's own flag, when the catalogue could be read.
    #[serde(default)]
    pub multiline: bool,
    /// What the workflow's author left in the box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl PromptSlot {
    /// The key that fills this slot on a run.
    ///
    /// It is the key [`super::workflow::prepare_workflow`] already substitutes
    /// on, so binding a describe stage's output into a downstream prompt needs
    /// no new plumbing at all — it is a `text_overrides` entry.
    pub fn override_key(&self) -> String {
        format!("{}.{}", self.node_id, self.field)
    }
}

/// A canonical setting, and the node field that carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageParam {
    pub name: ParamName,
    pub node_id: String,
    pub field: String,
    /// What ComfyUI says this field accepts — the node's own range. `None` when
    /// the catalogue could not be read; the field was found by name and nothing
    /// is known about what it will take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WidgetSpec>,
    pub current_value: Value,
}

#[allow(dead_code)] // The key a line writes a setting with.
impl StageParam {
    /// Same key as a prompt slot: params are substituted the same way.
    pub fn override_key(&self) -> String {
        format!("{}.{}", self.node_id, self.field)
    }
}

/// What the derivation was unsure about — the reason a contract is worth a
/// second look, and the thing the console puts a correction affordance next to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractWarning {
    /// ComfyUI could not be reached, so parameters carry no ranges and only the
    /// prompt boxes the old heuristics recognise were found. Re-derived on a
    /// later start, which is why it is recorded rather than merely logged.
    NoCatalog,
    /// Nothing in the graph saves anything Phos could recognise.
    NoOutputNode,
    /// Savers of more than one kind. One type was picked; check it.
    MixedOutputs,
    /// The only thing this graph saves is audio, which a line cannot carry.
    UnsupportedOutput,
    /// Nothing here reads a file. The graph starts a line rather than
    /// continuing one — or the loader is of a shape Phos cannot see.
    NoSourceLoader,
    /// The catalogue was read, and this graph uses classes it does not contain:
    /// a custom pack that is not installed on this server. Those nodes could
    /// not be typed, and a run would fail on them.
    UnknownClasses,
}

/// What a person said the derivation got wrong.
///
/// Kept beside the derived answer rather than replacing it, so the contract can
/// be re-derived — when ComfyUI comes back and the parameters can finally be
/// typed — without throwing the correction away.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ContractCorrections {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepts: Option<Accepts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produces: Option<MediaType>,
    /// `node_id` → the slot that loader really fills.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, SourceRole>,
    /// `"<node_id>.<field>"` → the prompt slot's name, or `null` for "that is
    /// not a prompt".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub slots: BTreeMap<String, Option<String>>,
    /// `"<node_id>.<field>"` → the setting it really is (one of [`ParamName`]),
    /// or `null` for "that is not one of ours".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, Option<ParamName>>,
}

impl ContractCorrections {
    pub fn is_empty(&self) -> bool {
        self == &ContractCorrections::default()
    }
}

/// What a workflow takes and what it hands on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageContract {
    #[serde(default = "default_version")]
    pub version: u32,
    pub accepts: Accepts,
    pub produces: MediaType,
    #[serde(default)]
    pub roles: Vec<RoleSlot>,
    #[serde(default)]
    pub slots: Vec<PromptSlot>,
    #[serde(default)]
    pub params: Vec<StageParam>,
    #[serde(default)]
    pub warnings: Vec<ContractWarning>,
    #[serde(default, skip_serializing_if = "ContractCorrections::is_empty")]
    pub corrections: ContractCorrections,
}

fn default_version() -> u32 {
    CONTRACT_VERSION
}

impl StageContract {
    /// Derive a contract from the graph and whatever ComfyUI could say.
    pub fn derive(workflow: &Value, catalog: Option<&NodeCatalog>) -> Self {
        Self::derive_with(workflow, catalog, ContractCorrections::default())
    }

    /// Derive a contract with a person's corrections taking part.
    ///
    /// Applied *during* derivation rather than to the result: "node 7 is the
    /// negative prompt" has to be able to name a text box the heuristics never
    /// offered, which a patch over a finished contract could not do.
    pub fn derive_with(
        workflow: &Value,
        catalog: Option<&NodeCatalog>,
        corrections: ContractCorrections,
    ) -> Self {
        let mut warnings = Vec::new();
        match catalog {
            None => warnings.push(ContractWarning::NoCatalog),
            Some(catalog) if unknown_classes(workflow, catalog) => {
                warnings.push(ContractWarning::UnknownClasses)
            }
            Some(_) => {}
        }

        let loaders = detect_loaders(workflow);
        let inputs = detect_inputs(workflow, catalog);

        let mut roles: Vec<RoleSlot> = loaders
            .iter()
            .map(|l| RoleSlot {
                role: corrections.roles.get(&l.node_id).copied().unwrap_or(l.role),
                node_id: l.node_id.clone(),
                node_type: l.node_type.clone(),
                kind: l.kind,
                title: l.title.clone(),
            })
            .collect();
        // Node ids are strings that are almost always numbers, and a reader
        // expects 9 before 12.
        roles.sort_by_key(|r| node_order(&r.node_id));

        let produces = derive_produces(workflow, catalog, &mut warnings);
        let slots = derive_slots(workflow, &inputs, &loaders, &corrections);
        let params = derive_params(&inputs, produces, &corrections);

        let derived_accepts = if roles.iter().any(|r| r.kind == LoaderKind::Video) {
            Accepts::Video
        } else if !roles.is_empty() {
            Accepts::Image
        } else if !slots.is_empty() {
            // No file goes in, but a prompt does: a text-to-image stage, which
            // is how a line begins once FR9 can write into it.
            Accepts::Text
        } else {
            Accepts::None
        };
        if roles.is_empty() {
            warnings.push(ContractWarning::NoSourceLoader);
        }

        StageContract {
            version: CONTRACT_VERSION,
            accepts: corrections.accepts.unwrap_or(derived_accepts),
            produces: corrections.produces.unwrap_or(produces),
            roles,
            slots,
            params,
            warnings,
            corrections,
        }
    }

    /// Would deriving this again, now that ComfyUI can be asked, do better?
    ///
    /// A contract derived while the server was down carries untyped parameters
    /// and only the prompt boxes the old heuristics knew. It is worth redoing,
    /// once, when the catalogue is available.
    pub fn wants_catalog(&self) -> bool {
        self.warnings.contains(&ContractWarning::NoCatalog)
    }

    /// The slot with this name, if the graph has one.
    #[allow(dead_code)] // How a prompt compiler finds where to write.
    pub fn slot(&self, name: &str) -> Option<&PromptSlot> {
        self.slots.iter().find(|s| s.name == name)
    }

    /// Every field carrying this setting. More than one is normal and correct:
    /// a graph with two samplers has two seeds, and pinning one of them is not
    /// reproducibility.
    #[allow(dead_code)] // How a line sets a setting it does not know the node for.
    pub fn params_named(&self, name: ParamName) -> impl Iterator<Item = &StageParam> {
        self.params.iter().filter(move |p| p.name == name)
    }

    /// Fold this workflow's role corrections into one run's override map.
    ///
    /// A loader its author mistitled is corrected once, on the contract, and
    /// every run then binds the upload to the slot it really fills. The binder
    /// already reads `role:<node_id>` out of a task's overrides (see
    /// [`super::loaders::role_directives`]), so that is where the correction
    /// joins the run — no new plumbing, and nothing in the worker to change.
    ///
    /// Anything the caller said explicitly is left exactly as they said it.
    pub fn apply_role_corrections(
        &self,
        overrides: &mut std::collections::HashMap<String, String>,
    ) {
        for (node_id, role) in &self.corrections.roles {
            overrides
                .entry(format!(
                    "{}{}",
                    super::loaders::ROLE_OVERRIDE_PREFIX,
                    node_id
                ))
                .or_insert_with(|| role.as_str().to_string());
        }
    }
}
// `BTreeSet<MediaType>` needs an order; the values themselves have no rank.
impl PartialOrd for MediaType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MediaType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::super::nodes::{fixtures::object_info, parse_object_info};
    use super::*;
    use serde_json::json;

    // === Fixtures ===========================================================
    //
    // Six graphs, one for each shape a stage comes in. They are written the way
    // ComfyUI's "Save (API Format)" writes them, because that is the only shape
    // Phos ever sees.

    /// What a loaded ComfyUI would say: the shared core fixture plus the video,
    /// upscale and vision-language classes these graphs use.
    fn catalog() -> NodeCatalog {
        let mut doc = object_info();
        let extra = json!({
            "VAEEncode": { "input": { "required": {
                "pixels": ["IMAGE"], "vae": ["VAE"] } }, "output": ["LATENT"] },
            "VAEDecode": { "input": { "required": {
                "samples": ["LATENT"], "vae": ["VAE"] } }, "output": ["IMAGE"] },
            "VHS_LoadVideo": { "input": { "required": {
                "video": [["clip.mp4", "other.mp4"], { "video_upload": true }],
                "force_rate": ["INT", { "default": 0, "min": 0, "max": 60 }],
                "frame_load_cap": ["INT", { "default": 0, "min": 0, "max": 10000 }],
                "skip_first_frames": ["INT", { "default": 0, "min": 0, "max": 10000 }] } },
                "output": ["IMAGE", "MASK", "INT"], "category": "Video Helper Suite" },
            "VHS_VideoCombine": { "input": { "required": {
                "images": ["IMAGE"],
                "frame_rate": ["FLOAT", { "default": 8.0, "min": 1.0, "max": 120.0, "step": 1.0 }],
                "filename_prefix": ["STRING", { "default": "AnimateDiff" }],
                "format": [["video/h264-mp4", "image/gif", "image/webp"]] } },
                "output": [], "output_node": true, "category": "Video Helper Suite" },
            "SaveVideo": { "input": { "required": {
                "video": ["VIDEO"],
                "filename_prefix": ["STRING", { "default": "video/ComfyUI" }] } },
                "output": [], "output_node": true },
            "SaveAudio": { "input": { "required": {
                "audio": ["AUDIO"],
                "filename_prefix": ["STRING", { "default": "audio/ComfyUI" }] } },
                "output": [], "output_node": true },
            "UNETLoader": { "input": { "required": {
                "unet_name": [["wan2.1_i2v_480p.safetensors"]],
                "weight_dtype": [["default", "fp8_e4m3fn"]] } }, "output": ["MODEL"] },
            "CLIPLoader": { "input": { "required": {
                "clip_name": [["umt5_xxl.safetensors"]],
                "type": [["wan", "sdxl"]] } }, "output": ["CLIP"] },
            "VAELoader": { "input": { "required": {
                "vae_name": [["wan_2.1_vae.safetensors"]] } }, "output": ["VAE"] },
            "WanImageToVideo": { "input": { "required": {
                "positive": ["CONDITIONING"], "negative": ["CONDITIONING"], "vae": ["VAE"],
                "width": ["INT", { "default": 832, "min": 16, "max": 4096, "step": 16 }],
                "height": ["INT", { "default": 480, "min": 16, "max": 4096, "step": 16 }],
                "length": ["INT", { "default": 81, "min": 1, "max": 1000, "step": 4 }],
                "batch_size": ["INT", { "default": 1, "min": 1, "max": 4096 }] },
                "optional": { "start_image": ["IMAGE"], "end_image": ["IMAGE"] } },
                "output": ["CONDITIONING", "CONDITIONING", "LATENT"] },
            "UpscaleModelLoader": { "input": { "required": {
                "model_name": [["4x-UltraSharp.pth"]] } }, "output": ["UPSCALE_MODEL"] },
            "ImageUpscaleWithModel": { "input": { "required": {
                "upscale_model": ["UPSCALE_MODEL"], "image": ["IMAGE"] } },
                "output": ["IMAGE"] },
            "ConditioningZeroOut": { "input": { "required": {
                "conditioning": ["CONDITIONING"] } }, "output": ["CONDITIONING"] },
            "PrimitiveStringMultiline": { "input": { "required": {
                "value": ["STRING", { "multiline": true }] } }, "output": ["STRING"] },
            "QwenVLLoader": { "input": { "required": {
                "model": [["Qwen2.5-VL-7B-Instruct"]] } }, "output": ["QWENVL"] },
            "QwenVLRun": { "input": { "required": {
                "model": ["QWENVL"], "image": ["IMAGE"],
                "prompt": ["STRING", { "multiline": true,
                                       "default": "Describe this photograph." }],
                "max_tokens": ["INT", { "default": 128, "min": 1, "max": 4096 }] } },
                "output": ["STRING"] },
            // The `pysssss` text viewer: not named like a saver, no
            // `filename_prefix`, and terminal. Only ComfyUI knows it ends a graph.
            "ShowText|pysssss": { "input": { "required": { "text": ["STRING"] } },
                "output": ["STRING"], "output_node": true },
            "PreviewText": { "input": { "required": { "text": ["STRING"] } },
                "output": [], "output_node": true }
        });
        let (Some(doc_map), Some(extra_map)) = (doc.as_object_mut(), extra.as_object()) else {
            panic!("the fixtures are objects");
        };
        for (k, v) in extra_map {
            doc_map.insert(k.clone(), v.clone());
        }
        parse_object_info(&doc)
    }

    /// image → image. An SDXL img2img refiner: one loader, two untitled prompt
    /// boxes told apart only by which socket of the sampler they feed.
    fn upscaler() -> Value {
        json!({
            "4": { "class_type": "CheckpointLoaderSimple",
                   "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" } },
            "10": { "class_type": "LoadImage", "inputs": { "image": "photo.png" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a sharp photograph", "clip": ["4", 1] } },
            "7": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "blurry, jpeg artifacts", "clip": ["4", 1] } },
            "11": { "class_type": "VAEEncode",
                    "inputs": { "pixels": ["10", 0], "vae": ["4", 2] } },
            "3": { "class_type": "KSampler", "inputs": {
                     "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0],
                     "latent_image": ["11", 0], "seed": 42, "steps": 20, "cfg": 8.0,
                     "sampler_name": "euler", "scheduler": "normal", "denoise": 0.45 } },
            "8": { "class_type": "VAEDecode",
                   "inputs": { "samples": ["3", 0], "vae": ["4", 2] } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["8", 0], "filename_prefix": "ComfyUI" } }
        })
    }

    /// image → video. Wan i2v: a titled start frame, both prompts passing
    /// through a conditioning node before they reach the sampler, an mp4 at the
    /// end.
    fn i2v() -> Value {
        json!({
            "37": { "class_type": "UNETLoader", "inputs": {
                      "unet_name": "wan2.1_i2v_480p.safetensors",
                      "weight_dtype": "default" } },
            "38": { "class_type": "CLIPLoader",
                    "inputs": { "clip_name": "umt5_xxl.safetensors", "type": "wan" } },
            "39": { "class_type": "VAELoader",
                    "inputs": { "vae_name": "wan_2.1_vae.safetensors" } },
            "52": { "class_type": "LoadImage", "inputs": { "image": "start.png" },
                    "_meta": { "title": "Start Frame" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a slow dolly in", "clip": ["38", 0] } },
            "7": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "static, watermark", "clip": ["38", 0] } },
            "50": { "class_type": "WanImageToVideo", "inputs": {
                      "positive": ["6", 0], "negative": ["7", 0], "vae": ["39", 0],
                      "start_image": ["52", 0], "width": 832, "height": 480,
                      "length": 81, "batch_size": 1 } },
            "3": { "class_type": "KSampler", "inputs": {
                     "model": ["37", 0], "positive": ["50", 0], "negative": ["50", 1],
                     "latent_image": ["50", 2], "seed": 123456, "steps": 30, "cfg": 6.0,
                     "sampler_name": "euler", "scheduler": "normal", "denoise": 1.0 } },
            "8": { "class_type": "VAEDecode",
                   "inputs": { "samples": ["3", 0], "vae": ["39", 0] } },
            "30": { "class_type": "VHS_VideoCombine", "inputs": {
                      "images": ["8", 0], "frame_rate": 16.0,
                      "filename_prefix": "wan", "format": "video/h264-mp4" } }
        })
    }

    /// video → video. No sampler, no prompts, nothing to say to it: a clip goes
    /// in and a bigger clip comes out.
    fn video_upscaler() -> Value {
        json!({
            "1": { "class_type": "VHS_LoadVideo", "inputs": {
                     "video": "clip.mp4", "force_rate": 0, "frame_load_cap": 0,
                     "skip_first_frames": 0 } },
            "2": { "class_type": "UpscaleModelLoader",
                   "inputs": { "model_name": "4x-UltraSharp.pth" } },
            "3": { "class_type": "ImageUpscaleWithModel",
                   "inputs": { "upscale_model": ["2", 0], "image": ["1", 0] } },
            "4": { "class_type": "VHS_VideoCombine", "inputs": {
                     "images": ["3", 0], "frame_rate": 24.0,
                     "filename_prefix": "upscaled", "format": "video/h264-mp4" } }
        })
    }

    /// Two pictures → video. The interpolator FR2 was built for: the start and
    /// end frames are different photographs and are not interchangeable.
    fn interpolator() -> Value {
        json!({
            "37": { "class_type": "UNETLoader", "inputs": {
                      "unet_name": "wan2.1_i2v_480p.safetensors",
                      "weight_dtype": "default" } },
            "38": { "class_type": "CLIPLoader",
                    "inputs": { "clip_name": "umt5_xxl.safetensors", "type": "wan" } },
            "39": { "class_type": "VAELoader",
                    "inputs": { "vae_name": "wan_2.1_vae.safetensors" } },
            "12": { "class_type": "LoadImage", "inputs": { "image": "a.png" },
                    "_meta": { "title": "Start Frame" } },
            "14": { "class_type": "LoadImage", "inputs": { "image": "b.png" },
                    "_meta": { "title": "End Frame" } },
            "9": { "class_type": "LoadImage", "inputs": { "image": "pose.png" },
                   "_meta": { "title": "Style reference" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a smooth transition", "clip": ["38", 0] },
                   "_meta": { "title": "Positive Prompt" } },
            "7": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "flicker", "clip": ["38", 0] },
                   "_meta": { "title": "Negative Prompt" } },
            "50": { "class_type": "WanImageToVideo", "inputs": {
                      "positive": ["6", 0], "negative": ["7", 0], "vae": ["39", 0],
                      "start_image": ["12", 0], "end_image": ["14", 0],
                      "width": 832, "height": 480, "length": 49, "batch_size": 1 } },
            "3": { "class_type": "KSampler", "inputs": {
                     "model": ["37", 0], "positive": ["50", 0], "negative": ["50", 1],
                     "latent_image": ["50", 2], "seed": 7, "steps": 25, "cfg": 6.0,
                     "sampler_name": "euler", "scheduler": "normal", "denoise": 1.0 } },
            "8": { "class_type": "VAEDecode",
                   "inputs": { "samples": ["3", 0], "vae": ["39", 0] } },
            "30": { "class_type": "SaveVideo",
                    "inputs": { "video": ["8", 0], "filename_prefix": "interp" } }
        })
    }

    /// image → text. FR9's describe stage: a vision-language model reads a
    /// photograph and the graph ends in a node that writes no file at all.
    fn describe() -> Value {
        json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "photo.png" } },
            "2": { "class_type": "QwenVLLoader",
                   "inputs": { "model": "Qwen2.5-VL-7B-Instruct" } },
            "3": { "class_type": "QwenVLRun", "inputs": {
                     "model": ["2", 0], "image": ["1", 0],
                     "prompt": "Describe this photograph in one sentence.",
                     "max_tokens": 128 } },
            "4": { "class_type": "ShowText|pysssss", "inputs": { "text": ["3", 0] } }
        })
    }

    /// A graph made entirely of classes this server has never heard of.
    fn unknown() -> Value {
        json!({
            "1": { "class_type": "MysteryLoaderXL", "inputs": { "image": "in.png" } },
            "2": { "class_type": "MysteryProcessor",
                   "inputs": { "image": ["1", 0], "amount": 0.5 } },
            "3": { "class_type": "MysteryWriter",
                   "inputs": { "result": ["2", 0], "filename_prefix": "out" } }
        })
    }

    fn slot_names(c: &StageContract) -> Vec<&str> {
        c.slots.iter().map(|s| s.name.as_str()).collect()
    }

    fn param_keys(c: &StageContract) -> Vec<String> {
        c.params
            .iter()
            .map(|p| format!("{}={}", p.name.as_str(), p.override_key()))
            .collect()
    }

    fn roles(c: &StageContract) -> Vec<(&str, &str)> {
        c.roles
            .iter()
            .map(|r| {
                (
                    r.node_id.as_str(),
                    match r.role {
                        SourceRole::Start => "start",
                        SourceRole::End => "end",
                        SourceRole::Reference => "reference",
                    },
                )
            })
            .collect()
    }

    // === The shapes a stage comes in ========================================

    #[test]
    fn an_image_upscaler_takes_a_picture_and_gives_one_back() {
        let c = StageContract::derive(&upscaler(), Some(&catalog()));
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Image);
        assert_eq!(roles(&c), [("10", "start")]);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn an_image_to_video_workflow_takes_a_picture_and_gives_back_a_clip() {
        let c = StageContract::derive(&i2v(), Some(&catalog()));
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Video);
        // The loader the author titled, in the slot the title names.
        assert_eq!(roles(&c), [("52", "start")]);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn a_video_upscaler_takes_a_clip_and_gives_one_back() {
        let c = StageContract::derive(&video_upscaler(), Some(&catalog()));
        assert_eq!(c.accepts, Accepts::Video);
        assert_eq!(c.produces, MediaType::Video);
        assert_eq!(roles(&c), [("1", "start")]);
        // Nothing to say to it: no prompt, and the only knob is the frame rate.
        assert!(c.slots.is_empty(), "{:?}", slot_names(&c));
        assert_eq!(param_keys(&c), ["fps=4.frame_rate"]);
    }

    #[test]
    fn an_interpolator_keeps_its_three_pictures_apart() {
        // The bug FR2 fixed, restated as a fact a line can read: these are three
        // different slots, and handing the same picture to all of them is what
        // made this workflow impossible to run.
        let c = StageContract::derive(&interpolator(), Some(&catalog()));
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Video);
        assert_eq!(
            roles(&c),
            [("9", "reference"), ("12", "start"), ("14", "end")]
        );
    }

    #[test]
    fn a_describe_stage_produces_text_and_the_catalogue_is_what_says_so() {
        // `ShowText|pysssss` is not named like a saver and carries no
        // `filename_prefix`, so `detect_outputs` cannot see it. ComfyUI marks it
        // `OUTPUT_NODE`, which is the only reason this graph can be typed.
        let c = StageContract::derive(&describe(), Some(&catalog()));
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Text);
        // A text stage writes no file; nothing downstream should look for one.
        assert_ne!(c.produces, MediaType::Image);
        // Its instruction is a prompt slot like any other, so a compiler fills
        // it the same way.
        assert_eq!(slot_names(&c), ["positive"]);
        assert_eq!(c.slot("positive").unwrap().override_key(), "3.prompt");
        assert!(c.slot("positive").unwrap().multiline);
    }

    #[test]
    fn a_text_viewer_named_like_one_is_typed_without_a_catalogue_at_all() {
        // `PreviewText` is caught by `detect_outputs`' `Preview*` rule, so the
        // name alone is enough when ComfyUI cannot be asked.
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "photo.png" } },
            "3": { "class_type": "QwenVLRun",
                   "inputs": { "image": ["1", 0], "prompt": "Describe it." } },
            "4": { "class_type": "PreviewText", "inputs": { "text": ["3", 0] } }
        });
        assert_eq!(
            StageContract::derive(&wf, None).produces,
            MediaType::Text,
            "a node named like a text viewer writes text"
        );
    }

    #[test]
    fn a_graph_of_classes_the_server_does_not_have_still_yields_a_contract() {
        let c = StageContract::derive(&unknown(), Some(&catalog()));
        // Shape is enough for the types: something loads a picture, something
        // writes a file.
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Image);
        assert_eq!(roles(&c), [("1", "start")]);
        // But nothing could be typed, and a run would fail on those nodes, so
        // the contract says so rather than looking confident.
        assert!(c.warnings.contains(&ContractWarning::UnknownClasses));
        assert!(c.params.is_empty());
        assert!(c.slots.is_empty());
    }

    // === Prompt slots =======================================================

    #[test]
    fn two_untitled_prompt_boxes_are_told_apart_by_the_socket_they_feed() {
        // The commonest workflow in circulation: no titles anywhere, and the
        // only difference between node 6 and node 7 is which side of the
        // sampler they are wired into.
        let c = StageContract::derive(&upscaler(), Some(&catalog()));
        assert_eq!(slot_names(&c), ["positive", "negative"]);
        assert_eq!(c.slot("positive").unwrap().node_id, "6");
        assert_eq!(c.slot("negative").unwrap().node_id, "7");
        assert_eq!(
            c.slot("negative").unwrap().default.as_deref(),
            Some("blurry, jpeg artifacts")
        );
    }

    #[test]
    fn the_nearest_conditioning_socket_wins_over_a_distant_one() {
        // In the Wan graph both prompts pass through `WanImageToVideo` before
        // they reach the sampler, so the negative prompt is one hop from a
        // `negative` socket and two hops from a `positive` one. Counting
        // reachability alone would call it ambiguous and name neither.
        let c = StageContract::derive(&i2v(), Some(&catalog()));
        assert_eq!(slot_names(&c), ["positive", "negative"]);
        assert_eq!(c.slot("positive").unwrap().node_id, "6");
        assert_eq!(c.slot("negative").unwrap().node_id, "7");
    }

    #[test]
    fn a_prompt_reached_through_a_conditioning_node_is_still_found() {
        let wf = json!({
            "4": { "class_type": "CheckpointLoaderSimple",
                   "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" } },
            "10": { "class_type": "LoadImage", "inputs": { "image": "photo.png" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] } },
            "7": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "blurry", "clip": ["4", 1] } },
            "20": { "class_type": "ConditioningZeroOut",
                    "inputs": { "conditioning": ["7", 0] } },
            "3": { "class_type": "KSampler", "inputs": {
                     "model": ["4", 0], "positive": ["6", 0], "negative": ["20", 0],
                     "latent_image": ["10", 0], "seed": 1, "steps": 20, "cfg": 8.0,
                     "sampler_name": "euler", "scheduler": "normal", "denoise": 1.0 } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["3", 0], "filename_prefix": "out" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        assert_eq!(c.slot("negative").map(|s| s.node_id.as_str()), Some("7"));
    }

    #[test]
    fn a_title_names_the_prompt_when_the_wiring_would_too() {
        let c = StageContract::derive(&interpolator(), Some(&catalog()));
        assert_eq!(slot_names(&c), ["positive", "negative"]);
        assert_eq!(
            c.slot("positive").unwrap().node_title.as_deref(),
            Some("Positive Prompt")
        );
    }

    #[test]
    fn a_lone_prompt_box_is_the_prompt_whatever_its_author_called_it() {
        // Naming it after its title would mean a compiler has to guess which
        // slot to bind into on every workflow. One name, everywhere.
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] },
                   "_meta": { "title": "Scene description" } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["6", 0], "filename_prefix": "out" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        assert_eq!(slot_names(&c), ["positive"]);
        // The author's own name is kept where a person can read it.
        assert_eq!(
            c.slot("positive").unwrap().node_title.as_deref(),
            Some("Scene description")
        );
    }

    #[test]
    fn a_third_prompt_box_gets_a_name_of_its_own_rather_than_colliding() {
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a", "clip": ["4", 1] },
                   "_meta": { "title": "Positive Prompt" } },
            "7": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "b", "clip": ["4", 1] },
                   "_meta": { "title": "Negative Prompt" } },
            "8": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "c", "clip": ["4", 1] },
                   "_meta": { "title": "Refiner Prompt" } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["6", 0], "filename_prefix": "out" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        assert_eq!(slot_names(&c), ["positive", "negative", "refiner_prompt"]);
    }

    #[test]
    fn the_shots_own_filename_is_never_mistaken_for_a_prompt() {
        // `LoadImage.image` and `VHS_LoadVideo.video` are strings, and on some
        // packs one of them is a text widget. Neither is something to type into:
        // the shot goes there.
        for wf in [upscaler(), video_upscaler(), i2v()] {
            for cat in [None, Some(catalog())] {
                let c = StageContract::derive(&wf, cat.as_ref());
                for s in &c.slots {
                    assert!(
                        s.field != "image" && s.field != "video",
                        "{} was offered as a prompt",
                        s.override_key()
                    );
                }
            }
        }
    }

    #[test]
    fn the_slot_key_is_the_key_the_run_actually_substitutes_on() {
        // Not a coincidence to rely on by inspection: bind the slot the way a
        // prompt compiler would and check the graph really changed.
        use crate::comfyui::loaders::{LoaderKind, SourceBinding, SourceRole};
        let wf = upscaler();
        let c = StageContract::derive(&wf, Some(&catalog()));
        let key = c.slot("negative").unwrap().override_key();
        let overrides: std::collections::HashMap<String, String> =
            [(key, "written by a describe stage".to_string())]
                .into_iter()
                .collect();
        let no_roles = std::collections::HashMap::new();
        let prepared = crate::comfyui::workflow::prepare_workflow(
            &wf,
            &SourceBinding {
                uploaded_filename: "new.png",
                kind: LoaderKind::Image,
                role: SourceRole::Start,
                role_overrides: &no_roles,
            },
            &overrides,
            None,
        );
        assert_eq!(
            prepared["7"]["inputs"]["text"].as_str(),
            Some("written by a describe stage")
        );
    }

    // === Parameters =========================================================

    #[test]
    fn the_params_carry_the_nodes_own_ranges() {
        let c = StageContract::derive(&upscaler(), Some(&catalog()));
        assert_eq!(
            param_keys(&c),
            [
                "seed=3.seed",
                "steps=3.steps",
                "cfg=3.cfg",
                "denoise=3.denoise"
            ]
        );
        let steps = c.params_named(ParamName::Steps).next().unwrap();
        assert_eq!(
            steps.widget,
            Some(WidgetSpec::Int {
                default: Some(20),
                min: Some(1),
                max: Some(10000),
                step: None
            })
        );
        assert_eq!(steps.current_value, json!(20));
        let seed = c.params_named(ParamName::Seed).next().unwrap();
        assert!(matches!(seed.widget, Some(WidgetSpec::Seed { .. })));
    }

    #[test]
    fn a_seed_is_whatever_comfyui_marks_as_one() {
        // `SamplerCustom` calls it `noise_seed`, and ComfyUI's own
        // `control_after_generate` flag is what identifies it.
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "2": { "class_type": "SamplerCustom",
                   "inputs": { "add_noise": true, "noise_seed": 99 } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["2", 0], "filename_prefix": "out" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        // `add_noise` is a boolean nobody named, and not one of ours.
        assert_eq!(param_keys(&c), ["seed=2.noise_seed"]);
    }

    #[test]
    fn a_frame_count_is_only_a_frame_count_on_a_graph_that_moves() {
        // `length` on the Wan node is 81 frames. The same word on a latent is a
        // batch dimension, which is why the graph's own output type decides.
        let c = StageContract::derive(&i2v(), Some(&catalog()));
        assert_eq!(
            c.params_named(ParamName::Frames)
                .map(|p| p.override_key())
                .collect::<Vec<_>>(),
            ["50.length"]
        );
        assert_eq!(
            c.params_named(ParamName::Fps)
                .map(|p| p.override_key())
                .collect::<Vec<_>>(),
            ["30.frame_rate"]
        );

        let still = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "2": { "class_type": "WanImageToVideo", "inputs": {
                     "width": 512, "height": 512, "length": 1, "batch_size": 1 } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["2", 0], "filename_prefix": "out" } }
        });
        let c = StageContract::derive(&still, Some(&catalog()));
        assert_eq!(c.produces, MediaType::Image);
        assert_eq!(c.params_named(ParamName::Frames).count(), 0);
        // Width and height are still knobs on a still.
        assert_eq!(param_keys(&c), ["width=2.width", "height=2.height"]);
    }

    #[test]
    fn two_samplers_mean_two_seeds_rather_than_a_favourite() {
        // Pinning one of them is not reproducibility, so a line is told about
        // both and can set both.
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "3": { "class_type": "KSampler", "inputs": {
                     "seed": 1, "steps": 20, "cfg": 8.0, "sampler_name": "euler",
                     "scheduler": "normal", "denoise": 1.0 } },
            "17": { "class_type": "KSampler", "inputs": {
                      "seed": 2, "steps": 10, "cfg": 4.0, "sampler_name": "euler",
                      "scheduler": "normal", "denoise": 0.4 } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["17", 0], "filename_prefix": "out" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        assert_eq!(
            c.params_named(ParamName::Seed)
                .map(|p| p.override_key())
                .collect::<Vec<_>>(),
            ["3.seed", "17.seed"],
            "node ids sort as numbers, not as strings"
        );
    }

    #[test]
    fn a_checkpoint_or_a_lora_is_not_a_setting_a_line_dials() {
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "4": { "class_type": "CheckpointLoaderSimple",
                   "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" } },
            "10": { "class_type": "LoraLoaderModelOnly", "inputs": {
                      "model": ["4", 0], "lora_name": "add_detail.safetensors",
                      "strength_model": 0.8 } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["10", 0], "filename_prefix": "out" } }
        });
        assert!(StageContract::derive(&wf, Some(&catalog()))
            .params
            .is_empty());
    }

    // === Degrading without a catalogue ======================================

    #[test]
    fn a_contract_derived_with_no_server_is_still_a_contract() {
        let c = StageContract::derive(&upscaler(), None);
        // The types come from the graph's shape, which needs nothing from
        // ComfyUI.
        assert_eq!(c.accepts, Accepts::Image);
        assert_eq!(c.produces, MediaType::Image);
        assert_eq!(roles(&c), [("10", "start")]);
        // And the prompt boxes the old heuristics knew are still found.
        assert_eq!(slot_names(&c), ["positive", "negative"]);
        // What is lost is the typing: no ranges, so no settings.
        assert!(c.params.is_empty());
        assert!(c.warnings.contains(&ContractWarning::NoCatalog));
        assert!(c.wants_catalog(), "it should be derived again later");

        // And with a catalogue it stops asking.
        assert!(!StageContract::derive(&upscaler(), Some(&catalog())).wants_catalog());
    }

    #[test]
    fn the_video_types_survive_without_a_catalogue_too() {
        for (wf, accepts, produces) in [
            (video_upscaler(), Accepts::Video, MediaType::Video),
            (i2v(), Accepts::Image, MediaType::Video),
            (interpolator(), Accepts::Image, MediaType::Video),
        ] {
            let c = StageContract::derive(&wf, None);
            assert_eq!(c.accepts, accepts);
            assert_eq!(c.produces, produces);
        }
    }

    // === Odd graphs =========================================================

    #[test]
    fn a_graph_with_nothing_to_load_starts_a_line() {
        // Text to image. Today's import gate turns this away, but the type
        // system has to hold it: it is where a line begins.
        let wf = json!({
            "4": { "class_type": "CheckpointLoaderSimple",
                   "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" } },
            "5": { "class_type": "EmptyLatentImage",
                   "inputs": { "width": 1024, "height": 1024, "batch_size": 1 } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["6", 0], "filename_prefix": "out" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        // A prompt goes in, and nothing else does.
        assert_eq!(c.accepts, Accepts::Text);
        assert_eq!(c.produces, MediaType::Image);
        assert!(c.warnings.contains(&ContractWarning::NoSourceLoader));

        // With no prompt either, it takes nothing at all.
        let bare = json!({
            "5": { "class_type": "EmptyLatentImage",
                   "inputs": { "width": 1024, "height": 1024, "batch_size": 1 } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["5", 0], "filename_prefix": "out" } }
        });
        assert_eq!(
            StageContract::derive(&bare, Some(&catalog())).accepts,
            Accepts::None
        );
    }

    #[test]
    fn a_graph_that_saves_nothing_says_so_rather_than_guessing_quietly() {
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "2": { "class_type": "VAEEncode",
                   "inputs": { "pixels": ["1", 0], "vae": ["4", 2] } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        assert_eq!(c.produces, MediaType::Image, "the commonest guess");
        assert!(c.warnings.contains(&ContractWarning::NoOutputNode));
    }

    #[test]
    fn a_graph_that_only_makes_a_sound_is_not_pretended_to_be_a_picture() {
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "3": { "class_type": "SaveAudio",
                   "inputs": { "audio": ["1", 0], "filename_prefix": "audio/x" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        assert!(c.warnings.contains(&ContractWarning::UnsupportedOutput));
    }

    #[test]
    fn a_graph_that_saves_two_different_things_says_which_it_picked() {
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["1", 0], "filename_prefix": "still" } },
            "30": { "class_type": "SaveVideo",
                    "inputs": { "video": ["1", 0], "filename_prefix": "clip" } }
        });
        let c = StageContract::derive(&wf, Some(&catalog()));
        // A graph that makes something that moves makes a clip; the still is a
        // contact sheet on the way past.
        assert_eq!(c.produces, MediaType::Video);
        assert!(c.warnings.contains(&ContractWarning::MixedOutputs));
    }

    #[test]
    fn a_graph_of_the_wrong_shape_is_a_contract_rather_than_a_panic() {
        for wrong in [json!([]), json!("nope"), json!(null), json!({ "1": {} })] {
            for cat in [None, Some(catalog())] {
                let c = StageContract::derive(&wrong, cat.as_ref());
                assert_eq!(c.accepts, Accepts::None);
                assert_eq!(c.produces, MediaType::Image);
                assert!(c.roles.is_empty());
            }
        }
    }

    // === Correcting a contract ==============================================

    #[test]
    fn a_correction_beats_the_heuristic_and_survives_re_derivation() {
        // The describe graph, imported while ComfyUI was down: nothing then
        // knows `ShowText|pysssss` ends the graph, so it types as an image
        // workflow.
        let wrong = StageContract::derive(&describe(), None);
        assert_eq!(wrong.produces, MediaType::Image);

        let corrections = ContractCorrections {
            produces: Some(MediaType::Text),
            ..Default::default()
        };
        let fixed = StageContract::derive_with(&describe(), None, corrections.clone());
        assert_eq!(fixed.produces, MediaType::Text);

        // And when ComfyUI comes back, deriving again keeps what the person
        // said while picking up everything it could not see before.
        let again = StageContract::derive_with(&describe(), Some(&catalog()), corrections);
        assert_eq!(again.produces, MediaType::Text);
        assert!(!again.wants_catalog());
        assert_eq!(slot_names(&again), ["positive"]);
        assert_eq!(again.corrections.produces, Some(MediaType::Text));
    }

    #[test]
    fn a_correction_moves_a_loader_into_the_slot_it_really_fills() {
        // The author's titles are backwards for what this graph does. Saying so
        // once fixes it for every run, rather than per task.
        let c = StageContract::derive(&interpolator(), Some(&catalog()));
        assert_eq!(
            roles(&c),
            [("9", "reference"), ("12", "start"), ("14", "end")]
        );

        let corrections = ContractCorrections {
            roles: [
                ("12".to_string(), SourceRole::End),
                ("14".to_string(), SourceRole::Start),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let fixed = StageContract::derive_with(&interpolator(), Some(&catalog()), corrections);
        assert_eq!(
            roles(&fixed),
            [("9", "reference"), ("12", "end"), ("14", "start")]
        );
        // And they reach a run in the shape the binder already reads.
        let mut overrides: std::collections::HashMap<String, String> =
            [("role:12".to_string(), "reference".to_string())]
                .into_iter()
                .collect();
        fixed.apply_role_corrections(&mut overrides);
        assert_eq!(
            overrides.get("role:14").map(String::as_str),
            Some("start"),
            "the correction rides in as a role directive"
        );
        assert_eq!(
            overrides.get("role:12").map(String::as_str),
            Some("reference"),
            "what the caller said explicitly is left alone"
        );
        // And the binder reads it back.
        let (_, per_node) = crate::comfyui::loaders::role_directives(&overrides);
        assert_eq!(per_node.get("14"), Some(&SourceRole::Start));
    }

    #[test]
    fn a_correction_can_name_a_prompt_box_the_heuristics_never_offered() {
        // Node 8 is a text box wired nowhere near a conditioning socket. A patch
        // over a finished contract could only rename slots that already exist;
        // a correction that takes part in the derivation can add one.
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] },
                   "_meta": { "title": "Positive Prompt" } },
            "8": { "class_type": "PrimitiveStringMultiline",
                   "inputs": { "value": "handwritten note" } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["6", 0], "filename_prefix": "out" } }
        });
        let before = StageContract::derive(&wf, Some(&catalog()));
        assert_eq!(before.slot("negative"), None);

        let corrections = ContractCorrections {
            slots: [("8.value".to_string(), Some("negative".to_string()))]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let after = StageContract::derive_with(&wf, Some(&catalog()), corrections);
        assert_eq!(after.slot("negative").unwrap().node_id, "8");
        assert_eq!(after.slot("negative").unwrap().override_key(), "8.value");
    }

    #[test]
    fn a_correction_can_say_that_is_not_a_prompt_at_all() {
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "6": { "class_type": "CLIPTextEncode",
                   "inputs": { "text": "a photograph", "clip": ["4", 1] } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["6", 0], "filename_prefix": "out" } }
        });
        let corrections = ContractCorrections {
            slots: [("6.text".to_string(), None)].into_iter().collect(),
            ..Default::default()
        };
        let c = StageContract::derive_with(&wf, Some(&catalog()), corrections);
        assert!(c.slots.is_empty(), "{:?}", slot_names(&c));
    }

    #[test]
    fn a_correction_can_name_a_setting_the_aliases_do_not_know() {
        // A pack that spells the frame count `num_frames_total`.
        let doc = json!({
            "OddVideoNode": { "input": { "required": {
                "num_frames_total": ["INT", { "default": 16, "min": 1, "max": 512 }] } } }
        });
        let wf = json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "2": { "class_type": "OddVideoNode", "inputs": { "num_frames_total": 24 } },
            "30": { "class_type": "SaveVideo",
                    "inputs": { "video": ["2", 0], "filename_prefix": "clip" } }
        });
        let cat = parse_object_info(&doc);
        assert!(StageContract::derive(&wf, Some(&cat)).params.is_empty());

        let corrections = ContractCorrections {
            params: [("2.num_frames_total".to_string(), Some(ParamName::Frames))]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let c = StageContract::derive_with(&wf, Some(&cat), corrections);
        let frames = c.params_named(ParamName::Frames).next().unwrap();
        assert_eq!(frames.override_key(), "2.num_frames_total");
        assert_eq!(frames.current_value, json!(24));
        // And it still carries the node's own range.
        assert_eq!(
            frames.widget,
            Some(WidgetSpec::Int {
                default: Some(16),
                min: Some(1),
                max: Some(512),
                step: None
            })
        );
    }

    #[test]
    fn a_correction_naming_a_node_the_graph_no_longer_has_is_ignored() {
        // A workflow can be re-imported against a graph its author edited; a
        // correction pointing at a node that is gone must not break anything.
        let corrections = ContractCorrections {
            roles: [("999".to_string(), SourceRole::End)].into_iter().collect(),
            slots: [("999.text".to_string(), Some("negative".to_string()))]
                .into_iter()
                .collect(),
            params: [("999.steps".to_string(), Some(ParamName::Steps))]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let c = StageContract::derive_with(&upscaler(), Some(&catalog()), corrections);
        assert_eq!(roles(&c), [("10", "start")]);
        assert_eq!(slot_names(&c), ["positive", "negative"]);
        assert!(c.params_named(ParamName::Steps).all(|p| p.node_id != "999"));
    }

    // === What a line asks it ================================================

    #[test]
    fn admits_is_the_single_rule_a_chain_is_validated_with() {
        let photo_to_clip = StageContract::derive(&i2v(), Some(&catalog()));
        let clip_to_clip = StageContract::derive(&video_upscaler(), Some(&catalog()));
        let photo_to_photo = StageContract::derive(&upscaler(), Some(&catalog()));

        // photo → clip → 4K clip is a line.
        assert!(clip_to_clip.accepts.admits(photo_to_clip.produces));
        // Handing an image upscaler a clip is not.
        assert!(!photo_to_photo.accepts.admits(photo_to_clip.produces));
        // And a picture is what the i2v stage wants.
        assert!(photo_to_clip.accepts.admits(photo_to_photo.produces));

        // A stage that takes nothing can only be first.
        assert!(!Accepts::None.admits(MediaType::Image));
        assert!(Accepts::None.starts_a_line());
        assert!(!Accepts::Image.starts_a_line());

        // Text binds only into a stage that asked for it.
        let describing = StageContract::derive(&describe(), Some(&catalog()));
        assert_eq!(describing.produces, MediaType::Text);
        assert!(!photo_to_clip.accepts.admits(describing.produces));
        assert!(Accepts::Text.admits(MediaType::Text));
        assert_eq!(Accepts::from(MediaType::Video), Accepts::Video);
    }

    // === Storage ============================================================

    #[test]
    fn a_stored_contract_reads_back_exactly() {
        let c = StageContract::derive_with(
            &interpolator(),
            Some(&catalog()),
            ContractCorrections {
                accepts: Some(Accepts::Video),
                roles: [("14".to_string(), SourceRole::Start)]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&c).unwrap();
        let back: StageContract = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.version, CONTRACT_VERSION);
        // The corrections travel with it, so the next derivation keeps them.
        assert_eq!(back.corrections.accepts, Some(Accepts::Video));
    }

    #[test]
    fn a_contract_written_by_an_older_build_still_reads() {
        // Only the two types are required; everything else defaults, so adding
        // a field later needs no migration.
        let old: StageContract =
            serde_json::from_value(json!({ "accepts": "image", "produces": "video" })).unwrap();
        assert_eq!(old.accepts, Accepts::Image);
        assert_eq!(old.produces, MediaType::Video);
        assert_eq!(old.version, CONTRACT_VERSION);
        assert!(old.corrections.is_empty());
        assert!(old.roles.is_empty());
    }

    #[test]
    fn the_wire_names_are_the_ones_the_console_and_the_line_editor_read() {
        let c = StageContract::derive(&i2v(), Some(&catalog()));
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["accepts"], json!("image"));
        assert_eq!(v["produces"], json!("video"));
        assert_eq!(v["roles"][0]["role"], json!("start"));
        assert_eq!(v["roles"][0]["kind"], json!("image"));
        assert_eq!(v["params"][0]["name"], json!("seed"));
        // An uncorrected contract carries no corrections object at all.
        assert!(v.get("corrections").is_none());
    }
}
