use crate::db;
use crate::models::NewFile;
use crate::scanner;
use crate::schema::{comfyui_workflows, enhancement_tasks, files};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

/// One client id for the whole process, so ComfyUI can attribute our prompts to
/// us. Sent on `/prompt`; it is also the id a future `/ws` listener would
/// subscribe with, which is why it has to be stable rather than per-request.
fn client_id() -> &'static str {
    static CLIENT_ID: OnceLock<String> = OnceLock::new();
    CLIENT_ID.get_or_init(|| format!("phos-{}", Uuid::new_v4().simple()))
}

// ---------------------------------------------------------------------------
// ComfyUI HTTP client
// ---------------------------------------------------------------------------

pub struct ComfyUiClient {
    base_url: String,
}

impl ComfyUiClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Check if ComfyUI is reachable.
    pub fn health_check(&self) -> anyhow::Result<()> {
        let url = format!("{}/system_stats", self.base_url);
        let resp = ureq::get(&url)
            .call()
            .map_err(|e| anyhow::anyhow!("ComfyUI health check failed: {}", e))?;
        if resp.status() != 200 {
            anyhow::bail!("ComfyUI returned status {}", resp.status());
        }
        Ok(())
    }

    /// Upload an image to ComfyUI's /upload/image endpoint using manual multipart.
    pub fn upload_image(&self, filename: &str, image_data: &[u8]) -> anyhow::Result<String> {
        let boundary = format!("----PhosUpload{}", Uuid::new_v4().simple());

        let mut body = Vec::new();
        // image field
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"image\"; filename=\"{}\"\r\n",
                filename
            )
            .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
        body.extend_from_slice(image_data);
        body.extend_from_slice(b"\r\n");
        // overwrite field (always true so repeated uploads of same name work)
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"overwrite\"\r\n\r\n");
        body.extend_from_slice(b"true\r\n");
        // closing boundary
        body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

        let url = format!("{}/upload/image", self.base_url);
        let content_type = format!("multipart/form-data; boundary={}", boundary);

        let mut resp = ureq::post(&url)
            .header("Content-Type", &content_type)
            .send(body.as_slice())
            .map_err(|e| anyhow::anyhow!("Upload failed: {}", e))?;

        let json: Value = resp.body_mut().read_json()?;
        let name = json["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No 'name' in upload response"))?;
        Ok(name.to_string())
    }

    /// Queue a prompt (workflow JSON) on ComfyUI.
    pub fn queue_prompt(&self, workflow: &Value) -> anyhow::Result<String> {
        let payload = serde_json::json!({ "prompt": workflow, "client_id": client_id() });
        let url = format!("{}/prompt", self.base_url);

        let bytes = serde_json::to_vec(&payload)?;
        // Read the body ourselves rather than letting ureq turn a 4xx into a bare
        // status: ComfyUI answers 400 with JSON naming the offending node, and
        // that JSON is the only thing that tells the user what to fix.
        let mut resp = ureq::post(&url)
            .header("Content-Type", "application/json")
            .config()
            .http_status_as_error(false)
            .build()
            .send(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("Queue prompt failed: {}", e))?;

        let status = resp.status().as_u16();
        let json: Value = resp.body_mut().read_json().unwrap_or(Value::Null);

        // A prompt ComfyUI refuses is refused for good — a bad graph does not
        // become good on the third try — so this is reported as a validation
        // error, which `classify_failure` reads as permanent.
        if status >= 400 || json.get("error").is_some() {
            let error_msg = json
                .get("error")
                .and_then(|e| {
                    e.get("message")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("HTTP {}", status));
            let node_errors = json
                .get("node_errors")
                .filter(|v| {
                    !matches!(v, Value::Null) && v.as_object().is_none_or(|o| !o.is_empty())
                })
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default();
            if node_errors.is_empty() {
                anyhow::bail!("{}: {}", PROMPT_REJECTED, error_msg);
            } else {
                anyhow::bail!(
                    "{}: {}. Node errors: {}",
                    PROMPT_REJECTED,
                    error_msg,
                    node_errors
                );
            }
        }

        let prompt_id = json["prompt_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No 'prompt_id' in queue response: {}", json))?;
        Ok(prompt_id.to_string())
    }

    /// Ask ComfyUI to stop whatever it is executing right now.
    pub fn interrupt(&self) -> anyhow::Result<()> {
        let url = format!("{}/interrupt", self.base_url);
        ureq::post(&url)
            .header("Content-Type", "application/json")
            .send(b"{}".as_slice())
            .map_err(|e| anyhow::anyhow!("Interrupt failed: {}", e))?;
        Ok(())
    }

    /// Drop a prompt from ComfyUI's pending queue. A no-op if it already ran.
    pub fn delete_queued(&self, prompt_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/queue", self.base_url);
        let payload = serde_json::json!({ "delete": [prompt_id] });
        let bytes = serde_json::to_vec(&payload)?;
        ureq::post(&url)
            .header("Content-Type", "application/json")
            .send(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("Queue delete failed: {}", e))?;
        Ok(())
    }

    /// Is this prompt the one ComfyUI is executing right now (as opposed to
    /// merely queued)? Only the running one is worth an `/interrupt`.
    pub fn is_prompt_running(&self, prompt_id: &str) -> anyhow::Result<bool> {
        let url = format!("{}/queue", self.base_url);
        let mut resp = ureq::get(&url)
            .call()
            .map_err(|e| anyhow::anyhow!("Queue fetch failed: {}", e))?;
        let json: Value = resp.body_mut().read_json()?;
        Ok(queue_contains(&json, "queue_running", prompt_id))
    }

    /// Get execution history for a prompt.
    pub fn get_history(&self, prompt_id: &str) -> anyhow::Result<Option<Value>> {
        let url = format!("{}/history/{}", self.base_url, prompt_id);
        let mut resp = ureq::get(&url)
            .call()
            .map_err(|e| anyhow::anyhow!("History fetch failed: {}", e))?;
        let json: Value = resp.body_mut().read_json()?;
        if let Some(entry) = json.get(prompt_id) {
            Ok(Some(entry.clone()))
        } else {
            Ok(None)
        }
    }

    /// Check if a prompt is still in ComfyUI's queue (pending or running).
    pub fn is_prompt_in_queue(&self, prompt_id: &str) -> anyhow::Result<bool> {
        let url = format!("{}/queue", self.base_url);
        let mut resp = ureq::get(&url)
            .call()
            .map_err(|e| anyhow::anyhow!("Queue fetch failed: {}", e))?;
        let json: Value = resp.body_mut().read_json()?;
        Ok(queue_contains(&json, "queue_running", prompt_id)
            || queue_contains(&json, "queue_pending", prompt_id))
    }

    /// Download an output file from ComfyUI.
    ///
    /// Errors name the status, the file and the URL. The old message said only
    /// "Download failed", and the caller then reported "no output images found",
    /// which pointed the user at their workflow when the real answer was a 404.
    pub fn download_output(&self, out: &OutputRef) -> anyhow::Result<Vec<u8>> {
        let url = self.view_url(out);
        let mut resp = ureq::get(&url).call().map_err(|e| match e {
            ureq::Error::StatusCode(status) => anyhow::anyhow!(
                "ComfyUI /view returned HTTP {} for {} ({})",
                status,
                out.describe(),
                url
            ),
            other => anyhow::anyhow!("Download of {} failed: {}", out.describe(), other),
        })?;
        let bytes = resp.body_mut().read_to_vec()?;
        // A zero-byte answer means the file exists but is still being written;
        // treat it as a miss so the settle loop keeps waiting instead of saving
        // an empty file over the user's library.
        if bytes.is_empty() {
            anyhow::bail!("ComfyUI returned an empty body for {}", out.describe());
        }
        Ok(bytes)
    }

    fn view_url(&self, out: &OutputRef) -> String {
        format!(
            "{}/view?filename={}&subfolder={}&type={}",
            self.base_url,
            urlencoding::encode(&out.filename),
            urlencoding::encode(&out.subfolder),
            urlencoding::encode(&out.output_type),
        )
    }
}

