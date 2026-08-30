//! What a generated file says about how it was made.
//!
//! A synthetic photograph indistinguishable from a real one, sitting in a family
//! archive, is the failure mode this exists to prevent. Ten years from now the
//! only thing that can still answer "was this a memory or a machine?" is the
//! record written at the moment the file was made.
//!
//! # Forward compatibility
//!
//! The pipeline is going to grow — generation lines, multi-stage runs, seeds,
//! the compiled prompt. So this is a *versioned object with optional fields*,
//! not a struct every later change has to migrate:
//!
//! * [`MANIFEST_VERSION`] says which shape wrote it. Readers must tolerate a
//!   higher number rather than reject it.
//! * Everything but the four facts that are always true is `Option`, so a field
//!   a later stage fills in simply is not there yet.
//! * `extra` catches keys this binary does not know, so a manifest written by a
//!   newer Phos survives being read and re-written by an older one.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The shape written by this build. Bump only when an existing field changes
/// meaning — adding an optional field does not need it.
pub const MANIFEST_VERSION: u32 = 1;

/// The generator that made the file. One today; the field exists so a second
/// one does not need a schema change.
pub const GENERATOR_COMFYUI: &str = "comfyui";

/// How a generated file came to exist.
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq)]
pub struct ProvenanceManifest {
    /// Which shape of this record was written. See [`MANIFEST_VERSION`].
    pub version: u32,
    /// What produced the file — [`GENERATOR_COMFYUI`] today.
    pub generator: String,
    /// When Phos brought the file into the library, UTC, `YYYY-MM-DD HH:MM:SS`.
    pub generated_at: String,
    /// The `enhancement_tasks` row that produced it.
    pub task_id: String,
    /// The workflow that was run.
    pub workflow_id: String,
    /// The text the user put into the workflow, node id → prompt.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    #[schema(value_type = Object)]
    pub text_overrides: serde_json::Map<String, serde_json::Value>,
    /// The typed values this run ran with, `"<node_id>.<field_name>"` → value,
    /// exactly as the task row carried them.
    ///
    /// This is what makes a fanned-out run reproducible one file at a time: four
    /// takes of the same prompt differ only here, and without it their manifests
    /// are indistinguishable.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    #[schema(value_type = Object)]
    pub parameters: serde_json::Map<String, serde_json::Value>,
    /// ComfyUI's own id for the run, which is how its history can still be
    /// matched up if the server keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfyui_prompt_id: Option<String>,
    /// The library file the run was given to work from, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file_id: Option<String>,
    /// The name ComfyUI gave the output, before Phos renamed it into the
    /// library — the last thread back to the run that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_filename: Option<String>,

    // ── Filled in by later stages of the pipeline (FR4 / FR5 / FR9). ──
    /// The generation line this file belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    /// Position of this run in a multi-stage line, zero-based.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_index: Option<i64>,
    /// The seed the sampler actually ran with, lifted out of [`Self::parameters`]
    /// because it is the value asked for most often. A graph with two samplers
    /// has two seeds; this is the first in key order, and the map beside it is
    /// the complete answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// The prompt as submitted, after every override was applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub compiled_prompt: Option<serde_json::Value>,

