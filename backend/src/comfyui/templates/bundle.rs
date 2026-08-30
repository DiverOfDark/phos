//! The shape a bundled template is written in — which is FR5d's line export.
//!
//! A template is not a new kind of thing. It is **a line, the workflow graphs
//! its stages run, their contracts, and a manifest of what has to be
//! installed** — which is exactly what exporting a line carries and importing
//! one reads. So the five files in `bundles/` are `phos.line` documents: paste
//! one into the Lines tab's import box and it imports, byte for byte.
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
//! # This file is a stand-in
//!
//! FR5d owns this format — `comfyui::portable::LineBundle` — and landed in
//! parallel on the same base, so FR6 cannot compile against it yet. Every type
//! below is that type, field for field, and exists only so these five files can
//! be read before the two branches meet.
//!
//! **At integration: delete [`LineBundle`], [`BundleLine`], [`BundleStage`] and
//! [`BundleWorkflow`] and `use comfyui::portable::*` instead.** [`Template`] is
//! the only thing here that is FR6's, and it is additive: `template` is one
//! extra top-level object, ignored by FR5d's reader, so an exported template is
//! an ordinary line export and an imported line is a template with no version
//! to track.
//!
//! Three of FR5d's decisions this follows exactly, because disagreeing with any
//! of them would make the two formats different formats:
//!
//! * `stages[].workflow` is a **key into this document**, never a library id.
//! * **Stage order is array order.** There is no `stage_idx` in the file.
//! * **`requirements` is documentation, not input.** What is checked is derived
//!   from the graphs in the same file — see [`super::readiness`] — so a
//!   hand-written manifest cannot make a template claim something untrue.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

use crate::comfyui::contract::StageContract;
use crate::comfyui::params::{ParameterMap, VaryMap};

/// The `format` discriminator. FR5d's constant.
pub const BUNDLE_FORMAT: &str = "phos.line";
/// The `format_version` this build reads.
pub const BUNDLE_VERSION: u32 = 1;

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
/// FR6's, and the only field in the document FR5d does not define. Absent on an
/// ordinary export, which is exactly right — a line somebody sent you is not
/// something Phos should later overwrite.
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

// ===== FR5d's document, stood in for =======================================

/// One weights file a graph names, and where it names it.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
pub struct ModelRef {
    pub class_type: String,
    pub field: String,
    pub name: String,
}

/// What a line needs installed on the ComfyUI that will run it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Requirements {
    #[serde(default)]
    pub node_classes: Vec<String>,
    #[serde(default)]
    pub models: Vec<ModelRef>,
}

/// One step of a line, as it travels. `workflow` is a key into `workflows`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BundleStage {
    pub workflow: String,
    #[serde(default)]
    pub text_overrides: HashMap<String, String>,
    #[serde(default)]
    pub parameters: ParameterMap,
    #[serde(default)]
    pub vary: VaryMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_mode: Option<String>,
    #[serde(default)]
    pub keep_output: bool,
}

/// The line itself. Stage order is array order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleLine {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub stages: Vec<BundleStage>,
}

/// A workflow, carried whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleWorkflow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The contract it carried where it came from. Re-derived on install
    /// against *this* box's catalogue; only `corrections` survive verbatim,
    /// because a correction is the one part of a contract that cannot be worked
    /// out again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<StageContract>,
    /// The ComfyUI API-format graph.
    pub graph: Value,
}

/// A line, its stages, its workflows and what it needs installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineBundle {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub format_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_at: Option<String>,
    /// Present on a bundled template, absent on somebody's export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<Template>,
    pub line: BundleLine,
    #[serde(default)]
    pub workflows: BTreeMap<String, BundleWorkflow>,
    /// Documentation. What is checked is derived from the graphs.
    #[serde(default)]
    pub requirements: Requirements,
}

impl LineBundle {
    /// The template block, on a document that has one.
    pub fn template(&self) -> Option<&Template> {
        self.template.as_ref()
    }

    /// The key an upgrade matches on. Empty for a document with no template
    /// block, which nothing in this module ever seeds.
    pub fn key(&self) -> &str {
        self.template.as_ref().map_or("", |t| t.key.as_str())
    }

    pub fn version(&self) -> u32 {
        self.template.as_ref().map_or(0, |t| t.version)
    }

    pub fn workflow(&self, key: &str) -> Option<&BundleWorkflow> {
        self.workflows.get(key)
    }

    /// The graphs, for anything that derives from all of them at once.
    pub fn graphs(&self) -> impl Iterator<Item = &Value> {
        self.workflows.values().map(|w| &w.graph)
    }

    /// Everything that is structurally wrong with this document, in English.
    ///
    /// Run as a test over the five shipped ones, so a template that could not
    /// install can never reach a release. Deliberately not run at startup: the
    /// bundles are compiled in, so there is nothing a user could do about a
    /// failure and nothing that could have changed since the test.
    #[allow(dead_code)] // The build-time check on the shipped five.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.format != BUNDLE_FORMAT {
            problems.push(format!(
                "format is {:?}, not {:?}",
                self.format, BUNDLE_FORMAT
            ));
        }
        if self.format_version > BUNDLE_VERSION {
            problems.push(format!(
                "format_version {} is newer than this build reads ({})",
                self.format_version, BUNDLE_VERSION
            ));
        }
        match self.template.as_ref() {
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
        if self.workflows.is_empty() {
            problems.push("no workflows".to_string());
        }
        if self.line.name.trim().is_empty() {
            problems.push("the line has no name".to_string());
        }
        if self.line.stages.is_empty() {
            problems.push("the line has no stages".to_string());
        }
        for (key, wf) in &self.workflows {
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
        for (idx, stage) in self.line.stages.iter().enumerate() {
            if self.workflow(&stage.workflow).is_none() {
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
}