/// `queue_running`/`queue_pending` are arrays of `[number, prompt_id, ...]`.
fn queue_contains(queue: &Value, key: &str, prompt_id: &str) -> bool {
    queue
        .get(key)
        .and_then(|v| v.as_array())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.as_array()
                    .and_then(|arr| arr.get(1))
                    .and_then(|v| v.as_str())
                    == Some(prompt_id)
            })
        })
}

// ---------------------------------------------------------------------------
// Reading what ComfyUI said
//
// Everything below is a pure function over `serde_json::Value` so the whole
// completion path can be exercised against recorded `/history` payloads without
// a live ComfyUI. The HTTP calls stay on `ComfyUiClient`.
// ---------------------------------------------------------------------------

/// Marker in the message of a prompt ComfyUI refused. A refused graph is
/// refused for good, so `classify_failure` reads this as permanent.
pub const PROMPT_REJECTED: &str = "ComfyUI rejected the prompt";

/// The timestamp format every column in this table uses.
const TS_FMT: &str = "%Y-%m-%d %H:%M:%S";

fn format_ts(dt: chrono::NaiveDateTime) -> String {
    dt.format(TS_FMT).to_string()
}

fn parse_ts(raw: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(raw.trim(), TS_FMT)
        .ok()
        // Sqlite's CURRENT_TIMESTAMP and chrono both round-trip this, but a row
        // written with sub-second precision should not derail the settle clock.
        .or_else(|| chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S%.f").ok())
}

/// One file ComfyUI says it wrote, in the terms `/view` wants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutputRef {
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
pub fn collect_output_refs(outputs: Option<&Value>) -> Vec<OutputRef> {
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

/// What a `/history/{prompt_id}` entry means for the task that queued it.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryVerdict {
    /// Still executing; nothing to decide yet.
    Running,
    /// Finished, and named these files.
    Outputs(Vec<OutputRef>),
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
pub fn interpret_history(entry: &Value) -> HistoryVerdict {
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
    if refs.is_empty() {
        HistoryVerdict::NoOutputs
    } else {
        HistoryVerdict::Outputs(refs)
    }
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
pub fn execution_error_traceback(entry: &Value) -> Option<String> {
    let tb = execution_error_data(entry)?
        .get("traceback")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("");
    (!tb.is_empty()).then_some(tb)
}

// ---------------------------------------------------------------------------
// Deterministic output names
// ---------------------------------------------------------------------------

/// The subfolder every Phos-queued workflow is made to write into.
pub const OUTPUT_SUBFOLDER: &str = "phos";

/// The `filename_prefix` a task's output nodes are rewritten to. Knowing this
/// before the run starts is what lets Phos find a file when history is empty,
/// unhelpful, or gone with a ComfyUI restart.
pub fn output_prefix_for_task(task_id: &str) -> String {
    format!("{}/{}", OUTPUT_SUBFOLDER, task_id)
}

/// Where ComfyUI would have put a task's files, given the prefix it was told to
/// use. Probed when history names nothing — a file on disk beats a silent
/// history entry.
///
/// The suffixes are the ones ComfyUI's own savers produce: `SaveImage` and
/// friends append `_00001_`, the video combiners append `_00001`.
pub fn fallback_output_candidates(output_prefix: &str) -> Vec<OutputRef> {
    let (subfolder, stem) = match output_prefix.rsplit_once('/') {
        Some((dir, stem)) => (dir.to_string(), stem.to_string()),
        None => (String::new(), output_prefix.to_string()),
    };
    let mut out = Vec::new();
    for counter in ["_00001_", "_00001"] {
        for ext in ["png", "webp", "jpg", "mp4", "webm", "gif", "flac", "mp3"] {
            out.push(OutputRef {
                filename: format!("{}{}.{}", stem, counter, ext),
                subfolder: subfolder.clone(),
                output_type: "output".to_string(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Settling: "no files yet" is a state, not a verdict
// ---------------------------------------------------------------------------

/// Budget for an image-only workflow. Files are closed by the time the history
/// entry lands; this only covers the write-then-publish gap.
pub const SETTLE_BUDGET_IMAGE: Duration = Duration::from_secs(60);

/// Budget for a workflow with a video output. `VHS_VideoCombine` shells out to
/// ffmpeg, so a large mp4 can be in history minutes before it is on disk.
pub const SETTLE_BUDGET_VIDEO: Duration = Duration::from_secs(15 * 60);

/// How long a completed-but-empty prompt is given to publish its files.
pub fn settle_budget(workflow: &Value) -> Duration {
    if workflow_has_slow_output(workflow) {
        SETTLE_BUDGET_VIDEO
    } else {
        SETTLE_BUDGET_IMAGE
    }
}

/// Does this workflow save something that is muxed or encoded after the graph
/// finishes? Those are the runs worth waiting a quarter of an hour for.
pub fn workflow_has_slow_output(workflow: &Value) -> bool {
    detect_outputs(workflow).iter().any(|o| {
        let t = o.node_type.to_ascii_lowercase();
        t.contains("video") || t.contains("webm") || t.contains("animated") || t.contains("audio")
    })
}

/// Re-check spacing while settling: tight at first, because most runs settle in
/// a second or two, then backing off so a 15-minute video wait is not 300 polls.
pub fn settle_recheck_delay(elapsed: Duration) -> Duration {
    match elapsed.as_secs() {
        0..=9 => Duration::from_secs(2),
        10..=59 => Duration::from_secs(5),
        60..=299 => Duration::from_secs(15),
        _ => Duration::from_secs(30),
    }
}

/// What to do with a task whose prompt finished without naming a file.
#[derive(Debug, Clone, PartialEq)]
pub enum SettleDecision {
    /// First sighting — start the clock and come back at `recheck_at`.
    Start {
        deadline: chrono::NaiveDateTime,
        recheck_at: chrono::NaiveDateTime,
    },
    /// Inside the budget — probe the deterministic names, then come back.
    Wait { recheck_at: chrono::NaiveDateTime },
    /// The budget is spent. Probe once more, then give up.
    Expired,
}

pub fn decide_settle(
    now: chrono::NaiveDateTime,
    settle_until: Option<chrono::NaiveDateTime>,
    budget: Duration,
) -> SettleDecision {
    let budget_secs = budget.as_secs() as i64;
    match settle_until {
        None => SettleDecision::Start {
            deadline: now + chrono::Duration::seconds(budget_secs),
            recheck_at: now
                + chrono::Duration::seconds(settle_recheck_delay(Duration::ZERO).as_secs() as i64),
        },
        Some(deadline) if now < deadline => {
            let remaining = (deadline - now).num_seconds().max(0);
            let elapsed = Duration::from_secs((budget_secs - remaining).max(0) as u64);
            SettleDecision::Wait {
                recheck_at: now
                    + chrono::Duration::seconds(settle_recheck_delay(elapsed).as_secs() as i64),
            }
        }
        Some(_) => SettleDecision::Expired,
    }
}

// ---------------------------------------------------------------------------
// Transient vs permanent failure
// ---------------------------------------------------------------------------

/// Total attempts a transient failure gets, the first one included.
pub const MAX_ATTEMPTS: i32 = 4;

/// Where a task fell over. The site decides whether trying again can possibly
/// help — a missing source file will still be missing, a dropped connection
/// probably will not be dropped again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureSite {
    /// Could not read/decode the library file being enhanced.
    SourceImage,
    /// Could not push the source image to ComfyUI.
    Upload,
    /// The stored workflow is not valid JSON.
    WorkflowJson,
    /// `/prompt` did not accept the graph.
    Queue,
    /// `/history` could not be read, or the prompt vanished from both history
    /// and queue.
    History,
    /// A node raised during execution.
    Execution,
    /// `/view` refused or truncated a file history had named.
    Download,
    /// The settle budget ran out with nothing on disk.
    Settle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Permanent,
    Transient,
}

pub fn classify_failure(site: FailureSite, message: &str) -> FailureKind {
    match site {
        // Bad input, bad graph, or a node that raised: identical next time.
        FailureSite::SourceImage
        | FailureSite::WorkflowJson
        | FailureSite::Execution
        | FailureSite::Settle => FailureKind::Permanent,
        // A rejected prompt is a validation failure; anything else on /prompt is
        // transport.
        FailureSite::Queue => {
            if message.contains(PROMPT_REJECTED) {
                FailureKind::Permanent
            } else {
                FailureKind::Transient
            }
        }
        FailureSite::Upload | FailureSite::History | FailureSite::Download => {
            FailureKind::Transient
        }
    }
}

/// What to do about a failure, given how many attempts this task has already had.
#[derive(Debug, Clone, PartialEq)]
pub enum FailureAction {
    /// Give up, recording this message.
    Fail(String),
    /// Queue attempt `attempt` of [`MAX_ATTEMPTS`] after `delay`.
    Retry {
        attempt: i32,
        delay: Duration,
        message: String,
    },
}

/// After a transient failure, is the ComfyUI prompt still worth going back to?
///
/// A prompt that already ran does not need to run again — re-polling it is
/// cheaper and far likelier to work than re-executing the graph. Only failures
/// that happened before the prompt was accepted start over.
pub fn retry_resumes_prompt(site: FailureSite) -> bool {
    matches!(site, FailureSite::History | FailureSite::Download)
}

/// Backoff before the next attempt: 5s, 15s, 45s.
pub fn retry_backoff(retry_count: i32) -> Duration {
    let step = retry_count.clamp(0, 4) as u32;
    Duration::from_secs(5 * 3u64.pow(step))
}

/// Decide between another attempt and a final failure. `retry_count` is what the
/// row already records, so the first failure arrives with 0.
pub fn plan_failure(site: FailureSite, message: &str, retry_count: i32) -> FailureAction {
    let attempts_made = retry_count + 1;
    if classify_failure(site, message) == FailureKind::Transient && attempts_made < MAX_ATTEMPTS {
        FailureAction::Retry {
            attempt: attempts_made + 1,
            delay: retry_backoff(retry_count),
            message: message.to_string(),
        }
    } else if attempts_made > 1 {
        FailureAction::Fail(format!("{} (after {} attempts)", message, attempts_made))
    } else {
        FailureAction::Fail(message.to_string())
    }
}

// ---------------------------------------------------------------------------
// Workflow analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub node_id: String,
    pub node_type: String,
    pub field_name: String,
    pub current_value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutput {
    pub node_id: String,
    pub node_type: String,
}

/// Detect input nodes that the user can override.
pub fn detect_inputs(workflow: &Value) -> Vec<WorkflowInput> {
    let mut inputs = Vec::new();
    if let Some(nodes) = workflow.as_object() {
        for (node_id, node) in nodes {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let node_inputs = node.get("inputs");

            match class_type {
                "LoadImage" => {
                    if let Some(inp) = node_inputs {
                        if let Some(val) = inp.get("image") {
                            inputs.push(WorkflowInput {
                                node_id: node_id.clone(),
                                node_type: class_type.to_string(),
                                field_name: "image".to_string(),
                                current_value: val.clone(),
                            });
                        }
                    }
                }
                "CLIPTextEncode" => {
                    if let Some(inp) = node_inputs {
                        if let Some(val) = inp.get("text") {
                            // Only include if text is a string (not a link to another node)
                            if val.is_string() {
                                inputs.push(WorkflowInput {
                                    node_id: node_id.clone(),
                                    node_type: class_type.to_string(),
                                    field_name: "text".to_string(),
                                    current_value: val.clone(),
                                });
                            }
                        }
                    }
                }
                _ => {
                    // Check for String (Multiline) widget pattern
                    if let Some(inp) = node_inputs {
                        if let Some(obj) = inp.as_object() {
                            for (field, val) in obj {
                                if val.is_string()
                                    && (class_type.contains("String")
                                        || class_type.contains("Text"))
                                {
                                    inputs.push(WorkflowInput {
                                        node_id: node_id.clone(),
                                        node_type: class_type.to_string(),
                                        field_name: field.clone(),
                                        current_value: val.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    inputs
}

/// Detect output nodes (SaveImage, SaveVideo, VHS_VideoCombine, etc.).
///
/// Recognised by shape rather than by a fixed list: anything named `Save*` or
/// `Preview*`, plus anything carrying a `filename_prefix` input, which is how
/// every saver in the ecosystem is told where to write. A hardcoded list missed
/// core `SaveVideo` and every custom saver.
pub fn detect_outputs(workflow: &Value) -> Vec<WorkflowOutput> {
    let mut outputs = Vec::new();
    if let Some(nodes) = workflow.as_object() {
        for (node_id, node) in nodes {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if is_output_node(class_type, node) {
                outputs.push(WorkflowOutput {
                    node_id: node_id.clone(),
                    node_type: class_type.to_string(),
                });
            }
        }
    }
    outputs.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    outputs
}

fn is_output_node(class_type: &str, node: &Value) -> bool {
    if class_type.starts_with("Save")
        || class_type.starts_with("Preview")
        || class_type.contains("VideoCombine")
    {
        return true;
    }
    node.get("inputs")
        .and_then(|i| i.get("filename_prefix"))
        .is_some_and(|v| v.is_string())
}

/// Substitute inputs into a workflow copy: set LoadImage.image to the uploaded
/// filename, apply any text overrides, and pin every saver's `filename_prefix`
/// to `output_prefix`.
///
/// Pinning the prefix is what turns a lost history entry from a dead end into a
/// lookup: Phos knows the filename before the run starts, so it can ask `/view`
/// directly instead of depending on ComfyUI to tell it what it wrote.
pub fn prepare_workflow(
    workflow: &Value,
    uploaded_filename: &str,
    text_overrides: &std::collections::HashMap<String, String>,
    output_prefix: Option<&str>,
) -> Value {
    let mut wf = workflow.clone();
    if let Some(nodes) = wf.as_object_mut() {
        for (node_id, node) in nodes.iter_mut() {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if class_type == "LoadImage" {
                if let Some(inputs) = node.get_mut("inputs") {
                    inputs["image"] = Value::String(uploaded_filename.to_string());
                }
            }

            // Apply text overrides keyed by "node_id.field_name"
            if let Some(inputs) = node.get_mut("inputs") {
                if let Some(obj) = inputs.as_object_mut() {
                    for (field, val) in obj.iter_mut() {
                        let key = format!("{}.{}", node_id, field);
                        if let Some(override_val) = text_overrides.get(&key) {
                            if val.is_string() {
                                *val = Value::String(override_val.clone());
                            }
                        }
                    }
                }
            }

            // Every saver in the ecosystem takes `filename_prefix`; overwrite it
            // wherever it is a literal. A prefix wired from another node is left
            // alone — rewriting it would break the link.
            if let Some(prefix) = output_prefix {
                if let Some(existing) = node
                    .get_mut("inputs")
                    .and_then(|i| i.get_mut("filename_prefix"))
                {
                    if existing.is_string() {
                        *existing = Value::String(prefix.to_string());
                    }
                }
            }
        }
    }
    wf
}

// ---------------------------------------------------------------------------
// Source image extraction
// ---------------------------------------------------------------------------

/// Get the source image bytes (PNG-encoded) for a shot.
/// If `source_file_id` is provided, uses that specific file; otherwise falls back to the original.
/// For images: reads the file directly.
/// For videos: extracts the first frame.
fn get_source_image(
    conn: &mut SqliteConnection,
    shot_id: &str,
    source_file_id: Option<&str>,
    library_root: &Path,
) -> anyhow::Result<(Vec<u8>, String)> {
    // If a specific source file is requested, use it; otherwise fall back to the original
    let (file_id_used, file_path, mime_type): (String, String, String) =
        if let Some(file_id) = source_file_id {
            let (fp, mt) = files::table
                .filter(files::id.eq(file_id).and(files::shot_id.eq(shot_id)))
                .select((
                    files::path,
                    diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')"),
                ))
                .first::<(String, String)>(conn)
                .map_err(|_| {
                    anyhow::anyhow!("Source file {} not found for shot {}", file_id, shot_id)
                })?;
            (file_id.to_string(), fp, mt)
        } else {
            let (fid, fp, mt) = files::table
                .filter(files::shot_id.eq(shot_id).and(files::is_original.eq(true)))
                .order(files::created_at.asc())
                .select((
                    files::id,
                    files::path,
                    diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')"),
                ))
                .first::<(String, String, String)>(conn)
                .map_err(|_| anyhow::anyhow!("No original file found for shot {}", shot_id))?;
            (fid, fp, mt)
        };

    let path = db::resolve_path(library_root, &file_path);
    if !path.exists() {
        anyhow::bail!("Source file does not exist: {}", file_path);
    }

    let img: DynamicImage = if mime_type.starts_with("video/") {
        scanner::extract_first_video_frame(&path)?
    } else {
        scanner::open_image(&path)?
    };

    // Encode to PNG bytes
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    img.write_to(&mut cursor, image::ImageFormat::Png)?;

    // Include file ID in the upload name so ComfyUI doesn't reuse a cached image from a different variant
    let upload_name = format!(
        "phos_{}_{}.png",
        &shot_id[..8.min(shot_id.len())],
        &file_id_used[..8.min(file_id_used.len())]
    );
    Ok((buf, upload_name))
}

// ---------------------------------------------------------------------------
// Background worker
// ---------------------------------------------------------------------------

/// Spawn the enhancement worker. Returns a JoinHandle.
/// Follows the scanner.rs pattern: uses `spawn_blocking` with its own DB connection.
pub fn spawn_enhancement_worker(
    db_path: PathBuf,
    comfyui_url: String,
    shutdown: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let library_root = db_path.parent().unwrap().to_path_buf();
        let mut conn = match db::open_diesel_connection(&db_path) {
            Ok(c) => c,
            Err(e) => {
                error!("ComfyUI worker: failed to open DB: {}", e);
                return;
            }
        };
        let client = ComfyUiClient::new(&comfyui_url);
        info!("ComfyUI enhancement worker started (url: {})", comfyui_url);

        // Recover tasks that were mid-processing when we last shut down
        recover_interrupted_tasks(&mut conn);

        let (lock, cvar) = &*shutdown;
        loop {
            // Check shutdown
            if *lock.lock().unwrap() {
                info!("ComfyUI worker shutting down");
                break;
            }

            process_pending_tasks(&mut conn, &client, &library_root);
            poll_active_tasks(&mut conn, &client, &library_root);
            cleanup_completed_tasks(&mut conn);

            // Sleep 3 seconds or until shutdown
            let guard = lock.lock().unwrap();
            let _ = cvar
                .wait_timeout(guard, std::time::Duration::from_secs(3))
                .unwrap();
        }
    })
}

/// Re-attach tasks that were mid-flight when we last shut down.
///
/// A task that already has a ComfyUI prompt id is *not* restarted: the prompt
/// may well have run, or still be running, while Phos was down. Restarting it
/// re-does the work and, worse, was one way a finished job came back as a
/// failure. Those go back to `processing` so the poller re-reads history (and,
/// if history is gone, probes the deterministic filenames). A task still
/// settling stays settling.
fn recover_interrupted_tasks(conn: &mut SqliteConnection) {
    // Had a prompt on ComfyUI: resume polling it rather than re-running it.
    if let Err(e) = diesel::update(
        enhancement_tasks::table.filter(
            enhancement_tasks::status
                .eq_any(&["queued", "processing", "downloading"])
                .and(enhancement_tasks::comfyui_prompt_id.is_not_null()),
        ),
    )
    .set(enhancement_tasks::status.eq("processing"))
    .execute(conn)
    {
        warn!("Failed to re-attach in-flight tasks: {}", e);
    }

    // Never reached ComfyUI: start over.
    if let Err(e) = diesel::update(
        enhancement_tasks::table.filter(
            enhancement_tasks::status
                .eq_any(&["uploading", "queued", "processing", "downloading"])
                .and(enhancement_tasks::comfyui_prompt_id.is_null()),
        ),
    )
    .set((
        enhancement_tasks::status.eq("pending"),
        enhancement_tasks::error_message.eq("Recovered after restart"),
        enhancement_tasks::next_attempt_at.eq(None::<String>),
    ))
    .execute(conn)
    {
        warn!("Failed to recover interrupted tasks: {}", e);
    }

    // `awaiting_output` is left exactly as it is — its deadline is still valid
    // and the poller picks it up — but the re-check clock is cleared so it is
    // looked at immediately rather than after a stale backoff.
    if let Err(e) = diesel::update(
        enhancement_tasks::table.filter(enhancement_tasks::status.eq(STATUS_AWAITING_OUTPUT)),
    )
    .set(enhancement_tasks::next_attempt_at.eq(None::<String>))
    .execute(conn)
    {
        warn!("Failed to resume settling tasks: {}", e);
    }
}

/// Pick up pending tasks and start processing them.
fn process_pending_tasks(conn: &mut SqliteConnection, client: &ComfyUiClient, library_root: &Path) {
    let now_dt = chrono::Utc::now().naive_utc();
    let now = format_ts(now_dt);

    type PendingRow = (String, String, String, String, String, Option<String>, i32);
    let tasks: Vec<PendingRow> = match enhancement_tasks::table
        .inner_join(
            comfyui_workflows::table.on(comfyui_workflows::id.eq(enhancement_tasks::workflow_id)),
        )
        .filter(
            enhancement_tasks::status.eq("pending").and(
                // A transient failure is re-queued with a backoff; do not pick it
                // up before that time.
                enhancement_tasks::next_attempt_at
                    .is_null()
                    .or(enhancement_tasks::next_attempt_at.le(&now)),
            ),
        )
        .order(enhancement_tasks::created_at.asc())
        .limit(5)
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            enhancement_tasks::workflow_id,
            comfyui_workflows::workflow_json,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "COALESCE(enhancement_tasks.text_overrides, '{}')",
            ),
            enhancement_tasks::source_file_id,
            diesel::dsl::sql::<diesel::sql_types::Integer>(
                "COALESCE(enhancement_tasks.retry_count, 0)",
            ),
        ))
        .load::<PendingRow>(conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to query pending tasks: {}", e);
            return;
        }
    };

    for (
        task_id,
        shot_id,
        _workflow_id,
        workflow_json_str,
        text_overrides_str,
        source_file_id,
        retry_count,
    ) in tasks
    {
        // Set uploading
        let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task_id)))
            .set((
                enhancement_tasks::status.eq("uploading"),
                enhancement_tasks::started_at.eq(&now),
                enhancement_tasks::next_attempt_at.eq(None::<String>),
            ))
            .execute(conn);

        // 1. Get source image (use specific file if provided, otherwise original)
        let (image_data, upload_name) =
            match get_source_image(conn, &shot_id, source_file_id.as_deref(), library_root) {
                Ok(v) => v,
                Err(e) => {
                    handle_failure(
                        conn,
                        &task_id,
                        FailureSite::SourceImage,
                        &format!("Source image extraction failed: {}", e),
                        retry_count,
                    );
                    continue;
                }
            };

        // 2. Upload to ComfyUI
        let uploaded_name = match client.upload_image(&upload_name, &image_data) {
            Ok(name) => name,
            Err(e) => {
                handle_failure(
                    conn,
                    &task_id,
                    FailureSite::Upload,
                    &format!("Upload failed: {}", e),
                    retry_count,
                );
                continue;
            }
        };

        // 3. Parse workflow and prepare
        let workflow: Value = match serde_json::from_str(&workflow_json_str) {
            Ok(v) => v,
            Err(e) => {
                handle_failure(
                    conn,
                    &task_id,
                    FailureSite::WorkflowJson,
                    &format!("Invalid workflow JSON: {}", e),
                    retry_count,
                );
                continue;
            }
        };

        let text_overrides: std::collections::HashMap<String, String> =
            serde_json::from_str(&text_overrides_str).unwrap_or_default();

        // Pin the output names before the run starts, and record the prefix so a
        // later poll can find the files even if history never mentions them.
        let output_prefix = output_prefix_for_task(&task_id);
        let prepared = prepare_workflow(
            &workflow,
            &uploaded_name,
            &text_overrides,
            Some(&output_prefix),
        );

        // 4. Queue prompt
        let prompt_id = match client.queue_prompt(&prepared) {
            Ok(id) => id,
            Err(e) => {
                handle_failure(
                    conn,
                    &task_id,
                    FailureSite::Queue,
                    &format!("Queue failed: {}", e),
                    retry_count,
                );
                continue;
            }
        };

        // 5. Set queued with comfyui_prompt_id
        let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task_id)))
            .set((
                enhancement_tasks::status.eq("queued"),
                enhancement_tasks::comfyui_prompt_id.eq(&prompt_id),
                enhancement_tasks::output_prefix.eq(&output_prefix),
                enhancement_tasks::settle_until.eq(None::<String>),
            ))
            .execute(conn);

        info!(
            "Task {} queued as ComfyUI prompt {} (output prefix {})",
            task_id, prompt_id, output_prefix
        );
    }
}

/// Statuses the poller owns.
pub const STATUS_AWAITING_OUTPUT: &str = "awaiting_output";
pub const STATUS_CANCELLED: &str = "cancelled";

/// A task the poller is following, with everything it needs to decide.
struct ActiveTask {
    id: String,
    shot_id: String,
    prompt_id: String,
    workflow_id: String,
    workflow_json: String,
    text_overrides: String,
    status: String,
    output_prefix: Option<String>,
    settle_until: Option<String>,
    retry_count: i32,
}

type ActiveTaskRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i32,
);

/// Poll tasks that are queued/processing/settling against ComfyUI history.
fn poll_active_tasks(conn: &mut SqliteConnection, client: &ComfyUiClient, library_root: &Path) {
    let now_dt = chrono::Utc::now().naive_utc();
    let now = format_ts(now_dt);

    let rows: Vec<ActiveTaskRow> = match enhancement_tasks::table
        .inner_join(
            comfyui_workflows::table.on(comfyui_workflows::id.eq(enhancement_tasks::workflow_id)),
        )
        .filter(
            enhancement_tasks::status
                .eq_any(&["queued", "processing", STATUS_AWAITING_OUTPUT])
                .and(enhancement_tasks::comfyui_prompt_id.is_not_null())
                // A settling task sets its own re-check time; leave it alone
                // until then.
                .and(
                    enhancement_tasks::next_attempt_at
                        .is_null()
                        .or(enhancement_tasks::next_attempt_at.le(&now)),
                ),
        )
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            enhancement_tasks::comfyui_prompt_id.assume_not_null(),
            enhancement_tasks::workflow_id,
            comfyui_workflows::workflow_json,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "COALESCE(enhancement_tasks.text_overrides, '{}')",
            ),
            enhancement_tasks::status,
            enhancement_tasks::output_prefix,
            enhancement_tasks::settle_until,
            diesel::dsl::sql::<diesel::sql_types::Integer>(
                "COALESCE(enhancement_tasks.retry_count, 0)",
            ),
        ))
        .load::<ActiveTaskRow>(conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to query active tasks: {}", e);
            return;
        }
    };

    for row in rows {
        let task = ActiveTask {
            id: row.0,
            shot_id: row.1,
            prompt_id: row.2,
            workflow_id: row.3,
            workflow_json: row.4,
            text_overrides: row.5,
            status: row.6,
            output_prefix: row.7,
            settle_until: row.8,
            retry_count: row.9,
        };
        poll_one_task(conn, client, library_root, &task, now_dt);
    }
}

fn poll_one_task(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &ActiveTask,
    now_dt: chrono::NaiveDateTime,
) {
    // Move out of `queued` as soon as we start watching it.
    if task.status == "queued" {
        let _ = diesel::update(
            enhancement_tasks::table.filter(
                enhancement_tasks::id
                    .eq(&task.id)
                    .and(enhancement_tasks::status.eq("queued")),
            ),
        )
        .set(enhancement_tasks::status.eq("processing"))
        .execute(conn);
    }

    let settling = task.status == STATUS_AWAITING_OUTPUT;

    let history = match client.get_history(&task.prompt_id) {
        Ok(Some(h)) => Some(h),
        Ok(None) => {
            // Not in history. Still queued or running? Then just wait.
            match client.is_prompt_in_queue(&task.prompt_id) {
                Ok(true) => return,
                Ok(false) => {
                    // The prompt is in neither history nor queue. It may have run
                    // and been lost to a ComfyUI restart, which clears history but
                    // not the output directory — so look on disk before calling it
                    // lost. This is why the prefix is pinned up front.
                    None
                }
                Err(e) => {
                    warn!("Failed to check queue for prompt {}: {}", task.prompt_id, e);
                    return;
                }
            }
        }
        Err(e) => {
            // History is unreadable — a transport problem, not a workflow
            // problem. Keep the task alive; only give up after MAX_ATTEMPTS.
            handle_failure(
                conn,
                &task.id,
                FailureSite::History,
                &format!("History fetch failed for prompt {}: {}", task.prompt_id, e),
                task.retry_count,
            );
            return;
        }
    };

    let verdict = match history.as_ref() {
        Some(h) => interpret_history(h),
        // No history entry at all: treat it as "finished, named nothing" so the
        // settle path gets its chance to find the file by name.
        None => HistoryVerdict::NoOutputs,
    };

    match verdict {
        HistoryVerdict::Running => {
            // Still executing. If we were settling, ComfyUI re-queued the prompt;
            // drop back to processing and let the run finish.
            if settling {
                let _ = diesel::update(
                    enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)),
                )
                .set((
                    enhancement_tasks::status.eq("processing"),
                    enhancement_tasks::next_attempt_at.eq(None::<String>),
                ))
                .execute(conn);
            }
        }
        HistoryVerdict::Failed(message) => {
            // A node raised. Nothing about retrying changes that, so report the
            // real message and stop.
            if let Some(tb) = history.as_ref().and_then(execution_error_traceback) {
                error!("Task {} traceback:\n{}", task.id, tb);
            }
            handle_failure(
                conn,
                &task.id,
                FailureSite::Execution,
                &message,
                task.retry_count,
            );
        }
        HistoryVerdict::Outputs(refs) => {
            download_all(conn, client, library_root, task, &refs);
        }
        HistoryVerdict::NoOutputs => {
            settle_task(conn, client, library_root, task, now_dt, history.as_ref());
        }
    }
}

