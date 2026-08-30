//! Bringing a finished file into the library.
//!
//! Downloads what ComfyUI named and saves it beside the shot's original as a
//! non-original variant. Re-running the same task is safe: an identical file
//! already on disk is recognised by hash and reused, and a *different* file at
//! the expected path gets its own suffixed name rather than overwriting
//! something the user may already have looked at.
//!
//! Every row this module writes is marked `synthetic` and carries a
//! [`ProvenanceManifest`]. That is the one place in Phos where a machine-made
//! picture enters a library of real ones, so it is the one place that has to
//! say so — the face pipeline reads the flag, and a person ten years from now
//! reads the manifest.

use super::complete::ActiveTask;
use super::status::live_task;
use crate::comfyui::client::ComfyUiClient;
use crate::comfyui::manifest::ProvenanceManifest;
use crate::comfyui::outputs::OutputRef;
use crate::comfyui::timestamp::format_ts;
use crate::db;
use crate::models::{NewFile, SyntheticProvenance};
use crate::schema::{enhancement_tasks, files};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing::{info, warn};
use uuid::Uuid;

/// Download an output file from ComfyUI and save it alongside the original.
pub(super) fn download_and_save_output(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    task: &ActiveTask,
    out: &OutputRef,
    library_root: &Path,
) -> anyhow::Result<()> {
    let data = client.download_output(out)?;
    let file_id = store_output_file(conn, task, out, &data, library_root)?;

    // Store the output file ID on the task
    diesel::update(live_task(&task.id))
        .set(enhancement_tasks::output_file_id.eq(&file_id))
        .execute(conn)?;

    Ok(())
}

/// Write the bytes next to the shot's original and register them as a file row.
/// Returns the id of the file the task should point at.
fn store_output_file(
    conn: &mut SqliteConnection,
    task: &ActiveTask,
    out: &OutputRef,
    data: &[u8],
    library_root: &Path,
) -> anyhow::Result<String> {
    let shot_id = task.shot_id.as_str();

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
    let ext = Path::new(&out.filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png");

    // Compute hash before writing to disk so we can check for duplicates
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hex::encode(hasher.finalize());

    let manifest = build_manifest(conn, task, out);
    let manifest_json = serde_json::to_string(&manifest)
        .map_err(|e| anyhow::anyhow!("Failed to serialize provenance manifest: {e}"))?;

    let task_short = &task.id[..8.min(task.id.len())];
    let base_output_path = parent.join(format!("{}_enhanced_{}.{}", stem, task_short, ext));
    let base_path_str = db::make_relative(library_root, &base_output_path);

    // Check if a file with the expected path already exists in the DB (from a previous attempt)
    let existing: Option<(String, String)> = files::table
        .filter(files::path.eq(&base_path_str))
        .select((files::id, files::hash))
        .first::<(String, String)>(conn)
        .ok();

    if let Some((existing_id, existing_hash)) = &existing {
        if *existing_hash == hash {
            // Same content already saved — nothing to write. The row may be an
            // earlier attempt at this task, or one the file watcher indexed in
            // the moment between the bytes landing and this insert; either way
            // it is this run's output and has to say so.
            info!(
                "Task {} output already exists with same hash, skipping write",
                task.id
            );
            mark_synthetic(conn, existing_id, task, &manifest_json)?;
            return Ok(existing_id.clone());
        }
    }

    let path_taken = existing.is_some();
    let target = if path_taken {
        // Path is taken but content differs — save as a new variant with a unique suffix
        let unique = &Uuid::new_v4().to_string()[..8];
        parent.join(format!(
            "{}_enhanced_{}_{}.{}",
            stem, task_short, unique, ext
        ))
    } else {
        base_output_path
    };

    std::fs::write(&target, data)?;
    if path_taken {
        info!("Saved enhanced output (new variant) to {:?}", target);
    } else {
        info!("Saved enhanced output to {:?}", target);
    }

    let path_str = db::make_relative(library_root, &target);
    let file_id = Uuid::new_v4().to_string();
    diesel::insert_into(files::table)
        .values(NewFile {
            id: &file_id,
            shot_id,
            path: &path_str,
            hash: &hash,
            mime_type: Some(mime_for(ext)),
            file_size: Some(data.len() as i32),
            is_original: Some(false),
            visual_embedding: None,
            source_workflow_id: Some(&task.workflow_id),
            source_text_overrides: Some(&task.text_overrides),
            synthetic: Some(true),
            manifest_json: Some(&manifest_json),
        })
        .execute(conn)?;
    Ok(file_id)
}

/// The record that travels with the file: what ran, what it was given, and what
/// it produced.
fn build_manifest(
    conn: &mut SqliteConnection,
    task: &ActiveTask,
    out: &OutputRef,
) -> ProvenanceManifest {
    // The source file is on the task row, not on `ActiveTask` — reading it here
    // keeps the polling query the shape the completion path wants.
    let source_file_id: Option<String> = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq(&task.id))
        .select(enhancement_tasks::source_file_id)
        .first::<Option<String>>(conn)
        .ok()
        .flatten();

    ProvenanceManifest::for_comfyui_run(
        &task.id,
        &task.workflow_id,
        &task.text_overrides,
        Some(&task.prompt_id),
        source_file_id.as_deref(),
        Some(&out.filename),
        format_ts(chrono::Utc::now().naive_utc()),
    )
}

