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
        let mut resp = match ureq::post(&url)
            .header("Content-Type", "application/json")
            .send(bytes.as_slice())
        {
            Ok(resp) => resp,
            Err(ureq::Error::StatusCode(status)) => {
                // ComfyUI returns 400 with JSON body for validation errors (bad prompts, missing nodes, etc.)
                anyhow::bail!("Queue prompt rejected by ComfyUI (HTTP {})", status);
            }
            Err(e) => {
                anyhow::bail!("Queue prompt failed: {}", e);
            }
        };

        let json: Value = resp.body_mut().read_json()?;

        // Check for error field in response (ComfyUI may return 200 with error details)
        if let Some(error) = json.get("error") {
            let error_msg = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown prompt validation error");
            let node_errors = json.get("node_errors")
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default();
            if node_errors.is_empty() {
                anyhow::bail!("ComfyUI prompt error: {}", error_msg);
            } else {
                anyhow::bail!("ComfyUI prompt error: {}. Node errors: {}", error_msg, node_errors);
            }
        }

        let prompt_id = json["prompt_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No 'prompt_id' in queue response: {}", json))?;
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

    /// Check if a prompt is still in ComfyUI's queue (pending or running).
    pub fn is_prompt_in_queue(&self, prompt_id: &str) -> anyhow::Result<bool> {
        let url = format!("{}/queue", self.base_url);
        let mut resp = ureq::get(&url)
            .call()
            .map_err(|e| anyhow::anyhow!("Queue fetch failed: {}", e))?;
        let json: Value = resp.body_mut().read_json()?;
        // queue_running and queue_pending are arrays of [number, prompt_id, ...]
        for key in &["queue_running", "queue_pending"] {
            if let Some(items) = json.get(*key).and_then(|v| v.as_array()) {
                for item in items {
                    if let Some(arr) = item.as_array() {
                        if arr.get(1).and_then(|v| v.as_str()) == Some(prompt_id) {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
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
/// If `source_file_id` is provided, uses that specific file; otherwise falls back to the original.
/// For images: reads the file directly.
/// For videos: extracts the first frame.
fn get_source_image(conn: &mut SqliteConnection, shot_id: &str, source_file_id: Option<&str>, library_root: &Path) -> anyhow::Result<(Vec<u8>, String)> {
    // If a specific source file is requested, use it; otherwise fall back to the original
    let (file_id_used, file_path, mime_type): (String, String, String) = if let Some(file_id) = source_file_id {
        let (fp, mt) = files::table
            .filter(files::id.eq(file_id).and(files::shot_id.eq(shot_id)))
            .select((files::path, diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')")))
            .first::<(String, String)>(conn)
            .map_err(|_| anyhow::anyhow!("Source file {} not found for shot {}", file_id, shot_id))?;
        (file_id.to_string(), fp, mt)
    } else {
        let (fid, fp, mt) = files::table
            .filter(files::shot_id.eq(shot_id).and(files::is_original.eq(true)))
            .order(files::created_at.asc())
            .select((files::id, files::path, diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')")))
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
    let upload_name = format!("phos_{}_{}.png", &shot_id[..8.min(shot_id.len())], &file_id_used[..8.min(file_id_used.len())]);
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

/// Mark any tasks that were in intermediate states as needing retry.
fn recover_interrupted_tasks(conn: &mut SqliteConnection) {
    let intermediate_states = ["uploading", "queued", "processing", "downloading"];
    if let Err(e) = diesel::update(
        enhancement_tasks::table
            .filter(enhancement_tasks::status.eq_any(&intermediate_states)),
    )
    .set((
        enhancement_tasks::status.eq("pending"),
        enhancement_tasks::error_message.eq("Recovered after restart"),
    ))
    .execute(conn)
    {
        warn!("Failed to recover interrupted tasks: {}", e);
    }
}

/// Pick up pending tasks and start processing them.
fn process_pending_tasks(conn: &mut SqliteConnection, client: &ComfyUiClient, library_root: &Path) {
    let tasks: Vec<(String, String, String, String, String, Option<String>)> = match enhancement_tasks::table
        .inner_join(comfyui_workflows::table.on(comfyui_workflows::id.eq(enhancement_tasks::workflow_id)))
        .filter(enhancement_tasks::status.eq("pending"))
        .order(enhancement_tasks::created_at.asc())
        .limit(5)
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            enhancement_tasks::workflow_id,
            comfyui_workflows::workflow_json,
            diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(enhancement_tasks.text_overrides, '{}')"),
            enhancement_tasks::source_file_id,
        ))
        .load::<(String, String, String, String, String, Option<String>)>(conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to query pending tasks: {}", e);
            return;
        }
    };

    let now = chrono::Utc::now().naive_utc().format("%Y-%m-%d %H:%M:%S").to_string();

    for (task_id, shot_id, _workflow_id, workflow_json_str, text_overrides_str, source_file_id) in tasks {
        // Set uploading
        let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task_id)))
            .set((
                enhancement_tasks::status.eq("uploading"),
                enhancement_tasks::started_at.eq(&now),
            ))
            .execute(conn);

        // 1. Get source image (use specific file if provided, otherwise original)
        let (image_data, upload_name) = match get_source_image(conn, &shot_id, source_file_id.as_deref(), library_root) {
            Ok(v) => v,
            Err(e) => {
                mark_failed(
                    conn,
                    &task_id,
                    &format!("Source image extraction failed: {}", e),
                );
                continue;
            }
        };

        // 2. Upload to ComfyUI
        let uploaded_name = match client.upload_image(&upload_name, &image_data) {
            Ok(name) => name,
            Err(e) => {
                mark_failed(conn, &task_id, &format!("Upload failed: {}", e));
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
                );
                continue;
            }
        };

        let text_overrides: std::collections::HashMap<String, String> =
            serde_json::from_str(&text_overrides_str).unwrap_or_default();

        let prepared = prepare_workflow(&workflow, &uploaded_name, &text_overrides);

        // 4. Queue prompt
        let prompt_id = match client.queue_prompt(&prepared) {
            Ok(id) => id,
            Err(e) => {
                mark_failed(conn, &task_id, &format!("Queue failed: {}", e));
                continue;
            }
        };

        // 5. Set queued with comfyui_prompt_id
        let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task_id)))
            .set((
                enhancement_tasks::status.eq("queued"),
                enhancement_tasks::comfyui_prompt_id.eq(&prompt_id),
            ))
            .execute(conn);

        info!("Task {} queued as ComfyUI prompt {}", task_id, prompt_id);
    }
}

/// Poll tasks that are queued/processing against ComfyUI history.
fn poll_active_tasks(conn: &mut SqliteConnection, client: &ComfyUiClient, library_root: &Path) {
    let tasks: Vec<(String, String, String, String, String)> = match enhancement_tasks::table
        .filter(
            enhancement_tasks::status.eq_any(&["queued", "processing"])
                .and(enhancement_tasks::comfyui_prompt_id.is_not_null()),
        )
        .select((
            enhancement_tasks::id,
            enhancement_tasks::shot_id,
            enhancement_tasks::comfyui_prompt_id.assume_not_null(),
            enhancement_tasks::workflow_id,
            diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(text_overrides, '{}')"),
        ))
        .load::<(String, String, String, String, String)>(conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to query active tasks: {}", e);
            return;
        }
    };

    for (task_id, shot_id, prompt_id, workflow_id, text_overrides_str) in tasks {
        // Update to processing if still queued
        let _ = diesel::update(
            enhancement_tasks::table.filter(
                enhancement_tasks::id.eq(&task_id).and(enhancement_tasks::status.eq("queued")),
            ),
        )
        .set(enhancement_tasks::status.eq("processing"))
        .execute(conn);

        // Check ComfyUI history
        let history = match client.get_history(&prompt_id) {
            Ok(Some(h)) => h,
            Ok(None) => {
                // Not in history yet — check if still queued/running in ComfyUI
                match client.is_prompt_in_queue(&prompt_id) {
                    Ok(true) => continue, // Still pending/running, check again next cycle
                    Ok(false) => {
                        mark_failed(conn, &task_id, &format!(
                            "Prompt {} not found in ComfyUI history or queue (job lost)", prompt_id
                        ));
                    }
                    Err(e) => {
                        warn!("Failed to check queue for prompt {}: {}", prompt_id, e);
                    }
                }
                continue;
            }
            Err(e) => {
                warn!("Failed to get history for prompt {}: {}", prompt_id, e);
                continue;
            }
        };

        // Check for execution error
        if let Some(status) = history.get("status") {
            let completed = status.get("completed").and_then(|v| v.as_bool()).unwrap_or(false);
            if !completed {
                // Look for explicit error messages
                let mut found_error = false;
                if let Some(msgs) = status.get("messages").and_then(|v| v.as_array()) {
                    for msg in msgs {
                        if let Some(arr) = msg.as_array() {
                            if arr.first().and_then(|v| v.as_str()) == Some("execution_error") {
                                let err_data = arr.get(1);
                                let exception_msg = err_data
                                    .and_then(|v| v.get("exception_message"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown error");
                                let node_type = err_data
                                    .and_then(|v| v.get("node_type"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                let node_id = err_data
                                    .and_then(|v| v.get("node_id"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?");
                                let traceback = err_data
                                    .and_then(|v| v.get("traceback"))
                                    .and_then(|v| v.as_array())
                                    .map(|tb| tb.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(""))
                                    .unwrap_or_default();

                                let err_detail = format!(
                                    "ComfyUI execution error in node {} ({}): {}",
                                    node_id, node_type, exception_msg
                                );
                                if !traceback.is_empty() {
                                    error!("Task {} traceback:\n{}", task_id, traceback);
                                }
                                mark_failed(conn, &task_id, &err_detail);
                                found_error = true;
                                break;
                            }
                        }
                    }
                }
                if found_error {
                    continue;
                }

                // Check if status_str indicates a non-success state
                let status_str = status.get("status_str").and_then(|v| v.as_str()).unwrap_or("");
                if status_str == "error" {
                    let err_msg = format!(
                        "ComfyUI prompt failed with status '{}'. Status details: {}",
                        status_str,
                        serde_json::to_string(status).unwrap_or_else(|_| "N/A".to_string())
                    );
                    mark_failed(conn, &task_id, &err_msg);
                    continue;
                }

                // Not completed and no error — still running
                continue;
            }
        }

        // Try to extract output images from history.
        // ComfyUI has a race condition where the history endpoint can report
        // completion before outputs are fully populated. If completed but outputs
        // are empty, re-fetch history a few times before giving up.
        let history = if !outputs_has_downloadable_items(history.get("outputs")) {
            const EMPTY_OUTPUT_RETRIES: u32 = 5;
            const EMPTY_OUTPUT_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

            let mut resolved_history = None;
            for attempt in 1..=EMPTY_OUTPUT_RETRIES {
                info!(
                    "Task {} completed but outputs empty, re-fetching history (attempt {}/{})",
                    task_id, attempt, EMPTY_OUTPUT_RETRIES
                );
                std::thread::sleep(EMPTY_OUTPUT_DELAY);

                match client.get_history(&prompt_id) {
                    Ok(Some(h)) => {
                        if outputs_has_downloadable_items(h.get("outputs")) {
                            info!("Task {} outputs became available on retry {}", task_id, attempt);
                            resolved_history = Some(h);
                            break;
                        }
                    }
                    Ok(None) => {
                        warn!("Task {} history entry disappeared on retry {}", task_id, attempt);
                    }
                    Err(e) => {
                        warn!("Task {} history re-fetch failed on retry {}: {}", task_id, attempt, e);
                    }
                }
            }

            match resolved_history {
                Some(h) => h,
                None => {
                    // If outputs was truly absent/null, continue polling on next cycle
                    let original_outputs = history.get("outputs");
                    if original_outputs.is_none()
                        || original_outputs.map(|v| v.is_null()).unwrap_or(false)
                    {
                        continue;
                    }
                    // Outputs present but empty — fall through to failure path
                    history
                }
            }
        } else {
            history
        };

        // Set downloading
        let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task_id)))
            .set(enhancement_tasks::status.eq("downloading"))
            .execute(conn);

        // Find output images in any node's output
        let outputs = history.get("outputs");
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
                                conn, client, &task_id, &shot_id, filename, subfolder, out_type, library_root, &workflow_id, &text_overrides_str,
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
                                conn, client, &task_id, &shot_id, filename, subfolder, out_type, library_root, &workflow_id, &text_overrides_str,
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

        if !downloaded {
            // Check if a previous attempt already saved an output file for this task
            let has_existing_output: bool = enhancement_tasks::table
                .filter(
                    enhancement_tasks::id.eq(&task_id)
                        .and(enhancement_tasks::output_file_id.is_not_null()),
                )
                .count()
                .get_result::<i64>(conn)
                .map(|c| c > 0)
                .unwrap_or(false);
            if has_existing_output {
                downloaded = true;
                info!("Task {} has output from a previous attempt, marking as completed", task_id);
            }
        }

        let now = chrono::Utc::now().naive_utc().format("%Y-%m-%d %H:%M:%S").to_string();

        if downloaded {
            let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&task_id)))
                .set((
                    enhancement_tasks::status.eq("completed"),
                    enhancement_tasks::completed_at.eq(&now),
                ))
                .execute(conn);
            info!("Task {} completed successfully", task_id);
        } else {
            // Log the full outputs for debugging
            let outputs_debug = history.get("outputs")
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "N/A".to_string()))
                .unwrap_or_else(|| "null".to_string());
            let status_debug = history.get("status")
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "N/A".to_string()))
                .unwrap_or_else(|| "null".to_string());
            error!(
                "Task {} produced no downloadable outputs. Status: {}, Outputs: {}",
                task_id, status_debug, outputs_debug
            );
            mark_failed(
                conn,
                &task_id,
                "No output images found in ComfyUI response (workflow completed but produced no downloadable files)",
            );
        }
    }
}

