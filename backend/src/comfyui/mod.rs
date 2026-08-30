//! ComfyUI integration: running a user's workflow against a shot and bringing
//! the result back into the library.
//!
//! # The pipeline
//!
//! A row in `enhancement_tasks` walks one path, driven by a per-library
//! background thread ([`worker`]) that wakes every three seconds:
//!
//! ```text
//!   pending ──> uploading ──> queued ──> processing ──> downloading ──> completed
//!      ^                                     │                │
//!      │                                     v                │
//!      │                            awaiting_output ──────────┘
//!      │                                     │
//!      └────── retry (transient) ────────────┴──> failed / cancelled
//! ```
//!
//! [`worker::dispatch`] owns the left half: read the shot's source image, push
//! it to ComfyUI, rewrite the graph for this run, queue the prompt.
//! [`worker::complete`] owns the right half: read `/history` until the prompt
//! resolves, then download what it named.
//!
//! # Why it is split this way
//!
//! The bug this module was reorganised around was a *reading* bug — a finished
//! run reported as a failure — so the code that reads and decides is kept
//! separate from the code that does IO, and is pure:
//!
//! * [`history`] — what did ComfyUI say? Outputs, still running, or an error.
//! * [`policy`] — what do we do about it? How long to wait for a file that has
//!   not appeared, and whether a failure is worth another attempt.
//! * [`workflow`] — what does this graph take and produce, and how do we
//!   rewrite it for one run?
//! * [`loaders`] — which node reads the file we uploaded, and which slot of a
//!   multi-input graph does it fill?
//! * [`contract`] — put those together: what does this workflow *accept* and
//!   what does it *produce*, so it can be a stage in a line?
//! * [`nodes`] — what does ComfyUI say its installed node classes take?
//! * [`overrides`] — so which of this graph's fields can a person change, and
//!   what kind of control does each one want?
//! * [`params`] — what did they set them to, and when they asked for a sweep
//!   rather than a value, which runs does that come to?
//!
//! Those all take `serde_json::Value` in and give an answer out, so the whole
//! completion path is tested against recorded `/history` payloads with no
//! server involved. [`client`] holds the HTTP calls and decides nothing;
//! [`worker`] holds the database writes.
//!
//! # Three things worth knowing
//!
//! * **Output filenames are pinned before the run starts.** Every saver's
//!   `filename_prefix` is rewritten to `phos/<task_id>-<attempt>`, so when
//!   history is empty, unhelpful, or lost to a ComfyUI restart, the file can
//!   still be fetched from `/view` by a name we already know. The attempt token
//!   is fresh per dispatch, because ComfyUI keeps an earlier run's file and
//!   advances the counter — a shared prefix would find the stale one.
//! * **"Finished but no file yet" is a state, not a verdict.** It is
//!   `awaiting_output`, budgeted at a minute for images and a quarter of an
//!   hour for video, because `VHS_VideoCombine` shells out to ffmpeg and lands
//!   in history well before the mp4 is closed. A file history names but `/view`
//!   still 404s is the same state and gets the same budget.
//! * **A cancelled row is never written again by the worker.** Cancel claims
//!   the row with one conditional update before talking to ComfyUI; every
//!   worker write filters `status != cancelled`, so a worker mid-flight cannot
//!   move the task back.
//! * **Failures are split by site.** A refused graph or a node exception fails
//!   at once with the real message; a dropped connection backs off and tries
//!   again.
//! * **Everything this module creates is marked synthetic.** A generated file
//!   is flagged on its row and carries a [`manifest::ProvenanceManifest`]. The
//!   flag keeps machine-made faces out of the person model; the manifest is how
//!   anyone, years from now, can still tell which memories were real.
//! * **A video can go in whole.** [`source`] decides what a run consumes — a
//!   frame of the clip (the first, the last, one at a timestamp, one of the
//!   indexed keyframes) or the file itself, which is the default whenever the
//!   graph has a video loader to read it.

mod client;
pub mod contract;
mod history;
mod loaders;
mod manifest;
pub mod nodes;
mod outputs;
mod overrides;
pub mod params;
mod policy;
mod source;
mod timestamp;
mod worker;
mod workflow;

