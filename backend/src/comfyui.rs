use crate::db;
use crate::scanner;
use image::DynamicImage;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

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
        let payload = serde_json::json!({ "prompt": workflow });
        let url = format!("{}/prompt", self.base_url);

        let bytes = serde_json::to_vec(&payload)?;
        let mut resp = ureq::post(&url)
            .header("Content-Type", "application/json")
            .send(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("Queue prompt failed: {}", e))?;

        let json: Value = resp.body_mut().read_json()?;
        let prompt_id = json["prompt_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No 'prompt_id' in queue response"))?;
        Ok(prompt_id.to_string())
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

    /// Download an output image from ComfyUI.
    pub fn download_output(
        &self,
        filename: &str,
        subfolder: &str,
        output_type: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let url = format!(
            "{}/view?filename={}&subfolder={}&type={}",
            self.base_url,
            urlencoding::encode(filename),
            urlencoding::encode(subfolder),
            urlencoding::encode(output_type),
        );
        let mut resp = ureq::get(&url)
            .call()
            .map_err(|e| anyhow::anyhow!("Download failed: {}", e))?;
        let bytes = resp.body_mut().read_to_vec()?;
        Ok(bytes)
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

/// Detect output nodes (SaveImage, VHS_VideoCombine, etc.).
pub fn detect_outputs(workflow: &Value) -> Vec<WorkflowOutput> {
    let mut outputs = Vec::new();
    if let Some(nodes) = workflow.as_object() {
        for (node_id, node) in nodes {
            let class_type = node
                .get("class_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match class_type {
                "SaveImage" | "PreviewImage" | "VHS_VideoCombine" | "SaveAnimatedWEBP"
                | "SaveAnimatedPNG" => {
                    outputs.push(WorkflowOutput {
                        node_id: node_id.clone(),
                        node_type: class_type.to_string(),
                    });
                }
                _ => {}
            }
        }
    }
    outputs
}

/// Substitute inputs into a workflow copy: set LoadImage.image to the uploaded filename,
/// and apply any text overrides.
pub fn prepare_workflow(
    workflow: &Value,
    uploaded_filename: &str,
    text_overrides: &std::collections::HashMap<String, String>,
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
        }
    }
    wf
}

// ---------------------------------------------------------------------------
// Source image extraction
// ---------------------------------------------------------------------------

/// Get the source image bytes (PNG-encoded) for a shot.
/// For images: reads the original file.
/// For videos: extracts the first frame.
fn get_source_image(conn: &Connection, shot_id: &str, library_root: &Path) -> anyhow::Result<(Vec<u8>, String)> {
    // Find the original file for this shot
    let (file_path, mime_type): (String, String) = conn
        .query_row(
            "SELECT f.path, COALESCE(f.mime_type, '') FROM files f
             WHERE f.shot_id = ?1 AND f.is_original = 1
             ORDER BY f.created_at ASC LIMIT 1",
            params![shot_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| anyhow::anyhow!("No original file found for shot {}", shot_id))?;

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

    // Use the shot_id as the upload filename base
    let upload_name = format!("phos_{}.png", &shot_id[..8.min(shot_id.len())]);
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
        let conn = match db::open_connection(&db_path) {
            Ok(c) => c,
            Err(e) => {
                error!("ComfyUI worker: failed to open DB: {}", e);
                return;
            }
        };
        let client = ComfyUiClient::new(&comfyui_url);
        info!("ComfyUI enhancement worker started (url: {})", comfyui_url);

        let timeout_secs: u64 = std::env::var("PHOS_COMFYUI_TIMEOUT")
            .unwrap_or_else(|_| "3600".to_string())
            .parse()
            .unwrap_or(3600);

        // Recover tasks that were mid-processing when we last shut down
        recover_interrupted_tasks(&conn);

        let (lock, cvar) = &*shutdown;
        loop {
            // Check shutdown
            if *lock.lock().unwrap() {
                info!("ComfyUI worker shutting down");
                break;
            }

            process_pending_tasks(&conn, &client, &library_root);
            poll_active_tasks(&conn, &client, timeout_secs, &library_root);
            check_retries(&conn);
            cleanup_completed_tasks(&conn);

            // Sleep 3 seconds or until shutdown
            let guard = lock.lock().unwrap();
            let _ = cvar
                .wait_timeout(guard, std::time::Duration::from_secs(3))
                .unwrap();
        }
    })
}

