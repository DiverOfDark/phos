//! What makes a line export a *bundled template*, and what makes one valid.
//!
//! A template is not a new kind of thing. It is **a line, the workflow graphs
//! its stages run, their contracts, and a manifest of what has to be
//! installed** — which is exactly what exporting a line carries and importing
//! one reads. So the five files in `bundles/` are
//! [`crate::comfyui::portable`]'s `phos.line` documents: paste one into the
//! Lines tab's import box and it imports, byte for byte.
//!
//! ```json
//! {
//!   "format": "phos.line", "format_version": 1,
//!   "template": { "key": "photo-to-clip", "version": 1,
//!                 "confidence": "unverified", "summary": "…", "notes": "…" },
//!   "line": { "name": "…", "description": "…",
//!             "stages": [{ "workflow": "wan-i2v", "keep_output": true,
//!                          "source_mode": "last_frame" }] },
//!   "workflows": { "wan-i2v": { "name": "…", "description": "…",
//!                               "contract": { …, "corrections": { … } },
//!                               "graph": { …ComfyUI API format… } } },
//!   "requirements": { "node_classes": ["…"],
//!                     "models": [{ "class_type": "…", "field": "…", "name": "…" }] }
//! }
//! ```
//!
//! # Only two things here are FR6's
//!
//! [`Template`] and [`Confidence`] — the one block FR5d's format does not
//! define, carrying the key an upgrade matches on and the version it compares.
//! It is additive: absent on somebody's export, which is exactly right, because
//! a line a person sent you is not something Phos should later overwrite. The
//! document type itself is [`LineBundle`], re-exported below rather than
//! restated: two definitions of one format are two formats.
//!
//! And [`problems`], which is the build-time check on the five shipped files —
//! stricter than the importer, because a template that could not install must
//! not reach a release, while a line somebody hands you should be read as
//! generously as it can be.
//!
//! Three of FR5d's decisions the shipped files follow exactly, because
//! disagreeing with any of them would make the two formats different formats:
//!
//! * `stages[].workflow` is a **key into this document**, never a library id.
//! * **Stage order is array order.** There is no `stage_idx` in the file.
//! * **`requirements` is documentation, not input.** What is checked is derived
//!   from the graphs in the same file — see [`super::readiness`] — so a
//!   hand-written manifest cannot make a template claim something untrue.

use crate::comfyui::portable::{LineBundle, BUNDLE_FORMAT, BUNDLE_VERSION};
use serde::{Deserialize, Serialize};

/// How much Phos claims to know about whether this template runs.
///
/// Stated because the shipped graphs were built from published node definitions
/// rather than exported from a running ComfyUI, and saying so is better than
/// implying otherwise. The readiness check is the machine-readable half of the
/// same honesty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Core ComfyUI nodes whose signatures are stable and well known.
    High,
    /// Assembled from published node definitions, never run against a server.
    Unverified,
}

/// What makes this line export a *bundled template* rather than somebody's
/// export: an identity that is stable across releases, and a version.
///
/// The only field in the document FR5d does not define. Absent on an ordinary
/// export, which is exactly right — a line somebody sent you is not something
/// Phos should later overwrite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Template {
    /// Stable across releases; the identity an upgrade matches on.
    pub key: String,
    /// Bumped whenever the shipped content changes.
    pub version: u32,
    #[serde(default = "default_confidence")]
    pub confidence: Confidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What a person should know before running it: which custom node packs,
    /// which weights, what the frame rates mean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

fn default_confidence() -> Confidence {
    Confidence::Unverified
}

/// Everything that is structurally wrong with a shipped bundle, in English.
///
/// Run as a test over the five shipped ones, so a template that could not
/// install can never reach a release. Deliberately not run at startup: the
/// bundles are compiled in, so there is nothing a user could do about a failure
/// and nothing that could have changed since the test.
///
/// Not the same question [`LineBundle::parse`] asks. That one reads a document
/// a person handed Phos, and is as generous as it can be; this one reads a
/// document *Phos ships*, and demands everything an install needs.
#[allow(dead_code)] // The build-time check on the shipped five.
pub fn problems(bundle: &LineBundle) -> Vec<String> {
    let mut problems = Vec::new();
    if bundle.format != BUNDLE_FORMAT {
        problems.push(format!(
            "format is {:?}, not {:?}",
            bundle.format, BUNDLE_FORMAT
        ));
    }
    if bundle.format_version > BUNDLE_VERSION {
        problems.push(format!(
            "format_version {} is newer than this build reads ({})",
            bundle.format_version, BUNDLE_VERSION
        ));
    }
    match bundle.template.as_ref() {
        None => problems.push("no template block, so nothing could track it".to_string()),
        Some(t) => {
            if t.key.trim().is_empty() {
                problems.push("template.key is empty".to_string());
            }
            if t.version == 0 {
                problems.push("template.version must start at 1".to_string());
            }
        }
    }
    if bundle.workflows.is_empty() {
        problems.push("no workflows".to_string());
    }
    if bundle.line.name.trim().is_empty() {
        problems.push("the line has no name".to_string());
    }
    if bundle.line.stages.is_empty() {
        problems.push("the line has no stages".to_string());
    }
    for (key, wf) in &bundle.workflows {
        if !wf.graph.is_object() {
            problems.push(format!(
                "workflow {:?}: the graph is not a JSON object of nodes",
                key
            ));
            continue;
        }
        if let Err(e) = crate::comfyui::importable(&wf.graph) {
            problems.push(format!("workflow {:?}: {}", key, e));
        }
        if crate::comfyui::detect_outputs(&wf.graph).is_empty() {
            problems.push(format!("workflow {:?}: nothing saves anything", key));
        }
        if wf.graph.get(super::marker::MARKER_KEY).is_some() {
            problems.push(format!(
                "workflow {:?}: ships with a {} block; the marker is written at \
                 install time, from the hash of the graph without one",
                key,
                super::marker::MARKER_KEY
            ));
        }
    }
    for (idx, stage) in bundle.line.stages.iter().enumerate() {
        if bundle.workflow(&stage.workflow).is_none() {
            problems.push(format!(
                "stage {} names workflow {:?}, which the document does not carry",
                idx + 1,
                stage.workflow
            ));
        }
        if let Some(mode) = stage.source_mode.as_deref() {
            if mode.parse::<crate::comfyui::SourceMode>().is_err() {
                problems.push(format!(
                    "stage {}: {:?} is not a source mode",
                    idx + 1,
                    mode
                ));
            }
        }
    }
    problems
}
