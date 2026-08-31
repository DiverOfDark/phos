//! Contract tests against a real ComfyUI.
//!
//! Every defect in the completion path so far was a mismatch with what ComfyUI
//! actually does — which key a node publishes its file under, how the counter
//! in the filename is spelled, what a rejected prompt's body looks like. The
//! unit tests in `src/comfyui/` pin *our reading* of that contract; these pin
//! the contract itself, by driving the real worker against a real server with
//! workflows built from core nodes only (no model is ever downloaded).
//!
//! Where the server comes from, in order:
//!
//! * `PHOS_COMFYUI_TEST_URL` — an instance you already have running.
//! * `PHOS_COMFYUI_TEST_IMAGE` — a Docker image (see `docker/comfyui-test/`)
//!   started through testcontainers.
//! * Neither set — the test is skipped, so plain `cargo test` never pulls an
//!   image.
//!
//! It is one `#[test]` with named steps rather than one test per scenario, so
//! the container is owned by a local and is stopped when the test returns —
//! a container parked in a `static` outlives the process and is left running.
//! Each step gets a fresh library and its own time budget; the container gets
//! its own for starting up.
//!
//! Run locally:
//!
//! ```text
//! docker build -t comfyui-test docker/comfyui-test
//! PHOS_COMFYUI_TEST_IMAGE=comfyui-test:latest cargo test --test comfyui_contract_test -- --nocapture
//! ```

use diesel::prelude::*;
use phos_backend::comfyui::nodes::{parse_object_info, WideInt, WidgetSpec};
use phos_backend::comfyui::{detect_inputs, spawn_enhancement_worker, ComfyUiClient, NodeCatalog};
use phos_backend::models::{NewComfyuiWorkflow, NewEnhancementTask, NewFile, NewShot};
use phos_backend::schema::{comfyui_workflows, enhancement_tasks, files, shots};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use testcontainers::core::wait::HttpWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, GenericImage, ImageExt};

// ===== Where ComfyUI comes from ==============================================

/// ComfyUI has this long to answer `/system_stats` after the container starts.
const STARTUP_BUDGET: Duration = Duration::from_secs(120);

/// Each scenario has this long once the server is up. The worker wakes every
/// three seconds and every graph here runs in well under one, so a step that
/// needs more than this is waiting on something it should not be.
const STEP_BUDGET: Duration = Duration::from_secs(10);

enum Server {
    External(String),
    Container {
        url: String,
        container: Box<Container<GenericImage>>,
    },
}

impl Server {
    fn url(&self) -> &str {
        match self {
            Server::External(u) => u,
            Server::Container { url, .. } => url,
        }
    }

    /// Everything ComfyUI has written to its console so far.
    fn console(&self) -> String {
        match self {
            Server::External(_) => String::new(),
            Server::Container { container, .. } => {
                let mut out = container.stdout_to_vec().unwrap_or_default();
                out.extend(container.stderr_to_vec().unwrap_or_default());
                String::from_utf8_lossy(&out).into_owned()
            }
        }
    }
}

/// A live ComfyUI, or `None` if this run has nowhere to send work.
fn start_server() -> Option<Server> {
    if let Ok(url) = std::env::var("PHOS_COMFYUI_TEST_URL") {
        eprintln!("comfyui contract: using {}", url);
        return Some(Server::External(url.trim_end_matches('/').to_string()));
    }
    let Ok(image) = std::env::var("PHOS_COMFYUI_TEST_IMAGE") else {
        eprintln!(
            "comfyui contract: neither PHOS_COMFYUI_TEST_URL nor PHOS_COMFYUI_TEST_IMAGE \
             is set; skipping"
        );
        return None;
    };
    let (name, tag) = image
        .rsplit_once(':')
        .filter(|(_, t)| !t.contains('/'))
        .unwrap_or((image.as_str(), "latest"));
    eprintln!("comfyui contract: starting {}:{}", name, tag);

    // Wait on the API rather than a log line: ComfyUI logs to stderr, and the
    // exact wording of its banner is nobody's contract. /system_stats answers
    // as soon as the server can take a prompt.
    let started = Instant::now();
    let container = GenericImage::new(name, tag)
        .with_exposed_port(8188.tcp())
        .with_wait_for(WaitFor::http(
            HttpWaitStrategy::new("/system_stats")
                .with_port(8188.tcp())
                .with_expected_status_code(200u16),
        ))
        .with_startup_timeout(STARTUP_BUDGET)
        .start()
        .unwrap_or_else(|e| {
            panic!(
                "ComfyUI did not answer /system_stats within {:?}: {}",
                STARTUP_BUDGET, e
            )
        });
    let port = container
        .get_host_port_ipv4(8188)
        .expect("mapped port for ComfyUI");
    let url = format!("http://127.0.0.1:{}", port);
    eprintln!(
        "comfyui contract: ComfyUI is up at {} after {:.1}s",
        url,
        started.elapsed().as_secs_f32()
    );
    Some(Server::Container {
        url,
        container: Box::new(container),
    })
}