/// Mark any tasks that were in intermediate states as needing retry.
fn recover_interrupted_tasks(conn: &Connection) {
    let intermediate_states = ["uploading", "queued", "processing", "downloading"];
    for state in &intermediate_states {
        if let Err(e) = conn.execute(
            "UPDATE enhancement_tasks SET status = 'pending', error_message = 'Recovered after restart'
             WHERE status = ?1",
            params![state],
        ) {
            warn!("Failed to recover {} tasks: {}", state, e);
        }
    }
}

/// Pick up pending tasks and start processing them.
fn process_pending_tasks(conn: &Connection, client: &ComfyUiClient, library_root: &Path) {
    let tasks: Vec<(String, String, String, String)> = {
        let mut stmt = match conn.prepare(
            "SELECT t.id, t.shot_id, t.workflow_id, w.workflow_json
             FROM enhancement_tasks t
             JOIN comfyui_workflows w ON t.workflow_id = w.id
             WHERE t.status = 'pending'
             ORDER BY t.created_at ASC
             LIMIT 5",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to query pending tasks: {}", e);
                return;
            }
        };
        let result = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                error!("Failed to fetch pending tasks: {}", e);
                return;
            }
        };
        result
    };

    for (task_id, shot_id, _workflow_id, workflow_json_str) in tasks {
        // Set uploading
        let _ = conn.execute(
            "UPDATE enhancement_tasks SET status = 'uploading', started_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![task_id],
        );

        // 1. Get source image
        let (image_data, upload_name) = match get_source_image(conn, &shot_id, library_root) {
            Ok(v) => v,
            Err(e) => {
                mark_failed(
                    conn,
                    &task_id,
                    &format!("Source image extraction failed: {}", e),
                    true,
                );
                continue;
            }
        };

        // 2. Upload to ComfyUI
        let uploaded_name = match client.upload_image(&upload_name, &image_data) {
            Ok(name) => name,
            Err(e) => {
                mark_failed(conn, &task_id, &format!("Upload failed: {}", e), false);
                continue;
            }
        };

        // 3. Parse workflow and prepare
        let workflow: Value = match serde_json::from_str(&workflow_json_str) {
            Ok(v) => v,
            Err(e) => {
                mark_failed(
                    conn,
                    &task_id,
                    &format!("Invalid workflow JSON: {}", e),
                    true,
                );
                continue;
            }
        };

        // Load text overrides from the task
        let text_overrides_str: String = conn
            .query_row(
                "SELECT COALESCE(text_overrides, '{}') FROM enhancement_tasks WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "{}".to_string());

        let text_overrides: std::collections::HashMap<String, String> =
            serde_json::from_str(&text_overrides_str).unwrap_or_default();

        let prepared = prepare_workflow(&workflow, &uploaded_name, &text_overrides);

        // 4. Queue prompt
        let prompt_id = match client.queue_prompt(&prepared) {
            Ok(id) => id,
            Err(e) => {
                mark_failed(conn, &task_id, &format!("Queue failed: {}", e), false);
                continue;
            }
        };

        // 5. Set queued with comfyui_prompt_id
        let _ = conn.execute(
            "UPDATE enhancement_tasks SET status = 'queued', comfyui_prompt_id = ?2 WHERE id = ?1",
            params![task_id, prompt_id],
        );

        info!("Task {} queued as ComfyUI prompt {}", task_id, prompt_id);
    }
}