/// Check whether a history response's `outputs` contains any downloadable items
/// (images or gifs with a filename in any node's output).
fn outputs_has_downloadable_items(outputs: Option<&Value>) -> bool {
    let obj = match outputs.and_then(|v| v.as_object()) {
        Some(o) if !o.is_empty() => o,
        _ => return false,
    };
    for (_node_id, node_output) in obj {
        if let Some(images) = node_output.get("images").and_then(|v| v.as_array()) {
            if images.iter().any(|img| img.get("filename").and_then(|v| v.as_str()).is_some()) {
                return true;
            }
        }
        if let Some(gifs) = node_output.get("gifs").and_then(|v| v.as_array()) {
            if gifs.iter().any(|g| g.get("filename").and_then(|v| v.as_str()).is_some()) {
                return true;
            }
        }
    }
    false
}

/// Download an output file from ComfyUI and save it alongside the original.
fn download_and_save_output(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    task_id: &str,
    shot_id: &str,
    filename: &str,
    subfolder: &str,
    output_type: &str,
    library_root: &Path,
    workflow_id: &str,
    text_overrides_json: &str,
) -> anyhow::Result<()> {
    let data = client.download_output(filename, subfolder, output_type)?;

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
    let hash = format!("{:x}", hasher.finalize());

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
            info!("Task {} output already exists with same hash, skipping write", task_id);
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

/// Mark a task as failed with an error message.
fn mark_failed(conn: &mut SqliteConnection, task_id: &str, error_msg: &str) {
    error!("Task {} failed: {}", task_id, error_msg);
    let _ = diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
        .set((
            enhancement_tasks::status.eq("failed"),
            enhancement_tasks::error_message.eq(error_msg),
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
