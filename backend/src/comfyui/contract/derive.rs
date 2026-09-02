//! Working the contract out of a graph.
//!
//! Every function here is pure: a `serde_json::Value` and whatever ComfyUI
//! could be persuaded to say about its own nodes go in, and an answer comes
//! out. They are separate from the types in [`super`] because the types are
//! what the rest of Phos reads, and these are heuristics that will be revised.
//!
//! Each one is written to degrade rather than to fail. A catalogue that could
//! not be read, a class from a pack that is not installed, a graph that is not
//! a graph at all: none of those is an error, they are a less confident
//! contract with a warning on it.

use super::{ContractCorrections, ContractWarning, MediaType, ParamName, PromptSlot, StageParam};
use crate::comfyui::loaders::LoaderNode;
use crate::comfyui::nodes::{NodeCatalog, WidgetSpec};
use crate::comfyui::overrides::WorkflowInput;
use crate::comfyui::workflow::{detect_outputs, saver_kind, SaverKind};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Does this graph use a node class the server has never heard of?
pub(super) fn unknown_classes(workflow: &Value, catalog: &NodeCatalog) -> bool {
    let Some(nodes) = workflow.as_object() else {
        return false;
    };
    nodes.values().any(|node| {
        node.get("class_type")
            .and_then(|v| v.as_str())
            .is_some_and(|c| catalog.get(c).is_none())
    })
}

// ===== produces =============================================================

/// Every node that ends the graph.
///
/// [`detect_outputs`] finds savers by shape — `Save*`, `Preview*`, anything
/// carrying a `filename_prefix`. That is the right rule for *files*, which is
/// what it exists for, and it misses a terminal node that writes no file:
/// `ShowText|pysssss` is neither named like a saver nor has a prefix. ComfyUI
/// marks those itself, so the catalogue is asked as well.
fn contract_outputs(workflow: &Value, catalog: Option<&NodeCatalog>) -> Vec<(String, String)> {
    let mut seen: BTreeMap<String, String> = detect_outputs(workflow)
        .into_iter()
        .map(|o| (o.node_id, o.node_type))
        .collect();
    if let (Some(catalog), Some(nodes)) = (catalog, workflow.as_object()) {
        for (node_id, node) in nodes {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if catalog.get(class_type).is_some_and(|c| c.output_node) {
                seen.insert(node_id.clone(), class_type.to_string());
            }
        }
    }
    seen.into_iter().collect()
}

/// What one terminal node writes, or `None` when it is something a line cannot
/// carry (audio, today).
fn output_media(
    node_id: &str,
    node_type: &str,
    workflow: &Value,
    catalog: Option<&NodeCatalog>,
) -> Option<MediaType> {
    let wired = wired_link_types(node_id, node_type, workflow, catalog);
    // A saver that is handed anything media-shaped is a media saver, whatever
    // else is wired into it: plenty of custom savers also take a caption or the
    // compiled prompt as a `STRING`, and that socket says nothing about what
    // the node writes.
    let fed_media = ["IMAGE", "VIDEO", "LATENT", "AUDIO"]
        .iter()
        .any(|t| wired.contains(*t));

    // 1. Text first, because a `PreviewText` is named like nothing in
    //    particular and would otherwise fall through to "a picture". The
    //    catalogue is definitive here: a terminal node fed a `STRING` socket
    //    and nothing media-shaped emits a sentence, not a file.
    if !fed_media {
        if wired.contains("STRING") {
            return Some(MediaType::Text);
        }
        let t = node_type.to_ascii_lowercase();
        if t.contains("text") || t.contains("string") || t.contains("caption") {
            return Some(MediaType::Text);
        }
    }

    // 2. Moving pictures, by the rule the worker already trusts. It has to come
    //    before the catalogue: `VHS_VideoCombine` is handed `IMAGE` frames and
    //    writes an mp4, so what it consumes says nothing about what it writes.
    match saver_kind(node_type) {
        SaverKind::Video | SaverKind::Animated => return Some(MediaType::Video),
        SaverKind::Audio => return None,
        SaverKind::Image => {}
    }

    // 3. Otherwise take ComfyUI's word for what it was handed.
    if wired.contains("VIDEO") {
        return Some(MediaType::Video);
    }
    Some(MediaType::Image)
}

