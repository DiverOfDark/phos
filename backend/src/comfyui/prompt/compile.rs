//! Reading a describe stage's answer, turning it into a prompt, and writing it
//! where the next stage will find it.
//!
//! The other half of [`super`]: that one composes the instruction out of what
//! Phos knows, this one takes what came back and makes it usable. Split because
//! they fail differently — the instruction is a wording problem, and this is a
//! parsing problem with a model that will not always do as it is told.

use super::{split_constraints, Intent, DEFAULT_SLOT, NEGATIVE_SLOT, REFRESH_KEY, SLOT_KEY};
use crate::comfyui::contract::StageContract;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ===== What comes back =====================================================

/// The structured description a describe stage returns.
///
/// Structured on purpose: a paragraph is a thing to read, and this is a thing
/// to *use* — the fields are recombined per stage, the constraints become a
/// negative prompt, and the whole is cached and re-read by later runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Analysis {
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub setting: String,
    #[serde(default)]
    pub lighting: String,
    #[serde(default)]
    pub camera: String,
    #[serde(default)]
    pub motion_affordance: String,
    /// One constraint per entry. A model that answers with a single string
    /// instead of a list is still answering; see [`StringOrList`].
    #[serde(default)]
    #[schema(value_type = Vec<String>)]
    pub do_not: StringOrList,
}

impl Analysis {
    fn is_empty(&self) -> bool {
        self.subject.trim().is_empty()
            && self.setting.trim().is_empty()
            && self.lighting.trim().is_empty()
            && self.camera.trim().is_empty()
            && self.motion_affordance.trim().is_empty()
    }
}

/// `["a", "b"]` or `"a; b"`, because both come back from real models and
/// refusing one of them throws away a good answer over its punctuation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrList {
    #[default]
    Absent,
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    pub fn items(&self) -> Vec<String> {
        match self {
            StringOrList::Absent => Vec::new(),
            StringOrList::One(s) => split_constraints(s),
            StringOrList::Many(items) => items
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }
}

/// Read a describe stage's answer.
///
/// Models wrap JSON in prose, in ```json fences, or in an apology. All three
/// are the model doing its job badly rather than not doing it, so the first
/// balanced `{...}` in the text is taken and parsed. `None` means there was no
/// object in there at all, or it carried none of the fields — which is a real
/// answer too, and [`compile_from_text`] falls back to the prose.
pub fn parse_analysis(text: &str) -> Option<Analysis> {
    let object = first_json_object(text)?;
    let analysis: Analysis = serde_json::from_str(object).ok()?;
    (!analysis.is_empty() || !analysis.do_not.items().is_empty()).then_some(analysis)
}

/// The first balanced `{...}` in a string, ignoring braces inside strings.
fn first_json_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return text.get(start..=i);
                }
            }
            _ => {}
        }
    }
    None
}

// ===== The compiled prompt =================================================

/// The two strings a generation stage takes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CompiledPrompt {
    pub positive: String,
    pub negative: String,
}