/// Say, on a row that already exists, that a machine made it.
///
/// Also drops any faces already detected on it. A file the watcher indexed
/// before this row was claimed went through face detection as if it were a
/// photograph, and those boxes are exactly what must never reach clustering.
fn mark_synthetic(
    conn: &mut SqliteConnection,
    file_id: &str,
    task: &ActiveTask,
    manifest_json: &str,
) -> anyhow::Result<()> {
    diesel::update(files::table.filter(files::id.eq(file_id)))
        .set(SyntheticProvenance {
            synthetic: true,
            manifest_json: Some(manifest_json),
            source_workflow_id: Some(&task.workflow_id),
            source_text_overrides: Some(&task.text_overrides),
        })
        .execute(conn)?;

    match crate::scanner::purge_faces_on_synthetic_files(conn) {
        Ok(0) => {}
        Ok(n) => warn!(
            "Task {}: dropped {} face(s) detected on generated files before they could be clustered",
            task.id, n
        ),
        Err(e) => warn!("Task {}: failed to purge synthetic faces: {}", task.id, e),
    }
    Ok(())
}

/// Guess mime type from extension.
fn mime_for(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NewFace, NewFile, NewShot};
    use crate::schema::{faces, shots};
    use diesel::connection::SimpleConnection;

    #[test]
    fn extensions_map_to_the_mime_types_the_library_expects() {
        assert_eq!(mime_for("png"), "image/png");
        assert_eq!(mime_for("jpeg"), "image/jpeg");
        assert_eq!(mime_for("mp4"), "video/mp4");
        assert_eq!(mime_for("exr"), "application/octet-stream");
    }

    /// A library with one shot, its original on disk, and a finished task ready
    /// to have its output stored.
    fn library_with_a_finished_task() -> (tempfile::TempDir, std::path::PathBuf, SqliteConnection) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let db_path = root.join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();

        std::fs::write(root.join("holiday.jpg"), b"the photograph").unwrap();
        diesel::insert_into(shots::table)
            .values(NewShot {
                id: "shot-1",
                main_file_id: Some("file-original"),
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
                id: "file-original",
                shot_id: "shot-1",
                path: "holiday.jpg",
                hash: "originalhash",
                mime_type: Some("image/jpeg"),
                file_size: Some(14),
                is_original: Some(true),
                visual_embedding: None,
                source_workflow_id: None,
                source_text_overrides: None,
                synthetic: None,
                manifest_json: None,
            })
            .execute(&mut conn)
            .unwrap();
        conn.batch_execute(
            "INSERT INTO comfyui_workflows (id, name, workflow_json)
             VALUES ('wf-portrait', 'Portrait', '{}');
             INSERT INTO enhancement_tasks (id, shot_id, workflow_id, status, source_file_id)
             VALUES ('task-1234', 'shot-1', 'wf-portrait', 'downloading', 'file-original');",
        )
        .unwrap();

        (dir, root, conn)
    }

    fn a_task() -> ActiveTask {
        ActiveTask {
            id: "task-1234".to_string(),
            shot_id: "shot-1".to_string(),
            prompt_id: "prompt-abcd".to_string(),
            workflow_id: "wf-portrait".to_string(),
            workflow_json: "{}".to_string(),
            text_overrides: r#"{"6":"a lighthouse at dusk"}"#.to_string(),
            status: "downloading".to_string(),
            output_prefix: Some("phos/task-1234".to_string()),
            settle_until: None,
            retry_count: 0,
        }
    }

    fn an_output() -> OutputRef {
        OutputRef {
            filename: "task-1234_00001.png".to_string(),
            subfolder: "phos".to_string(),
            output_type: "output".to_string(),
        }
    }

    fn stored_manifest(conn: &mut SqliteConnection, file_id: &str) -> ProvenanceManifest {
        let raw: String = files::table
            .filter(files::id.eq(file_id))
            .select(files::manifest_json.assume_not_null())
            .first(conn)
            .expect("a generated file must carry a manifest");
        serde_json::from_str(&raw).expect("and it must parse")
    }

    /// The one place a machine-made picture enters a library of real ones says
    /// so, on the row, at the moment it arrives.
    #[test]
    fn a_file_the_worker_brings_in_says_a_machine_made_it() {
        let (_dir, root, mut conn) = library_with_a_finished_task();

        let file_id =
            store_output_file(&mut conn, &a_task(), &an_output(), b"generated", &root).unwrap();

        let synthetic: bool = files::table
            .filter(files::id.eq(&file_id))
            .select(files::synthetic)
            .first(&mut conn)
            .unwrap();
        assert!(synthetic);

        let m = stored_manifest(&mut conn, &file_id);
        assert_eq!("comfyui", m.generator);
        assert_eq!("task-1234", m.task_id);
        assert_eq!("wf-portrait", m.workflow_id);
        assert_eq!(Some("prompt-abcd".to_string()), m.comfyui_prompt_id);
        assert_eq!(Some("file-original".to_string()), m.source_file_id);
        assert_eq!(Some("task-1234_00001.png".to_string()), m.output_filename);
        assert_eq!(
            Some(&serde_json::json!("a lighthouse at dusk")),
            m.text_overrides.get("6"),
        );
    }

    /// The race the flag exists to survive: the watcher indexed the bytes —
    /// faces and all — before the worker claimed the row. Claiming it says what
    /// the file is, and takes the faces back out.
    #[test]
    fn a_row_the_watcher_got_to_first_is_claimed_and_its_faces_dropped() {
        let (_dir, root, mut conn) = library_with_a_finished_task();
        let data = b"generated";

        // What the watcher would have written: the same bytes, at the path this
        // task is about to claim, indexed as an ordinary photograph.
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hex::encode(hasher.finalize());
        diesel::insert_into(files::table)
            .values(NewFile {
                id: "file-raced",
                shot_id: "shot-1",
                path: "holiday_enhanced_task-123.png",
                hash: &hash,
                mime_type: Some("image/png"),
                file_size: Some(data.len() as i32),
                is_original: Some(false),
                visual_embedding: None,
                source_workflow_id: None,
                source_text_overrides: None,
                synthetic: None,
                manifest_json: None,
            })
            .execute(&mut conn)
            .unwrap();
        diesel::insert_into(faces::table)
            .values(NewFace {
                id: "face-that-never-was",
                file_id: "file-raced",
                person_id: None,
                box_x1: Some(0.0),
                box_y1: Some(0.0),
                box_x2: Some(100.0),
                box_y2: Some(100.0),
                embedding: Some(&crate::embedding::encode_embedding(&vec![0.5f32; 512])),
                score: Some(0.99),
            })
            .execute(&mut conn)
            .unwrap();

        let file_id = store_output_file(&mut conn, &a_task(), &an_output(), data, &root).unwrap();

        assert_eq!("file-raced", file_id, "the same bytes, so the same row");
        let synthetic: bool = files::table
            .filter(files::id.eq("file-raced"))
            .select(files::synthetic)
            .first(&mut conn)
            .unwrap();
        assert!(synthetic, "claiming the row says what the file is");
        assert_eq!(
            "task-1234",
            stored_manifest(&mut conn, "file-raced").task_id
        );
        assert_eq!(
            0i64,
            faces::table
                .filter(faces::file_id.eq("file-raced"))
                .count()
                .get_result::<i64>(&mut conn)
                .unwrap(),
            "and takes back the face that should never have been detected",
        );
    }
}
