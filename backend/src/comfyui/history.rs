//! What a `/history/{prompt_id}` entry means.
//!
//! A pure function over [`serde_json::Value`], so the completion path can be
//! exercised against recorded payloads without a live ComfyUI.
//!
//! The rule worth remembering: a finished prompt that names no file is a
//! *state*, not a verdict. [`interpret_history`] reports it as
//! [`HistoryVerdict::NoOutputs`] and leaves the waiting to [`super::policy`].
//! Errors are checked before completion, because ComfyUI has shipped builds
//! that set `completed: true` alongside an `execution_error`.
//!
//! The second rule, learned later: a finished prompt that names no file may
//! still have said something. A describe stage's whole product is a sentence
//! published inline in `outputs`, and the refusal to hard-code output keys that
//! [`super::outputs`] applies to files applies to text as well — see
//! [`HistoryVerdict::Text`].

use super::outputs::{collect_output_refs, collect_text_values, OutputRef};
use serde_json::Value;

/// What a `/history/{prompt_id}` entry means for the task that queued it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HistoryVerdict {
    /// Still executing; nothing to decide yet.
    Running,
    /// Finished, and named these files.
    Outputs(Vec<OutputRef>),
    /// Finished, and published text but no file. A describe stage's whole
    /// product: FR5a's `produces: text` writes no `files` row, so this is not a
    /// degenerate [`HistoryVerdict::Outputs`] and must not be settled as one.
    Text(Vec<String>),
    /// Finished, but named no files (yet). Caller decides whether to keep
    /// waiting — this is a state, not a verdict.
    NoOutputs,
    /// A node raised or the prompt was rejected. Never worth another attempt.
    Failed(String),
}

/// Read a history entry without touching the network.
///
/// Errors are checked before completion, because ComfyUI has shipped builds that
/// set `completed: true` alongside an `execution_error` message.
pub(crate) fn interpret_history(entry: &Value) -> HistoryVerdict {
    let status = entry.get("status");

    if let Some(err) = execution_error_detail(entry) {
        return HistoryVerdict::Failed(err);
    }

    if let Some(status) = status {
        if status.get("status_str").and_then(|v| v.as_str()) == Some("error") {
            return HistoryVerdict::Failed(format!(
                "ComfyUI reported status 'error'. Status details: {}",
                serde_json::to_string(status).unwrap_or_else(|_| "N/A".to_string())
            ));
        }
        let completed = status
            .get("completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !completed {
            return HistoryVerdict::Running;
        }
    }

    let refs = collect_output_refs(entry.get("outputs"));
    if !refs.is_empty() {
        return HistoryVerdict::Outputs(refs);
    }
    // No file, but perhaps a sentence. A graph that both saves a picture and
    // shows a caption is a picture stage whose caption is a preview, so files
    // are asked about first; a stage whose contract says `produces: text` reads
    // the text directly with `text_outputs` and never gets here.
    let text = collect_text_values(entry.get("outputs"));
    if !text.is_empty() {
        return HistoryVerdict::Text(text);
    }
    HistoryVerdict::NoOutputs
}

/// The inline text a finished entry published, whatever else it published.
///
/// Read by the completion path when the task's stage is declared
/// `produces: text`: such a graph may well preview the photograph it read, and
/// that preview is not the product.
pub(crate) fn text_outputs(entry: &Value) -> Vec<String> {
    collect_text_values(entry.get("outputs"))
}