/// The declared types of the sockets this node actually has something wired
/// into. Empty whenever the catalogue has nothing to say about the class.
fn wired_link_types(
    node_id: &str,
    node_type: &str,
    workflow: &Value,
    catalog: Option<&NodeCatalog>,
) -> BTreeSet<String> {
    let mut types = BTreeSet::new();
    let Some(class) = catalog.and_then(|c| c.get(node_type)) else {
        return types;
    };
    let Some(inputs) = workflow
        .get(node_id)
        .and_then(|n| n.get("inputs"))
        .and_then(|i| i.as_object())
    else {
        return types;
    };
    for (field, value) in inputs {
        if !value.is_array() {
            continue;
        }
        if let Some(WidgetSpec::Link { data_type }) = class.input(field).map(|i| &i.widget) {
            types.insert(data_type.to_ascii_uppercase());
        }
    }
    types
}

pub(super) fn derive_produces(
    workflow: &Value,
    catalog: Option<&NodeCatalog>,
    warnings: &mut Vec<ContractWarning>,
) -> MediaType {
    let outputs = contract_outputs(workflow, catalog);
    if outputs.is_empty() {
        warnings.push(ContractWarning::NoOutputNode);
        return MediaType::Image;
    }

    let mut kinds: BTreeSet<MediaType> = BTreeSet::new();
    let mut unsupported = false;
    for (node_id, node_type) in &outputs {
        match output_media(node_id, node_type, workflow, catalog) {
            Some(kind) => {
                kinds.insert(kind);
            }
            None => unsupported = true,
        }
    }

    if kinds.len() > 1 {
        warnings.push(ContractWarning::MixedOutputs);
    }
    // A graph that makes something that moves makes a clip, whatever stills it
    // previewed on the way; a graph that writes a sentence and nothing moving
    // is a describe stage.
    if kinds.contains(&MediaType::Video) {
        MediaType::Video
    } else if kinds.contains(&MediaType::Text) {
        MediaType::Text
    } else if kinds.contains(&MediaType::Image) {
        MediaType::Image
    } else {
        if unsupported {
            warnings.push(ContractWarning::UnsupportedOutput);
        } else {
            warnings.push(ContractWarning::NoOutputNode);
        }
        MediaType::Image
    }
}

pub(super) fn derive_slots(
    workflow: &Value,
    inputs: &[WorkflowInput],
    loaders: &[LoaderNode],
    corrections: &ContractCorrections,
) -> Vec<PromptSlot> {
    let bound: BTreeSet<String> = loaders
        .iter()
        .map(|l| format!("{}.{}", l.node_id, l.field))
        .collect();

    let mut candidates: Vec<&WorkflowInput> = inputs
        .iter()
        .filter(|i| !bound.contains(&input_key(i)))
        .filter(|i| match &i.widget {
            // With a catalogue, ComfyUI says which fields are text boxes.
            Some(WidgetSpec::Text { .. }) => true,
            Some(_) => false,
            // Without one, the heuristics already only offer strings, and the
            // only non-prompt among them is the shot's own filename.
            None => i.current_value.is_string() && i.node_type != "LoadImage",
        })
        .collect();
    candidates.sort_by_key(|c| node_order(&c.node_id));

    // A correction can name a field the heuristics never offered — the whole
    // point of correcting one.
    for key in corrections.slots.keys() {
        if candidates.iter().any(|c| &input_key(c) == key) {
            continue;
        }
        if let Some(extra) = inputs.iter().find(|i| &input_key(i) == key) {
            candidates.push(extra);
        }
    }

    let conditioning = conditioning_sources(workflow);
    let mut named: Vec<PromptSlot> = Vec::new();
    let mut used: BTreeSet<String> = BTreeSet::new();

    for input in &candidates {
        let key = input_key(input);
        // An explicit correction wins over every heuristic, including "this is
        // not a prompt at all".
        if let Some(said) = corrections.slots.get(&key) {
            match said {
                Some(name) if !name.trim().is_empty() => {
                    named.push(slot(input, unique(name.trim(), &input.node_id, &mut used)));
                }
                _ => {}
            }
            continue;
        }

        let guess = input
            .node_title
            .as_deref()
            .and_then(name_from_title)
            .or_else(|| conditioning.get(&input.node_id).copied());
        if let Some(name) = guess {
            named.push(slot(input, unique(name, &input.node_id, &mut used)));
        }
    }

    // Whatever is left. A lone unnamed prompt box is the prompt; the rest are
    // named after their title, or after the node, so a compiler can address
    // them at all.
    for input in &candidates {
        let key = input_key(input);
        if corrections.slots.contains_key(&key) || named.iter().any(|s| s.override_key() == key) {
            continue;
        }
        let name = if !used.contains("positive") {
            "positive".to_string()
        } else {
            input
                .node_title
                .as_deref()
                .map(slugify)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("prompt_{}", input.node_id))
        };
        named.push(slot(input, unique(&name, &input.node_id, &mut used)));
    }

    named.sort_by_key(|s| node_order(&s.node_id));
    named
}

