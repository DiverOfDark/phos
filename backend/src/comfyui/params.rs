//! Typed parameters, and one queue request becoming several tasks.
//!
//! [`super::overrides`] answers *which* of a graph's fields a person can change
//! and what kind of control each one wants. This module is the other half: what
//! they set them to, how that gets written back into the graph, and what
//! happens when they ask for a value to be swept rather than pinned.
//!
//! # Two override channels, deliberately
//!
//! `text_overrides` is a `String → String` map and stays exactly as it was —
//! prompts, and the `role` directives [`super::loaders`] reads out of it. The
//! typed map added here is `String → Value`, so a seed stays an integer, a cfg
//! stays a float, a checkbox stays a boolean and a checkpoint stays the exact
//! string ComfyUI listed. Both are keyed `"<node_id>.<field_name>"`, both are
//! stored on the task row, and a run is replayable from either.
//!
//! Splitting them rather than migrating text into the typed map keeps every
//! preset, every stored generation and every queued task working unchanged —
//! and keeps FR3's fallback intact, since an input the catalogue could not type
//! is a string by construction and belongs in the old channel.
//!
//! # Fan-out
//!
//! A parameter can be *varied* instead of fixed. Each varied parameter is an
//! axis; the request expands to the cross-product of its axes, and **each task
//! stores its own fully resolved parameter map**. Nothing is left to be decided
//! at dispatch time, so "which seed made this?" is answered by reading the row.
//!
//! Runs do not exist yet (FR5). Until they do, fan-out produces N independent
//! tasks and the API hands back their ids in order. When runs arrive, the only
//! change needed here is that the caller stamps one `run_id` on every row
//! [`expand`] returned — the expansion is already the single place that knows a
//! request became several tasks, and it already returns them ordered.

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::BTreeMap;
use utoipa::ToSchema;

/// A person's typed answers for one run, keyed `"<node_id>.<field_name>"`.
///
/// Ordered, because it is serialized into a task row and into a preset: a map
/// that reorders itself between writes makes two identical runs look different.
pub type ParameterMap = BTreeMap<String, Value>;

/// The largest seed a random draw will produce: 2^53 − 1.
///
/// ComfyUI's own seed widgets go to `i64::MAX`, but a console that cannot
/// display the number it ran with is worse than a slightly smaller space —
/// every client here reads JSON through a parser with 53 bits of integer
/// precision. A seed a person pins by hand is not touched by this.
pub const MAX_RANDOM_SEED: i64 = (1i64 << 53) - 1;

/// How many tasks one queue request may expand into.
///
/// A sweep is a convenience, not a batch scheduler: three axes of four values
/// each is already 64 renders, and a typo in a `count` should be a 400 rather
/// than a queue nobody asked for.
pub const MAX_FANOUT: usize = 64;

/// How a swept parameter picks its values when no explicit list is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaryMode {
    /// A fresh draw per task. The default, matching what ComfyUI's own
    /// `control_after_generate` does to a seed.
    #[default]
    Random,
    /// The pinned value, then that value plus one, and so on — a sweep you can
    /// describe in a sentence and reproduce from the first row.
    Increment,
}

/// The long form of a sweep, for when it needs to say how.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ToSchema)]
pub struct VarySpec {
    /// Run exactly these values, in this order.
    ///
    /// Any scalar JSON value: `[4.0, 6.0, 8.0]` for a cfg,
    /// `["sd15.safetensors", "sdxl.safetensors"]` for a checkpoint. Declared
    /// as unconstrained values, not objects — `Vec<Object>` would make a
    /// generated client refuse every valid list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Value>)]
    pub values: Vec<Value>,
    /// Or: how many values to generate. Ignored when `values` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// How to generate them.
    #[serde(default)]
    pub mode: VaryMode,
    /// The range a generated value stays inside — the node's own bounds, when
    /// the catalogue knew them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
}

/// One parameter's sweep.
///
/// Three spellings of the same idea, so the everyday cases are one token:
///
/// ```json
/// { "3.seed": 4,                      // four runs, four fresh seeds
///   "3.cfg":  [4.0, 6.0, 8.0],        // three runs, one per value
///   "3.steps": { "count": 3, "mode": "increment" } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum Variation {
    /// `4` — four runs of this parameter.
    Count(u32),
    /// `[4.0, 6.0, 8.0]` — one run per listed value.
    Values(Vec<Value>),
    /// The long form.
    Spec(VarySpec),
}