/// Lines in ComfyUI's console that mean something went wrong at startup. A
/// server that came up complaining — a missing dependency, a failed import,
/// a broken database migration — will pass `/system_stats` and then fail
/// prompts in ways that look like our bug.
///
/// INFO lines are exempt even when they quote an exception: on CPU, ComfyUI
/// reports its optional accelerator backends as
/// `[INFO] ... {'unavailable_reason': "ImportError: No module named 'triton'"}`,
/// which is a status report, not a failure.
fn console_errors(console: &str) -> Vec<&str> {
    console
        .lines()
        .filter(|l| !l.contains("[INFO]") && !l.contains("[DEBUG]"))
        .filter(|l| {
            l.contains("[ERROR]")
                || l.contains("[CRITICAL]")
                || l.contains("Traceback (most recent call last)")
                || l.contains("IMPORT FAILED")
                || is_bare_exception_line(l)
        })
        .collect()
}

/// `SomethingError: message` — the last line of a Python traceback.
fn is_bare_exception_line(line: &str) -> bool {
    let Some((head, _)) = line.split_once(": ") else {
        return false;
    };
    let head = head.trim();
    (head.ends_with("Error") || head.ends_with("Exception"))
        && head
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

// ===== A throwaway library ====================================================

/// One shot with one original, in a temp directory with its own `.phos.db`.
struct Library {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db_path: PathBuf,
    shot_id: String,
    original: PathBuf,
}

const FIXTURE: &str = "celentano.jpg";

impl Library {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp library");
        let root = dir.path().to_path_buf();
        let db_path = root.join(".phos.db");
        phos_backend::db::init_and_migrate(&db_path).expect("create the library database");

        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(FIXTURE);
        let original = root.join(FIXTURE);
        std::fs::copy(&fixture, &original).expect("copy the fixture into the library");
        let bytes = std::fs::read(&original).unwrap();
        let hash = hex::encode(Sha256::digest(&bytes));

        let shot_id = uuid::Uuid::new_v4().to_string();
        let file_id = uuid::Uuid::new_v4().to_string();
        let mut conn = phos_backend::db::open_diesel_connection(&db_path).unwrap();
        diesel::insert_into(shots::table)
            .values(NewShot {
                id: &shot_id,
                main_file_id: Some(&file_id),
                timestamp: None,
                width: None,
                height: None,
                latitude: None,
                longitude: None,
                primary_person_id: None,
                folder_number: None,
                review_status: None,
                description: None,
            })
            .execute(&mut conn)
            .unwrap();
        diesel::insert_into(files::table)
            .values(NewFile {
                id: &file_id,
                shot_id: &shot_id,
                path: FIXTURE,
                hash: &hash,
                mime_type: Some("image/jpeg"),
                file_size: Some(bytes.len() as i32),
                is_original: Some(true),
                visual_embedding: None,
                source_workflow_id: None,
                source_text_overrides: None,
            })
            .execute(&mut conn)
            .unwrap();

        Self {
            _dir: dir,
            root,
            db_path,
            shot_id,
            original,
        }
    }

    fn conn(&self) -> SqliteConnection {
        phos_backend::db::open_diesel_connection(&self.db_path).unwrap()
    }

    fn add_workflow(&self, name: &str, graph: &Value) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let graph_json = serde_json::to_string(graph).unwrap();
        diesel::insert_into(comfyui_workflows::table)
            .values(NewComfyuiWorkflow {
                id: &id,
                name,
                description: None,
                workflow_json: &graph_json,
                inputs_json: None,
                outputs_json: None,
            })
            .execute(&mut self.conn())
            .unwrap();
        id
    }

    /// Queue a task exactly as `POST /api/comfyui/tasks` does.
    fn queue_task(&self, workflow_id: &str) -> String {
        self.queue_task_with(workflow_id, &json!({}))
    }

    /// Queue a task with overrides keyed `node_id.field`, as the console sends them.
    fn queue_task_with(&self, workflow_id: &str, overrides: &Value) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let overrides = serde_json::to_string(overrides).unwrap();
        diesel::insert_into(enhancement_tasks::table)
            .values(NewEnhancementTask {
                id: &id,
                shot_id: &self.shot_id,
                workflow_id,
                text_overrides: Some(&overrides),
                source_file_id: None,
            })
            .execute(&mut self.conn())
            .unwrap();
        id
    }

    fn task(&self, id: &str) -> TaskRow {
        let (status, error_message, output_file_id, retry_count, output_prefix) =
            enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(id))
                .select((
                    enhancement_tasks::status,
                    enhancement_tasks::error_message,
                    enhancement_tasks::output_file_id,
                    diesel::dsl::sql::<diesel::sql_types::Integer>("COALESCE(retry_count, 0)"),
                    enhancement_tasks::output_prefix,
                ))
                .first::<(String, Option<String>, Option<String>, i32, Option<String>)>(
                    &mut self.conn(),
                )
                .expect("task row");
        TaskRow {
            status,
            error_message,
            output_file_id,
            retry_count,
            output_prefix,
        }
    }

    /// Where the task's output landed on disk, and the row that describes it.
    fn output_file(&self, task: &TaskRow) -> (PathBuf, OutputFileRow) {
        let file_id = task
            .output_file_id
            .as_deref()
            .expect("task has an output file");
        let (path, mime_type, is_original, source_workflow_id) = files::table
            .filter(files::id.eq(file_id))
            .select((
                files::path,
                files::mime_type,
                files::is_original,
                files::source_workflow_id,
            ))
            .first::<(String, Option<String>, Option<bool>, Option<String>)>(&mut self.conn())
            .expect("output file row");
        (
            phos_backend::db::resolve_path(&self.root, &path),
            OutputFileRow {
                path,
                mime_type,
                is_original,
                source_workflow_id,
            },
        )
    }

    /// Run the real worker until the task reaches a terminal status or `budget`
    /// runs out. Whichever it is, the worker is stopped *before* this returns:
    /// a panic here would drop the tokio runtime while the worker thread still
    /// loops, and the runtime's drop waits for it forever.
    fn run_until_done(&self, comfyui_url: &str, task_id: &str, budget: Duration) -> TaskRow {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let shutdown = Arc::new((Mutex::new(false), Condvar::new()));
        let handle = {
            let _enter = rt.enter();
            spawn_enhancement_worker(
                self.db_path.clone(),
                comfyui_url.to_string(),
                shutdown.clone(),
            )
        };

        let started = Instant::now();
        let mut row = self.task(task_id);
        while !matches!(row.status.as_str(), "completed" | "failed" | "cancelled")
            && started.elapsed() < budget
        {
            std::thread::sleep(Duration::from_millis(250));
            row = self.task(task_id);
        }
        let elapsed = started.elapsed();

        *shutdown.0.lock().unwrap() = true;
        shutdown.1.notify_all();
        rt.block_on(handle).unwrap();
        rt.shutdown_timeout(Duration::from_secs(10));

        assert!(
            matches!(row.status.as_str(), "completed" | "failed" | "cancelled"),
            "task {} still `{}` after {:?} (budget {:?}): {:?}",
            task_id,
            row.status,
            elapsed,
            budget,
            row.error_message
        );
        row
    }
}