fn slot(input: &WorkflowInput, name: String) -> PromptSlot {
    PromptSlot {
        name,
        node_id: input.node_id.clone(),
        field: input.field_name.clone(),
        node_title: input.node_title.clone(),
        multiline: matches!(
            input.widget,
            Some(WidgetSpec::Text {
                multiline: true,
                ..
            })
        ),
        default: input.current_value.as_str().map(str::to_string),
    }
}

/// Two slots cannot share a name — a compiler binds by it.
///
/// The node id is the first tiebreak; a counter after that, because two fields
/// on the *same* node corrected to the same name would otherwise collide on
/// the very string meant to keep them apart.
fn unique(name: &str, node_id: &str, used: &mut BTreeSet<String>) -> String {
    let mut candidate = name.to_string();
    if used.contains(&candidate) {
        candidate = format!("{}_{}", name, node_id);
    }
    let mut n = 2;
    while used.contains(&candidate) {
        candidate = format!("{}_{}_{}", name, node_id, n);
        n += 1;
    }
    used.insert(candidate.clone());
    candidate
}

/// `positive` or `negative` when the author's title says so.
///
/// Whole words, for the reason [`SourceRole::from_title`] learned: a title is
/// free text and substring matching invents meaning that is not there.
fn name_from_title(title: &str) -> Option<&'static str> {
    for word in title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        match word.to_ascii_lowercase().as_str() {
            "negative" | "neg" => return Some("negative"),
            "positive" | "pos" => return Some("positive"),
            _ => {}
        }
    }
    None
}

/// How many hops back from a sampler's conditioning socket to look for the text
/// box feeding it. `KSampler.negative ← CLIPTextEncode` is one;
/// `← ConditioningZeroOut ← CLIPTextEncode` is two; three is generous.
const CONDITIONING_DEPTH: usize = 3;

/// `node_id` → `"positive"` / `"negative"`, worked out from the graph rather
/// than from titles.
///
/// Most workflows in circulation have two untitled `CLIPTextEncode` nodes, and
/// the only thing that tells them apart is which socket of the sampler each one
/// is wired into.
///
/// The nearest socket wins. A conditioning node in the middle — `WanImageToVideo`
/// takes both prompts and hands the sampler a bundle — means the negative prompt
/// is one hop from a `negative` socket and two hops from a `positive` one; the
/// close claim is the specific one. Only a genuine tie is ambiguous, and an
/// ambiguous node is left unnamed rather than guessed at.
fn conditioning_sources(workflow: &Value) -> BTreeMap<String, &'static str> {
    let Some(nodes) = workflow.as_object() else {
        return BTreeMap::new();
    };
    // node_id → (hops from the nearest conditioning socket, what it called it)
    let mut claims: BTreeMap<String, (usize, BTreeSet<&'static str>)> = BTreeMap::new();
    let mut claim = |id: String, depth: usize, name: &'static str| {
        let entry = claims.entry(id).or_insert((depth, BTreeSet::new()));
        match depth.cmp(&entry.0) {
            std::cmp::Ordering::Less => *entry = (depth, BTreeSet::from([name])),
            std::cmp::Ordering::Equal => {
                entry.1.insert(name);
            }
            std::cmp::Ordering::Greater => {}
        }
    };

    for node in nodes.values() {
        let Some(inputs) = node.get("inputs").and_then(|i| i.as_object()) else {
            continue;
        };
        for (field, value) in inputs {
            let name = match field.as_str() {
                "positive" => "positive",
                "negative" => "negative",
                _ => continue,
            };
            let Some(from) = link_target(value) else {
                continue;
            };
            // Walk back through whatever sits between the sampler and the text.
            let mut frontier = vec![from];
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for depth in 1..=CONDITIONING_DEPTH {
                let mut next = Vec::new();
                for id in frontier.drain(..) {
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    claim(id.clone(), depth, name);
                    if let Some(upstream) = nodes
                        .get(&id)
                        .and_then(|n| n.get("inputs"))
                        .and_then(|i| i.as_object())
                    {
                        next.extend(upstream.values().filter_map(link_target));
                    }
                }
                frontier = next;
                if frontier.is_empty() {
                    break;
                }
            }
        }
    }

    claims
        .into_iter()
        .filter_map(|(id, (_, names))| match names.len() {
            1 => names.into_iter().next().map(|n| (id, n)),
            // Equally close to both sockets: it says nothing about which it is.
            _ => None,
        })
        .collect()
}

