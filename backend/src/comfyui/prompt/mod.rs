//! Compiling a prompt out of what Phos already knows.
//!
//! Phos has a caption for every photograph, faces clustered to named people,
//! and the time and place the shutter fired. A person typing a prompt for a
//! video stage was retyping all of it, per shot. This module is the other way
//! round: the prompt is *compiled*.
//!
//! # Two halves, and the seam between them
//!
//! **Qwen supplies the looking; Phos supplies the knowing.** A vision model
//! reads a photograph very well and cannot possibly know that the woman on the
//! jetty is Anna, that it was July 2019, or that this line must not touch her
//! face. So Phos writes the *instruction* — [`describe_instruction`] — carrying
//! the person names from clustering, the EXIF date and place and the library
//! caption. The user's intent, the style preset and the stage's constraints
//! deliberately stay out of it: the answer is cached per shot, so the
//! description must be about the photograph and nothing else. The model answers
//! with [`Analysis`], and [`compile_prompt`] folds the intent in as it turns
//! that into the two strings a generation stage actually takes.
//!
//! # Why it is all pure
//!
//! Everything here is a function of its arguments. The describe stage runs
//! inside ComfyUI like every other stage — there is no second service and no
//! LLM client in this tree — so what is left to get right is the wording, the
//! parsing of a model that will not always do as it is told, and where the
//! answer is written. All three are testable with no ComfyUI and no GPU, which
//! is the whole reason [`super::history`] and [`super::policy`] are shaped this
//! way too.
//!
//! # Binding is one override
//!
//! FR5a keys prompt slots as `"<node_id>.<field>"` — the exact key
//! [`super::workflow::prepare_workflow`] substitutes on — so putting a
//! description into the next stage's prompt is one `text_overrides` entry and
//! no new plumbing. [`bind_description`] is that entry being written.

pub mod compile;
mod facts;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// The names the rest of the crate says. Everything else lives under
// `prompt::compile::` rather than being re-exported unused.
pub use compile::{
    bind_description, bind_instruction, compile_from_text, parse_analysis, wants_refresh, Analysis,
    CompiledPrompt,
};
pub(crate) use facts::{cache_analysis, cached_analysis_for, shot_facts};
// The raw entry, unvalidated: only the tests read it, to see what was written.
#[cfg(test)]
pub(crate) use facts::cached_analysis;

// ===== What the run says it wants ==========================================

/// Directives a line stage or an enhance request carries for the compiler.
///
/// They live in the same `text_overrides` map as prompt bindings and the
/// `role:<node>` loader directives, under a `phos:` prefix, for the reason that
/// worked there: the map is already stored on the stage, already stored on the
/// task, already exported with a line, and already reaches both the dispatch
/// path and the advance pass. A second column would have had to be threaded
/// through all four.
// Read by the tests and by anything that needs to tell a directive from a
// prompt; the keys below are what the code writes.
#[allow(dead_code)]
pub const DIRECTIVE_PREFIX: &str = "phos:";
/// The user's one line about what they are after.
pub const INTENT_KEY: &str = "phos:intent";
/// A style preset, in whatever words the preset is written in.
pub const STYLE_KEY: &str = "phos:style";
/// Constraints, one per line or separated by `;`.
pub const DO_NOT_KEY: &str = "phos:do_not";
/// Which prompt slot of the *next* stage a description binds into. `positive`
/// unless said otherwise.
pub const SLOT_KEY: &str = "phos:slot";
/// Set to `1`/`true` to describe again rather than read the shot's cache.
pub const REFRESH_KEY: &str = "phos:refresh";

/// What the person asked for, as opposed to what is in the photograph.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Intent {
    /// One line: "a slow push-in as the light fades".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// A style preset: "35mm film, muted palette".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// What the run must not do. Reaches the model as a rule *and* the
    /// generation stage as a negative prompt, because a model told not to do
    /// something is not the same as a sampler steered away from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub do_not: Vec<String>,
}

impl Intent {
    /// Read the compiler's directives out of an override map.
    pub fn from_overrides(overrides: &HashMap<String, String>) -> Self {
        Intent {
            intent: non_empty(overrides.get(INTENT_KEY)),
            style: non_empty(overrides.get(STYLE_KEY)),
            do_not: overrides
                .get(DO_NOT_KEY)
                .map(|s| split_constraints(s))
                .unwrap_or_default(),
        }
    }

    /// Write these directives into an override map, so a task's row still says
    /// what the prompt was compiled from.
    pub fn to_overrides(&self, overrides: &mut HashMap<String, String>) {
        set_or_clear(overrides, INTENT_KEY, self.intent.as_deref());
        set_or_clear(overrides, STYLE_KEY, self.style.as_deref());
        if self.do_not.is_empty() {
            overrides.remove(DO_NOT_KEY);
        } else {
            overrides.insert(DO_NOT_KEY.to_string(), self.do_not.join("\n"));
        }
    }

