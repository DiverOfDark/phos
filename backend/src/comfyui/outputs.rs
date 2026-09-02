//! What a run produced: the files, the files it might have produced, and the
//! text.
//!
//! [`collect_output_refs`] is deliberately blind to the key a node publishes
//! under: core `SaveImage` uses `images`, `VHS_VideoCombine` uses `gifs`, core
//! `SaveVideo` uses `videos`, `SaveAudio` uses `audio`, and a custom node uses
//! whatever its author chose. Enumerating two of those was why successful runs
//! were reported as failures.
//!
//! [`collect_text_values`] is the same refusal applied to values that are not
//! files at all. A describe stage's whole product is a sentence published
//! inline in the history entry — `ShowText|pysssss` uses `text`, others use
//! `string`, and a custom node uses whatever its author chose, sometimes as an
//! array of strings and sometimes as a bare one. Hard-coding `text` here would
//! be the same mistake in a new place.
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

/// Every inline text value a history entry's `outputs` publishes.
///
/// The counterpart of [`collect_output_refs`], and blind to the key for the
/// same reason: `ShowText|pysssss` publishes under `text`, other nodes under
/// `string`, and a custom node under whatever its author chose. Both spellings
/// of the value are taken — a bare string and an array of them — because both
/// are in circulation.
///
/// What it will *not* take is the strings inside a file reference. A
/// `{"filename": "a.png", "subfolder": "", "type": "output"}` is three strings
/// and no text at all, so anything shaped like a file is skipped whole; and
/// `[false]`, `[1]`, `[null]` are not text either, which is what keeps
/// `VHS_VideoCombine`'s `animated` flag out of a prompt.
///
/// Order is the order ComfyUI wrote them, and repeats are dropped: a graph that
/// both previews and saves the same sentence said it once.
pub(crate) fn collect_text_values(outputs: Option<&Value>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    if let Some(Value::Object(nodes)) = outputs {
        for node in nodes.values() {
            collect_text_into(node, 0, &mut found);
        }
    }
    found
}

/// How deep a custom node may wrap its sentence before we stop looking.
const MAX_TEXT_DEPTH: u8 = 3;

fn collect_text_into(value: &Value, depth: u8, found: &mut Vec<String>) {
    if depth > MAX_TEXT_DEPTH {
        return;
    }
    match value {
        Value::String(s) => push_text(s, found),
        Value::Array(items) => {
            for item in items {
                match item {
                    // Only strings, and only at this level: an array of file
                    // references is a file output, and descending into one
                    // would turn its filename into a prompt.
                    Value::String(s) => push_text(s, found),
                    Value::Object(_) | Value::Array(_) => collect_text_into(item, depth + 1, found),
                    _ => {}
                }
            }
        }
        Value::Object(map) => {
            // Shaped like a file: nothing in it is text.
            if map.contains_key("filename") {
                return;
            }
            for v in map.values() {
                collect_text_into(v, depth + 1, found);
            }
        }
        _ => {}
    }
}

fn push_text(s: &str, found: &mut Vec<String>) {
    let trimmed = s.trim();
    if trimmed.is_empty() || found.iter().any(|f| f == trimmed) {
        return;
    }
    found.push(trimmed.to_string());
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

    // === Text is published under whatever key its node picked ==============

    fn text_of(outputs: serde_json::Value) -> Vec<String> {
        collect_text_values(Some(&outputs))
    }

    /// One entry of a file output array, the way ComfyUI writes one.
    fn file(name: &str) -> serde_json::Value {
        serde_json::json!({ "filename": name, "subfolder": "", "type": "output" })
    }

    #[test]
    fn text_is_found_under_any_key_and_in_either_spelling() {
        use serde_json::json;
        // An array of strings is what `ShowText|pysssss` publishes, and it is
        // the shape actually seen. A bare string is the other spelling in
        // circulation. The key is not ours to guess in either case.
        for outputs in [
            json!({ "9": { "text": ["a woman on a jetty"] } }),
            json!({ "9": { "text": "a woman on a jetty" } }),
            json!({ "9": { "string": ["a woman on a jetty"] } }),
            json!({ "9": { "string": "a woman on a jetty" } }),
            json!({ "9": { "qwen_caption": ["a woman on a jetty"] } }),
            json!({ "9": { "result": "a woman on a jetty" } }),
        ] {
            assert_eq!(
                text_of(outputs.clone()),
                vec!["a woman on a jetty".to_string()],
                "text was not found in {}",
                outputs
            );
        }
    }

    #[test]
    fn a_custom_node_may_wrap_its_sentence_in_an_object() {
        use serde_json::json;
        assert_eq!(
            text_of(json!({ "9": { "ui": { "text": ["a woman on a jetty"] } } })),
            vec!["a woman on a jetty".to_string()]
        );
    }

    #[test]
    fn several_strings_come_back_in_the_order_they_were_written() {
        use serde_json::json;
        assert_eq!(
            text_of(json!({ "9": { "text": ["first line", "second line"] } })),
            vec!["first line".to_string(), "second line".to_string()]
        );
    }

    #[test]
    fn a_filename_is_never_mistaken_for_text() {
        use serde_json::json;
        // Three strings in that object, and not one of them is a description.
        assert!(text_of(json!({ "9": { "images": [file("a.png")] } })).is_empty());
        assert!(text_of(json!({ "9": { "gifs": [
            { "filename": "a.mp4", "subfolder": "phos", "type": "output" }
        ] } }))
        .is_empty());
    }

    #[test]
    fn flags_and_numbers_are_not_text() {
        use serde_json::json;
        // `animated` rides along with every VHS_VideoCombine output.
        assert!(text_of(json!({ "9": { "animated": [false], "count": [1] } })).is_empty());
    }

    #[test]
    fn the_same_sentence_previewed_and_saved_is_said_once() {
        use serde_json::json;
        assert_eq!(
            text_of(json!({
                "9":  { "text": ["a woman on a jetty"] },
                "10": { "string": ["a woman on a jetty"] },
            })),
            vec!["a woman on a jetty".to_string()]
        );
    }

    #[test]
    fn empty_strings_are_not_an_answer() {
        use serde_json::json;
        assert!(text_of(json!({ "9": { "text": ["", "   "] } })).is_empty());
        assert!(collect_text_values(None).is_empty());
    }

    #[test]
    fn a_graph_that_saves_a_file_and_shows_text_yields_both() {
        use serde_json::json;
        let outputs = json!({
            "9":  { "images": [file("a.png")] },
            "12": { "text": ["a woman on a jetty"] },
        });
        assert_eq!(collect_output_refs(Some(&outputs)).len(), 1);
        assert_eq!(
            collect_text_values(Some(&outputs)),
            vec!["a woman on a jetty".to_string()]
        );
    }

    #[test]
    fn only_the_named_suffixes_are_probed() {
        // Every extra candidate is a `/view` request that can only 404, and the
        // settle loop repeats them for as long as fifteen minutes.
        let candidates = fallback_output_candidates("phos/t", &["_00001_.png", "_00001_.webp"]);
        assert_eq!(candidates.len(), 2);
    }
}