impl Variation {
    fn spec(&self) -> VarySpec {
        match self {
            Variation::Count(count) => VarySpec {
                count: Some(*count),
                ..VarySpec::default()
            },
            Variation::Values(values) => VarySpec {
                values: values.clone(),
                ..VarySpec::default()
            },
            Variation::Spec(spec) => spec.clone(),
        }
    }
}

/// The sweeps a queue request asked for, keyed like [`ParameterMap`].
pub type VaryMap = BTreeMap<String, Variation>;

// ===== Validation ===========================================================

/// Can each swept key actually change the graph it is aimed at?
///
/// [`apply_to_node`] silently skips a key that names no node, no field, or a
/// wired socket — right for a pinned parameter (a preset can outlive a graph
/// edit), and wrong for a sweep: a typo like `"3.sead"` would queue up to
/// [`MAX_FANOUT`] tasks that all run the same graph while their rows claim
/// distinct values. So a sweep's targets are checked against the workflow
/// before anything is queued, and the caller gets a 400 naming the key.
///
/// `filename_prefix` is refused too, although it is a literal: the output
/// prefix is pinned after substitution precisely so nothing can take it away,
/// and a sweep over it would also be N identical runs.
pub fn check_sweep_targets(workflow: &Value, vary: &VaryMap) -> Result<(), String> {
    for key in vary.keys() {
        let Some((node_id, field)) = key.split_once('.') else {
            return Err(format!(
                "'{}' is not a sweepable parameter; keys look like \"<node_id>.<field_name>\".",
                key
            ));
        };
        let current = workflow.get(node_id).map(|node| &node["inputs"][field]);
        match current {
            None => {
                return Err(format!(
                    "'{}' names a node this workflow does not have.",
                    key
                ))
            }
            Some(Value::Null) => {
                return Err(format!(
                    "'{}' names a field node {} does not have.",
                    key, node_id
                ))
            }
            Some(Value::Array(_)) | Some(Value::Object(_)) => {
                return Err(format!(
                    "'{}' is wired from another node, not a value to sweep.",
                    key
                ))
            }
            Some(_) if field == "filename_prefix" => {
                return Err(format!(
                    "'{}' is the output prefix, which Phos pins; sweeping it would run \
                     the same graph every time.",
                    key
                ))
            }
            Some(_) => {}
        }
    }
    Ok(())
}

// ===== Expansion ============================================================

/// Turn one queue request into the tasks it asks for.
///
/// The result is never empty: with no sweeps it is `base` alone, which is the
/// single-task path every caller had before. Axes are walked in key order with
/// the last one moving fastest, so the order is stable and readable — a seed
/// sweep inside a cfg sweep comes out grouped by cfg.
///
/// `Err` is a message for the user: an empty value list, a count of zero, or a
/// cross-product larger than [`MAX_FANOUT`].
pub fn expand(base: &ParameterMap, vary: &VaryMap) -> Result<Vec<ParameterMap>, String> {
    expand_with(base, vary, &mut random_in)
}

/// [`expand`], with the randomness handed in.
///
/// `draw(min, max)` must answer inside `[min, max]`. Tests pass a counter; the
/// server passes the platform CSPRNG.
pub fn expand_with(
    base: &ParameterMap,
    vary: &VaryMap,
    draw: &mut impl FnMut(i64, i64) -> i64,
) -> Result<Vec<ParameterMap>, String> {
    let mut axes: Vec<(&String, Vec<Value>)> = Vec::new();
    let mut total: usize = 1;
    for (key, variation) in vary {
        let values = resolve_axis(key, &variation.spec(), base.get(key), draw)?;
        total = total.saturating_mul(values.len());
        if total > MAX_FANOUT {
            return Err(format!(
                "That asks for more than {} runs at once. Narrow one of the swept parameters.",
                MAX_FANOUT
            ));
        }
        axes.push((key, values));
    }

    let mut tasks = vec![base.clone()];
    // Odometer: each axis multiplies what is there, and the axis added last
    // ends up as the fastest-moving digit.
    for (key, values) in axes {
        let mut next = Vec::with_capacity(tasks.len() * values.len());
        for task in &tasks {
            for value in &values {
                let mut copy = task.clone();
                copy.insert(key.clone(), value.clone());
                next.push(copy);
            }
        }
        tasks = next;
    }
    Ok(tasks)
}