/// The node id a `["6", 0]` link points at.
fn link_target(value: &Value) -> Option<String> {
    value.as_array()?.first()?.as_str().map(str::to_string)
}

fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().take(48) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

// ===== params ===============================================================

pub(super) fn derive_params(
    inputs: &[WorkflowInput],
    produces: MediaType,
    corrections: &ContractCorrections,
) -> Vec<StageParam> {
    let mut params: Vec<StageParam> = Vec::new();
    for input in inputs {
        let key = input_key(input);
        let name = match corrections.params.get(&key) {
            // A correction is the last word, "not one of ours" included.
            Some(said) => match said {
                Some(name) => Some(*name),
                None => continue,
            },
            None => canonical_param(input, produces),
        };
        let Some(name) = name else { continue };
        params.push(StageParam {
            name,
            node_id: input.node_id.clone(),
            field: input.field_name.clone(),
            widget: input.widget.clone(),
            current_value: input.current_value.clone(),
        });
    }
    params.sort_by(|a, b| {
        node_order(&a.node_id)
            .cmp(&node_order(&b.node_id))
            .then(a.name.cmp(&b.name))
    });
    params
}

/// Which canonical setting, if any, this field carries.
fn canonical_param(input: &WorkflowInput, produces: MediaType) -> Option<ParamName> {
    // A field ComfyUI marks `control_after_generate` is a seed whatever its
    // author called it.
    if matches!(input.widget, Some(WidgetSpec::Seed { .. })) {
        return Some(ParamName::Seed);
    }
    // Text and enums are prompts and model pickers, never settings a line
    // dials. Without a catalogue nothing is known, so fall through on name.
    if matches!(
        input.widget,
        Some(WidgetSpec::Text { .. }) | Some(WidgetSpec::Combo { .. })
    ) {
        return None;
    }

    let field = input.field_name.to_ascii_lowercase();
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
    if let Some(found) = ALL
        .iter()
        .copied()
        .find(|p| p.aliases().contains(&field.as_str()))
    {
        return Some(found);
    }
    // `length` is a frame count on a graph that makes something that moves, and
    // a batch dimension everywhere else.
    if field == "length" && produces == MediaType::Video {
        return Some(ParamName::Frames);
    }
    None
}

/// The `text_overrides` key that rewrites one of the graph's fields. The same
/// `"<node_id>.<field>"` a slot and a param are addressed by, because it is the
/// key `prepare_workflow` already substitutes on.
fn input_key(input: &WorkflowInput) -> String {
    format!("{}.{}", input.node_id, input.field_name)
}

/// Node ids are strings that are almost always numbers; `"10"` sorts before
/// `"9"` unless you say otherwise.
pub(super) fn node_order(id: &str) -> (u64, String) {
    (
        id.parse::<u64>().unwrap_or(u64::MAX),
        id.to_ascii_lowercase(),
    )
}