/// Download everything history named. Succeeding on any one file completes the
/// task; failing on all of them is transient, because a 404 from `/view` is very
/// often a file that is written but not yet closed.
fn download_all(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &ActiveTask,
    refs: &[OutputRef],
) {
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
        .set(enhancement_tasks::status.eq("downloading"))
        .execute(conn);

    let mut errors: Vec<String> = Vec::new();
    let mut downloaded = false;
    for out in refs {
        match download_and_save_output(conn, client, task, out, library_root) {
            Ok(_) => downloaded = true,
            Err(e) => {
                error!(
                    "Failed to download output {} for task {}: {}",
                    out.describe(),
                    task.id,
                    e
                );
                errors.push(e.to_string());
            }
        }
    }

    if downloaded || task_has_output(conn, &task.id) {
        mark_completed(conn, &task.id);
        return;
    }

    // Say what actually went wrong. The old message ("No output images found in
    // ComfyUI response") blamed the workflow for what was usually a 404.
    let detail = errors
        .first()
        .cloned()
        .unwrap_or_else(|| "no reason reported".to_string());
    let message = format!(
        "ComfyUI named {} output file(s) but none could be downloaded. First error: {}",
        refs.len(),
        detail
    );
    handle_failure(
        conn,
        &task.id,
        FailureSite::Download,
        &message,
        task.retry_count,
    );
}