/// The values one swept parameter runs with.
fn resolve_axis(
    key: &str,
    spec: &VarySpec,
    pinned: Option<&Value>,
    draw: &mut impl FnMut(i64, i64) -> i64,
) -> Result<Vec<Value>, String> {
    if !spec.values.is_empty() {
        return Ok(spec.values.clone());
    }
    let count = match spec.count {
        Some(0) | None => {
            return Err(format!(
                "'{}' is set to vary but says neither which values to run nor how many.",
                key
            ))
        }
        Some(count) => count as usize,
    };
    if count > MAX_FANOUT {
        return Err(format!(
            "'{}' asks for {} runs; the limit is {}.",
            key, count, MAX_FANOUT
        ));
    }

    let min = spec.min.unwrap_or(0);
    let max = spec.max.unwrap_or(MAX_RANDOM_SEED).max(min);
    Ok(match spec.mode {
        VaryMode::Random => (0..count)
            .map(|_| Value::Number(Number::from(draw(min, max))))
            .collect(),
        VaryMode::Increment => {
            let start = pinned.and_then(Value::as_i64).unwrap_or(min);
            let span = (max as i128 - min as i128) + 1;
            (0..count)
                .map(|i| {
                    // Wrap rather than saturate: a sweep that runs off the end
                    // of the range should keep producing distinct values, not
                    // the same one repeatedly.
                    let offset = (start as i128 - min as i128 + i as i128).rem_euclid(span);
                    Value::Number(Number::from((min as i128 + offset) as i64))
                })
                .collect()
        }
    })
}

/// A value in `[min, max]` from the platform CSPRNG.
///
/// Via UUID v4, which is how [`crate::cli_auth`] already gets random bytes here
/// — 122 bits from `getrandom` with no extra dependency.
fn random_in(min: i64, max: i64) -> i64 {
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let raw = u64::from_le_bytes(bytes[0..8].try_into().unwrap_or([0; 8]));
    let span = (max as i128 - min as i128 + 1).max(1);
    (min as i128 + (raw as i128 % span)) as i64
}

// ===== Another take of the same stage =======================================

/// Move every seed on, and nothing else.
///
/// What *regenerate* means at a hold point: the same stage, the same prompt,
/// the same source, the same everything a person set — run again, differently.
/// The only thing that may move is the noise, so this touches exactly the keys
/// the workflow's contract calls a seed and leaves the rest of the map alone.
///
/// Three cases, because a stage says one of three things about a seed:
///
/// * **Swept by count.** `Random` needs nothing: [`expand`] draws afresh every
///   time it is called. `Increment` is advanced by the width of the sweep, so
///   1000–1003 becomes 1004–1007 — fresh seeds that keep the character of the
///   sweep somebody asked for, rather than being quietly turned into random
///   ones.
/// * **Swept by an explicit list.** Left alone. A person who typed the seeds
///   asked for those seeds, and silently running different ones is the one
///   thing regenerate must not do.
/// * **Pinned, or not set at all.** A fresh draw. Not setting it is the case
///   worth being careful about: the stage would otherwise run the graph's own
///   literal seed again and produce the identical clip, which makes the button
///   look broken. Writing a seed the line did not have is the smallest possible
///   change that makes "again, differently" true.
pub fn reseed(seed_keys: &[String], base: &mut ParameterMap, vary: &mut VaryMap) {
    reseed_with(seed_keys, base, vary, &mut random_in)
}