/// The user-facing message for an `execution_error` in `status.messages`.
fn execution_error_detail(entry: &Value) -> Option<String> {
    let data = execution_error_data(entry)?;
    let exception_msg = data
        .get("exception_message")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown error");
    let node_type = data
        .get("node_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let node_id = data
        .get("node_id")
        .map(|v| match v.as_str() {
            Some(s) => s.to_string(),
            None => v.to_string(),
        })
        .unwrap_or_else(|| "?".to_string());
    Some(format!(
        "ComfyUI execution error in node {} ({}): {}",
        node_id, node_type, exception_msg
    ))
}

fn execution_error_data(entry: &Value) -> Option<&Value> {
    entry
        .get("status")?
        .get("messages")?
        .as_array()?
        .iter()
        .find_map(|msg| {
            let arr = msg.as_array()?;
            (arr.first()?.as_str()? == "execution_error").then(|| arr.get(1))?
        })
}

/// The traceback ComfyUI attached to a failing node, for the log.
pub(crate) fn execution_error_traceback(entry: &Value) -> Option<String> {
    let tb = execution_error_data(entry)?
        .get("traceback")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("");
    (!tb.is_empty()).then_some(tb)
}

/// Fixtures shaped like real `/history/{prompt_id}` payloads. Shared with the
/// end-to-end test in the parent module.
#[cfg(test)]
pub(super) mod fixtures {
    use serde_json::{json, Value};

    /// A history entry the way ComfyUI writes one.
    pub(crate) fn history(outputs: Value, completed: bool) -> Value {
        json!({
            "prompt": [0, "abc", {}, {}, []],
            "outputs": outputs,
            "status": {
                "status_str": if completed { "success" } else { "running" },
                "completed": completed,
                "messages": [
                    ["execution_start", { "prompt_id": "abc" }],
                    ["execution_cached", { "nodes": ["4", "6"], "prompt_id": "abc" }],
                ],
            },
        })
    }

    /// One entry of an output array.
    pub(crate) fn file(name: &str) -> Value {
        json!({ "filename": name, "subfolder": "", "type": "output" })
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{file, history};
    use super::*;
    use serde_json::json;

    // === Defect 1 / scope B — outputs must be found under any key ============
    #[test]
    fn defect_1_finds_outputs_under_any_key() {
        // Every one of these is a real publisher: SaveImage -> images,
        // VHS_VideoCombine -> gifs, core SaveVideo -> videos, SaveAudio -> audio,
        // and a custom node under whatever key its author picked.
        for (key, name) in [
            ("images", "phos_00001_.png"),
            ("gifs", "phos_00001.gif"),
            ("videos", "phos_00001.mp4"),
            ("audio", "phos_00001.flac"),
            ("my_custom_saver", "phos_00001.exr"),
            ("result", "phos_00001.webp"),
        ] {
            let entry = history(json!({ "9": { key: [file(name)] } }), true);
            assert_eq!(
                interpret_history(&entry),
                HistoryVerdict::Outputs(vec![OutputRef {
                    filename: name.to_string(),
                    subfolder: String::new(),
                    output_type: "output".to_string(),
                }]),
                "output key {:?} was not recognised",
                key
            );
        }
    }

    #[test]
    fn defect_1_carries_subfolder_and_type() {
        let entry = history(
            json!({ "12": { "videos": [
                { "filename": "a.mp4", "subfolder": "phos", "type": "output" }
            ] } }),
            true,
        );
        assert_eq!(
            interpret_history(&entry),
            HistoryVerdict::Outputs(vec![OutputRef {
                filename: "a.mp4".to_string(),
                subfolder: "phos".to_string(),
                output_type: "output".to_string(),
            }])
        );
    }

    #[test]
    fn defect_1_defaults_subfolder_and_type_the_way_comfyui_does() {
        let entry = history(
            json!({ "9": { "images": [ { "filename": "a.png" } ] } }),
            true,
        );
        assert_eq!(
            interpret_history(&entry),
            HistoryVerdict::Outputs(vec![OutputRef {
                filename: "a.png".to_string(),
                subfolder: String::new(),
                output_type: "output".to_string(),
            }])
        );
    }

    #[test]
    fn defect_1_collects_from_several_nodes_and_keys_at_once() {
        let entry = history(
            json!({
                "9":  { "images": [file("a.png"), file("b.png")] },
                "12": { "gifs": [file("c.mp4")] },
                "15": { "videos": [file("d.webm")], "animated": [true] },
            }),
            true,
        );
        match interpret_history(&entry) {
            HistoryVerdict::Outputs(refs) => {
                // Node ids arrive in whatever order serde_json's map yields, and
                // download order does not matter; every file being named does.
                let mut names: Vec<&str> = refs.iter().map(|r| r.filename.as_str()).collect();
                names.sort_unstable();
                assert_eq!(names, ["a.png", "b.png", "c.mp4", "d.webm"]);
            }
            other => panic!("expected outputs, got {:?}", other),
        }
    }

    #[test]
    fn defect_1_ignores_arrays_that_name_no_file() {
        // `animated` is an array of bools; `text` an array of strings. Neither
        // is downloadable, and mistaking either for a file would be worse than
        // missing it. The caption is still read — as text, which is what it is.
        let entry = history(
            json!({ "9": { "animated": [false], "text": ["a caption"] } }),
            true,
        );
        assert_eq!(
            interpret_history(&entry),
            HistoryVerdict::Text(vec!["a caption".to_string()])
        );
        let entry = history(json!({ "9": { "animated": [false] } }), true);
        assert_eq!(interpret_history(&entry), HistoryVerdict::NoOutputs);
    }

    // === FR9 / a describe stage's product is inline text ====================
    #[test]
    fn a_finished_run_that_published_only_text_is_a_text_verdict() {
        for key in ["text", "string", "qwen_output"] {
            let entry = history(json!({ "9": { key: ["a woman on a jetty"] } }), true);
            assert_eq!(
                interpret_history(&entry),
                HistoryVerdict::Text(vec!["a woman on a jetty".to_string()]),
                "text under {:?} was not recognised",
                key
            );
        }
    }

    #[test]
    fn a_bare_string_is_as_good_as_an_array_of_one() {
        let entry = history(json!({ "9": { "text": "a woman on a jetty" } }), true);
        assert_eq!(
            interpret_history(&entry),
            HistoryVerdict::Text(vec!["a woman on a jetty".to_string()])
        );
    }

    #[test]
    fn a_file_still_wins_over_a_caption_beside_it() {
        // A generation graph that previews a caption is not a describe stage.
        let entry = history(
            json!({
                "9":  { "images": [file("a.png")] },
                "12": { "text": ["a caption"] },
            }),
            true,
        );
        assert!(matches!(
            interpret_history(&entry),
            HistoryVerdict::Outputs(_)
        ));
        // And the caption is still readable by a stage that wants it.
        assert_eq!(text_outputs(&entry), vec!["a caption".to_string()]);
    }

    #[test]
    fn a_running_prompt_is_not_text_however_much_it_has_printed() {
        let entry = history(json!({ "9": { "text": ["partial"] } }), false);
        assert_eq!(interpret_history(&entry), HistoryVerdict::Running);
    }

    #[test]
    fn defect_1_names_the_same_file_once() {
        let entry = history(
            json!({
                "9":  { "images": [file("a.png")] },
                "10": { "images": [file("a.png")] },
            }),
            true,
        );
        assert_eq!(
            interpret_history(&entry),
            HistoryVerdict::Outputs(vec![OutputRef {
                filename: "a.png".to_string(),
                subfolder: String::new(),
                output_type: "output".to_string(),
            }])
        );
    }

    // === Defect 2 / scope C — empty outputs are a state, not a verdict =======
    #[test]
    fn defect_2_empty_outputs_is_not_a_failure() {
        assert_eq!(
            interpret_history(&history(json!({}), true)),
            HistoryVerdict::NoOutputs
        );
    }

    #[test]
    fn defect_2_null_outputs_is_not_a_failure() {
        assert_eq!(
            interpret_history(&history(Value::Null, true)),
            HistoryVerdict::NoOutputs
        );
    }

    #[test]
    fn defect_2_incomplete_run_is_still_running() {
        assert_eq!(
            interpret_history(&history(json!({}), false)),
            HistoryVerdict::Running
        );
    }

    // === Defect 4 / scope E — error fidelity =================================
    #[test]
    fn defect_4_execution_errors_report_the_node_and_message() {
        let entry = json!({
            "outputs": {},
            "status": {
                "status_str": "error",
                "completed": false,
                "messages": [
                    ["execution_start", { "prompt_id": "abc" }],
                    ["execution_error", {
                        "node_id": "14",
                        "node_type": "KSampler",
                        "exception_message": "CUDA out of memory",
                        "traceback": ["Traceback...\n", "  line 1\n"],
                    }],
                ],
            },
        });
        assert_eq!(
            interpret_history(&entry),
            HistoryVerdict::Failed(
                "ComfyUI execution error in node 14 (KSampler): CUDA out of memory".to_string()
            )
        );
        assert!(execution_error_traceback(&entry)
            .unwrap()
            .contains("Traceback"));
    }

    #[test]
    fn defect_4_an_error_wins_over_a_completed_flag() {
        // Some builds set completed:true beside an execution_error. Reading the
        // flag first turns a real error into "no output images found".
        let entry = json!({
            "outputs": {},
            "status": {
                "status_str": "success",
                "completed": true,
                "messages": [
                    ["execution_error", {
                        "node_id": 14,
                        "node_type": "VHS_VideoCombine",
                        "exception_message": "ffmpeg exited with code 1",
                    }],
                ],
            },
        });
        assert_eq!(
            interpret_history(&entry),
            HistoryVerdict::Failed(
                "ComfyUI execution error in node 14 (VHS_VideoCombine): ffmpeg exited with code 1"
                    .to_string()
            )
        );
    }

    #[test]
    fn defect_4_a_status_of_error_fails_even_without_a_message() {
        let entry = json!({
            "outputs": {},
            "status": { "status_str": "error", "completed": false, "messages": [] },
        });
        match interpret_history(&entry) {
            HistoryVerdict::Failed(msg) => assert!(msg.contains("status 'error'"), "{}", msg),
            other => panic!("expected a failure, got {:?}", other),
        }
    }

    #[test]
    fn defect_4_a_cached_run_is_a_success_not_an_error() {
        // `execution_cached` sits in the same messages array as
        // `execution_error`; only the latter means trouble.
        let entry = history(json!({ "9": { "images": [file("a.png")] } }), true);
        assert!(matches!(
            interpret_history(&entry),
            HistoryVerdict::Outputs(_)
        ));
    }
}