#[derive(Debug)]
struct TaskRow {
    status: String,
    error_message: Option<String>,
    output_file_id: Option<String>,
    retry_count: i32,
    output_prefix: Option<String>,
}

#[derive(Debug)]
struct OutputFileRow {
    path: String,
    mime_type: Option<String>,
    is_original: Option<bool>,
    source_workflow_id: Option<String>,
}

// ===== Workflows from core nodes only =========================================

/// LoadImage → ImageInvert → SaveImage. The invert is there so the result is
/// verifiably *processed*, not merely copied.
fn invert_image_workflow() -> Value {
    json!({
        "1": { "class_type": "LoadImage",   "inputs": { "image": "replaced-by-phos.png" } },
        "2": { "class_type": "ImageInvert", "inputs": { "image": ["1", 0] } },
        "3": { "class_type": "SaveImage",   "inputs": { "images": ["2", 0], "filename_prefix": "ComfyUI" } }
    })
}

/// LoadImage → ImageScale → CreateVideo → SaveVideo: a one-frame mp4 through
/// the core video saver, which is the node whose output key and filename
/// spelling we got wrong before. The scale is not decoration: libx264 refuses
/// yuv420p frames with an odd dimension, and the fixture is 585 px wide.
fn one_frame_video_workflow() -> Value {
    json!({
        "1": { "class_type": "LoadImage",   "inputs": { "image": "replaced-by-phos.png" } },
        "4": { "class_type": "ImageScale",  "inputs": { "image": ["1", 0], "upscale_method": "nearest-exact",
                                                        "width": 256, "height": 256, "crop": "disabled" } },
        "2": { "class_type": "CreateVideo", "inputs": { "images": ["4", 0], "fps": 8.0 } },
        "3": { "class_type": "SaveVideo",   "inputs": { "video": ["2", 0], "filename_prefix": "video/ComfyUI",
                                                        "format": "mp4", "codec": "h264" } }
    })
}