/// [`reseed`], with the randomness handed in — tests pass a counter.
pub fn reseed_with(
    seed_keys: &[String],
    base: &mut ParameterMap,
    vary: &mut VaryMap,
    draw: &mut impl FnMut(i64, i64) -> i64,
) {
    for key in seed_keys {
        let Some(variation) = vary.get(key) else {
            base.insert(
                key.clone(),
                Value::Number(Number::from(draw(0, MAX_RANDOM_SEED))),
            );
            continue;
        };
        let spec = variation.spec();
        if !spec.values.is_empty() {
            continue;
        }
        let Some(count) = spec.count.filter(|c| *c > 0) else {
            continue;
        };
        if spec.mode != VaryMode::Increment {
            continue;
        }
        let min = spec.min.unwrap_or(0);
        let max = spec.max.unwrap_or(MAX_RANDOM_SEED).max(min);
        let span = (max as i128 - min as i128) + 1;
        let start = base.get(key).and_then(Value::as_i64).unwrap_or(min);
        let moved = (start as i128 - min as i128 + count as i128).rem_euclid(span);
        base.insert(
            key.clone(),
            Value::Number(Number::from((min as i128 + moved) as i64)),
        );
    }
}

// ===== Substitution =========================================================

/// Write this run's typed parameters over one node's literal inputs.
///
/// Called per node by [`super::workflow::prepare_workflow`], between the text
/// overrides and the pinned `filename_prefix` — so a parameter beats a text
/// override on the same field (it is the more specific channel), and neither
/// can take the output prefix away from Phos.
pub(crate) fn apply_to_node(
    node_id: &str,
    inputs: &mut serde_json::Map<String, Value>,
    parameters: &ParameterMap,
) {
    if parameters.is_empty() {
        return;
    }
    for (field, current) in inputs.iter_mut() {
        let key = format!("{}.{}", node_id, field);
        if let Some(wanted) = parameters.get(&key) {
            if let Some(next) = coerce(current, wanted) {
                *current = next;
            }
        }
    }
}