/// ComfyUI says it is done but has named no file. That is a state, not a
/// verdict: wait, and meanwhile look for the file under the name we pinned.
fn settle_task(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    library_root: &Path,
    task: &ActiveTask,
    now_dt: chrono::NaiveDateTime,
    history: Option<&Value>,
) {
    // The file may already be on disk under the deterministic prefix even though
    // history is silent about it. One hit finishes the task.
    if let Some(prefix) = task.output_prefix.as_deref() {
        for candidate in fallback_output_candidates(prefix) {
            match download_and_save_output(conn, client, task, &candidate, library_root) {
                Ok(_) => {
                    info!(
                        "Task {} recovered output {} by name; history never listed it",
                        task.id,
                        candidate.describe()
                    );
                    mark_completed(conn, &task.id);
                    return;
                }
                // A miss is the normal case for most candidates — only the right
                // extension exists — so this is not worth logging per file.
                Err(_) => continue,
            }
        }
    }

    if task_has_output(conn, &task.id) {
        mark_completed(conn, &task.id);
        return;
    }

    let workflow: Value = serde_json::from_str(&task.workflow_json).unwrap_or(Value::Null);
    let budget = settle_budget(&workflow);
    let settle_until = task.settle_until.as_deref().and_then(parse_ts);

    match decide_settle(now_dt, settle_until, budget) {
        SettleDecision::Start {
            deadline,
            recheck_at,
        } => {
            info!(
                "Task {} finished with no files listed; waiting up to {}s for them",
                task.id,
                budget.as_secs()
            );
            let _ =
                diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
                    .set((
                        enhancement_tasks::status.eq(STATUS_AWAITING_OUTPUT),
                        enhancement_tasks::settle_until.eq(format_ts(deadline)),
                        enhancement_tasks::next_attempt_at.eq(format_ts(recheck_at)),
                        enhancement_tasks::error_message.eq(None::<String>),
                    ))
                    .execute(conn);
        }
        SettleDecision::Wait { recheck_at } => {
            let _ =
                diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task.id)))
                    .set((
                        enhancement_tasks::status.eq(STATUS_AWAITING_OUTPUT),
                        enhancement_tasks::next_attempt_at.eq(format_ts(recheck_at)),
                    ))
                    .execute(conn);
        }
        SettleDecision::Expired => {
            let prefix = task.output_prefix.as_deref().unwrap_or("(none)");
            // "Finished but silent" and "vanished from ComfyUI entirely" are
            // different problems, and the user needs to be told which one.
            let message = match history {
                Some(h) => {
                    let outputs_debug = h
                        .get("outputs")
                        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "N/A".to_string()))
                        .unwrap_or_else(|| "null".to_string());
                    format!(
                        "ComfyUI reported prompt {} finished but published no file within {}s, \
                         and nothing was found under the pinned prefix {}. Outputs: {}",
                        task.prompt_id,
                        budget.as_secs(),
                        prefix,
                        outputs_debug
                    )
                }
                None => format!(
                    "Prompt {} is in neither ComfyUI's history nor its queue, and no file \
                     appeared under the pinned prefix {} within {}s (job lost, most likely a \
                     ComfyUI restart)",
                    task.prompt_id,
                    prefix,
                    budget.as_secs()
                ),
            };
            error!("Task {} gave up settling: {}", task.id, message);
            handle_failure(
                conn,
                &task.id,
                FailureSite::Settle,
                &message,
                task.retry_count,
            );
        }
    }
}