/// The same graph without the scale: the 585-px-wide fixture reaches libx264
/// as-is, which fails inside `SaveVideo` at run time — after `/prompt`
/// accepted the graph. A model-free way to get a genuine `execution_error`.
/// (If a future ComfyUI pads odd frames itself, this test will say so.)
fn odd_sized_video_workflow() -> Value {
    json!({
        "1": { "class_type": "LoadImage",   "inputs": { "image": "replaced-by-phos.png" } },
        "2": { "class_type": "CreateVideo", "inputs": { "images": ["1", 0], "fps": 8.0 } },
        "3": { "class_type": "SaveVideo",   "inputs": { "video": ["2", 0], "filename_prefix": "video/ComfyUI",
                                                        "format": "mp4", "codec": "h264" } }
    })
}

/// SaveImage wired to a node that does not exist: ComfyUI refuses this at
/// `/prompt`, before anything runs.
fn dangling_link_workflow() -> Value {
    json!({
        "1": { "class_type": "LoadImage", "inputs": { "image": "replaced-by-phos.png" } },
        "3": { "class_type": "SaveImage", "inputs": { "images": ["99", 0], "filename_prefix": "ComfyUI" } }
    })
}

// ===== Helpers ===============================================================

/// Does `/view` serve this file? This is how the by-name fallback finds a
/// file when history is silent, so the names it guesses have to be the names
/// ComfyUI writes.
/// LoadImage → ImageScale → SaveImage, with a text encoder hanging off a model
/// that is never loaded. The scale node carries an enum (`upscale_method`) the
/// console can override, and the encoder a titled prompt box; nothing in it
/// needs a model file, and the encoder never runs because nothing downstream
/// asks for it.
fn scale_and_prompt_workflow() -> Value {
    json!({
        "1": { "class_type": "LoadImage",   "inputs": { "image": "replaced-by-phos.png" } },
        "4": { "class_type": "ImageScale",  "inputs": { "image": ["1", 0], "upscale_method": "nearest-exact",
                                                        "width": 32, "height": 32, "crop": "disabled" },
               "_meta": { "title": "Resize" } },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "a cat", "clip": ["9", 0] },
               "_meta": { "title": "Positive Prompt" } },
        "3": { "class_type": "SaveImage",   "inputs": { "images": ["4", 0], "filename_prefix": "ComfyUI" } }
    })
}

/// The raw `/object_info` document, read the plain way so a test can compare
/// what the server said against what the catalogue made of it.
fn object_info(comfyui_url: &str) -> Value {
    ureq::get(&format!("{}/object_info", comfyui_url))
        .call()
        .expect("/object_info answers")
        .body_mut()
        .read_json()
        .expect("/object_info is JSON")
}

/// The catalogue as the import path sees it, read from the server now.
fn live_catalog(comfyui_url: &str) -> Arc<NodeCatalog> {
    phos_backend::comfyui::refresh_node_catalog(&ComfyUiClient::new(comfyui_url))
        .expect("a current ComfyUI serves /object_info and it parses to classes")
}

fn widget(catalog: &NodeCatalog, class: &str, field: &str) -> WidgetSpec {
    catalog
        .get(class)
        .unwrap_or_else(|| panic!("{} is a core node and must be in the catalogue", class))
        .input(field)
        .unwrap_or_else(|| panic!("{}.{} is declared by ComfyUI", class, field))
        .widget
        .clone()
}