pub use client::ComfyUiClient;
pub use contract::{Accepts, ContractCorrections, MediaType, ParamName, StageContract};
pub use loaders::{
    check_source_kind, default_binding_warnings, detect_loaders, importable, takes_video,
    LoaderKind, SourceRole,
};
pub use manifest::ProvenanceManifest;
pub use nodes::NodeCatalog;
pub use overrides::detect_inputs;
// The contract test reads it through the library crate; the binary compiles
// this module directly and has no use for it, which is what the allow is for.
#[allow(unused_imports)]
pub use overrides::WorkflowInput;
pub use params::{check_sweep_targets, expand, ParameterMap, VaryMap, VaryMode, VarySpec};
pub use source::SourceMode;
pub use worker::spawn_enhancement_worker;
pub use workflow::detect_outputs;

/// The node catalogue for this server, read from `/object_info` and cached
/// for a few minutes, or until the health check sees ComfyUI come back from
/// being down.
///
/// `None` whenever nothing could be learned — unreachable, too old to have the
/// endpoint, or an answer nothing parsed out of. Every caller treats that as
/// ordinary and falls back.
///
/// Blocking: call it from `spawn_blocking`, not from an async handler.
pub fn node_catalog(client: &ComfyUiClient) -> Option<std::sync::Arc<NodeCatalog>> {
    nodes::catalog_for(client)
}

/// [`node_catalog`], but read from ComfyUI now regardless of what is cached —
/// for a client that knows the installed models just changed.
///
/// Blocking, like [`node_catalog`].
pub fn refresh_node_catalog(client: &ComfyUiClient) -> Option<std::sync::Arc<NodeCatalog>> {
    nodes::refresh_for(client)
}

/// Statuses the worker owns, and that the API and queue UI switch on.
///
/// The rest of the vocabulary — `pending`, `uploading`, `queued`, `processing`,
/// `downloading`, `completed`, `failed` — is written inline where it is set.
/// These two are named because they are read outside this module.
pub const STATUS_AWAITING_OUTPUT: &str = "awaiting_output";
pub const STATUS_CANCELLED: &str = "cancelled";

#[cfg(test)]
mod tests {
    use super::history::{fixtures::history, interpret_history, HistoryVerdict};
    use super::outputs::OutputRef;
    use super::policy::{decide_settle, settle_budget, SettleDecision, SETTLE_BUDGET_VIDEO};
    use super::timestamp::parse_ts;
    use serde_json::json;

    /// The reported symptom, walked end to end across the modules that fix it.
    #[test]
    fn the_reported_symptom_no_longer_reads_as_a_failure() {
        // "the workflow is done but the result file is not in the response":
        // a completed VHS_VideoCombine run whose file has not been published yet.
        let wf = json!({
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff" } }
        });
        let entry = history(json!({}), true);

        // Step 1: reading history says "nothing yet", not "failed".
        assert_eq!(interpret_history(&entry), HistoryVerdict::NoOutputs);

        // Step 2: the task settles, with fifteen minutes rather than ten seconds.
        let now = parse_ts("2026-08-30 12:00:00").unwrap();
        let budget = settle_budget(&wf);
        assert_eq!(budget, SETTLE_BUDGET_VIDEO);
        let SettleDecision::Start { deadline, .. } = decide_settle(now, None, budget) else {
            panic!("should have started settling");
        };
        assert_eq!(deadline, parse_ts("2026-08-30 12:15:00").unwrap());

        // Step 3: meanwhile the file is findable by the name we pinned, even
        // though history never mentioned it.
        let prefix = super::workflow::output_prefix_for_task("task-1234", "a1b2c3d4");
        assert_eq!(prefix, "phos/task-1234-a1b2c3d4");
        let suffixes = super::workflow::expected_output_suffixes(&wf);
        assert!(
            super::outputs::fallback_output_candidates(&prefix, &suffixes)
                .iter()
                .any(|c| c.filename == "task-1234-a1b2c3d4_00001.mp4" && c.subfolder == "phos")
        );

        // Step 4: when it does show up under a key nobody enumerated, it counts.
        let late = history(
            json!({ "12": { "gifs": [
                { "filename": "task-1234_00001.mp4", "subfolder": "phos", "type": "output" }
            ] } }),
            true,
        );
        assert_eq!(
            interpret_history(&late),
            HistoryVerdict::Outputs(vec![OutputRef {
                filename: "task-1234_00001.mp4".to_string(),
                subfolder: "phos".to_string(),
                output_type: "output".to_string(),
            }])
        );
    }
}
