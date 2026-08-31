//! The ComfyUI HTTP surface: everything that talks over the wire.
//!
//! Nothing here decides anything. Reading what ComfyUI said lives in
//! [`super::history`], and what to do about it in [`super::policy`], so the
//! whole completion path can be tested without a server.

use super::outputs::OutputRef;
use super::policy::PROMPT_REJECTED;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

/// One client id for the whole process, so ComfyUI can attribute our prompts to
/// us. Sent on `/prompt`; it is also the id a future `/ws` listener would
/// subscribe with, which is why it has to be stable rather than per-request.
fn client_id() -> &'static str {
    static CLIENT_ID: OnceLock<String> = OnceLock::new();
    CLIENT_ID.get_or_init(|| format!("phos-{}", Uuid::new_v4().simple()))
}

/// How long each kind of call may take.
///
/// ureq bounds nothing by default, and the worker is a single blocking loop, so
/// one call that never returns stops the whole enhancement queue — indefinitely,
/// with no error and no log line. A paused container, a sleeping host or a
/// network partition all present that way: the socket is accepted and then
/// nothing comes back. It also defeats the settle budget, because a `get_history`
/// that never returns never gets to check its own deadline.
///
/// These are generous — ComfyUI is normally on the LAN or the same host, so they
/// exist to catch a dead server, not to police latency.
#[derive(Debug, Clone, Copy)]
struct Timeouts {
    /// Whole-call budget for the health check, which answers out of memory. If
    /// it cannot do that promptly, "unreachable" is the honest answer.
    health: Duration,
    /// Whole-call budget for the small JSON endpoints.
    json: Duration,
    /// Whole-call budget for `/prompt`, which validates the entire graph
    /// server-side before answering.
    queue: Duration,
    /// Whole-call budget for `/object_info`, which is one JSON document but a
    /// large one — megabytes on a box with many custom node packs.
    catalog: Duration,
    /// Whole-call budget for the two calls that move a file. A video can cross
    /// this connection, so it needs real headroom.
    transfer: Duration,
    /// How long any server may take to send *response headers*. This is the
    /// black-hole guard on the transfer calls: it trips on a host that accepted
    /// the connection and went silent, without capping a large, healthy body.
    response: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            health: Duration::from_secs(5),
            json: Duration::from_secs(15),
            queue: Duration::from_secs(30),
            catalog: Duration::from_secs(30),
            transfer: Duration::from_secs(15 * 60),
            response: Duration::from_secs(30),
        }
    }
}

pub struct ComfyUiClient {
    base_url: String,
    timeouts: Timeouts,
}