fn view_exists(comfyui_url: &str, subfolder: &str, filename: &str) -> bool {
    let url = format!(
        "{}/view?filename={}&subfolder={}&type=output",
        comfyui_url, filename, subfolder
    );
    ureq::get(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

fn split_prefix(prefix: &str) -> (&str, &str) {
    prefix.rsplit_once('/').unwrap_or(("", prefix))
}

// ===== The scenarios =========================================================

fn an_image_workflow_round_trips_into_the_library(url: &str) {
    let lib = Library::new();
    let wf = lib.add_workflow("invert", &invert_image_workflow());
    let task_id = lib.queue_task(&wf);

    let task = lib.run_until_done(url, &task_id, STEP_BUDGET);
    assert_eq!(task.status, "completed", "{:?}", task);
    assert_eq!(task.error_message, None);
    assert_eq!(task.retry_count, 0, "a clean run must not spend retries");

    let (path, row) = lib.output_file(&task);
    assert!(path.exists(), "output missing on disk: {:?}", path);
    assert_eq!(
        path.parent(),
        lib.original.parent(),
        "saved beside the original"
    );
    assert!(
        row.path.starts_with("celentano_enhanced_") && row.path.ends_with(".png"),
        "unexpected output name {:?}",
        row.path
    );
    assert_eq!(row.mime_type.as_deref(), Some("image/png"));
    assert_eq!(row.is_original, Some(false));
    assert_eq!(row.source_workflow_id.as_deref(), Some(wf.as_str()));

    // It is the source, inverted — not a copy, and not something else.
    let original = image::open(&lib.original).unwrap().to_rgb8();
    let output = image::open(&path).unwrap().to_rgb8();
    assert_eq!(output.dimensions(), original.dimensions());
    let (w, h) = original.dimensions();
    for (x, y) in [(w / 2, h / 2), (w / 4, h / 4), (w - 1, h - 1)] {
        let o = original.get_pixel(x, y).0;
        let p = output.get_pixel(x, y).0;
        for c in 0..3 {
            let expected = 255 - o[c] as i32;
            assert!(
                (p[c] as i32 - expected).abs() <= 2,
                "pixel ({},{}) channel {}: original {} → output {}, expected ~{}",
                x,
                y,
                c,
                o[c],
                p[c],
                expected
            );
        }
    }

    // The name contract behind the by-name fallback: SaveImage writes
    // <prefix>_00001_.png under the pinned subfolder.
    let prefix = task.output_prefix.expect("prefix pinned before the run");
    let (subfolder, stem) = split_prefix(&prefix);
    assert_eq!(subfolder, "phos");
    assert!(
        view_exists(url, subfolder, &format!("{}_00001_.png", stem)),
        "SaveImage did not write {}/{}_00001_.png",
        subfolder,
        stem
    );
}

fn a_video_workflow_round_trips_into_the_library(url: &str) {
    let lib = Library::new();
    let wf = lib.add_workflow("one-frame-video", &one_frame_video_workflow());
    let task_id = lib.queue_task(&wf);

    let task = lib.run_until_done(url, &task_id, STEP_BUDGET);
    assert_eq!(task.status, "completed", "{:?}", task);
    assert_eq!(task.retry_count, 0, "a clean run must not spend retries");

    let (path, row) = lib.output_file(&task);
    assert!(
        row.path.ends_with(".mp4"),
        "unexpected output name {:?}",
        row.path
    );
    assert_eq!(row.mime_type.as_deref(), Some("video/mp4"));
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        bytes.len() > 100,
        "mp4 is suspiciously small: {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[4..8], b"ftyp", "not an ISO media file");

    // The name contract: core SaveVideo spells the counter `_00001_` with a
    // trailing underscore, like SaveImage — not `_00001` like VHS_VideoCombine.
    let prefix = task.output_prefix.expect("prefix pinned before the run");
    let (subfolder, stem) = split_prefix(&prefix);
    assert!(
        view_exists(url, subfolder, &format!("{}_00001_.mp4", stem)),
        "SaveVideo did not write {}/{}_00001_.mp4",
        subfolder,
        stem
    );
}

fn a_rejected_prompt_fails_at_once_with_comfyuis_reason(url: &str) {
    let lib = Library::new();
    let wf = lib.add_workflow("dangling", &dangling_link_workflow());
    let task_id = lib.queue_task(&wf);

    let task = lib.run_until_done(url, &task_id, STEP_BUDGET);
    assert_eq!(task.status, "failed", "{:?}", task);
    let message = task.error_message.clone().unwrap_or_default();
    assert!(
        message.contains("ComfyUI rejected the prompt"),
        "a validation failure should be reported as a rejection, got: {}",
        message
    );
    assert!(
        message.contains("99") || message.to_lowercase().contains("required input"),
        "the message should carry ComfyUI's own reason, got: {}",
        message
    );
    assert_eq!(
        task.retry_count, 0,
        "a rejected graph is rejected for good; retrying it burns nothing but time"
    );
}

fn a_failing_node_reports_its_id_and_message_without_retrying(url: &str) {
    let lib = Library::new();
    let wf = lib.add_workflow("odd-video", &odd_sized_video_workflow());
    let task_id = lib.queue_task(&wf);

    let task = lib.run_until_done(url, &task_id, STEP_BUDGET);
    assert_eq!(task.status, "failed", "{:?}", task);
    let message = task.error_message.clone().unwrap_or_default();
    assert!(
        message.contains("execution error in node 3 (SaveVideo)"),
        "the user needs to know which node raised, got: {}",
        message
    );
    assert!(
        message.contains("avcodec_open2") || message.contains("libx264"),
        "the node's own message should survive, got: {}",
        message
    );
    assert_eq!(
        task.retry_count, 0,
        "a node that raised will raise again; this must not be retried"
    );
    assert_eq!(task.output_file_id, None);
}

/// The scenario the settle path exists for, with a real file: the prompt is
/// gone from ComfyUI (restart, cleared history) but its output is on disk
/// under the prefix we pinned. The worker must find it by name.
fn lost_prompt_is_recovered_by_name(url: &str, graph: Value, name: &str) {
    let lib = Library::new();
    let wf = lib.add_workflow(name, &graph);

    // Run it for real once, so ComfyUI has written the file.
    let first = lib.run_until_done(url, &lib.queue_task(&wf), STEP_BUDGET);
    assert_eq!(first.status, "completed", "{:?}", first);
    let prefix = first.output_prefix.clone().expect("prefix pinned");

    // Now a task that claims that prefix but whose prompt ComfyUI has never
    // heard of. History is empty, the queue is empty: the only way to complete
    // is to guess the filename correctly.
    let lost_id = lib.queue_task(&wf);
    let ghost_prompt = uuid::Uuid::new_v4().to_string();
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&lost_id)))
        .set((
            enhancement_tasks::status.eq("processing"),
            enhancement_tasks::comfyui_prompt_id.eq(&ghost_prompt),
            enhancement_tasks::output_prefix.eq(&prefix),
        ))
        .execute(&mut lib.conn())
        .unwrap();

    let lost = lib.run_until_done(url, &lost_id, STEP_BUDGET);
    assert_eq!(
        lost.status, "completed",
        "the file exists under {} but the by-name probe did not find it: {:?}",
        prefix, lost
    );
    let (path, _) = lib.output_file(&lost);
    assert!(path.exists());
}