    /// Anything a newer Phos wrote that this one has no field for. Kept so a
    /// round trip through an older binary does not quietly drop provenance.
    ///
    /// Flattened, so it has no name of its own in the JSON and nothing to
    /// publish in the schema — a client generated from an older spec simply
    /// does not see the newer keys, which is the point.
    #[serde(flatten)]
    #[schema(ignore)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ProvenanceManifest {
    /// The record for one finished ComfyUI run.
    ///
    /// Both override maps are the task's stored JSON; either one being
    /// unreadable costs that map, never the rest of the record.
    pub fn for_comfyui_run(run: &ComfyuiRun) -> Self {
        let parameters: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(run.parameters).unwrap_or_default();
        Self {
            version: MANIFEST_VERSION,
            generator: GENERATOR_COMFYUI.to_string(),
            generated_at: run.generated_at.to_string(),
            task_id: run.task_id.to_string(),
            workflow_id: run.workflow_id.to_string(),
            text_overrides: serde_json::from_str(run.text_overrides).unwrap_or_default(),
            seed: seed_of(&parameters),
            parameters,
            comfyui_prompt_id: run.comfyui_prompt_id.map(str::to_string),
            source_file_id: run.source_file_id.map(str::to_string),
            output_filename: run.output_filename.map(str::to_string),
            line_id: None,
            stage_index: None,
            compiled_prompt: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// One finished ComfyUI run, as the completion path knows it.
///
/// A struct rather than eight positional arguments: two of them are `&str` of
/// stored JSON — the prompts and the typed values — and swapping them would
/// file a seed where a prompt belongs without ever failing to compile.
pub struct ComfyuiRun<'a> {
    pub task_id: &'a str,
    pub workflow_id: &'a str,
    /// The task's stored `text_overrides` JSON.
    pub text_overrides: &'a str,
    /// The task's stored `parameters` JSON: the resolved typed values this run
    /// was queued with — one task's worth of a fan-out.
    pub parameters: &'a str,
    pub comfyui_prompt_id: Option<&'a str>,
    pub source_file_id: Option<&'a str>,
    pub output_filename: Option<&'a str>,
    /// UTC, `YYYY-MM-DD HH:MM:SS`.
    pub generated_at: &'a str,
}

/// The seed a parameter map names, if it names one.
///
/// The map is keyed `"<node_id>.<field_name>"`, so this is a question about
/// field names: ComfyUI's samplers call it `seed`, and `KSamplerAdvanced` calls
/// it `noise_seed`. Anything that is not a whole number is not a seed — a graph
/// whose author wired that socket from another node has no literal to record.
fn seed_of(parameters: &serde_json::Map<String, serde_json::Value>) -> Option<i64> {
    parameters
        .iter()
        .filter(|(key, _)| is_seed_field(key))
        .find_map(|(_, value)| value.as_i64())
}

fn is_seed_field(key: &str) -> bool {
    matches!(
        key.rsplit_once('.').map(|(_, field)| field),
        Some("seed") | Some("noise_seed")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A run with prompts but no typed values: every task queued before FR4,
    /// and every workflow whose fields the catalogue could not describe.
    fn a_run() -> ProvenanceManifest {
        ProvenanceManifest::for_comfyui_run(&ComfyuiRun {
            task_id: "task-1234",
            workflow_id: "wf-portrait",
            text_overrides: r#"{"6":"a lighthouse at dusk"}"#,
            parameters: "{}",
            comfyui_prompt_id: Some("prompt-abcd"),
            source_file_id: Some("file-source"),
            output_filename: Some("task-1234_00001.png"),
            generated_at: "2026-08-30 12:00:00",
        })
    }

    /// The same run, queued with the typed values FR4 records.
    fn a_typed_run(parameters: &str) -> ProvenanceManifest {
        ProvenanceManifest::for_comfyui_run(&ComfyuiRun {
            parameters,
            ..ComfyuiRun {
                task_id: "task-1234",
                workflow_id: "wf-portrait",
                text_overrides: r#"{"6":"a lighthouse at dusk"}"#,
                parameters: "{}",
                comfyui_prompt_id: Some("prompt-abcd"),
                source_file_id: Some("file-source"),
                output_filename: Some("task-1234_00001.png"),
                generated_at: "2026-08-30 12:00:00",
            }
        })
    }

    #[test]
    fn a_finished_run_records_what_made_the_file() {
        let m = a_run();
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["version"], json!(1));
        assert_eq!(v["generator"], json!("comfyui"));
        assert_eq!(v["task_id"], json!("task-1234"));
        assert_eq!(v["workflow_id"], json!("wf-portrait"));
        assert_eq!(v["comfyui_prompt_id"], json!("prompt-abcd"));
        assert_eq!(v["source_file_id"], json!("file-source"));
        assert_eq!(v["output_filename"], json!("task-1234_00001.png"));
        assert_eq!(v["text_overrides"]["6"], json!("a lighthouse at dusk"));
    }

    /// Fields the later stages will fill in are absent, not null — so adding a
    /// value later is a write, never a migration.
    #[test]
    fn the_fields_later_stages_fill_in_are_simply_absent() {
        let v: serde_json::Value = serde_json::to_value(a_run()).unwrap();
        let obj = v.as_object().unwrap();
        for later in ["line_id", "stage_index", "compiled_prompt", "parameters"] {
            assert!(
                !obj.contains_key(later),
                "{later} should not be written yet"
            );
        }
        // A run with no typed values has no seed to name either, and says so by
        // omission rather than by a null.
        assert!(!obj.contains_key("seed"));
    }

    // === FR4 — the values that make a take reproducible ======================

    #[test]
    fn a_run_records_the_typed_values_it_ran_with() {
        let m = a_typed_run(
            r#"{"3.seed":4242,"3.steps":28,"3.cfg":6.5,
                "4.ckpt_name":"sd_xl_base_1.0.safetensors","12.add_noise":true}"#,
        );
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["parameters"]["3.seed"], json!(4242));
        assert_eq!(v["parameters"]["3.steps"], json!(28));
        assert_eq!(v["parameters"]["3.cfg"], json!(6.5));
        assert_eq!(
            v["parameters"]["4.ckpt_name"],
            json!("sd_xl_base_1.0.safetensors")
        );
        assert_eq!(v["parameters"]["12.add_noise"], json!(true));
        // Types survive: a seed read back as a string reproduces nothing.
        assert!(v["parameters"]["3.seed"].is_i64());
        // And the prompt half is still where it was.
        assert_eq!(v["text_overrides"]["6"], json!("a lighthouse at dusk"));
    }

