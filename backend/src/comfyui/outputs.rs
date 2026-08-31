//! The files a run produced, and the files it might have produced.
//!
//! [`collect_output_refs`] is deliberately blind to the key a node publishes
//! under: core `SaveImage` uses `images`, `VHS_VideoCombine` uses `gifs`, core
//! `SaveVideo` uses `videos`, `SaveAudio` uses `audio`, and a custom node uses
//! whatever its author chose. Enumerating two of those was why successful runs
//! were reported as failures.
//!
//! [`fallback_output_candidates`] goes the other way — from the prefix we
//! pinned before the run to the names ComfyUI would have written — so a file
//! can be found even when history never mentions it.

use serde_json::Value;

/// One file ComfyUI says it wrote, in the terms `/view` wants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct OutputRef {
    pub filename: String,
    pub subfolder: String,
    pub output_type: String,
}

impl OutputRef {
    /// Read one entry of an output array. `subfolder` and `type` default the way
    /// ComfyUI defaults them.
    fn from_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let filename = obj.get("filename")?.as_str()?;
        if filename.is_empty() {
            return None;
        }
        Some(Self {
            filename: filename.to_string(),
            subfolder: obj
                .get("subfolder")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            output_type: obj
                .get("type")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("output")
                .to_string(),
        })
    }

    /// How this file is named in an error message.
    pub fn describe(&self) -> String {
        if self.subfolder.is_empty() {
            format!("{} (type={})", self.filename, self.output_type)
        } else {
            format!(
                "{}/{} (type={})",
                self.subfolder, self.filename, self.output_type
            )
        }
    }
}

/// Every downloadable file named anywhere in a history entry's `outputs`.
///
/// Deliberately blind to the key a node publishes under: core `SaveImage` uses
/// `images`, `VHS_VideoCombine` uses `gifs`, core `SaveVideo` uses `videos`,
/// `SaveAudio` uses `audio`, and a custom node uses whatever its author chose.
/// Anything shaped like `[{ "filename": ... }]` counts.
pub(crate) fn collect_output_refs(outputs: Option<&Value>) -> Vec<OutputRef> {
    let mut found = Vec::new();
    if let Some(outputs) = outputs {
        collect_output_refs_into(outputs, 0, &mut found);
    }
    // Two nodes can name the same file (a preview beside a save); fetch it once.
    let mut seen = std::collections::HashSet::new();
    found.retain(|r| seen.insert(r.clone()));
    found
}

fn collect_output_refs_into(value: &Value, depth: u8, found: &mut Vec<OutputRef>) {
    if depth > 6 {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                match OutputRef::from_value(item) {
                    Some(r) => found.push(r),
                    None => collect_output_refs_into(item, depth + 1, found),
                }
            }
        }
        Value::Object(map) => {
            // A node output object never carries a bare `filename`, so a match
            // here is a custom node wrapping its file in an object rather than
            // an array.
            if depth > 0 {
                if let Some(r) = OutputRef::from_value(value) {
                    found.push(r);
                    return;
                }
            }
            for v in map.values() {
                collect_output_refs_into(v, depth + 1, found);
            }
        }
        _ => {}
    }
}

/// Where ComfyUI would have put a task's files, given the prefix it was told to
/// use and the suffixes its savers produce. Probed when history names nothing —
/// a file on disk beats a silent history entry.
///
/// Get `suffixes` from [`super::workflow::expected_output_suffixes`], which
/// reads them off the graph rather than guessing every combination.
pub(crate) fn fallback_output_candidates(output_prefix: &str, suffixes: &[&str]) -> Vec<OutputRef> {
    let (subfolder, stem) = match output_prefix.rsplit_once('/') {
        Some((dir, stem)) => (dir, stem),
        None => ("", output_prefix),
    };
    suffixes
        .iter()
        .map(|suffix| OutputRef {
            filename: format!("{}{}", stem, suffix),
            subfolder: subfolder.to_string(),
            output_type: "output".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_a_candidate_names_match_what_comfyui_writes() {
        let candidates =
            fallback_output_candidates("phos/task-1234", &["_00001_.png", "_00001.mp4"]);
        // SaveImage writes <prefix>_00001_.png into output/phos/.
        assert!(candidates.contains(&OutputRef {
            filename: "task-1234_00001_.png".to_string(),
            subfolder: "phos".to_string(),
            output_type: "output".to_string(),
        }));
        // VHS_VideoCombine writes <prefix>_00001.mp4 — no trailing underscore.
        assert!(candidates.contains(&OutputRef {
            filename: "task-1234_00001.mp4".to_string(),
            subfolder: "phos".to_string(),
            output_type: "output".to_string(),
        }));
        assert!(candidates.iter().all(|c| c.subfolder == "phos"));
    }

    #[test]
    fn scope_a_candidates_survive_a_prefix_without_a_subfolder() {
        let candidates = fallback_output_candidates("task-1234", &["_00001_.png"]);
        assert!(candidates.iter().all(|c| c.subfolder.is_empty()));
        assert!(candidates
            .iter()
            .any(|c| c.filename == "task-1234_00001_.png"));
    }

    #[test]
    fn only_the_named_suffixes_are_probed() {
        // Every extra candidate is a `/view` request that can only 404, and the
        // settle loop repeats them for as long as fifteen minutes.
        let candidates = fallback_output_candidates("phos/t", &["_00001_.png", "_00001_.webp"]);
        assert_eq!(candidates.len(), 2);
    }
}