fn a_lost_image_prompt_is_recovered_by_name(url: &str) {
    lost_prompt_is_recovered_by_name(url, invert_image_workflow(), "invert");
}

fn a_lost_video_prompt_is_recovered_by_name(url: &str) {
    lost_prompt_is_recovered_by_name(url, one_frame_video_workflow(), "one-frame-video");
}

// ===== What ComfyUI says about its nodes =====================================

/// The parser skips what it cannot read rather than failing, which is right
/// for a custom pack nobody has seen — and wrong if it is quietly skipping
/// inputs on the server everyone has. So: every input the live document
/// declares, on every class, must come out the other side.
fn nothing_in_the_live_object_info_is_skipped_silently(url: &str) {
    let doc = object_info(url);
    let catalog = parse_object_info(&doc);
    let classes = doc.as_object().expect("/object_info is an object");
    assert!(
        classes.len() > 100,
        "a bare ComfyUI has a few hundred core classes, this one has {}",
        classes.len()
    );

    let mut skipped = Vec::new();
    for (name, entry) in classes {
        let declared: usize = ["required", "optional"]
            .iter()
            .map(|section| {
                entry["input"][section]
                    .as_object()
                    .map_or(0, |fields| fields.len())
            })
            .sum();
        let read = catalog.get(name).map_or(0, |class| class.inputs.len());
        if read != declared {
            let unread: Vec<&str> = ["required", "optional"]
                .iter()
                .flat_map(|section| {
                    entry["input"][section]
                        .as_object()
                        .into_iter()
                        .flat_map(|fields| fields.keys())
                })
                .filter(|field| catalog.get(name).and_then(|c| c.input(field)).is_none())
                .map(String::as_str)
                .collect();
            skipped.push(format!(
                "{}: declares {} inputs, {} read; unread: {:?}",
                name, declared, read, unread
            ));
        }
    }
    assert!(
        skipped.is_empty(),
        "{} classes have inputs the parser silently dropped — a spec shape it \
         does not know:\n{}",
        skipped.len(),
        skipped.join("\n")
    );
}