/// Did an earlier attempt already save a file for this task?
fn task_has_output(conn: &mut SqliteConnection, task_id: &str) -> bool {
    enhancement_tasks::table
        .filter(
            enhancement_tasks::id
                .eq(task_id)
                .and(enhancement_tasks::output_file_id.is_not_null()),
        )
        .count()
        .get_result::<i64>(conn)
        .map(|c| c > 0)
        .unwrap_or(false)
}

fn mark_completed(conn: &mut SqliteConnection, task_id: &str) {
    let now = format_ts(chrono::Utc::now().naive_utc());
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
        .set((
            enhancement_tasks::status.eq("completed"),
            enhancement_tasks::completed_at.eq(&now),
            enhancement_tasks::error_message.eq(None::<String>),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
            enhancement_tasks::settle_until.eq(None::<String>),
        ))
        .execute(conn);
    info!("Task {} completed successfully", task_id);
}

/// Download an output file from ComfyUI and save it alongside the original.
fn download_and_save_output(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    task: &ActiveTask,
    out: &OutputRef,
    library_root: &Path,
) -> anyhow::Result<()> {
    let task_id = task.id.as_str();
    let shot_id = task.shot_id.as_str();
    let workflow_id = task.workflow_id.as_str();
    let text_overrides_json = task.text_overrides.as_str();
    let filename = out.filename.as_str();

    let data = client.download_output(out)?;

    // Get the original file path to determine where to save
    let original_path_str: String = files::table
        .filter(files::shot_id.eq(shot_id).and(files::is_original.eq(true)))
        .select(files::path)
        .first::<String>(conn)
        .map_err(|_| anyhow::anyhow!("No original file found for shot {}", shot_id))?;

    let original = db::resolve_path(library_root, &original_path_str);
    let parent = original
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No parent directory"))?;
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    // Determine extension from the downloaded filename
    let ext = Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png");

    // Compute hash before writing to disk so we can check for duplicates
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let hash = hex::encode(hasher.finalize());

    let file_size = data.len() as i64;

    // Guess mime type from extension
    let mime_type = match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    };

    let task_short = &task_id[..8.min(task_id.len())];
    let base_output_filename = format!("{}_enhanced_{}.{}", stem, task_short, ext);
    let base_output_path = parent.join(&base_output_filename);
    let base_path_str = db::make_relative(library_root, &base_output_path);

    // Check if a file with the expected path already exists in the DB (from a previous attempt)
    let existing: Option<(String, String)> = files::table
        .filter(files::path.eq(&base_path_str))
        .select((files::id, files::hash))
        .first::<(String, String)>(conn)
        .ok();

    let actual_file_id: String = match existing {
        Some((existing_id, existing_hash)) if existing_hash == hash => {
            // Same content already saved — nothing to do
            info!(
                "Task {} output already exists with same hash, skipping write",
                task_id
            );
            existing_id
        }
        Some(_) => {
            // Path is taken but content differs — save as a new variant with a unique suffix
            let unique = &Uuid::new_v4().to_string()[..8];
            let variant_filename = format!("{}_enhanced_{}_{}.{}", stem, task_short, unique, ext);
            let variant_path = parent.join(&variant_filename);

            std::fs::write(&variant_path, &data)?;
            info!("Saved enhanced output (new variant) to {:?}", variant_path);

            let variant_path_str = db::make_relative(library_root, &variant_path);
            let file_id = Uuid::new_v4().to_string();
            diesel::insert_into(files::table)
                .values(NewFile {
                    id: &file_id,
                    shot_id,
                    path: &variant_path_str,
                    hash: &hash,
                    mime_type: Some(mime_type),
                    file_size: Some(file_size as i32),
                    is_original: Some(false),
                    visual_embedding: None,
                    source_workflow_id: Some(workflow_id),
                    source_text_overrides: Some(text_overrides_json),
                })
                .execute(conn)?;
            file_id
        }
        None => {
            // No existing file — normal save
            std::fs::write(&base_output_path, &data)?;
            info!("Saved enhanced output to {:?}", base_output_path);

            let file_id = Uuid::new_v4().to_string();
            diesel::insert_into(files::table)
                .values(NewFile {
                    id: &file_id,
                    shot_id,
                    path: &base_path_str,
                    hash: &hash,
                    mime_type: Some(mime_type),
                    file_size: Some(file_size as i32),
                    is_original: Some(false),
                    visual_embedding: None,
                    source_workflow_id: Some(workflow_id),
                    source_text_overrides: Some(text_overrides_json),
                })
                .execute(conn)?;
            file_id
        }
    };

    // Store the output file ID on the task
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
        .set(enhancement_tasks::output_file_id.eq(&actual_file_id))
        .execute(conn)?;

    Ok(())
}