/// Poll tasks that are queued/processing against ComfyUI history.
fn poll_active_tasks(conn: &Connection, client: &ComfyUiClient, timeout_secs: u64, library_root: &Path) {
    let tasks: Vec<(String, String, String, String)> = {
        let mut stmt = match conn.prepare(
            "SELECT t.id, t.shot_id, t.comfyui_prompt_id, t.started_at
             FROM enhancement_tasks t
             WHERE t.status IN ('queued', 'processing')
             AND t.comfyui_prompt_id IS NOT NULL",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to query active tasks: {}", e);
                return;
            }
        };
        let result = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                error!("Failed to fetch active tasks: {}", e);
                return;
            }
        };
        result
    };

    for (task_id, shot_id, prompt_id, started_at) in tasks {
        // Check timeout
        if let Ok(started) = chrono::NaiveDateTime::parse_from_str(&started_at, "%Y-%m-%d %H:%M:%S")
        {
            let elapsed = chrono::Utc::now()
                .naive_utc()
                .signed_duration_since(started)
                .num_seconds() as u64;
            if elapsed > timeout_secs {
                mark_failed(conn, &task_id, "Timed out waiting for ComfyUI", false);
                continue;
            }
        }

        // Update to processing if still queued
        let _ = conn.execute(
            "UPDATE enhancement_tasks SET status = 'processing' WHERE id = ?1 AND status = 'queued'",
            params![task_id],
        );

        // Check ComfyUI history
        let history = match client.get_history(&prompt_id) {
            Ok(Some(h)) => h,
            Ok(None) => continue, // Not done yet
            Err(e) => {
                warn!("Failed to get history for prompt {}: {}", prompt_id, e);
                continue;
            }
        };

        // Check for execution error
        if let Some(status) = history.get("status") {
            if let Some(true) = status.get("completed").and_then(|v| v.as_bool()) {
                // Completed — no error
            } else if status.get("messages").is_some() {
                // Check for error messages
                if let Some(msgs) = status.get("messages").and_then(|v| v.as_array()) {
                    for msg in msgs {
                        if let Some(arr) = msg.as_array() {
                            if arr.first().and_then(|v| v.as_str()) == Some("execution_error") {
                                let err_detail = arr
                                    .get(1)
                                    .and_then(|v| v.get("exception_message"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown execution error");
                                mark_failed(conn, &task_id, err_detail, false);
                                continue;
                            }
                        }
                    }
                }
            }
        }

        // Try to extract output images from history
        let outputs = history.get("outputs");
        if outputs.is_none() {
            continue; // Still processing
        }

        // Set downloading
        let _ = conn.execute(
            "UPDATE enhancement_tasks SET status = 'downloading' WHERE id = ?1",
            params![task_id],
        );

        // Find output images in any node's output
        let mut downloaded = false;
        if let Some(outputs) = outputs.and_then(|v| v.as_object()) {
            for (_node_id, node_output) in outputs {
                let images = node_output.get("images").and_then(|v| v.as_array());
                if let Some(images) = images {
                    for img_info in images {
                        let filename = img_info.get("filename").and_then(|v| v.as_str());
                        let subfolder = img_info
                            .get("subfolder")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let out_type = img_info
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("output");

                        if let Some(filename) = filename {
                            match download_and_save_output(
                                conn, client, &task_id, &shot_id, filename, subfolder, out_type, library_root,
                            ) {
                                Ok(_) => {
                                    downloaded = true;
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to download output {} for task {}: {}",
                                        filename, task_id, e
                                    );
                                }
                            }
                        }
                    }
                }
                // Also check for gifs/videos
                let gifs = node_output.get("gifs").and_then(|v| v.as_array());
                if let Some(gifs) = gifs {
                    for gif_info in gifs {
                        let filename = gif_info.get("filename").and_then(|v| v.as_str());
                        let subfolder = gif_info
                            .get("subfolder")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let out_type = gif_info
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("output");

                        if let Some(filename) = filename {
                            match download_and_save_output(
                                conn, client, &task_id, &shot_id, filename, subfolder, out_type, library_root,
                            ) {
                                Ok(_) => {
                                    downloaded = true;
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to download gif output {} for task {}: {}",
                                        filename, task_id, e
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        if downloaded {
            let _ = conn.execute(
                "UPDATE enhancement_tasks SET status = 'completed', completed_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![task_id],
            );
            info!("Task {} completed successfully", task_id);
        } else {
            mark_failed(
                conn,
                &task_id,
                "No output images found in ComfyUI response",
                false,
            );
        }
    }
}

/// Download an output file from ComfyUI and save it alongside the original.
fn download_and_save_output(
    conn: &Connection,
    client: &ComfyUiClient,
    task_id: &str,
    shot_id: &str,
    filename: &str,
    subfolder: &str,
    output_type: &str,
    library_root: &Path,
) -> anyhow::Result<()> {
    let data = client.download_output(filename, subfolder, output_type)?;

    // Get the original file path to determine where to save
    let original_path_str: String = conn
        .query_row(
            "SELECT path FROM files WHERE shot_id = ?1 AND is_original = 1 LIMIT 1",
            params![shot_id],
            |row| row.get(0),
        )
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

    let task_short = &task_id[..8.min(task_id.len())];
    let output_filename = format!("{}_enhanced_{}.{}", stem, task_short, ext);
    let output_path = parent.join(&output_filename);

    std::fs::write(&output_path, &data)?;
    info!("Saved enhanced output to {:?}", output_path);

    // Compute hash
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let hash = format!("{:x}", hasher.finalize());

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

    let file_id = Uuid::new_v4().to_string();
    let file_size = data.len() as i64;
    let path_str = db::make_relative(library_root, &output_path);

    conn.execute(
        "INSERT INTO files (id, shot_id, path, hash, mime_type, file_size, is_original)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        params![file_id, shot_id, path_str, hash, mime_type, file_size],
    )?;

    // Store the output file ID on the task
    conn.execute(
        "UPDATE enhancement_tasks SET output_file_id = ?2 WHERE id = ?1",
        params![task_id, file_id],
    )?;

    Ok(())
}

/// Mark a task as failed with an error message.
/// If `permanent` is true, set retry_count to max so it won't be retried.
fn mark_failed(conn: &Connection, task_id: &str, error_msg: &str, permanent: bool) {
    error!("Task {} failed: {}", task_id, error_msg);
    if permanent {
        let _ = conn.execute(
            "UPDATE enhancement_tasks SET status = 'failed', error_message = ?2, retry_count = 3 WHERE id = ?1",
            params![task_id, error_msg],
        );
    } else {
        let _ = conn.execute(
            "UPDATE enhancement_tasks SET status = 'failed', error_message = ?2 WHERE id = ?1",
            params![task_id, error_msg],
        );
    }
}

/// Check failed tasks that can be retried (retry_count < 3).
/// Uses exponential backoff: 10s, 30s, 120s.
fn check_retries(conn: &Connection) {
    let backoff_seconds = [10, 30, 120];
    let tasks: Vec<(String, i64, String)> = {
        let mut stmt = match conn.prepare(
            "SELECT id, retry_count, COALESCE(completed_at, created_at) as last_attempt
             FROM enhancement_tasks
             WHERE status = 'failed' AND retry_count < 3",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let result = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => return,
        };
        result
    };

    for (task_id, retry_count, _last_attempt) in tasks {
        let idx = (retry_count as usize).min(backoff_seconds.len() - 1);
        let wait = backoff_seconds[idx];

        // Simple approach: check if enough time has passed since the task was last updated
        let ready: bool = conn
            .query_row(
                "SELECT (strftime('%s','now') - strftime('%s', COALESCE(completed_at, created_at))) > ?2
                 FROM enhancement_tasks WHERE id = ?1",
                params![task_id, wait],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if ready {
            info!("Retrying task {} (attempt {})", task_id, retry_count + 1);
            let _ = conn.execute(
                "UPDATE enhancement_tasks SET status = 'pending', retry_count = retry_count + 1, error_message = NULL WHERE id = ?1",
                params![task_id],
            );
        }
    }
}

/// Remove completed tasks older than 5 minutes.
fn cleanup_completed_tasks(conn: &Connection) {
    match conn.execute(
        "DELETE FROM enhancement_tasks WHERE status = 'completed' AND completed_at IS NOT NULL AND (strftime('%s','now') - strftime('%s', completed_at)) > 300",
        [],
    ) {
        Ok(n) if n > 0 => info!("Cleaned up {} completed enhancement tasks", n),
        Err(e) => warn!("Failed to clean up completed tasks: {}", e),
        _ => {}
    }
}