/// The shapes the unit fixtures were transcribed from, checked against the
/// document a real server actually sends.
fn core_nodes_are_read_the_way_the_fixtures_assume(url: &str) {
    let doc = object_info(url);
    let catalog = live_catalog(url);

    // The seed marker is where the fixture puts it, and the range is the full
    // unsigned one — not clamped on the way through.
    assert_eq!(
        doc["KSampler"]["input"]["required"]["seed"][1]["control_after_generate"],
        json!(true),
        "the seed marker has moved: {}",
        doc["KSampler"]["input"]["required"]["seed"]
    );
    match widget(&catalog, "KSampler", "seed") {
        WidgetSpec::Seed { min, max, .. } => {
            assert_eq!(min, Some(WideInt(0)));
            assert_eq!(max, Some(WideInt(u64::MAX as i128)));
        }
        other => panic!("KSampler.seed should be a seed, got {:?}", other),
    }
    match widget(&catalog, "KSampler", "steps") {
        WidgetSpec::Int {
            min: Some(1),
            max: Some(10000),
            ..
        } => {}
        other => panic!("KSampler.steps range: {:?}", other),
    }
    assert!(matches!(
        widget(&catalog, "KSampler", "cfg"),
        WidgetSpec::Float { .. }
    ));
    match widget(&catalog, "KSampler", "sampler_name") {
        WidgetSpec::Combo { choices, .. } => {
            assert!(choices.contains(&json!("euler")), "{:?}", choices)
        }
        other => panic!("sampler_name should be an enum, got {:?}", other),
    }
    match widget(&catalog, "KSampler", "scheduler") {
        WidgetSpec::Combo { choices, .. } => {
            assert!(choices.contains(&json!("karras")), "{:?}", choices)
        }
        other => panic!("scheduler should be an enum, got {:?}", other),
    }
    assert_eq!(
        widget(&catalog, "KSampler", "model"),
        WidgetSpec::Link {
            data_type: "MODEL".into()
        }
    );

    // A prompt box is multiline by ComfyUI's say-so, not by a length guess.
    assert!(matches!(
        widget(&catalog, "CLIPTextEncode", "text"),
        WidgetSpec::Text {
            multiline: true,
            ..
        }
    ));

    // A saver is an output node, and its prefix is the text Phos pins.
    let save = catalog.get("SaveImage").unwrap();
    assert!(save.output_node);
    assert!(matches!(
        widget(&catalog, "SaveImage", "filename_prefix"),
        WidgetSpec::Text { .. }
    ));

    // An enum of model files is an enum even when no model is installed —
    // which on the test image is the case. An empty list must not turn into
    // a socket, and it has no default to invent.
    match widget(&catalog, "CheckpointLoaderSimple", "ckpt_name") {
        WidgetSpec::Combo {
            choices, default, ..
        } => assert_eq!(
            default.is_none(),
            choices.is_empty(),
            "an empty enum has no default; a non-empty one does: {:?} / {:?}",
            choices,
            default
        ),
        other => panic!("ckpt_name should be an enum, got {:?}", other),
    }
    assert!(matches!(
        widget(&catalog, "LoadImage", "image"),
        WidgetSpec::Combo { .. }
    ));

    // A resolution: bounded, and stepping by 8 as latents require.
    match widget(&catalog, "EmptyLatentImage", "width") {
        WidgetSpec::Int {
            min: Some(16),
            step: Some(8),
            max: Some(max),
            ..
        } => assert!(max >= 8192, "max {}", max),
        other => panic!("EmptyLatentImage.width: {:?}", other),
    }
    assert!(matches!(
        widget(&catalog, "EmptyLatentImage", "batch_size"),
        WidgetSpec::Int { .. }
    ));
}

/// What an import stores, computed against the live catalogue: real kinds
/// for what the server knows, the shot's own input left alone, the pinned
/// prefix and every link kept out.
fn an_import_against_the_live_catalogue_gets_typed_inputs(url: &str) {
    let catalog = live_catalog(url);
    let inputs = detect_inputs(&scale_and_prompt_workflow(), Some(&catalog));
    let find = |key: &str| {
        inputs
            .iter()
            .find(|i| format!("{}.{}", i.node_id, i.field_name) == key)
            .unwrap_or_else(|| {
                panic!(
                    "{} missing from {:?}",
                    key,
                    inputs
                        .iter()
                        .map(|i| format!("{}.{}", i.node_id, i.field_name))
                        .collect::<Vec<_>>()
                )
            })
    };
    let absent = |key: &str| {
        assert!(
            !inputs
                .iter()
                .any(|i| format!("{}.{}", i.node_id, i.field_name) == key),
            "{} must not be offered as a control",
            key
        )
    };

    let method = find("4.upscale_method");
    assert_eq!(method.current_value, json!("nearest-exact"));
    assert_eq!(method.node_title.as_deref(), Some("Resize"));
    match &method.widget {
        Some(WidgetSpec::Combo { choices, .. }) => {
            assert!(choices.contains(&json!("nearest-exact")), "{:?}", choices);
            assert!(choices.contains(&json!("lanczos")), "{:?}", choices);
        }
        other => panic!("upscale_method should be an enum, got {:?}", other),
    }
    assert!(matches!(
        find("4.width").widget,
        Some(WidgetSpec::Int { .. })
    ));
    assert!(matches!(
        find("4.crop").widget,
        Some(WidgetSpec::Combo { .. })
    ));

    let text = find("6.text");
    assert_eq!(text.node_title.as_deref(), Some("Positive Prompt"));
    assert!(matches!(
        text.widget,
        Some(WidgetSpec::Text {
            multiline: true,
            ..
        })
    ));

    // The shot is the image; the console never asks for one, so it stays the
    // untyped field it has always been.
    assert_eq!(find("1.image").widget, None);
    absent("3.filename_prefix");
    absent("4.image");
    absent("3.images");
    absent("6.clip");

    // The record goes into SQLite as JSON and comes back for every workflow
    // list; it must survive the trip whole.
    let stored = serde_json::to_string(&inputs).unwrap();
    let back: Vec<phos_backend::comfyui::WorkflowInput> = serde_json::from_str(&stored).unwrap();
    assert_eq!(back, inputs);
}