/// Record a failure, retrying it if the site says another attempt could help.
///
/// This is where `retry_count` finally earns its column: a transport hiccup used
/// to be as terminal as a broken graph, which is why a working workflow could
/// need several manual reruns.
fn handle_failure(
    conn: &mut SqliteConnection,
    task_id: &str,
    site: FailureSite,
    message: &str,
    retry_count: i32,
) {
    match plan_failure(site, message, retry_count) {
        FailureAction::Retry {
            attempt,
            delay,
            message,
        } => {
            let retry_at = format_ts(
                chrono::Utc::now().naive_utc() + chrono::Duration::seconds(delay.as_secs() as i64),
            );
            warn!(
                "Task {} hit a transient failure, attempt {}/{} in {}s: {}",
                task_id,
                attempt,
                MAX_ATTEMPTS,
                delay.as_secs(),
                message
            );
            let note = format!(
                "Retrying (attempt {}/{}): {}",
                attempt, MAX_ATTEMPTS, message
            );
            let filter = enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id));
            let _ = if retry_resumes_prompt(site) {
                // The prompt already reached ComfyUI; go back to watching it
                // rather than paying for the whole graph a second time.
                diesel::update(filter)
                    .set((
                        enhancement_tasks::status.eq("processing"),
                        enhancement_tasks::retry_count.eq(retry_count + 1),
                        enhancement_tasks::next_attempt_at.eq(&retry_at),
                        enhancement_tasks::error_message.eq(&note),
                    ))
                    .execute(conn)
            } else {
                diesel::update(filter)
                    .set((
                        enhancement_tasks::status.eq("pending"),
                        enhancement_tasks::retry_count.eq(retry_count + 1),
                        enhancement_tasks::next_attempt_at.eq(&retry_at),
                        enhancement_tasks::settle_until.eq(None::<String>),
                        enhancement_tasks::comfyui_prompt_id.eq(None::<String>),
                        enhancement_tasks::error_message.eq(&note),
                    ))
                    .execute(conn)
            };
        }
        FailureAction::Fail(message) => mark_failed(conn, task_id, &message),
    }
}