    #[test]
    fn the_seed_is_lifted_out_of_the_map_so_the_question_has_one_answer() {
        assert_eq!(
            a_typed_run(r#"{"3.seed":4242,"3.steps":28}"#).seed,
            Some(4242)
        );
        // KSamplerAdvanced spells it differently.
        assert_eq!(a_typed_run(r#"{"3.noise_seed":77}"#).seed, Some(77));
        // A field merely containing the word is not one.
        assert_eq!(a_typed_run(r#"{"3.seed_offset":5}"#).seed, None);
        // Neither is a value that is not a whole number.
        assert_eq!(a_typed_run(r#"{"3.seed":"random"}"#).seed, None);
        assert_eq!(a_typed_run(r#"{"3.seed":1.5}"#).seed, None);
        // Nothing typed at all: nothing to name.
        assert_eq!(a_typed_run("{}").seed, None);
        // Two samplers: the first in key order answers, and the map beside it
        // still carries both.
        let two = a_typed_run(r#"{"3.seed":11,"9.noise_seed":22}"#);
        assert_eq!(two.seed, Some(11));
        assert_eq!(two.parameters["9.noise_seed"], json!(22));
    }

    #[test]
    fn a_take_out_of_a_fan_out_is_told_apart_by_its_manifest() {
        // Four takes of one prompt: identical but for the seed, which is the
        // whole reason the seed has to be in the record.
        let takes: Vec<ProvenanceManifest> = [1000, 1001, 1002, 1003]
            .iter()
            .map(|seed| a_typed_run(&format!(r#"{{"3.seed":{},"3.steps":28}}"#, seed)))
            .collect();
        let seeds: Vec<Option<i64>> = takes.iter().map(|m| m.seed).collect();
        assert_eq!(seeds, [Some(1000), Some(1001), Some(1002), Some(1003)]);
        // Distinguishable as whole records, not just in the convenience field.
        let records: std::collections::HashSet<String> = takes
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect();
        assert_eq!(records.len(), 4, "two takes wrote the same manifest");
    }

    #[test]
    fn unreadable_parameters_cost_the_parameters_and_nothing_else() {
        let m = a_typed_run("not json at all");
        assert!(m.parameters.is_empty());
        assert_eq!(m.seed, None);
        assert_eq!("task-1234", m.task_id);
        assert_eq!(
            Some(&json!("a lighthouse at dusk")),
            m.text_overrides.get("6")
        );
    }

    #[test]
    fn a_manifest_written_before_the_parameters_existed_still_reads() {
        // Every generated file already in a library: no `parameters` key at all.
        let old = json!({
            "version": 1, "generator": "comfyui",
            "generated_at": "2026-08-30 12:00:00",
            "task_id": "t", "workflow_id": "w",
            "text_overrides": { "6": "a lighthouse at dusk" },
        });
        let parsed: ProvenanceManifest = serde_json::from_value(old).unwrap();
        assert!(parsed.parameters.is_empty());
        assert_eq!(parsed.seed, None);
    }

    /// The point of the version and the catch-all: a manifest from a newer Phos
    /// reads here, and comes back out with everything it arrived with.
    #[test]
    fn a_manifest_from_a_newer_phos_survives_a_round_trip() {
        let newer = json!({
            "version": 7,
            "generator": "comfyui",
            "generated_at": "2036-01-01 00:00:00",
            "task_id": "t",
            "workflow_id": "w",
            "line_id": "line-9",
            "stage_index": 2,
            "seed": 42,
            "provenance_signature": "0xdeadbeef",
            "c2pa_claim": { "issuer": "phos" },
        });
        let parsed: ProvenanceManifest = serde_json::from_value(newer.clone()).unwrap();

        assert_eq!(7, parsed.version, "a higher version is read, not rejected");
        assert_eq!(Some("line-9".to_string()), parsed.line_id);
        assert_eq!(Some(2), parsed.stage_index);
        assert_eq!(Some(42), parsed.seed);

        let back = serde_json::to_value(&parsed).unwrap();
        assert_eq!(
            newer["provenance_signature"], back["provenance_signature"],
            "a field this build has no name for must not be dropped"
        );
        assert_eq!(newer["c2pa_claim"], back["c2pa_claim"]);
    }

    /// Overrides that will not parse cost the overrides, not the manifest.
    #[test]
    fn unparseable_overrides_do_not_take_the_rest_of_the_record_down() {
        let m = ProvenanceManifest::for_comfyui_run(&ComfyuiRun {
            task_id: "t",
            workflow_id: "w",
            text_overrides: "not json at all",
            parameters: "{}",
            comfyui_prompt_id: None,
            source_file_id: None,
            output_filename: None,
            generated_at: "2026-08-30 12:00:00",
        });
        assert!(m.text_overrides.is_empty());
        assert_eq!("t", m.task_id);
    }
}