/// Turn a description and an intent into a prompt.
///
/// The order is subject, setting, lighting, camera, motion, then the style and
/// the intent — general to specific, with what the person asked for last, which
/// is the order these models weight. Constraints never go in the positive
/// prompt: "do not add people" in a positive prompt adds people.
pub fn compile_prompt(analysis: &Analysis, intent: &Intent) -> CompiledPrompt {
    let mut parts: Vec<&str> = Vec::new();
    for field in [
        &analysis.subject,
        &analysis.setting,
        &analysis.lighting,
        &analysis.camera,
        &analysis.motion_affordance,
    ] {
        let trimmed = field.trim().trim_end_matches('.').trim();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    let mut positive = parts.join(". ");
    for extra in [intent.style.as_deref(), intent.intent.as_deref()]
        .into_iter()
        .flatten()
    {
        let extra = extra.trim().trim_end_matches('.').trim();
        if extra.is_empty() {
            continue;
        }
        if !positive.is_empty() {
            positive.push_str(". ");
        }
        positive.push_str(extra);
    }
    if !positive.is_empty() {
        positive.push('.');
    }

    CompiledPrompt {
        positive,
        negative: merge_constraints(&analysis.do_not.items(), &intent.do_not),
    }
}

/// Compile whatever a describe stage returned, structured or not.
///
/// A model that answered in prose still described the photograph, and throwing
/// that away to report a parse failure would be the tool being pedantic at the
/// user's expense.
pub fn compile_from_text(text: &str, intent: &Intent) -> CompiledPrompt {
    match parse_analysis(text) {
        Some(analysis) => compile_prompt(&analysis, intent),
        None => compile_prompt(
            &Analysis {
                subject: text.trim().to_string(),
                ..Analysis::default()
            },
            intent,
        ),
    }
}

/// The model's constraints and the person's, in that order, without repeats.
fn merge_constraints(from_model: &[String], from_person: &[String]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for item in from_model.iter().chain(from_person.iter()) {
        let item = item.trim().trim_end_matches(['.', ',']).trim();
        if item.is_empty() {
            continue;
        }
        if seen.iter().any(|s| s.eq_ignore_ascii_case(item)) {
            continue;
        }
        seen.push(item.to_string());
    }
    seen.join(", ")
}

// ===== Writing it into the next stage ======================================

/// Where a description could not be put, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct BindError {
    pub message: String,
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Write a compiled prompt into a stage's override map.
///
/// This is the whole of "the description becomes the next stage's prompt": one
/// entry keyed `"<node_id>.<field>"`, which is
/// [`super::contract::PromptSlot::override_key`], which is the key
/// [`super::workflow::prepare_workflow`] already substitutes on. No new
/// plumbing, no new column, and nothing in the worker to change.
///
/// The positive slot is *overwritten*. A person who put a describe stage in
/// front of a generation stage asked for the description to be the prompt; a
/// leftover default in the box is not an opinion worth beating it. The
/// constraints are *appended* to the negative slot instead, because a negative
/// prompt is usually a tuned list somebody meant.
pub fn bind_description(
    contract: &StageContract,
    overrides: &mut HashMap<String, String>,
    prompt: &CompiledPrompt,
) -> Result<(), BindError> {
    let wanted = overrides
        .get(SLOT_KEY)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SLOT)
        .to_string();

    let target = contract.slot(&wanted).or_else(|| {
        // A graph with exactly one text box has no ambiguity to resolve, whatever
        // its author called it.
        (contract.slots.len() == 1 && wanted == DEFAULT_SLOT).then(|| &contract.slots[0])
    });
    let Some(target) = target else {
        let names: Vec<&str> = contract.slots.iter().map(|s| s.name.as_str()).collect();
        return Err(BindError {
            message: if names.is_empty() {
                "the stage after a describe stage has no prompt box to write the \
                 description into"
                    .to_string()
            } else {
                format!(
                    "the stage after a describe stage has no '{}' prompt slot (it has: {})",
                    wanted,
                    names.join(", ")
                )
            },
        });
    };
    overrides.insert(target.override_key(), prompt.positive.clone());

    if !prompt.negative.trim().is_empty() {
        if let Some(negative) = contract.slot(NEGATIVE_SLOT) {
            let key = negative.override_key();
            let existing = overrides
                .get(&key)
                .cloned()
                .or_else(|| negative.default.clone())
                .unwrap_or_default();
            let merged = merge_constraints(
                &split_constraints_commas(&existing),
                &split_constraints_commas(&prompt.negative),
            );
            overrides.insert(key, merged);
        }
    }
    Ok(())
}

/// A negative prompt is comma-separated by convention, so it splits differently
/// from a constraint list a person typed.
fn split_constraints_commas(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Did this run ask for a fresh description rather than the shot's cache?
pub fn wants_refresh(overrides: &HashMap<String, String>) -> bool {
    overrides
        .get(REFRESH_KEY)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Put the instruction Phos wrote into the describe stage's own prompt box.
///
/// Same mechanism as [`bind_description`] and for the same reason: a describe
/// stage is a workflow, its instruction is a prompt, and a prompt is one
/// override.
///
/// Unlike [`bind_description`] this one *yields* to a person. An instruction
/// somebody typed is the thing they are trying to test, and a compiler that
/// silently discarded it would make the describe workflow impossible to debug.
/// "Typed" means "differs from what the graph's author left in the box",
/// because the Enhance dialog sends every text field's current value whether or
/// not anybody touched it.
pub fn bind_instruction(
    contract: &StageContract,
    overrides: &mut HashMap<String, String>,
    instruction: &str,
) -> Result<(), BindError> {
    let target = contract
        .slot(DEFAULT_SLOT)
        .or_else(|| contract.slots.first());
    let Some(target) = target else {
        return Err(BindError {
            message: "this describe workflow has no text box to put the instruction in".to_string(),
        });
    };
    let key = target.override_key();
    let untouched = match overrides.get(&key) {
        None => true,
        Some(value) if value.trim().is_empty() => true,
        Some(value) => target.default.as_deref().map(str::trim) == Some(value.trim()),
    };
    if untouched {
        overrides.insert(key, instruction.to_string());
    }
    Ok(())
}
