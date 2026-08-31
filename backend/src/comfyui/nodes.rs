//! What ComfyUI says its node classes take: reading `/object_info`.
//!
//! Phos used to guess a node's widgets from substrings in its class name, which
//! is why only text was ever overridable. ComfyUI publishes the truth instead —
//! every input's name, type, default, range, and for an enum the *contents*,
//! meaning the checkpoints, samplers, schedulers and LoRAs installed on that
//! particular box.
//!
//! # Parse defensively
//!
//! `/object_info` is a large, loosely typed document. Its shape has drifted
//! across ComfyUI versions, and every custom node pack in the wild adds entries
//! written to whatever the author believed the schema was. So nothing here
//! returns an error: an entry that cannot be read is *skipped*, and a class with
//! no readable inputs is still a class. Callers get a catalogue that is
//! possibly incomplete but never wrong, and [`super::workflow::detect_inputs`]
//! falls back to its old heuristics for any class the catalogue does not know.
//!
//! # The cache
//!
//! Kept in memory, keyed by base URL. Three things end an entry's life: it
//! ages past [`CATALOG_TTL`]; the health check sees ComfyUI come back after
//! being down, which is exactly when the installed model list may have
//! changed; or a client asks for a fresh read outright (`refresh=true` on
//! `/api/comfyui/nodes`). Deliberately *not* a database table: a cache that
//! must be invalidated on reconnect does not want to outlive the process, and
//! skipping it keeps this change migration-free.

use super::client::ComfyUiClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How many enum entries a workflow's stored input snapshot keeps.
///
/// A combo listing every file in ComfyUI's input directory can run to
/// thousands; the snapshot lives in SQLite and is shipped with every workflow
/// list. Past this many the snapshot says `truncated` and the client asks
/// `/api/comfyui/nodes` for the live list instead.
pub(crate) const MAX_STORED_CHOICES: usize = 256;

/// One input of a node class, as ComfyUI describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WidgetSpec {
    /// A text box. `multiline` is ComfyUI's own flag, not a length guess.
    Text {
        multiline: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },
    Int {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<i64>,
    },
    /// An int ComfyUI marks with `control_after_generate` — a seed. Worth its
    /// own kind because the control for one is an int box *and* a re-roll.
    ///
    /// The bounds are [`WideInt`]s because ComfyUI declares a seed's `max` as
    /// `0xffffffffffffffff`, and a graph can carry a seed from the upper half
    /// of that range. Narrowing to `i64` would make the snapshot claim such a
    /// seed is out of range, and a client that validates against the range
    /// could then neither reproduce nor pick it.
    Seed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<WideInt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<WideInt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<WideInt>,
    },
    Float {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
    },
    Boolean {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<bool>,
    },
    /// An enum, and what is actually in it on this box.
    ///
    /// Choices are JSON scalars, not strings: ComfyUI enums are usually
    /// filenames or mode names, but a custom pack can enumerate numbers
    /// (`[512, 768, 1024]`) or booleans, and the graph then carries the value
    /// with that same type. Stringifying would leave a client unable to
    /// submit what the node actually accepts.
    Combo {
        choices: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<Value>,
        /// Set when `choices` was cut down to [`MAX_STORED_CHOICES`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    /// Not a widget at all — a socket another node wires into (`MODEL`,
    /// `LATENT`, a custom pack's own type). Recorded so the catalogue can
    /// describe a whole class, never offered as something to override.
    Link { data_type: String },
}

/// An integer wide enough for anything a JSON document can hold — every `i64`
/// and every `u64` — written back out as the plain integer it came from.
///
/// serde_json will *write* an `i128` but its text deserializer will not read
/// one, so the reading side is spelled out here: a JSON integer past `i64`
/// arrives as a `u64`, and both fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WideInt(pub i128);

impl Serialize for WideInt {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i128(self.0)
    }
}

