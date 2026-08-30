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
    /// The seed the sampler actually ran with.
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
    /// `text_overrides` is the task's stored JSON; anything unparseable becomes
    /// an empty object rather than losing the rest of the manifest.
    pub fn for_comfyui_run(
        task_id: &str,
        workflow_id: &str,
        text_overrides: &str,
        comfyui_prompt_id: Option<&str>,
        source_file_id: Option<&str>,
        output_filename: Option<&str>,
        generated_at: String,
    ) -> Self {
        Self {
            version: MANIFEST_VERSION,
            generator: GENERATOR_COMFYUI.to_string(),
            generated_at,
            task_id: task_id.to_string(),
            workflow_id: workflow_id.to_string(),
            text_overrides: serde_json::from_str(text_overrides).unwrap_or_default(),
            comfyui_prompt_id: comfyui_prompt_id.map(str::to_string),
            source_file_id: source_file_id.map(str::to_string),
            output_filename: output_filename.map(str::to_string),
            line_id: None,
            stage_index: None,
            seed: None,
            compiled_prompt: None,
            extra: serde_json::Map::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn a_run() -> ProvenanceManifest {
        ProvenanceManifest::for_comfyui_run(
            "task-1234",
            "wf-portrait",
            r#"{"6":"a lighthouse at dusk"}"#,
            Some("prompt-abcd"),
            Some("file-source"),
            Some("task-1234_00001.png"),
            "2026-08-30 12:00:00".to_string(),
        )
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
        for later in ["line_id", "stage_index", "seed", "compiled_prompt"] {
            assert!(
                !obj.contains_key(later),
                "{later} should not be written yet"
            );
        }
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
        let m = ProvenanceManifest::for_comfyui_run(
            "t",
            "w",
            "not json at all",
            None,
            None,
            None,
            "2026-08-30 12:00:00".to_string(),
        );
        assert!(m.text_overrides.is_empty());
        assert_eq!("t", m.task_id);
    }
}
