//! The ComfyUI HTTP surface: everything that talks over the wire.
//!
//! Nothing here decides anything. Reading what ComfyUI said lives in
//! [`super::history`], and what to do about it in [`super::policy`], so the
//! whole completion path can be tested without a server.

use super::outputs::OutputRef;
use super::policy::PROMPT_REJECTED;
use serde_json::Value;
use std::sync::OnceLock;
use uuid::Uuid;

/// One client id for the whole process, so ComfyUI can attribute our prompts to
/// us. Sent on `/prompt`; it is also the id a future `/ws` listener would
/// subscribe with, which is why it has to be stable rather than per-request.
fn client_id() -> &'static str {
    static CLIENT_ID: OnceLock<String> = OnceLock::new();
    CLIENT_ID.get_or_init(|| format!("phos-{}", Uuid::new_v4().simple()))
}

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

    /// Upload a source file to ComfyUI's `/upload/image` endpoint using manual
    /// multipart.
    ///
    /// The endpoint is named for images but is not limited to them: it drops
    /// whatever it is given into ComfyUI's input directory, which is exactly
    /// where the VHS video loaders read from. What it must not be told is that
    /// an mp4 is a PNG, which is what this used to hard-code.
    pub(crate) fn upload_file(
        &self,
        filename: &str,
        content_type: &str,
        data: &[u8],
    ) -> anyhow::Result<String> {
        let boundary = format!("----PhosUpload{}", Uuid::new_v4().simple());
        let body = upload_body(&boundary, filename, content_type, data);

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
    pub(crate) fn get_history(&self, prompt_id: &str) -> anyhow::Result<Option<Value>> {
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
    pub(crate) fn is_prompt_in_queue(&self, prompt_id: &str) -> anyhow::Result<bool> {
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
    pub(crate) fn download_output(&self, out: &OutputRef) -> anyhow::Result<Vec<u8>> {
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

/// The multipart body for one `/upload/image` request.
///
/// Split out so the part headers can be checked without a server: the
/// `Content-Type` of the file part is what decides whether ComfyUI's input
/// directory ends up holding a video or something it thinks is a PNG.
fn upload_body(boundary: &str, filename: &str, content_type: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    // file field — named `image` because that is what the endpoint expects,
    // whatever the file actually is
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"image\"; filename=\"{}\"\r\n",
            filename
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", content_type).as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(b"\r\n");
    // overwrite field (always true so repeated uploads of same name work)
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"overwrite\"\r\n\r\n");
    body.extend_from_slice(b"true\r\n");
    // closing boundary
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());
    body
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

    // FR2 — a video has to go up as a video.
    #[test]
    fn an_upload_carries_the_content_type_it_was_given() {
        let body = upload_body("BOUND", "phos_a_b_video.mp4", "video/mp4", b"\x00\x00moov");
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("Content-Type: video/mp4\r\n"),
            "an mp4 was announced as something else:\n{}",
            text
        );
        assert!(text.contains("filename=\"phos_a_b_video.mp4\""), "{}", text);
        // The bytes go through untouched, and the overwrite flag still rides along.
        assert!(body.windows(4).any(|w| w == b"moov"));
        assert!(text.contains("name=\"overwrite\"\r\n\r\ntrue"), "{}", text);
        assert!(text.ends_with("--BOUND--\r\n"), "{}", text);
    }

    #[test]
    fn an_image_upload_is_unchanged() {
        let body = upload_body("BOUND", "phos_a_b_first.png", "image/png", b"\x89PNG");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("Content-Type: image/png\r\n"), "{}", text);
    }
}