impl<'de> Deserialize<'de> for WideInt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = WideInt;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an integer")
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<WideInt, E> {
                Ok(WideInt(v as i128))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<WideInt, E> {
                Ok(WideInt(v as i128))
            }
            fn visit_i128<E: serde::de::Error>(self, v: i128) -> Result<WideInt, E> {
                Ok(WideInt(v))
            }
            fn visit_u128<E: serde::de::Error>(self, v: u128) -> Result<WideInt, E> {
                i128::try_from(v)
                    .map(WideInt)
                    .map_err(|_| E::custom("integer out of range"))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<WideInt, E> {
                if v.is_finite() {
                    Ok(WideInt(v as i128))
                } else {
                    Err(E::custom("not a finite number"))
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl WidgetSpec {
    /// Can a person type a value into this? Sockets cannot be overridden.
    pub fn is_widget(&self) -> bool {
        !matches!(self, WidgetSpec::Link { .. })
    }

    /// A copy fit to store beside a workflow: long enum lists are cut short.
    pub(crate) fn capped(&self, max: usize) -> WidgetSpec {
        match self {
            WidgetSpec::Combo {
                choices, default, ..
            } if choices.len() > max => WidgetSpec::Combo {
                choices: choices[..max].to_vec(),
                default: default.clone(),
                truncated: true,
            },
            other => other.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInput {
    pub name: String,
    pub required: bool,
    pub widget: WidgetSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeClass {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub output_node: bool,
    /// In ComfyUI's declared order, required inputs first.
    pub inputs: Vec<NodeInput>,
}

impl NodeClass {
    pub fn input(&self, name: &str) -> Option<&NodeInput> {
        self.inputs.iter().find(|i| i.name == name)
    }
}

/// Every node class one ComfyUI install knows about.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeCatalog {
    pub classes: BTreeMap<String, NodeClass>,
}

impl NodeCatalog {
    pub fn get(&self, class_type: &str) -> Option<&NodeClass> {
        self.classes.get(class_type)
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

// ===== Parsing ==============================================================

/// Turn a whole `/object_info` document into a catalogue.
///
/// Anything unreadable — a wrong top-level type, an entry that is not an
/// object, an input spec from a schema nobody has seen — is skipped rather
/// than fatal.
pub fn parse_object_info(doc: &Value) -> NodeCatalog {
    let mut classes = BTreeMap::new();
    let Some(obj) = doc.as_object() else {
        return NodeCatalog { classes };
    };
    for (name, entry) in obj {
        if let Some(class) = parse_node_class(name, entry) {
            classes.insert(name.clone(), class);
        }
    }
    NodeCatalog { classes }
}

/// One `/object_info` entry. `None` only when the entry is not an object at
/// all; a class whose inputs are all unreadable still exists, it just has none.
pub fn parse_node_class(name: &str, entry: &Value) -> Option<NodeClass> {
    let obj = entry.as_object()?;

    let mut inputs = Vec::new();
    if let Some(input) = obj.get("input").and_then(|v| v.as_object()) {
        // `hidden` is ComfyUI's own plumbing (prompt, unique_id) — never a
        // control. `required` before `optional` matches how ComfyUI draws it.
        for (section, required) in [("required", true), ("optional", false)] {
            let Some(fields) = input.get(section).and_then(|v| v.as_object()) else {
                continue;
            };
            let order = input_order(obj, section);
            let mut named: Vec<&String> = fields.keys().collect();
            if let Some(order) = order.as_ref() {
                named.sort_by_key(|n| order.iter().position(|o| o == *n).unwrap_or(usize::MAX));
            }
            for field in named {
                let spec = &fields[field];
                if let Some(widget) = parse_widget(field, spec) {
                    inputs.push(NodeInput {
                        name: field.clone(),
                        required,
                        widget,
                        tooltip: options_of(spec)
                            .and_then(|o| o.get("tooltip"))
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    });
                }
            }
        }
    }

    Some(NodeClass {
        name: obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .to_string(),
        display_name: obj
            .get("display_name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        category: obj
            .get("category")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        output_node: obj
            .get("output_node")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        inputs,
    })
}

/// `input_order` arrived in a later ComfyUI; without it the field order is
/// whatever the JSON map gives, which is stable but alphabetical.
fn input_order(entry: &serde_json::Map<String, Value>, section: &str) -> Option<Vec<String>> {
    let arr = entry.get("input_order")?.get(section)?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
    )
}

/// The `{default, min, max, ...}` map that follows the type in a spec.
fn options_of(spec: &Value) -> Option<&serde_json::Map<String, Value>> {
    match spec {
        // `["INT", { .. }]`
        Value::Array(items) => items.get(1).and_then(|v| v.as_object()),
        // A dict-shaped spec is its own options bag.
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Read one input's spec.
///
/// Two shapes are in the wild. The classic one is a tuple: `["INT", {..}]` for
/// a widget, `["MODEL"]` for a socket, `[["euler", "heun"], {..}]` for an enum.
/// Newer/V3-style packs write a dict instead: `{"type": "INT", "default": 0}`.
/// Both are read; anything else is skipped.
fn parse_widget(field: &str, spec: &Value) -> Option<WidgetSpec> {
    let opts = options_of(spec);
    let type_slot = match spec {
        Value::Array(items) => items.first()?,
        Value::Object(map) => map.get("type")?,
        _ => return None,
    };

    // `[["a", "b"], {..}]` — the enum's contents inline.
    if let Some(choices) = type_slot.as_array() {
        return Some(combo(choices, opts));
    }

    let type_name = type_slot.as_str()?;
    Some(match type_name {
        "INT" => {
            // `control_after_generate` is ComfyUI's own marker for a seed. Older
            // servers did not send it, and their frontend went by the name, so
            // accept both rather than losing the seed control on an old box.
            let is_seed = opts
                .and_then(|o| o.get("control_after_generate"))
                .is_some_and(|v| v.as_bool().unwrap_or(true))
                || field == "seed"
                || field == "noise_seed";
            if is_seed {
                WidgetSpec::Seed {
                    default: opts.and_then(|o| as_wide_int(o.get("default"))),
                    min: opts.and_then(|o| as_wide_int(o.get("min"))),
                    max: opts.and_then(|o| as_wide_int(o.get("max"))),
                }
            } else {
                WidgetSpec::Int {
                    default: opts.and_then(|o| as_int(o.get("default"))),
                    min: opts.and_then(|o| as_int(o.get("min"))),
                    max: opts.and_then(|o| as_int(o.get("max"))),
                    step: opts.and_then(|o| as_int(o.get("step"))),
                }
            }
        }
        "FLOAT" => WidgetSpec::Float {
            default: opts.and_then(|o| as_float(o.get("default"))),
            min: opts.and_then(|o| as_float(o.get("min"))),
            max: opts.and_then(|o| as_float(o.get("max"))),
            step: opts.and_then(|o| as_float(o.get("step"))),
        },
        "STRING" => WidgetSpec::Text {
            multiline: opts
                .and_then(|o| o.get("multiline"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            default: opts
                .and_then(|o| o.get("default"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
        },
        "BOOLEAN" => WidgetSpec::Boolean {
            default: opts
                .and_then(|o| o.get("default"))
                .and_then(|v| v.as_bool()),
        },
        // V3 spells an enum out instead of inlining it.
        "COMBO" => {
            let choices = opts
                .and_then(|o| o.get("options").or_else(|| o.get("choices")))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            combo(&choices, opts)
        }
        // `MODEL`, `CLIP`, `LATENT`, or whatever a custom pack invented: a
        // socket, not something to type into.
        other => WidgetSpec::Link {
            data_type: other.to_string(),
        },
    })
}

/// A value a combo can hold: a string, a number or a boolean, kept with its
/// JSON type. Objects, arrays and nulls are not enum members.
fn is_scalar(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

fn combo(choices: &[Value], opts: Option<&serde_json::Map<String, Value>>) -> WidgetSpec {
    let choices: Vec<Value> = choices.iter().filter(|v| is_scalar(v)).cloned().collect();
    let default = opts
        .and_then(|o| o.get("default"))
        .filter(|v| is_scalar(v))
        .cloned()
        // ComfyUI defaults an enum to its first entry.
        .or_else(|| choices.first().cloned());
    WidgetSpec::Combo {
        choices,
        default,
        truncated: false,
    }
}

/// An integer from JSON as a [`WideInt`], so nothing ComfyUI can declare is
/// narrowed: JSON integers are at most a `u64` or an `i64`, and both fit.
/// Used for seeds, whose `max` is `0xffffffffffffffff`.
fn as_wide_int(v: Option<&Value>) -> Option<WideInt> {
    let v = v?;
    if let Some(i) = v.as_i64() {
        return Some(WideInt(i as i128));
    }
    if let Some(u) = v.as_u64() {
        return Some(WideInt(u as i128));
    }
    let f = v.as_f64()?;
    if !f.is_finite() {
        return None;
    }
    Some(WideInt(f as i128))
}

/// An integer from JSON as an `i64`, for ordinary ints. A value past the
/// `i64` range is clamped rather than dropped, so a pack that declares a huge
/// `max` still gets a bounded control instead of none.
fn as_int(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(u) = v.as_u64() {
        return Some(u.min(i64::MAX as u64) as i64);
    }
    let f = v.as_f64()?;
    if !f.is_finite() {
        return None;
    }
    Some(f.clamp(i64::MIN as f64, i64::MAX as f64) as i64)
}

fn as_float(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().filter(|f| f.is_finite())
}

// ===== Cache ================================================================

/// How long a cached catalogue is trusted before it is read again.
///
/// The reconnect rule below catches a restart, but a person can drop a new
/// checkpoint or LoRA into ComfyUI's model directory while it stays up, and
/// ComfyUI lists it on the next `/object_info` without restarting. Without an
/// expiry that model would be missing from every import for the life of this
/// process. Five minutes is long enough that back-to-back imports share one
/// read of a document that can run to megabytes, and short enough that a
/// model installed before an import is there for it.
pub(crate) const CATALOG_TTL: Duration = Duration::from_secs(5 * 60);

struct Cached {
    fetched_at: Instant,
    catalog: Arc<NodeCatalog>,
}

impl Cached {
    fn is_fresh(&self) -> bool {
        self.fetched_at.elapsed() < CATALOG_TTL
    }
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn last_health() -> &'static Mutex<HashMap<String, bool>> {
    static HEALTH: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    HEALTH.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The catalogue for this server, read from ComfyUI and remembered for
/// [`CATALOG_TTL`].
///
/// `None` means "ask ComfyUI later": unreachable, or an answer nothing could be
/// read out of. A failure is deliberately *not* cached, so a box that comes
/// back up is picked up on the next import without waiting for a health check.
/// Past the TTL the document is read again; if that read fails, the copy
/// already held is returned rather than nothing — an older list of models is
/// still a better basis for an import than the name-sniffing fallback.
pub(crate) fn catalog_for(client: &ComfyUiClient) -> Option<Arc<NodeCatalog>> {
    let key = client.base_url().to_string();
    let stale = match cache()
        .lock()
        .ok()
        .and_then(|c| c.get(&key).map(|hit| (hit.is_fresh(), hit.catalog.clone())))
    {
        Some((true, hit)) => return Some(hit),
        Some((false, hit)) => Some(hit),
        None => None,
    };
    match fetch(client) {
        Some(catalog) => Some(catalog),
        None => stale,
    }
}

/// The catalogue for this server, read from ComfyUI *now* whatever the cache
/// holds. For a client that knows the models just changed.
pub(crate) fn refresh_for(client: &ComfyUiClient) -> Option<Arc<NodeCatalog>> {
    invalidate(client.base_url());
    fetch(client)
}

/// One read of `/object_info`, remembered if it parsed to anything.
fn fetch(client: &ComfyUiClient) -> Option<Arc<NodeCatalog>> {
    let key = client.base_url().to_string();
    let doc = match client.object_info() {
        Ok(doc) => doc,
        Err(e) => {
            // Older ComfyUI builds answer 404 here, and an unreachable one
            // answers nothing. Both are ordinary: the caller falls back.
            tracing::debug!("ComfyUI /object_info unavailable at {}: {}", key, e);
            return None;
        }
    };
    let catalog = Arc::new(parse_object_info(&doc));
    if catalog.is_empty() {
        tracing::warn!("ComfyUI /object_info at {} parsed to no node classes", key);
        return None;
    }
    if let Ok(mut c) = cache().lock() {
        c.insert(
            key,
            Cached {
                fetched_at: Instant::now(),
                catalog: catalog.clone(),
            },
        );
    }
    Some(catalog)
}

/// Record what the health check saw, and drop the cache when ComfyUI has come
/// back from being down.
///
/// A restart is the moment the installed checkpoints and LoRAs may have
/// changed, so a catalogue read before it is not to be trusted. The transition
/// is what matters — a run of successful checks re-fetches nothing.
pub(crate) fn observe_health(base_url: &str, healthy: bool) {
    let Ok(mut seen) = last_health().lock() else {
        return;
    };
    let was = seen.insert(base_url.to_string(), healthy);
    if healthy && was == Some(false) {
        drop(seen);
        tracing::info!("ComfyUI at {} is back; re-reading /object_info", base_url);
        invalidate(base_url);
    }
}

pub(crate) fn invalidate(base_url: &str) {
    if let Ok(mut c) = cache().lock() {
        c.remove(base_url);
    }
}

#[cfg(test)]
pub(crate) fn remember_for_test(base_url: &str, catalog: NodeCatalog) {
    cache().lock().unwrap().insert(
        base_url.to_string(),
        Cached {
            fetched_at: Instant::now(),
            catalog: Arc::new(catalog),
        },
    );
}

/// Pretend the cached entry was read `age` ago.
#[cfg(test)]
pub(crate) fn backdate_for_test(base_url: &str, age: Duration) {
    if let Some(hit) = cache().lock().unwrap().get_mut(base_url) {
        hit.fetched_at = Instant::now() - age;
    }
}

#[cfg(test)]
pub(crate) fn cached_for_test(base_url: &str) -> Option<Arc<NodeCatalog>> {
    cache()
        .lock()
        .unwrap()
        .get(base_url)
        .map(|hit| hit.catalog.clone())
}

#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::{json, Value};

    /// A recorded-shape `/object_info` covering the cases that matter: a core
    /// sampler (ints, floats, seed, two enums, sockets), a loader whose enum is
    /// the checkpoints on the box, a text encoder, an image loader, a boolean,
    /// and a LoRA picker.
    pub fn object_info() -> Value {
        json!({
            "KSampler": {
                "input": {
                    "required": {
                        "model": ["MODEL", { "tooltip": "The model used for denoising." }],
                        "seed": ["INT", { "default": 0, "min": 0,
                                          "max": 18446744073709551615u64,
                                          "control_after_generate": true }],
                        "steps": ["INT", { "default": 20, "min": 1, "max": 10000 }],
                        "cfg": ["FLOAT", { "default": 8.0, "min": 0.0, "max": 100.0,
                                           "step": 0.1, "round": 0.01 }],
                        "sampler_name": [["euler", "euler_ancestral", "dpmpp_2m"]],
                        "scheduler": [["normal", "karras", "exponential"],
                                      { "default": "karras" }],
                        "positive": ["CONDITIONING"],
                        "negative": ["CONDITIONING"],
                        "latent_image": ["LATENT"],
                        "denoise": ["FLOAT", { "default": 1.0, "min": 0.0, "max": 1.0,
                                               "step": 0.01 }]
                    }
                },
                "input_order": {
                    "required": ["model", "seed", "steps", "cfg", "sampler_name",
                                 "scheduler", "positive", "negative", "latent_image",
                                 "denoise"]
                },
                "output": ["LATENT"],
                "output_name": ["LATENT"],
                "name": "KSampler",
                "display_name": "KSampler",
                "description": "Uses the provided model to denoise the latent.",
                "python_module": "nodes",
                "category": "sampling",
                "output_node": false
            },
            "CheckpointLoaderSimple": {
                "input": {
                    "required": {
                        "ckpt_name": [["sd_xl_base_1.0.safetensors",
                                       "v1-5-pruned-emaonly.ckpt"],
                                      { "tooltip": "The name of the checkpoint." }]
                    }
                },
                "output": ["MODEL", "CLIP", "VAE"],
                "name": "CheckpointLoaderSimple",
                "display_name": "Load Checkpoint",
                "category": "loaders",
                "output_node": false
            },
            "LoraLoaderModelOnly": {
                "input": {
                    "required": {
                        "model": ["MODEL"],
                        "lora_name": [["add_detail.safetensors", "film_grain.safetensors"]],
                        "strength_model": ["FLOAT", { "default": 1.0, "min": -100.0,
                                                      "max": 100.0, "step": 0.01 }]
                    }
                },
                "output": ["MODEL"],
                "name": "LoraLoaderModelOnly",
                "display_name": "LoraLoaderModelOnly",
                "category": "loaders",
                "output_node": false
            },
            "CLIPTextEncode": {
                "input": {
                    "required": {
                        "text": ["STRING", { "multiline": true, "dynamicPrompts": true,
                                             "tooltip": "The text to be encoded." }],
                        "clip": ["CLIP"]
                    }
                },
                "output": ["CONDITIONING"],
                "name": "CLIPTextEncode",
                "display_name": "CLIP Text Encode (Prompt)",
                "category": "conditioning",
                "output_node": false
            },
            "LoadImage": {
                "input": {
                    "required": {
                        "image": [["one.png", "two.jpg", "three.webp"],
                                  { "image_upload": true }]
                    }
                },
                "output": ["IMAGE", "MASK"],
                "name": "LoadImage",
                "display_name": "Load Image",
                "category": "image",
                "output_node": false
            },
            "SaveImage": {
                "input": {
                    "required": {
                        "images": ["IMAGE"],
                        "filename_prefix": ["STRING", { "default": "ComfyUI" }]
                    },
                    "hidden": { "prompt": "PROMPT", "extra_pnginfo": "EXTRA_PNGINFO" }
                },
                "output": [],
                "name": "SaveImage",
                "display_name": "Save Image",
                "category": "image",
                "output_node": true
            },
            "EmptyLatentImage": {
                "input": {
                    "required": {
                        "width": ["INT", { "default": 512, "min": 16, "max": 16384,
                                           "step": 8 }],
                        "height": ["INT", { "default": 512, "min": 16, "max": 16384,
                                            "step": 8 }],
                        "batch_size": ["INT", { "default": 1, "min": 1, "max": 4096 }]
                    }
                },
                "output": ["LATENT"],
                "name": "EmptyLatentImage",
                "display_name": "Empty Latent Image",
                "category": "latent",
                "output_node": false
            },
            "SamplerCustom": {
                "input": {
                    "required": {
                        "model": ["MODEL"],
                        "add_noise": ["BOOLEAN", { "default": true, "label_on": "enable",
                                                   "label_off": "disable" }],
                        "noise_seed": ["INT", { "default": 0, "min": 0,
                                                "max": 18446744073709551615u64 }]
                    }
                },
                "output": ["LATENT", "LATENT"],
                "name": "SamplerCustom",
                "display_name": "SamplerCustom",
                "category": "sampling/custom_sampling",
                "output_node": false
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::object_info;
    use super::*;
    use serde_json::json;

    fn widget(catalog: &NodeCatalog, class: &str, field: &str) -> WidgetSpec {
        catalog
            .get(class)
            .unwrap_or_else(|| panic!("no class {}", class))
            .input(field)
            .unwrap_or_else(|| panic!("no input {}.{}", class, field))
            .widget
            .clone()
    }

    // === A core node ========================================================

    #[test]
    fn a_core_sampler_yields_every_widget_type_at_once() {
        let catalog = parse_object_info(&object_info());

        assert_eq!(
            widget(&catalog, "KSampler", "steps"),
            WidgetSpec::Int {
                default: Some(20),
                min: Some(1),
                max: Some(10000),
                step: None,
            }
        );
        assert_eq!(
            widget(&catalog, "KSampler", "cfg"),
            WidgetSpec::Float {
                default: Some(8.0),
                min: Some(0.0),
                max: Some(100.0),
                step: Some(0.1),
            }
        );
        assert_eq!(
            widget(&catalog, "KSampler", "sampler_name"),
            WidgetSpec::Combo {
                choices: vec!["euler".into(), "euler_ancestral".into(), "dpmpp_2m".into()],
                // ComfyUI defaults an enum to its first entry.
                default: Some("euler".into()),
                truncated: false,
            }
        );
        // An explicit default wins over the first entry.
        assert_eq!(
            widget(&catalog, "KSampler", "scheduler"),
            WidgetSpec::Combo {
                choices: vec!["normal".into(), "karras".into(), "exponential".into()],
                default: Some("karras".into()),
                truncated: false,
            }
        );
        assert_eq!(
            widget(&catalog, "CLIPTextEncode", "text"),
            WidgetSpec::Text {
                multiline: true,
                default: None,
            }
        );
        assert_eq!(
            widget(&catalog, "SamplerCustom", "add_noise"),
            WidgetSpec::Boolean {
                default: Some(true)
            }
        );

        // Sockets are recorded but are not controls.
        assert_eq!(
            widget(&catalog, "KSampler", "model"),
            WidgetSpec::Link {
                data_type: "MODEL".into()
            }
        );
        assert!(!widget(&catalog, "KSampler", "model").is_widget());
        assert!(widget(&catalog, "KSampler", "steps").is_widget());

        // Metadata the console can label a control with.
        let ks = catalog.get("KSampler").unwrap();
        assert_eq!(ks.display_name.as_deref(), Some("KSampler"));
        assert_eq!(ks.category.as_deref(), Some("sampling"));
        assert!(!ks.output_node);
        assert!(catalog.get("SaveImage").unwrap().output_node);
        assert_eq!(
            ks.input("model").unwrap().tooltip.as_deref(),
            Some("The model used for denoising.")
        );
    }

    #[test]
    fn input_order_is_honoured_rather_than_the_maps_alphabet() {
        let catalog = parse_object_info(&object_info());
        let names: Vec<&str> = catalog
            .get("KSampler")
            .unwrap()
            .inputs
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "model",
                "seed",
                "steps",
                "cfg",
                "sampler_name",
                "scheduler",
                "positive",
                "negative",
                "latent_image",
                "denoise"
            ]
        );
    }

    // === Seeds ==============================================================

    #[test]
    fn a_seed_is_its_own_kind_and_keeps_a_range_wider_than_an_i64() {
        let catalog = parse_object_info(&object_info());
        // Marked by ComfyUI with control_after_generate.
        assert_eq!(
            widget(&catalog, "KSampler", "seed"),
            WidgetSpec::Seed {
                default: Some(WideInt(0)),
                min: Some(WideInt(0)),
                // 0xffffffffffffffff does not fit an i64. It must be kept as
                // declared, not clamped: a graph can carry a seed from the
                // upper half of that range, and a snapshot claiming it is out
                // of range would stop a client reproducing it.
                max: Some(WideInt(u64::MAX as i128)),
            }
        );
        // An older server sends no marker, and its own frontend went by name.
        assert_eq!(
            widget(&catalog, "SamplerCustom", "noise_seed"),
            WidgetSpec::Seed {
                default: Some(WideInt(0)),
                min: Some(WideInt(0)),
                max: Some(WideInt(u64::MAX as i128)),
            }
        );
    }

    #[test]
    fn a_seed_from_the_upper_half_of_the_u64_range_survives_the_wire() {
        // What ComfyUI declares, what a graph can hold, and what a snapshot
        // in SQLite must give back — all the same number.
        let spec = WidgetSpec::Seed {
            default: Some(WideInt(u64::MAX as i128 - 1)),
            min: Some(WideInt(0)),
            max: Some(WideInt(u64::MAX as i128)),
        };
        let text = serde_json::to_string(&spec).unwrap();
        assert!(
            text.contains("18446744073709551615"),
            "max must be written as the plain integer ComfyUI declared: {}",
            text
        );
        assert_eq!(serde_json::from_str::<WidgetSpec>(&text).unwrap(), spec);
        // `to_value` is the path the API takes; it must not refuse the number.
        let value = serde_json::to_value(&spec).unwrap();
        assert_eq!(value["max"].as_u64(), Some(u64::MAX));
        // A seed field a custom pack allows to go negative (-1 for "random")
        // is not lost either.
        let doc = json!({ "X": { "input": { "required": {
            "seed": ["INT", { "default": -1, "min": -1, "max": 18446744073709551615u64 }]
        } } } });
        assert_eq!(
            widget(&parse_object_info(&doc), "X", "seed"),
            WidgetSpec::Seed {
                default: Some(WideInt(-1)),
                min: Some(WideInt(-1)),
                max: Some(WideInt(u64::MAX as i128))
            }
        );
    }

    // === An enum of model filenames =========================================

    #[test]
    fn an_enum_carries_the_models_installed_on_this_box() {
        let catalog = parse_object_info(&object_info());
        let WidgetSpec::Combo { choices, .. } =
            widget(&catalog, "CheckpointLoaderSimple", "ckpt_name")
        else {
            panic!("ckpt_name should be an enum");
        };
        assert_eq!(
            choices,
            [
                json!("sd_xl_base_1.0.safetensors"),
                json!("v1-5-pruned-emaonly.ckpt")
            ]
        );

        let WidgetSpec::Combo { choices, .. } =
            widget(&catalog, "LoraLoaderModelOnly", "lora_name")
        else {
            panic!("lora_name should be an enum");
        };
        assert_eq!(
            choices,
            [
                json!("add_detail.safetensors"),
                json!("film_grain.safetensors")
            ]
        );
    }

    #[test]
    fn an_enum_of_numbers_or_booleans_keeps_their_types() {
        // A custom pack enumerating resolutions, and one enumerating a flag.
        // The graph carries `1024` and `true`, not `"1024"` and `"true"`, and
        // a client must be able to submit the same thing back.
        let doc = json!({ "Sizes": { "input": { "required": {
            "size": [[512, 768, 1024], { "default": 1024 }],
            "flag": [[true, false]],
            "mixed": [["a", 1, null, ["nested"], { "o": 1 }]],
            "odd_default": [["a", "b"], { "default": ["not", "a", "scalar"] }]
        } } } });
        let catalog = parse_object_info(&doc);
        assert_eq!(
            widget(&catalog, "Sizes", "size"),
            WidgetSpec::Combo {
                choices: vec![json!(512), json!(768), json!(1024)],
                default: Some(json!(1024)),
                truncated: false,
            }
        );
        assert_eq!(
            widget(&catalog, "Sizes", "flag"),
            WidgetSpec::Combo {
                choices: vec![json!(true), json!(false)],
                default: Some(json!(true)),
                truncated: false,
            }
        );
        // Only scalars are enum members; the rest is dropped, not stringified.
        assert_eq!(
            widget(&catalog, "Sizes", "mixed"),
            WidgetSpec::Combo {
                choices: vec![json!("a"), json!(1)],
                default: Some(json!("a")),
                truncated: false,
            }
        );
        // A default that is not a scalar is not a default.
        assert_eq!(
            widget(&catalog, "Sizes", "odd_default"),
            WidgetSpec::Combo {
                choices: vec![json!("a"), json!("b")],
                default: Some(json!("a")),
                truncated: false,
            }
        );
    }

    #[test]
    fn a_huge_enum_is_cut_down_for_storage_and_says_so() {
        let many: Vec<Value> = (0..1000)
            .map(|i| json!(format!("file_{}.png", i)))
            .collect();
        let full = WidgetSpec::Combo {
            choices: many,
            default: Some(json!("file_0.png")),
            truncated: false,
        };
        let WidgetSpec::Combo {
            choices, truncated, ..
        } = full.capped(MAX_STORED_CHOICES)
        else {
            panic!("still an enum");
        };
        assert_eq!(choices.len(), MAX_STORED_CHOICES);
        assert!(truncated);

        // A short one is left exactly as it was.
        let short = WidgetSpec::Combo {
            choices: vec![json!("a")],
            default: None,
            truncated: false,
        };
        assert_eq!(short.capped(MAX_STORED_CHOICES), short);
    }

    // === Malformed and unknown ==============================================

    #[test]
    fn a_malformed_entry_is_skipped_and_never_fatal() {
        let doc = json!({
            "Fine": { "input": { "required": { "n": ["INT", { "default": 3 }] } } },
            // Not an object at all.
            "AString": "nonsense",
            "ANumber": 42,
            "AnArray": [1, 2, 3],
            "ANull": null,
            // An object, but nothing recognisable inside it.
            "NoInput": { "category": "weird" },
            "InputIsAString": { "input": "required" },
            "RequiredIsAnArray": { "input": { "required": ["a", "b"] } },
            "SpecsAreJunk": { "input": { "required": {
                "a": 5, "b": null, "c": [], "d": "STRING", "e": [[]]
            } } }
        });
        let catalog = parse_object_info(&doc);

        // The good entry survived.
        assert_eq!(
            widget(&catalog, "Fine", "n"),
            WidgetSpec::Int {
                default: Some(3),
                min: None,
                max: None,
                step: None
            }
        );
        // Non-objects are dropped entirely.
        for missing in ["AString", "ANumber", "AnArray", "ANull"] {
            assert!(catalog.get(missing).is_none(), "{} should be gone", missing);
        }
        // Objects survive as classes with nothing to offer.
        for empty in ["NoInput", "InputIsAString", "RequiredIsAnArray"] {
            assert!(
                catalog.get(empty).unwrap().inputs.is_empty(),
                "{} should have no inputs",
                empty
            );
        }
        // Field specs that make no sense are dropped one by one; the one that
        // does (an empty enum) survives.
        let junk = catalog.get("SpecsAreJunk").unwrap();
        let names: Vec<&str> = junk.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["e"]);
    }

    #[test]
    fn a_top_level_document_of_the_wrong_shape_yields_an_empty_catalogue() {
        for wrong in [json!([]), json!("nope"), json!(null), json!(7)] {
            assert!(parse_object_info(&wrong).is_empty(), "{:?}", wrong);
        }
    }

    // === A differently shaped version =======================================

    #[test]
    fn an_older_server_without_input_order_or_metadata_still_parses() {
        // 2023-era ComfyUI: no `input_order`, no `description`, no
        // `python_module`, no `output_node`, no options bag on some inputs.
        let doc = json!({
            "KSampler": {
                "input": { "required": {
                    "seed": ["INT", { "default": 0, "min": 0, "max": 18446744073709551615u64 }],
                    "steps": ["INT", { "default": 20, "min": 1, "max": 10000 }],
                    "sampler_name": [["euler", "ddim"]],
                    "model": ["MODEL"]
                } },
                "output": ["LATENT"],
                "category": "sampling"
            }
        });
        let catalog = parse_object_info(&doc);
        let ks = catalog.get("KSampler").unwrap();
        // Falls back to the map key when `name` is absent.
        assert_eq!(ks.name, "KSampler");
        assert_eq!(ks.display_name, None);
        assert!(!ks.output_node);
        // Alphabetical, because the server did not say otherwise — but present.
        let names: Vec<&str> = ks.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["model", "sampler_name", "seed", "steps"]);
        assert!(matches!(
            widget(&catalog, "KSampler", "seed"),
            WidgetSpec::Seed { .. }
        ));
    }

    #[test]
    fn a_dict_shaped_spec_from_a_newer_schema_is_read_too() {
        // Some packs (and ComfyUI's V3 schema) write the spec as an object.
        let doc = json!({
            "NewStyle": { "input": { "required": {
                "steps": { "type": "INT", "default": 25, "min": 1, "max": 150 },
                "prompt": { "type": "STRING", "multiline": true },
                "mode": { "type": "COMBO", "options": ["fast", "slow"], "default": "slow" },
                "model": { "type": "MODEL" }
            } } }
        });
        let catalog = parse_object_info(&doc);
        assert_eq!(
            widget(&catalog, "NewStyle", "steps"),
            WidgetSpec::Int {
                default: Some(25),
                min: Some(1),
                max: Some(150),
                step: None
            }
        );
        assert_eq!(
            widget(&catalog, "NewStyle", "prompt"),
            WidgetSpec::Text {
                multiline: true,
                default: None
            }
        );
        assert_eq!(
            widget(&catalog, "NewStyle", "mode"),
            WidgetSpec::Combo {
                choices: vec!["fast".into(), "slow".into()],
                default: Some("slow".into()),
                truncated: false,
            }
        );
        assert!(!widget(&catalog, "NewStyle", "model").is_widget());
    }

    #[test]
    fn an_unknown_socket_type_from_a_custom_pack_is_a_link_not_a_widget() {
        let doc = json!({
            "WanVideoSampler": { "input": { "required": {
                "wan_model": ["WANVIDEOMODEL"],
                "frame_count": ["INT", { "default": 81, "min": 1, "max": 1000 }]
            } } }
        });
        let catalog = parse_object_info(&doc);
        assert_eq!(
            widget(&catalog, "WanVideoSampler", "wan_model"),
            WidgetSpec::Link {
                data_type: "WANVIDEOMODEL".into()
            }
        );
        assert_eq!(
            widget(&catalog, "WanVideoSampler", "frame_count"),
            WidgetSpec::Int {
                default: Some(81),
                min: Some(1),
                max: Some(1000),
                step: None
            }
        );
    }

    #[test]
    fn optional_inputs_come_after_required_ones_and_are_marked() {
        let doc = json!({
            "N": {
                "input": {
                    "required": { "a": ["INT", { "default": 1 }] },
                    "optional": { "b": ["INT", { "default": 2 }] },
                    "hidden": { "prompt": "PROMPT" }
                }
            }
        });
        let n = parse_object_info(&doc);
        let n = n.get("N").unwrap();
        assert_eq!(n.inputs.len(), 2, "hidden plumbing is not a control");
        assert_eq!(
            (n.inputs[0].name.as_str(), n.inputs[0].required),
            ("a", true)
        );
        assert_eq!(
            (n.inputs[1].name.as_str(), n.inputs[1].required),
            ("b", false)
        );
    }

    // === Cache ==============================================================

    #[test]
    fn the_cache_is_dropped_when_comfyui_comes_back_from_being_down() {
        let url = "http://cache-test.invalid:8188";
        remember_for_test(url, parse_object_info(&object_info()));
        assert!(cached_for_test(url).is_some());

        // A first sighting is not a reconnect, whatever it says.
        observe_health(url, true);
        assert!(cached_for_test(url).is_some(), "first check kept the cache");
        // Nor is a run of healthy checks.
        observe_health(url, true);
        assert!(cached_for_test(url).is_some(), "steady state re-fetched");

        // Down, then back up: the models on the box may have changed.
        observe_health(url, false);
        assert!(cached_for_test(url).is_some(), "going down alone is not it");
        observe_health(url, true);
        assert!(
            cached_for_test(url).is_none(),
            "reconnect kept a stale cache"
        );
    }

    #[test]
    fn a_stale_entry_is_read_again_and_a_refresh_does_not_wait_for_that() {
        // A catalogue that knows one class, then a server that knows two: the
        // second is what a person who just installed a node pack expects.
        let old = parse_object_info(&json!({ "Old": {} }));
        let mut newer = object_info();
        newer["Newer"] = json!({});

        // Fresh: served from memory, no request made.
        let url = serve_once(http("200 OK", "application/json", &newer.to_string()));
        let client = ComfyUiClient::new(&url);
        remember_for_test(&url, old.clone());
        assert!(catalog_for(&client).unwrap().get("Old").is_some(), "fresh");

        // Past the TTL: read again.
        backdate_for_test(&url, CATALOG_TTL + Duration::from_secs(1));
        let read = catalog_for(&client).expect("stale entry should be re-read");
        assert!(read.get("Newer").is_some(), "the re-read was not served");
        assert!(read.get("Old").is_none());
        // ...and the re-read is now the fresh entry, so the (gone) server is
        // not asked again.
        assert!(catalog_for(&client).unwrap().get("Newer").is_some());

        // Past the TTL with ComfyUI down: the copy already held is better than
        // falling back to name-sniffing, so it is returned, not dropped.
        backdate_for_test(&url, CATALOG_TTL + Duration::from_secs(1));
        let held = catalog_for(&client).expect("a stale copy beats nothing");
        assert!(held.get("Newer").is_some());

        // An explicit refresh ignores a fresh entry and asks the server now.
        let url = serve_once(http("200 OK", "application/json", &newer.to_string()));
        let client = ComfyUiClient::new(&url);
        remember_for_test(&url, old);
        assert!(catalog_for(&client).unwrap().get("Old").is_some());
        let fresh = refresh_for(&client).expect("refresh should read the server");
        assert!(fresh.get("Newer").is_some());
        // A refresh that fails leaves nothing behind: the caller asked for the
        // truth, and a stale copy handed back as the answer would not be it.
        assert!(refresh_for(&client).is_none());
        assert!(cached_for_test(&url).is_none());
    }

    // === What a real server does, over a real socket =======================

    /// A one-shot HTTP server that answers the next request with `response`,
    /// then goes away. Enough to drive the real client through every way
    /// `/object_info` can disappoint it.
    fn serve_once(response: String) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Some(Ok(mut sock)) = listener.incoming().next() {
                let mut reader = BufReader::new(sock.try_clone().unwrap());
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                }
                let _ = sock.write_all(response.as_bytes());
                let _ = sock.flush();
            }
        });
        format!("http://{}", addr)
    }

    fn http(status: &str, content_type: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            content_type,
            body.len(),
            body
        )
    }

    #[test]
    fn a_server_that_answers_properly_is_read_once_and_remembered() {
        let body = serde_json::to_string(&object_info()).unwrap();
        let url = serve_once(http("200 OK", "application/json", &body));
        let client = ComfyUiClient::new(&url);

        let first = catalog_for(&client).expect("a good answer should parse");
        assert!(first.get("KSampler").is_some());

        // The server answered exactly one request and is now gone, so a second
        // read can only succeed from the cache.
        let second = catalog_for(&client).expect("second read should be cached");
        assert!(Arc::ptr_eq(&first, &second));

        invalidate(&url);
        assert!(
            catalog_for(&client).is_none(),
            "after invalidation there is nothing left to answer"
        );
    }

    #[test]
    fn every_way_object_info_can_be_missing_degrades_to_none() {
        // An older ComfyUI with no such endpoint.
        let url = serve_once(http("404 Not Found", "text/plain", "Not Found"));
        assert!(catalog_for(&ComfyUiClient::new(&url)).is_none(), "404");
        assert!(cached_for_test(&url).is_none(), "a 404 must not be cached");

        // A reverse proxy in front of a stopped ComfyUI, answering HTML.
        let url = serve_once(http("200 OK", "text/html", "<html>502 Bad Gateway</html>"));
        assert!(catalog_for(&ComfyUiClient::new(&url)).is_none(), "html");

        // Valid JSON that describes no node classes at all.
        let url = serve_once(http("200 OK", "application/json", r#"{"error":"nope"}"#));
        assert!(catalog_for(&ComfyUiClient::new(&url)).is_none(), "empty");
        assert!(
            cached_for_test(&url).is_none(),
            "an unreadable answer must not be cached as the truth"
        );

        // Nothing listening at all.
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = dead.local_addr().unwrap();
        drop(dead);
        let url = format!("http://{}", addr);
        assert!(catalog_for(&ComfyUiClient::new(&url)).is_none(), "refused");
    }

    #[test]
    fn serialisation_round_trips_so_the_console_and_the_snapshot_agree() {
        let catalog = parse_object_info(&object_info());
        let text = serde_json::to_string(&catalog).unwrap();
        let back: NodeCatalog = serde_json::from_str(&text).unwrap();
        assert_eq!(back, catalog);

        // The wire shape is a tagged union the console can switch on.
        let spec = serde_json::to_value(widget(&catalog, "KSampler", "seed")).unwrap();
        assert_eq!(spec["kind"], "seed");
        let spec = serde_json::to_value(widget(&catalog, "CLIPTextEncode", "text")).unwrap();
        assert_eq!(spec["kind"], "text");
        assert_eq!(spec["multiline"], true);
        // Absent options are omitted rather than sent as null.
        assert!(spec.get("default").is_none());
    }
}