/// A picked enum value travels the same road as a typed prompt: through the
/// stored overrides, `prepare_workflow`, and ComfyUI's own validation. The
/// proof that it *arrived* is that ComfyUI rejects one it does not list.
fn a_combo_override_reaches_comfyui(url: &str) {
    let lib = Library::new();
    let wf = lib.add_workflow("scale", &scale_and_prompt_workflow());

    // A value ComfyUI lists: the run completes, at the size the graph asked for.
    let task_id = lib.queue_task_with(&wf, &json!({ "4.upscale_method": "lanczos" }));
    let task = lib.run_until_done(url, &task_id, STEP_BUDGET);
    assert_eq!(task.status, "completed", "{:?}", task);
    let (path, _) = lib.output_file(&task);
    assert_eq!(image::open(&path).unwrap().to_rgb8().dimensions(), (32, 32));

    // A value it does not: rejected at validation, naming the field, and not
    // retried — so the override demonstrably reached the server.
    let task_id = lib.queue_task_with(&wf, &json!({ "4.upscale_method": "no-such-method" }));
    let task = lib.run_until_done(url, &task_id, STEP_BUDGET);
    assert_eq!(task.status, "failed", "{:?}", task);
    let message = task.error_message.clone().unwrap_or_default();
    assert!(
        message.contains("ComfyUI rejected the prompt"),
        "an enum value ComfyUI does not list is a validation failure, got: {}",
        message
    );
    assert!(
        message.contains("upscale_method"),
        "the message should name the field, got: {}",
        message
    );
    assert_eq!(task.retry_count, 0);
}

// ===== The test ==============================================================

/// One scenario, given the server's URL. Panics to fail.
type Step = fn(&str);

#[test]
fn comfyui_contract() {
    let Some(server) = start_server() else {
        return;
    };
    let url = server.url().to_string();

    // A server that came up complaining will fail prompts in ways that look
    // like our bug. Check before sending it anything.
    let console = server.console();
    let errors = console_errors(&console);
    assert!(
        errors.is_empty(),
        "ComfyUI started with errors in its console:\n{}",
        errors.join("\n")
    );

    let steps: &[(&str, Step)] = &[
        (
            "an image workflow round-trips into the library",
            an_image_workflow_round_trips_into_the_library,
        ),
        (
            "a video workflow round-trips into the library",
            a_video_workflow_round_trips_into_the_library,
        ),
        (
            "a rejected prompt fails at once with ComfyUI's reason",
            a_rejected_prompt_fails_at_once_with_comfyuis_reason,
        ),
        (
            "a failing node reports its id and message without retrying",
            a_failing_node_reports_its_id_and_message_without_retrying,
        ),
        (
            "a lost image prompt is recovered by name",
            a_lost_image_prompt_is_recovered_by_name,
        ),
        (
            "a lost video prompt is recovered by name",
            a_lost_video_prompt_is_recovered_by_name,
        ),
        (
            "nothing in the live /object_info is skipped silently",
            nothing_in_the_live_object_info_is_skipped_silently,
        ),
        (
            "core nodes are read the way the fixtures assume",
            core_nodes_are_read_the_way_the_fixtures_assume,
        ),
        (
            "an import against the live catalogue gets typed inputs",
            an_import_against_the_live_catalogue_gets_typed_inputs,
        ),
        (
            "a combo override reaches ComfyUI",
            a_combo_override_reaches_comfyui,
        ),
    ];

    let mut failures = Vec::new();
    for (name, step) in steps {
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(|| step(&url));
        let elapsed = started.elapsed();
        match outcome {
            Ok(()) if elapsed <= STEP_BUDGET => {
                eprintln!("  ok    {:.1}s  {}", elapsed.as_secs_f32(), name)
            }
            Ok(()) => {
                eprintln!("  SLOW  {:.1}s  {}", elapsed.as_secs_f32(), name);
                failures.push(format!(
                    "{}: passed but took {:.1}s, budget is {:?}",
                    name,
                    elapsed.as_secs_f32(),
                    STEP_BUDGET
                ));
            }
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "non-string panic".to_string());
                eprintln!("  FAIL  {:.1}s  {}", elapsed.as_secs_f32(), name);
                failures.push(format!("{}: {}", name, msg));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} steps failed:\n\n{}",
        failures.len(),
        steps.len(),
        failures.join("\n\n")
    );
    drop(server);
}