    /// Fill in anything this stage did not say from the describe stage that fed
    /// it.
    ///
    /// A person setting up a line says "a slow push-in, and don't touch her
    /// face" once, on the stage where it means something — the describe stage,
    /// which is being told what to look for. The generation stage after it
    /// wants the same words, and making them retype them on every stage would
    /// be the retyping this whole feature exists to end. Anything the stage
    /// *did* say wins, because it said it on purpose.
    pub fn inherit(mut self, upstream: Intent) -> Self {
        if self.intent.is_none() {
            self.intent = upstream.intent;
        }
        if self.style.is_none() {
            self.style = upstream.style;
        }
        if self.do_not.is_empty() {
            self.do_not = upstream.do_not;
        }
        self
    }
}

fn set_or_clear(overrides: &mut HashMap<String, String>, key: &str, value: Option<&str>) {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => {
            overrides.insert(key.to_string(), v.to_string());
        }
        None => {
            overrides.remove(key);
        }
    }
}

/// Which prompt slot a description goes into, on a stage that did not say.
pub const DEFAULT_SLOT: &str = "positive";
/// Where the constraints go, when the stage has one.
pub const NEGATIVE_SLOT: &str = "negative";

fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// "no faces; no extra people\nno text" → three constraints.
///
/// Newlines *and* semicolons, because a person typing into a textarea uses one
/// and a person typing into a single-line field uses the other, and guessing
/// wrong turns three rules into one long one the model reads as noise.
fn split_constraints(raw: &str) -> Vec<String> {
    raw.split(['\n', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

// ===== What Phos knows =====================================================

/// What the library already holds about one photograph.
///
/// Every field is what a vision model cannot get from the pixels: who these
/// people are, when this was, where it was, and the caption search already
/// matches on. Read from the database by [`shot_facts`]; a pure value here so
/// the wording can be exercised without one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, utoipa::ToSchema)]
pub struct ShotFacts {
    /// The named people clustered onto this shot, primary person first.
    /// Unnamed clusters are left out: "person 4f2a" helps nobody.
    pub people: Vec<String>,
    /// The EXIF capture time, as stored (`YYYY-MM-DD HH:MM:SS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken_at: Option<String>,
    /// EXIF coordinates. Phos has no geocoder, so this is degrees and says so
    /// — a model that recognises the place from the pixels can still use them
    /// as corroboration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place: Option<(f32, f32)>,
    /// The Florence-2 library caption from `shots.description`. Read as one
    /// input among several: it is what search matches on, and it is emphatically
    /// not the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

impl ShotFacts {
    /// Coordinates as a reader (and a model) sees them.
    fn place_line(&self) -> Option<String> {
        self.place
            .map(|(lat, lon)| format!("{:.4}, {:.4} (EXIF coordinates, no place name)", lat, lon))
    }

    fn is_empty(&self) -> bool {
        self.people.is_empty()
            && self.taken_at.is_none()
            && self.place.is_none()
            && self.caption.is_none()
    }
}

// ===== The instruction =====================================================

/// The schema the describe stage is asked to answer in. Quoted verbatim into
/// the instruction so the wording and the parser cannot drift apart.
const ANALYSIS_SCHEMA: &str = r#"{
  "subject": "who or what the shot is of, and what they are doing",
  "setting": "where this is, and what is around them",
  "lighting": "the quality, direction and colour of the light",
  "camera": "lens feel, framing and distance",
  "motion_affordance": "what could move here and what must stay still",
  "do_not": ["one constraint per entry"]
}"#;

/// The instruction Phos sends into the describe workflow.
///
/// Deterministic, so a test can prove that the person names and the EXIF date
/// actually reach it — the whole claim of this feature is that they do.
///
/// A function of the *photograph* alone, on purpose: the run's intent, style
/// and constraints are applied when the prompt is compiled downstream, never
/// sent to the vision model. The answer is cached per shot
/// ([`cache_analysis`]), and a description steered by one line's intent would
/// be quietly wrong for the next line over the same shot.
pub fn describe_instruction(facts: &ShotFacts) -> String {
    let mut out = String::new();
    out.push_str(
        "You are looking at one photograph. Describe it so that a video model can \
         animate this exact frame without inventing anything that is not in it.\n",
    );

    if !facts.is_empty() {
        out.push_str("\nWhat the library already knows about this photograph:\n");
        if !facts.people.is_empty() {
            out.push_str(&format!("- People in it: {}\n", facts.people.join(", ")));
        }
        if let Some(taken_at) = &facts.taken_at {
            out.push_str(&format!("- Taken: {}\n", taken_at));
        }
        if let Some(place) = facts.place_line() {
            out.push_str(&format!("- Place: {}\n", place));
        }
        if let Some(caption) = &facts.caption {
            out.push_str(&format!("- Existing caption: {}\n", caption));
        }
    }

    out.push_str("\nAnswer with one JSON object and nothing else, in this shape:\n");
    out.push_str(ANALYSIS_SCHEMA);
    out.push_str(
        "\n\nRules:\n\
         - Use the names above for the people in the photograph. Do not invent names, \
         and do not name anyone the list does not.\n\
         - Describe only what is visible. If you cannot tell, leave that field empty \
         rather than guessing.\n\
         - `motion_affordance` is the useful one: say what in this frame could \
         plausibly move, and what is fixed because of how the subject is posed or held.\n\
         - Put anything this photograph would be damaged by into `do_not`.\n",
    );
    out
}

#[cfg(test)]
mod tests;