impl ComfyUiClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            timeouts: Timeouts::default(),
        }
    }

    /// The same client with every budget shrunk to milliseconds, so the
    /// black-hole tests finish in a blink instead of half a minute.
    #[cfg(test)]
    fn with_tiny_timeouts(base_url: &str) -> Self {
        let tiny = Duration::from_millis(150);
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            timeouts: Timeouts {
                health: tiny,
                json: tiny,
                queue: tiny,
                catalog: tiny,
                transfer: tiny,
                response: tiny,
            },
        }
    }

    /// The server this client talks to, normalised. Used as the node-info
    /// cache key.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Check if ComfyUI is reachable.
    ///
    /// The result is also handed to [`super::nodes::observe_health`], which is
    /// what notices a *reconnect* and drops the cached `/object_info` — a
    /// restarted ComfyUI may have a different set of models installed. Doing it
    /// here rather than at each call site is deliberate: this is the only place
    /// that learns whether the server is up.
    pub fn health_check(&self) -> anyhow::Result<()> {
        let url = format!("{}/system_stats", self.base_url);
        let outcome = (|| {
            let resp = ureq::get(&url)
                .config()
                .timeout_global(Some(self.timeouts.health))
                .build()
                .call()
                .map_err(|e| anyhow::anyhow!("ComfyUI health check failed: {}", e))?;
            if resp.status() != 200 {
                anyhow::bail!("ComfyUI returned status {}", resp.status());
            }
            Ok(())
        })();
        super::nodes::observe_health(&self.base_url, outcome.is_ok());
        outcome
    }

    /// Ask ComfyUI what its installed node classes take.
    ///
    /// The answer runs to megabytes on a loaded box, so it has its own budget:
    /// an import must not hang on a server that accepted the connection and
    /// then went quiet. Reading the document is [`super::nodes`]' job; this
    /// only fetches it.
    pub(crate) fn object_info(&self) -> anyhow::Result<Value> {
        let url = format!("{}/object_info", self.base_url);
        let mut resp = ureq::get(&url)
            .config()
            .timeout_global(Some(self.timeouts.catalog))
            .build()
            .call()
            .map_err(|e| anyhow::anyhow!("object_info fetch failed: {}", e))?;
        Ok(resp.body_mut().read_json()?)
    }

    /// Upload an image to ComfyUI's /upload/image endpoint using manual multipart.
    pub(crate) fn upload_image(&self, filename: &str, image_data: &[u8]) -> anyhow::Result<String> {
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
            .config()
            .timeout_global(Some(self.timeouts.transfer))
            .timeout_recv_response(Some(self.timeouts.response))
            .build()
            .send(body.as_slice())
            .map_err(|e| anyhow::anyhow!("Upload failed: {}", e))?;

        let json: Value = resp.body_mut().read_json()?;
        let name = json["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No 'name' in upload response"))?;
        Ok(name.to_string())
    }

    /// Queue a prompt (workflow JSON) on ComfyUI.
    pub(crate) fn queue_prompt(&self, workflow: &Value) -> anyhow::Result<String> {
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
            .timeout_global(Some(self.timeouts.queue))
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
            .config()
            .timeout_global(Some(self.timeouts.json))
            .build()
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
            .config()
            .timeout_global(Some(self.timeouts.json))
            .build()
            .send(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("Queue delete failed: {}", e))?;
        Ok(())
    }

    /// Is this prompt the one ComfyUI is executing right now (as opposed to
    /// merely queued)? Only the running one is worth an `/interrupt`.
    pub fn is_prompt_running(&self, prompt_id: &str) -> anyhow::Result<bool> {
        let url = format!("{}/queue", self.base_url);
        let mut resp = ureq::get(&url)
            .config()
            .timeout_global(Some(self.timeouts.json))
            .build()
            .call()
            .map_err(|e| anyhow::anyhow!("Queue fetch failed: {}", e))?;
        let json: Value = resp.body_mut().read_json()?;
        Ok(queue_contains(&json, "queue_running", prompt_id))
    }

    /// Get execution history for a prompt.
    pub(crate) fn get_history(&self, prompt_id: &str) -> anyhow::Result<Option<Value>> {
        let url = format!("{}/history/{}", self.base_url, prompt_id);
        let mut resp = ureq::get(&url)
            .config()
            .timeout_global(Some(self.timeouts.json))
            .build()
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
    pub(crate) fn is_prompt_in_queue(&self, prompt_id: &str) -> anyhow::Result<bool> {
        let url = format!("{}/queue", self.base_url);
        let mut resp = ureq::get(&url)
            .config()
            .timeout_global(Some(self.timeouts.json))
            .build()
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
    pub(crate) fn download_output(&self, out: &OutputRef) -> anyhow::Result<Vec<u8>> {
        let url = self.view_url(out);
        let mut resp = ureq::get(&url)
            .config()
            .timeout_global(Some(self.timeouts.transfer))
            .timeout_recv_response(Some(self.timeouts.response))
            .build()
            .call()
            .map_err(|e| match e {
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
pub(crate) fn queue_contains(queue: &Value, key: &str, prompt_id: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Defect 5 — the /prompt payload carries a client_id.
    #[test]
    fn defect_5_client_id_is_stable_within_the_process() {
        let first = client_id();
        assert!(first.starts_with("phos-"), "{}", first);
        assert_eq!(first, client_id(), "client id must not change per call");
    }

    // Defect 6 / scope F — cancelling needs to find the running prompt.
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

    /// A server that accepts the connection and then says nothing, ever — which
    /// is what a paused container, a sleeping host or a partitioned network all
    /// look like from this side. Not a closed port: that would fail instantly
    /// and prove nothing about timeouts.
    fn black_hole() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            // Hold every accepted socket open for the life of the process. The
            // moment one is dropped the client would see a clean EOF and return
            // early, which is not the case under test.
            let mut held = Vec::new();
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => held.push(s),
                    Err(_) => break,
                }
            }
        });
        format!("http://{}", addr)
    }

    /// Run one client call against the black hole on its own thread, and insist
    /// it comes back. A call with no timeout would block forever; failing the
    /// test beats hanging CI.
    fn must_not_hang(
        label: &str,
        url: &str,
        call: impl FnOnce(ComfyUiClient) -> bool + Send + 'static,
    ) {
        let url = url.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let client = ComfyUiClient::with_tiny_timeouts(&url);
            let _ = tx.send(call(client));
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(errored) => assert!(
                errored,
                "{} answered Ok against a server that never replied",
                label
            ),
            Err(_) => panic!(
                "{} never returned. It is unbounded, and because the worker is a \
                 single blocking loop it would wedge the whole enhancement queue.",
                label
            ),
        }
    }

    /// Every call the worker makes must be bounded. One that is not stops the
    /// queue indefinitely, with no error and no log line — and a `get_history`
    /// that never returns also never gets to check its own settle deadline.
    #[test]
    fn no_call_hangs_when_comfyui_stops_answering() {
        let url = black_hole();

        must_not_hang("health_check", &url, |c| c.health_check().is_err());
        must_not_hang("object_info", &url, |c| c.object_info().is_err());
        must_not_hang("get_history", &url, |c| c.get_history("abc").is_err());
        must_not_hang("is_prompt_in_queue", &url, |c| {
            c.is_prompt_in_queue("abc").is_err()
        });
        must_not_hang("is_prompt_running", &url, |c| {
            c.is_prompt_running("abc").is_err()
        });
        must_not_hang("queue_prompt", &url, |c| {
            c.queue_prompt(&json!({})).is_err()
        });
        must_not_hang("interrupt", &url, |c| c.interrupt().is_err());
        must_not_hang("delete_queued", &url, |c| c.delete_queued("abc").is_err());
        must_not_hang("upload_image", &url, |c| {
            c.upload_image("x.png", &[0u8; 512]).is_err()
        });
        must_not_hang("download_output", &url, |c| {
            c.download_output(&OutputRef {
                filename: "x.png".to_string(),
                subfolder: "phos".to_string(),
                output_type: "output".to_string(),
            })
            .is_err()
        });
    }

    #[test]
    fn the_shipped_budgets_are_bounded_and_ordered() {
        // The black-hole test proves the wiring using tiny budgets; this pins the
        // values actually shipped, so neither can rot without the other noticing.
        let t = Timeouts::default();
        assert!(
            t.health < t.json,
            "a health check should give up sooner than a working call"
        );
        assert!(t.json <= t.queue, "/prompt validates the whole graph");
        assert!(
            t.json <= t.catalog,
            "/object_info is a large document, not a small one"
        );
        assert!(
            t.queue < t.transfer,
            "moving a video needs the most headroom"
        );
        assert!(
            t.response < t.transfer,
            "headers must arrive long before a large body finishes"
        );
        assert!(
            t.transfer <= Duration::from_secs(30 * 60),
            "a budget this large is unbounded in all but name"
        );
    }
}