/// What to write in place of `current`, or `None` to leave it alone.
///
/// The incoming value's own JSON type is trusted — the console read the widget
/// kind out of `/object_info`, so a float box sends a float — and `current` is
/// consulted for two things only:
///
/// * **Is it a literal at all?** An array is a wired socket and an override
///   there would break the graph; nothing else is rewritable either.
/// * **Was it written as an integer?** Then an integral value stays an integer,
///   because a node declaring `INT` can refuse `20.0`. A fractional value is
///   still written as-is: a graph whose author typed `cfg: 8` must not round
///   someone's `7.5` back to `8`.
fn coerce(current: &Value, wanted: &Value) -> Option<Value> {
    match current {
        Value::Bool(_) => as_bool(wanted).map(Value::Bool),
        Value::Number(current) => {
            let wanted = as_f64(wanted)?;
            if current.is_f64() {
                // The author wrote a float; keep it one.
                Number::from_f64(wanted).map(Value::Number)
            } else if wanted.fract() == 0.0 && wanted.abs() < i64::MAX as f64 {
                Some(Value::Number(Number::from(wanted as i64)))
            } else {
                Number::from_f64(wanted).map(Value::Number)
            }
        }
        // Combos, filenames and prompts all arrive here. A number is stringified
        // rather than refused: a JSON round trip through a browser turns the
        // choice "1024" into one.
        Value::String(_) => match wanted {
            Value::String(s) => Some(Value::String(s.clone())),
            Value::Number(n) => Some(Value::String(n.to_string())),
            Value::Bool(b) => Some(Value::String(b.to_string())),
            _ => None,
        },
        // A link (`["6", 0]`), an object, or a null: not something to type into.
        _ => None,
    }
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::String(s) => s.parse::<bool>().ok(),
        Value::Number(n) => n.as_f64().map(|n| n != 0.0),
        _ => None,
    }
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(pairs: &[(&str, Value)]) -> ParameterMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn vary(json: Value) -> VaryMap {
        serde_json::from_value(json).expect("vary map should parse")
    }

    /// A draw a test can predict: min, then min+1, and so on.
    fn counting_draw() -> impl FnMut(i64, i64) -> i64 {
        let mut n = 0i64;
        move |min, max| {
            let v = min + n;
            n += 1;
            v.min(max)
        }
    }

    // === The wire shape =====================================================

    #[test]
    fn the_three_spellings_of_a_sweep_all_parse() {
        let map = vary(json!({
            "3.seed": 4,
            "3.cfg": [4.0, 6.0, 8.0],
            "3.steps": { "count": 3, "mode": "increment", "min": 1, "max": 10000 }
        }));
        assert_eq!(map["3.seed"], Variation::Count(4));
        assert_eq!(
            map["3.cfg"],
            Variation::Values(vec![json!(4.0), json!(6.0), json!(8.0)])
        );
        assert_eq!(
            map["3.steps"].spec().mode,
            VaryMode::Increment,
            "the long form is the one that can say how"
        );
        // The short forms mean what the long form would.
        assert_eq!(map["3.seed"].spec().count, Some(4));
        assert_eq!(map["3.seed"].spec().mode, VaryMode::Random);
        assert_eq!(map["3.cfg"].spec().values.len(), 3);
    }

    // === Sweep targets ======================================================

    #[test]
    fn a_sweep_may_target_any_rewritable_literal() {
        let graph = json!({
            "3": { "class_type": "KSampler",
                   "inputs": { "seed": 42, "cfg": 8.0, "sampler_name": "euler",
                               "add_noise": true, "model": ["4", 0] } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["3", 0], "filename_prefix": "ComfyUI" } }
        });
        for ok in ["3.seed", "3.cfg", "3.sampler_name", "3.add_noise"] {
            assert_eq!(
                check_sweep_targets(&graph, &vary(json!({ ok: 2 }))),
                Ok(()),
                "{} is a literal and sweepable",
                ok
            );
        }
        assert_eq!(check_sweep_targets(&graph, &VaryMap::new()), Ok(()));
    }

    #[test]
    fn a_sweep_that_cannot_change_the_graph_is_named_and_refused() {
        let graph = json!({
            "3": { "class_type": "KSampler",
                   "inputs": { "seed": 42, "model": ["4", 0] } },
            "9": { "class_type": "SaveImage",
                   "inputs": { "images": ["3", 0], "filename_prefix": "ComfyUI" } }
        });
        // The typo that would otherwise queue N identical runs with rows
        // claiming distinct values.
        for bad in [
            "3.sead",            // no such field
            "7.seed",            // no such node
            "3.model",           // wired from another node
            "9.filename_prefix", // pinned by Phos after substitution
            "no-dot-here",       // not even a key
        ] {
            let err = check_sweep_targets(&graph, &vary(json!({ bad: 2 }))).unwrap_err();
            assert!(err.contains(bad), "'{}' should be named: {}", bad, err);
        }
    }

    // === Fan-out ============================================================

    #[test]
    fn no_sweep_is_one_task_carrying_exactly_what_was_asked_for() {
        let base = params(&[("3.seed", json!(42)), ("3.steps", json!(20))]);
        let tasks = expand(&base, &VaryMap::new()).unwrap();
        assert_eq!(tasks, vec![base]);
    }

    #[test]
    fn a_seed_count_queues_that_many_tasks_each_with_its_own_seed() {
        // The PRD's example: {"seed": 4} is four tasks.
        let base = params(&[("3.seed", json!(42)), ("6.text", json!("a lighthouse"))]);
        let tasks =
            expand_with(&base, &vary(json!({ "3.seed": 4 })), &mut counting_draw()).unwrap();

        assert_eq!(tasks.len(), 4);
        let seeds: Vec<i64> = tasks
            .iter()
            .map(|t| t["3.seed"].as_i64().unwrap())
            .collect();
        assert_eq!(seeds, [0, 1, 2, 3], "each task got its own draw");
        // Everything else rides along unchanged, so each row is a whole run.
        for task in &tasks {
            assert_eq!(task["6.text"], json!("a lighthouse"));
        }
    }

    #[test]
    fn an_enumerable_parameter_sweeps_by_value() {
        let base = params(&[("3.cfg", json!(8.0))]);
        let tasks = expand(&base, &vary(json!({ "3.cfg": [4.0, 6.0, 8.0] }))).unwrap();
        let cfgs: Vec<f64> = tasks.iter().map(|t| t["3.cfg"].as_f64().unwrap()).collect();
        assert_eq!(cfgs, [4.0, 6.0, 8.0]);

        // Any enumerable parameter, not just numbers: a checkpoint sweep works
        // the same way.
        let tasks = expand(
            &ParameterMap::new(),
            &vary(json!({ "4.ckpt_name": ["sd15.safetensors", "sdxl.safetensors"] })),
        )
        .unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1]["4.ckpt_name"], json!("sdxl.safetensors"));
    }

    #[test]
    fn increment_starts_from_the_pinned_value_and_wraps_inside_the_range() {
        let base = params(&[("3.seed", json!(1000))]);
        let tasks = expand(
            &base,
            &vary(json!({ "3.seed": { "count": 3, "mode": "increment" } })),
        )
        .unwrap();
        let seeds: Vec<i64> = tasks
            .iter()
            .map(|t| t["3.seed"].as_i64().unwrap())
            .collect();
        assert_eq!(seeds, [1000, 1001, 1002]);

        // Off the end of a narrow range it wraps rather than repeating.
        let tasks = expand(
            &params(&[("5.batch_size", json!(3))]),
            &vary(json!({
                "5.batch_size": { "count": 4, "mode": "increment", "min": 1, "max": 4 }
            })),
        )
        .unwrap();
        let sizes: Vec<i64> = tasks
            .iter()
            .map(|t| t["5.batch_size"].as_i64().unwrap())
            .collect();
        assert_eq!(sizes, [3, 4, 1, 2]);
    }

    #[test]
    fn increment_with_nothing_pinned_starts_at_the_bottom_of_the_range() {
        let tasks = expand(
            &ParameterMap::new(),
            &vary(json!({ "3.steps": { "count": 3, "mode": "increment", "min": 10 } })),
        )
        .unwrap();
        let steps: Vec<i64> = tasks
            .iter()
            .map(|t| t["3.steps"].as_i64().unwrap())
            .collect();
        assert_eq!(steps, [10, 11, 12]);
    }

    #[test]
    fn two_axes_are_the_cross_product_in_a_readable_order() {
        let tasks = expand(
            &ParameterMap::new(),
            &vary(json!({ "3.cfg": [4.0, 8.0], "9.steps": [10, 20, 30] })),
        )
        .unwrap();
        assert_eq!(tasks.len(), 6);
        let pairs: Vec<(f64, i64)> = tasks
            .iter()
            .map(|t| (t["3.cfg"].as_f64().unwrap(), t["9.steps"].as_i64().unwrap()))
            .collect();
        // Key order decides the axes; the last one moves fastest, so the runs
        // come out grouped by the first.
        assert_eq!(
            pairs,
            [
                (4.0, 10),
                (4.0, 20),
                (4.0, 30),
                (8.0, 10),
                (8.0, 20),
                (8.0, 30)
            ]
        );
    }

    #[test]
    fn a_sweep_that_says_nothing_is_a_message_rather_than_a_silent_single_run() {
        for bad in [json!({ "3.seed": {} }), json!({ "3.seed": { "count": 0 } })] {
            let err = expand(&ParameterMap::new(), &vary(bad)).unwrap_err();
            assert!(err.contains("3.seed"), "{}", err);
        }
        // An explicitly empty list is the same mistake spelled differently.
        assert!(expand(
            &ParameterMap::new(),
            &vary(json!({ "3.seed": { "values": [], "count": 0 } }))
        )
        .is_err());
    }

    #[test]
    fn an_enormous_sweep_is_refused_rather_than_queued() {
        let one_axis = expand(
            &ParameterMap::new(),
            &vary(json!({ "3.seed": MAX_FANOUT + 1 })),
        );
        assert!(one_axis.is_err());
        assert_eq!(
            expand(&ParameterMap::new(), &vary(json!({ "3.seed": MAX_FANOUT })))
                .unwrap()
                .len(),
            MAX_FANOUT
        );

        // And the cross-product is what is capped, not each axis on its own.
        let two_axes = expand(
            &ParameterMap::new(),
            &vary(json!({ "3.seed": 16, "3.cfg": [1.0, 2.0, 3.0, 4.0, 5.0] })),
        );
        assert!(two_axes.is_err(), "16 x 5 should not have been queued");
    }

    #[test]
    fn a_random_seed_stays_inside_what_a_json_parser_can_hold() {
        let tasks = expand(&ParameterMap::new(), &vary(json!({ "3.seed": 32 }))).unwrap();
        for task in &tasks {
            let seed = task["3.seed"].as_i64().unwrap();
            assert!(
                (0..=MAX_RANDOM_SEED).contains(&seed),
                "{} is outside the safe range",
                seed
            );
        }
        // Distinct in practice: 32 draws out of 2^53 colliding would be news.
        let seeds: std::collections::HashSet<i64> = tasks
            .iter()
            .map(|t| t["3.seed"].as_i64().unwrap())
            .collect();
        assert_eq!(seeds.len(), tasks.len());
    }

    #[test]
    fn a_declared_range_is_respected_by_a_random_draw() {
        let tasks = expand(
            &ParameterMap::new(),
            &vary(json!({ "5.width": { "count": 24, "min": 512, "max": 520 } })),
        )
        .unwrap();
        for task in &tasks {
            let w = task["5.width"].as_i64().unwrap();
            assert!((512..=520).contains(&w), "{} is outside 512..=520", w);
        }
    }

    // === Substitution =======================================================

    fn applied(inputs: Value, parameters: &[(&str, Value)]) -> Value {
        let mut map = inputs.as_object().unwrap().clone();
        apply_to_node("3", &mut map, &params(parameters));
        Value::Object(map)
    }

    #[test]
    fn every_widget_kind_lands_in_the_graph_as_its_own_type() {
        let out = applied(
            json!({ "seed": 1, "steps": 20, "cfg": 8.0, "denoise": 1.0,
                    "sampler_name": "euler", "add_noise": true }),
            &[
                ("3.seed", json!(156680208700286i64)),
                ("3.steps", json!(28)),
                ("3.cfg", json!(6.5)),
                ("3.denoise", json!(0.75)),
                ("3.sampler_name", json!("dpmpp_2m")),
                ("3.add_noise", json!(false)),
            ],
        );
        assert_eq!(out["seed"], json!(156680208700286i64));
        assert!(out["seed"].is_i64(), "a seed must not become a float");
        assert_eq!(out["steps"], json!(28));
        assert_eq!(out["cfg"], json!(6.5));
        assert_eq!(out["denoise"], json!(0.75));
        assert_eq!(out["sampler_name"], json!("dpmpp_2m"));
        assert_eq!(out["add_noise"], json!(false));
    }

    #[test]
    fn a_float_is_not_rounded_back_just_because_the_author_typed_a_whole_number() {
        // Graphs in the wild write `"cfg": 8`. Reading that as "this field is an
        // int" would quietly turn a 7.5 into an 8.
        let out = applied(json!({ "cfg": 8 }), &[("3.cfg", json!(7.5))]);
        assert_eq!(out["cfg"], json!(7.5));
        // But an integral value written into an integral field stays integral,
        // because a node declaring INT can refuse 20.0.
        let out = applied(json!({ "steps": 20 }), &[("3.steps", json!(24.0))]);
        assert_eq!(out["steps"], json!(24));
        assert!(out["steps"].is_i64());
        // A field the author wrote as a float stays a float.
        let out = applied(json!({ "denoise": 1.0 }), &[("3.denoise", json!(1))]);
        assert!(out["denoise"].is_f64());
    }

    #[test]
    fn a_wired_socket_is_never_overwritten() {
        let out = applied(
            json!({ "model": ["10", 0], "positive": ["6", 0] }),
            &[("3.model", json!("something")), ("3.positive", json!(4))],
        );
        assert_eq!(out["model"], json!(["10", 0]));
        assert_eq!(out["positive"], json!(["6", 0]));
    }

    #[test]
    fn a_parameter_for_a_field_the_graph_does_not_have_is_ignored() {
        let out = applied(json!({ "steps": 20 }), &[("3.nonexistent", json!(1))]);
        assert_eq!(out, json!({ "steps": 20 }));
        // And one addressed to another node does not leak across.
        let out = applied(json!({ "steps": 20 }), &[("9.steps", json!(1))]);
        assert_eq!(out["steps"], json!(20));
    }

    #[test]
    fn a_value_that_cannot_stand_in_for_the_one_there_leaves_it_alone() {
        // Nonsense in a number field, rather than a graph ComfyUI refuses.
        let out = applied(json!({ "steps": 20 }), &[("3.steps", json!("twenty"))]);
        assert_eq!(out["steps"], json!(20));
        let out = applied(json!({ "steps": 20 }), &[("3.steps", json!({ "a": 1 }))]);
        assert_eq!(out["steps"], json!(20));

        // But a number spelled as a string is a browser artefact, not nonsense.
        let out = applied(json!({ "steps": 20 }), &[("3.steps", json!("28"))]);
        assert_eq!(out["steps"], json!(28));
        let out = applied(
            json!({ "add_noise": true }),
            &[("3.add_noise", json!("false"))],
        );
        assert_eq!(out["add_noise"], json!(false));
        // A combo choice that looks numeric survives the round trip as a string.
        let out = applied(json!({ "size": "1024" }), &[("3.size", json!(768))]);
        assert_eq!(out["size"], json!("768"));
    }

    #[test]
    fn an_empty_parameter_map_changes_nothing() {
        let before = json!({ "steps": 20, "cfg": 8.0, "model": ["1", 0] });
        assert_eq!(applied(before.clone(), &[]), before);
    }

    // === Regenerating: fresh seeds and nothing else ==========================

    #[test]
    fn regenerating_moves_the_seed_and_leaves_everything_else_alone() {
        let mut base = params(&[
            ("3.seed", json!(42)),
            ("3.steps", json!(20)),
            ("6.text", json!("a lighthouse")),
        ]);
        let mut vary = VaryMap::new();
        reseed_with(
            &["3.seed".to_string()],
            &mut base,
            &mut vary,
            &mut counting_draw(),
        );
        assert_eq!(base["3.seed"], json!(0), "a fresh draw");
        assert_eq!(base["3.steps"], json!(20), "the craft is untouched");
        assert_eq!(base["6.text"], json!("a lighthouse"));
        assert!(vary.is_empty(), "and it did not become a sweep");
    }

    #[test]
    fn a_stage_that_never_pinned_a_seed_still_gets_a_new_one() {
        // Otherwise regenerate re-runs the graph's own literal seed and hands
        // back the identical clip, which reads as a broken button.
        let mut base = ParameterMap::new();
        let mut vary = VaryMap::new();
        reseed_with(
            &["3.seed".to_string()],
            &mut base,
            &mut vary,
            &mut counting_draw(),
        );
        assert_eq!(base["3.seed"], json!(0));
    }

    #[test]
    fn an_incrementing_sweep_moves_on_by_its_own_width() {
        // 1000–1003 was generation one; generation two is 1004–1007, which is
        // four fresh seeds that are still the sweep somebody asked for.
        let mut base = params(&[("3.seed", json!(1000))]);
        let mut vary = vary(json!({ "3.seed": { "count": 4, "mode": "increment" } }));
        reseed_with(
            &["3.seed".to_string()],
            &mut base,
            &mut vary,
            &mut counting_draw(),
        );
        assert_eq!(base["3.seed"], json!(1004));
        let seeds: Vec<i64> = expand_with(&base, &vary, &mut counting_draw())
            .unwrap()
            .iter()
            .map(|t| t["3.seed"].as_i64().unwrap())
            .collect();
        assert_eq!(seeds, [1004, 1005, 1006, 1007]);
    }

    #[test]
    fn a_random_sweep_needs_no_help_and_an_explicit_list_gets_none() {
        // Random redraws on every expansion, so the base is left as it is…
        let mut base = params(&[("3.seed", json!(42))]);
        let mut swept = vary(json!({ "3.seed": 4 }));
        reseed_with(
            &["3.seed".to_string()],
            &mut base,
            &mut swept,
            &mut counting_draw(),
        );
        assert_eq!(base["3.seed"], json!(42));

        // …and a list of exact seeds is what somebody typed, so it stands.
        let mut listed = vary(json!({ "3.seed": [11, 22, 33] }));
        reseed_with(
            &["3.seed".to_string()],
            &mut base,
            &mut listed,
            &mut counting_draw(),
        );
        assert_eq!(base["3.seed"], json!(42));
        assert_eq!(
            listed["3.seed"].spec().values,
            vec![json!(11), json!(22), json!(33)]
        );
    }
}