/// Mark a task as failed with an error message.
fn mark_failed(conn: &mut SqliteConnection, task_id: &str, error_msg: &str) {
    error!("Task {} failed: {}", task_id, error_msg);
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
        .set((
            enhancement_tasks::status.eq("failed"),
            enhancement_tasks::error_message.eq(error_msg),
            enhancement_tasks::next_attempt_at.eq(None::<String>),
        ))
        .execute(conn);
}

/// Remove completed tasks older than 5 minutes.
fn cleanup_completed_tasks(conn: &mut SqliteConnection) {
    let cutoff = (chrono::Utc::now().naive_utc() - chrono::Duration::seconds(300))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    match diesel::delete(
        enhancement_tasks::table.filter(
            enhancement_tasks::status
                .eq("completed")
                .and(enhancement_tasks::completed_at.is_not_null())
                .and(enhancement_tasks::completed_at.lt(&cutoff)),
        ),
    )
    .execute(conn)
    {
        Ok(n) if n > 0 => info!("Cleaned up {} completed enhancement tasks", n),
        Err(e) => warn!("Failed to clean up completed tasks: {}", e),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
//
// Fixtures are shaped like real `/history/{prompt_id}` payloads, because the bug
// these cover is entirely about reading that payload correctly. Each test names
// the defect it pins.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dt(s: &str) -> chrono::NaiveDateTime {
        parse_ts(s).expect("fixture timestamp")
    }

    /// A history entry the way ComfyUI writes one.
    fn history(outputs: Value, completed: bool) -> Value {
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

    fn file(name: &str) -> Value {
        json!({ "filename": name, "subfolder": "", "type": "output" })
    }

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
        // `animated` is an array of bools; `text` an array of strings. Neither is
        // downloadable, and mistaking them for files would be worse than missing
        // them.
        let entry = history(
            json!({ "9": { "animated": [false], "text": ["a caption"] } }),
            true,
        );
        assert_eq!(interpret_history(&entry), HistoryVerdict::NoOutputs);
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

    #[test]
    fn defect_2_video_workflows_get_the_long_budget() {
        let video = json!({
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff" } }
        });
        let images = json!({
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "ComfyUI" } }
        });
        assert_eq!(settle_budget(&video), SETTLE_BUDGET_VIDEO);
        assert_eq!(settle_budget(&images), SETTLE_BUDGET_IMAGE);
        // 10s of budget was never going to cover an ffmpeg mux.
        assert!(SETTLE_BUDGET_VIDEO > Duration::from_secs(10 * 60));
    }

    #[test]
    fn defect_2_core_save_video_is_an_output_node() {
        let wf = json!({
            "12": { "class_type": "SaveVideo", "inputs": { "filename_prefix": "video/ComfyUI" } }
        });
        assert_eq!(detect_outputs(&wf).len(), 1);
        assert!(workflow_has_slow_output(&wf));
    }

    #[test]
    fn defect_2_custom_saver_is_an_output_node() {
        // Recognised by its `filename_prefix` input, not by a name we knew.
        let wf = json!({
            "20": { "class_type": "ImageWriterXL",
                    "inputs": { "filename_prefix": "out", "images": ["19", 0] } }
        });
        assert_eq!(detect_outputs(&wf).len(), 1);
    }

    #[test]
    fn defect_2_settle_starts_a_clock_then_expires() {
        let now = dt("2026-08-30 12:00:00");
        let budget = Duration::from_secs(60);

        // First sighting: start the clock, come back shortly.
        let SettleDecision::Start {
            deadline,
            recheck_at,
        } = decide_settle(now, None, budget)
        else {
            panic!("first sighting should start the clock");
        };
        assert_eq!(deadline, dt("2026-08-30 12:01:00"));
        assert_eq!(recheck_at, dt("2026-08-30 12:00:02"));

        // Inside the budget: keep waiting, with a widening gap.
        assert_eq!(
            decide_settle(dt("2026-08-30 12:00:30"), Some(deadline), budget),
            SettleDecision::Wait {
                recheck_at: dt("2026-08-30 12:00:35")
            }
        );

        // Past the deadline: only now is it a failure.
        assert_eq!(
            decide_settle(dt("2026-08-30 12:01:01"), Some(deadline), budget),
            SettleDecision::Expired
        );
    }

    #[test]
    fn defect_2_settle_backoff_widens_with_elapsed_time() {
        assert_eq!(settle_recheck_delay(Duration::ZERO), Duration::from_secs(2));
        assert_eq!(
            settle_recheck_delay(Duration::from_secs(30)),
            Duration::from_secs(5)
        );
        assert_eq!(
            settle_recheck_delay(Duration::from_secs(120)),
            Duration::from_secs(15)
        );
        assert_eq!(
            settle_recheck_delay(Duration::from_secs(600)),
            Duration::from_secs(30)
        );
    }

    // === Defect 3 / scope D — transient vs permanent =========================

    #[test]
    fn defect_3_transient_failures_are_retried() {
        for site in [
            FailureSite::Upload,
            FailureSite::History,
            FailureSite::Download,
        ] {
            assert_eq!(
                classify_failure(site, "connection reset"),
                FailureKind::Transient,
                "{:?} should be retryable",
                site
            );
        }
    }

    #[test]
    fn defect_3_permanent_failures_are_not_retried() {
        for site in [
            FailureSite::SourceImage,
            FailureSite::WorkflowJson,
            FailureSite::Execution,
            FailureSite::Settle,
        ] {
            assert_eq!(
                classify_failure(site, "whatever"),
                FailureKind::Permanent,
                "{:?} should be terminal",
                site
            );
        }
    }

    #[test]
    fn defect_3_a_rejected_prompt_is_permanent_but_a_dropped_connection_is_not() {
        let rejected = format!(
            "Queue failed: {}: required input is missing",
            PROMPT_REJECTED
        );
        assert_eq!(
            classify_failure(FailureSite::Queue, &rejected),
            FailureKind::Permanent
        );
        assert_eq!(
            classify_failure(FailureSite::Queue, "Queue failed: connection refused"),
            FailureKind::Transient
        );
    }

    #[test]
    fn defect_3_retry_count_is_spent_then_the_real_error_stands() {
        let msg = "ComfyUI /view returned HTTP 404 for phos/x_00001_.png";
        // Attempts 1..3 come back for another go, with a widening delay.
        let mut delays = Vec::new();
        for retry_count in 0..MAX_ATTEMPTS - 1 {
            match plan_failure(FailureSite::Download, msg, retry_count) {
                FailureAction::Retry {
                    attempt,
                    delay,
                    message,
                } => {
                    assert_eq!(attempt, retry_count + 2);
                    assert_eq!(message, msg);
                    delays.push(delay);
                }
                other => panic!("attempt {} should retry, got {:?}", retry_count, other),
            }
        }
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(5),
                Duration::from_secs(15),
                Duration::from_secs(45)
            ]
        );

        // The last one keeps the real error rather than inventing a new one.
        match plan_failure(FailureSite::Download, msg, MAX_ATTEMPTS - 1) {
            FailureAction::Fail(text) => {
                assert!(text.starts_with(msg), "lost the real error: {}", text);
                assert!(text.contains("after 4 attempts"), "{}", text);
            }
            other => panic!("budget was spent, expected a failure, got {:?}", other),
        }
    }

    #[test]
    fn defect_3_a_retry_after_queueing_resumes_the_prompt() {
        // A prompt that already ran should be re-polled, not re-executed —
        // re-running it is expensive and can duplicate the output.
        assert!(retry_resumes_prompt(FailureSite::History));
        assert!(retry_resumes_prompt(FailureSite::Download));
        // Nothing reached ComfyUI yet, so these start over.
        assert!(!retry_resumes_prompt(FailureSite::Upload));
        assert!(!retry_resumes_prompt(FailureSite::Queue));
    }

    #[test]
    fn defect_3_a_permanent_failure_does_not_burn_attempts() {
        match plan_failure(FailureSite::Execution, "node blew up", 0) {
            FailureAction::Fail(text) => assert_eq!(text, "node blew up"),
            other => panic!("expected an immediate failure, got {:?}", other),
        }
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

    // === Scope A — deterministic filenames ===================================

    #[test]
    fn scope_a_pins_every_savers_filename_prefix() {
        let wf = json!({
            "9":  { "class_type": "SaveImage", "inputs": { "filename_prefix": "ComfyUI" } },
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff", "frame_rate": 8 } },
            "20": { "class_type": "MysterySaver", "inputs": { "filename_prefix": "whatever" } },
            "4":  { "class_type": "LoadImage", "inputs": { "image": "old.png" } },
        });
        let prefix = output_prefix_for_task("task-1234");
        assert_eq!(prefix, "phos/task-1234");

        let prepared = prepare_workflow(
            &wf,
            "uploaded.png",
            &std::collections::HashMap::new(),
            Some(&prefix),
        );
        for node in ["9", "12", "20"] {
            assert_eq!(
                prepared[node]["inputs"]["filename_prefix"].as_str(),
                Some("phos/task-1234"),
                "node {} kept its own prefix",
                node
            );
        }
        // The rest of prepare_workflow still does its job.
        assert_eq!(
            prepared["4"]["inputs"]["image"].as_str(),
            Some("uploaded.png")
        );
        assert_eq!(prepared["12"]["inputs"]["frame_rate"].as_i64(), Some(8));
    }

    #[test]
    fn scope_a_leaves_a_linked_prefix_alone() {
        // A prefix wired from another node is a link, not a literal; rewriting it
        // would break the graph.
        let wf = json!({
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": ["8", 0] } }
        });
        let prepared = prepare_workflow(
            &wf,
            "uploaded.png",
            &std::collections::HashMap::new(),
            Some("phos/task-1234"),
        );
        assert_eq!(prepared["9"]["inputs"]["filename_prefix"], json!(["8", 0]));
    }

    #[test]
    fn scope_a_candidate_names_match_what_comfyui_writes() {
        let candidates = fallback_output_candidates("phos/task-1234");
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
        let candidates = fallback_output_candidates("task-1234");
        assert!(candidates.iter().all(|c| c.subfolder.is_empty()));
        assert!(candidates
            .iter()
            .any(|c| c.filename == "task-1234_00001_.png"));
    }

    // === Defect 5 — the /prompt payload carries a client_id ==================

    #[test]
    fn defect_5_client_id_is_stable_within_the_process() {
        let first = client_id();
        assert!(first.starts_with("phos-"), "{}", first);
        assert_eq!(first, client_id(), "client id must not change per call");
    }

    // === Defect 6 / scope F — cancelling needs to find the running prompt ====

    #[test]
    fn defect_6_only_the_running_prompt_is_interruptible() {
        let queue = json!({
            "queue_running": [[0, "running-id", {}, {}, []]],
            "queue_pending": [[1, "pending-id", {}, {}, []]],
        });
        assert!(queue_contains(&queue, "queue_running", "running-id"));
        assert!(!queue_contains(&queue, "queue_running", "pending-id"));
        assert!(queue_contains(&queue, "queue_pending", "pending-id"));
        assert!(!queue_contains(&queue, "queue_pending", "absent-id"));
    }

    // === The reported symptom, end to end ====================================

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
        let now = dt("2026-08-30 12:00:00");
        let budget = settle_budget(&wf);
        assert_eq!(budget, SETTLE_BUDGET_VIDEO);
        let SettleDecision::Start { deadline, .. } = decide_settle(now, None, budget) else {
            panic!("should have started settling");
        };
        assert_eq!(deadline, dt("2026-08-30 12:15:00"));

        // Step 3: meanwhile the file is findable by the name we pinned, even
        // though history never mentioned it.
        assert!(fallback_output_candidates("phos/task-1234")
            .iter()
            .any(|c| c.filename == "task-1234_00001.mp4" && c.subfolder == "phos"));

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
