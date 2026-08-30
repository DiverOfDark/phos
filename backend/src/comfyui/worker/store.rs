//! Bringing a finished file into the library.
//!
//! Downloads what ComfyUI named and saves it beside the shot's original as a
//! non-original variant. Re-running the same task is safe: an identical file
//! already in the library is recognised by hash and reused, and a *different*
//! file under the expected name gets its own suffixed one rather than
//! overwriting something the user may already have looked at.
//!
//! The row is written *before* the bytes. `files.path` is `UNIQUE`, so the
//! insert is how a name is claimed, and a collision is an answer rather than a
//! failed task — see [`store_output_file`].
//!
//! Every row this module writes is marked `synthetic` and carries a
//! [`ProvenanceManifest`]. That is the one place in Phos where a machine-made
//! picture enters a library of real ones, so it is the one place that has to
//! say so — the face pipeline reads the flag, and a person ten years from now
//! reads the manifest.

use super::complete::ActiveTask;
use super::status::live_task;
use crate::comfyui::client::ComfyUiClient;
use crate::comfyui::manifest::{ComfyuiRun, ProvenanceManifest};
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

/// How many names to try before giving up on finding a free one.
const MAX_NAMING_ATTEMPTS: usize = 8;

/// Write the bytes next to the shot's original and register them as a file row.
/// Returns the id of the file the task should point at.
///
/// # The row is reserved before the bytes are written
///
/// `files.path` is `UNIQUE`, and the obvious order — look for a free name,
/// write the file, insert the row — has a window between the look and the
/// insert that the write itself opens: once the bytes are on disk, a scan or
/// the file watcher can walk past them and index the path first. The insert
/// then fails with `UNIQUE constraint failed: files.path` and the task is
/// reported as failed for a reason that has nothing to do with the workflow.
///
/// So the insert goes first and *is* the check: a name is claimed by taking it,
/// and a collision is an answer rather than an error. That also closes the
/// larger hole, because the reserved row already says `synthetic` — by the time
/// the bytes exist for anything to find, the library already knows a machine
/// made them, and no scanner can index the path as a photograph.
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
    let mut target = parent.join(format!("{}_enhanced_{}.{}", stem, task_short, ext));

    for _ in 0..MAX_NAMING_ATTEMPTS {
        let path_str = db::make_relative(library_root, &target);
        let file_id = Uuid::new_v4().to_string();

        let reserved = diesel::insert_into(files::table)
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
            .execute(conn);

        match reserved {
            Ok(_) => {
                // The name is ours, and the row already says a machine made it.
                // Only now do the bytes appear.
                if let Err(e) = std::fs::write(&target, data) {
                    // A row pointing at a file that does not exist is worse than
                    // no row: it shows up in the library as a broken variant and
                    // holds the name against the retry. Give the name back.
                    let _ =
                        diesel::delete(files::table.filter(files::id.eq(&file_id))).execute(conn);
                    return Err(anyhow::anyhow!("Failed to write {:?}: {}", target, e));
                }
                info!("Saved enhanced output to {:?}", target);
                return Ok(file_id);
            }
            Err(e) if is_unique_violation(&e) => {
                // Someone holds this name: an earlier attempt at this same task,
                // or a scan that walked past the bytes before the row landed.
                let (holder_id, holder_hash): (String, String) = files::table
                    .filter(files::path.eq(&path_str))
                    .select((files::id, files::hash))
                    .first(conn)?;

                if holder_hash == hash {
                    // The same bytes, so it is this run's output whoever put it
                    // there. Claim it rather than making a second copy.
                    info!(
                        "Task {} output is already in the library as {}",
                        task.id, holder_id
                    );
                    mark_synthetic(conn, &holder_id, task, &manifest_json)?;
                    // A reservation whose write never happened is finished here.
                    ensure_bytes_on_disk(&target, data)?;
                    return Ok(holder_id);
                }

                // Different content under that name — take another one rather
                // than overwrite something the user may already have looked at.
                let unique = &Uuid::new_v4().to_string()[..8];
                target = parent.join(format!(
                    "{}_enhanced_{}_{}.{}",
                    stem, task_short, unique, ext
                ));
            }
            Err(e) => return Err(e.into()),
        }
    }

    Err(anyhow::anyhow!(
        "Could not find a free name for task {} output after {} attempts",
        task.id,
        MAX_NAMING_ATTEMPTS
    ))
}

/// Whether a database error is the `files.path` uniqueness constraint saying
/// somebody else got there first.
fn is_unique_violation(e: &diesel::result::Error) -> bool {
    matches!(
        e,
        diesel::result::Error::DatabaseError(diesel::result::DatabaseErrorKind::UniqueViolation, _)
    )
}

/// Put the bytes where the row says they are, if they are not there already.
///
/// This is the other half of reserving the row first: a crash between the
/// insert and the write leaves a row pointing at nothing. The next attempt at
/// the same task recognises its own reservation by hash and finishes it, so the
/// two orderings converge instead of one of them stranding a file.
fn ensure_bytes_on_disk(target: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Ok(meta) = std::fs::metadata(target) {
        if meta.len() == data.len() as u64 {
            return Ok(());
        }
    }
    std::fs::write(target, data)?;
    info!(
        "Wrote the bytes a reserved row was still missing: {:?}",
        target
    );
    Ok(())
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

    ProvenanceManifest::for_comfyui_run(&ComfyuiRun {
        task_id: &task.id,
        workflow_id: &task.workflow_id,
        text_overrides: &task.text_overrides,
        // The values this take ran with. Four takes of one prompt differ here
        // and nowhere else; without them their manifests are the same record.
        parameters: &task.parameters,
        comfyui_prompt_id: Some(&task.prompt_id),
        source_file_id: source_file_id.as_deref(),
        output_filename: Some(&out.filename),
        generated_at: &format_ts(chrono::Utc::now().naive_utc()),
    })
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
            parameters: "{}".to_string(),
            status: "downloading".to_string(),
            output_prefix: Some("phos/task-1234".to_string()),
            settle_until: None,
            retry_count: 0,
            produces_text: false,
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

    /// The partial-write race: the watcher indexed the path this task is about
    /// to claim, but with *different* bytes — a half-flushed file caught
    /// mid-write. The run still has to land.
    #[test]
    fn a_path_taken_by_different_bytes_does_not_sink_the_task() {
        let (_dir, root, mut conn) = library_with_a_finished_task();

        // What the watcher caught: a truncated file, indexed as a photograph.
        let partial = b"gener";
        let mut hasher = Sha256::new();
        hasher.update(partial);
        let partial_hash = hex::encode(hasher.finalize());
        std::fs::write(root.join("holiday_enhanced_task-123.png"), partial).unwrap();
        diesel::insert_into(files::table)
            .values(NewFile {
                id: "file-partial",
                shot_id: "shot-1",
                path: "holiday_enhanced_task-123.png",
                hash: &partial_hash,
                mime_type: Some("image/png"),
                file_size: Some(partial.len() as i32),
                is_original: Some(false),
                visual_embedding: None,
                source_workflow_id: None,
                source_text_overrides: None,
                synthetic: None,
                manifest_json: None,
            })
            .execute(&mut conn)
            .unwrap();

        let result = store_output_file(&mut conn, &a_task(), &an_output(), b"generated", &root);

        let file_id = result.expect("a taken path must not fail the run");
        assert_ne!(
            "file-partial", file_id,
            "the truncated file is not the output"
        );
        let (path, synthetic): (String, bool) = files::table
            .filter(files::id.eq(&file_id))
            .select((files::path, files::synthetic))
            .first(&mut conn)
            .unwrap();
        assert!(synthetic);
        assert_eq!(
            b"generated".to_vec(),
            std::fs::read(root.join(&path)).unwrap(),
            "the bytes on disk are the ones the run produced",
        );
    }

    /// The race the reservation exists for: between deciding a name was free
    /// and inserting the row, the bytes are on disk and a scan can index the
    /// path first. Taking the name by inserting makes that a collision to
    /// resolve rather than `UNIQUE constraint failed: files.path` reported as a
    /// failed workflow.
    #[test]
    fn a_scan_that_indexes_the_output_first_does_not_fail_the_task() {
        let (_dir, root, mut conn) = library_with_a_finished_task();
        let data = b"generated";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hex::encode(hasher.finalize());

        // What a concurrent scan leaves behind: the same bytes, at the name
        // this task is about to take, indexed as an ordinary photograph.
        std::fs::write(root.join("holiday_enhanced_task-123.png"), data).unwrap();
        diesel::insert_into(files::table)
            .values(NewFile {
                id: "file-from-scan",
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

        let file_id = store_output_file(&mut conn, &a_task(), &an_output(), data, &root)
            .expect("a name taken by a scan must not fail the run");

        assert_eq!("file-from-scan", file_id, "the same bytes, so the same row");
        let synthetic: bool = files::table
            .filter(files::id.eq("file-from-scan"))
            .select(files::synthetic)
            .first(&mut conn)
            .unwrap();
        assert!(synthetic, "and the row the scan made is corrected");
        assert_eq!(
            2i64,
            files::table.count().get_result::<i64>(&mut conn).unwrap(),
            "no second copy",
        );
    }

    /// The window itself, which only real concurrency can show: a row landing
    /// at the chosen name *between* deciding it was free and inserting it.
    ///
    /// Two writers, one name. Checking first and inserting later leaves a gap
    /// wide enough for the loser to be handed `UNIQUE constraint failed:
    /// files.path`, which the caller reports as a failed workflow. Taking the
    /// name by inserting has no gap: whichever loses discovers it lost from the
    /// constraint and resolves it. No interleaving may produce an error.
    #[test]
    fn two_writers_racing_for_the_same_name_both_land() {
        let (_dir, root, _conn) = library_with_a_finished_task();
        let db_path = root.join(".phos.db");
        let barrier = std::sync::Barrier::new(2);

        let results: Vec<anyhow::Result<String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let db_path = db_path.clone();
                    let root = root.clone();
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
                        barrier.wait();
                        store_output_file(&mut conn, &a_task(), &an_output(), b"generated", &root)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for r in &results {
            assert!(
                r.is_ok(),
                "a lost race is a name to resolve, not a failed run: {:?}",
                r.as_ref().err()
            );
        }
        assert_eq!(
            results[0].as_ref().unwrap(),
            results[1].as_ref().unwrap(),
            "the same bytes must converge on one row",
        );

        let mut conn = crate::db::open_diesel_connection(&db_path).unwrap();
        assert_eq!(
            2i64,
            files::table.count().get_result::<i64>(&mut conn).unwrap(),
            "the original and exactly one output",
        );
    }

    /// The reservation is a promise about a name, so a write that never lands
    /// has to give the name back — a row pointing at a file that does not exist
    /// would show as a broken variant and block the retry.
    #[test]
    fn a_write_that_fails_leaves_no_row_behind() {
        let (_dir, root, mut conn) = library_with_a_finished_task();

        // A directory where the output file wants to be: the write fails, the
        // insert before it does not.
        std::fs::create_dir(root.join("holiday_enhanced_task-123.png")).unwrap();

        let result = store_output_file(&mut conn, &a_task(), &an_output(), b"generated", &root);

        assert!(result.is_err(), "the failure is reported, not swallowed");
        assert_eq!(
            1i64,
            files::table.count().get_result::<i64>(&mut conn).unwrap(),
            "only the original — the reservation was given back",
        );
    }

    /// A reservation whose write never happened is finished by the next
    /// attempt, rather than leaving a row pointing at nothing forever.
    #[test]
    fn a_retry_finishes_a_reservation_whose_bytes_never_landed() {
        let (_dir, root, mut conn) = library_with_a_finished_task();
        let data = b"generated";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hex::encode(hasher.finalize());

        // The row a crash between the insert and the write would leave.
        diesel::insert_into(files::table)
            .values(NewFile {
                id: "file-reserved",
                shot_id: "shot-1",
                path: "holiday_enhanced_task-123.png",
                hash: &hash,
                mime_type: Some("image/png"),
                file_size: Some(data.len() as i32),
                is_original: Some(false),
                visual_embedding: None,
                source_workflow_id: Some("wf-portrait"),
                source_text_overrides: Some("{}"),
                synthetic: Some(true),
                manifest_json: Some("{}"),
            })
            .execute(&mut conn)
            .unwrap();
        assert!(!root.join("holiday_enhanced_task-123.png").exists());

        let file_id = store_output_file(&mut conn, &a_task(), &an_output(), data, &root).unwrap();

        assert_eq!("file-reserved", file_id);
        assert_eq!(
            data.to_vec(),
            std::fs::read(root.join("holiday_enhanced_task-123.png")).unwrap(),
            "the bytes the row was already promising are there now",
        );
    }

    /// A second attempt at the same task converges on one file and one row.
    #[test]
    fn a_second_attempt_at_the_same_task_converges() {
        let (_dir, root, mut conn) = library_with_a_finished_task();

        let first =
            store_output_file(&mut conn, &a_task(), &an_output(), b"generated", &root).unwrap();
        let second =
            store_output_file(&mut conn, &a_task(), &an_output(), b"generated", &root).unwrap();

        assert_eq!(first, second, "the same run must not make a second file");
        assert_eq!(
            2i64,
            files::table.count().get_result::<i64>(&mut conn).unwrap(),
            "the original and one output",
        );
    }

    /// The record a fan-out has to leave behind.
    ///
    /// Four takes of one prompt land as four files that look alike. Without the
    /// seed on each one's manifest, "make another like *that* one" has nothing
    /// to point at — and a manifest that cannot reproduce its file is not doing
    /// the job the manifest exists for.
    #[test]
    fn four_takes_of_one_prompt_each_carry_the_seed_that_made_them() {
        let (_dir, root, mut conn) = library_with_a_finished_task();

        // Exactly what the enhance endpoint would queue for
        // `vary: {"3.seed": {"count": 4, "mode": "increment"}}`.
        let base: crate::comfyui::ParameterMap = [("3.seed".to_string(), serde_json::json!(1000))]
            .into_iter()
            .collect();
        let vary: crate::comfyui::VaryMap = serde_json::from_value(
            serde_json::json!({ "3.seed": { "count": 4, "mode": "increment" } }),
        )
        .unwrap();
        let runs = crate::comfyui::expand(&base, &vary).unwrap();
        assert_eq!(runs.len(), 4);

        let mut manifests = Vec::new();
        for (i, run) in runs.iter().enumerate() {
            let task_id = format!("fanout-{}", i);
            let parameters = serde_json::to_string(run).unwrap();
            conn.batch_execute(&format!(
                "INSERT INTO enhancement_tasks
                   (id, shot_id, workflow_id, status, source_file_id, parameters)
                 VALUES ('{}', 'shot-1', 'wf-portrait', 'downloading', 'file-original', '{}');",
                task_id, parameters
            ))
            .unwrap();

            let task = ActiveTask {
                id: task_id.clone(),
                parameters,
                ..a_task()
            };
            let file_id = store_output_file(
                &mut conn,
                &task,
                &an_output(),
                format!("take {}", i).as_bytes(),
                &root,
            )
            .unwrap();
            manifests.push(stored_manifest(&mut conn, &file_id));
        }

        // Every take names the seed it ran with, and they are four seeds.
        let seeds: Vec<Option<i64>> = manifests.iter().map(|m| m.seed).collect();
        assert_eq!(seeds, [Some(1000), Some(1001), Some(1002), Some(1003)]);

        // And the seed on the manifest is the seed on the row, not one the
        // manifest guessed — this is the claim that makes a take reproducible.
        for (i, manifest) in manifests.iter().enumerate() {
            let stored: Option<String> = enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(format!("fanout-{}", i)))
                .select(enhancement_tasks::parameters)
                .first(&mut conn)
                .unwrap();
            let row: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&stored.expect("the task kept its parameters")).unwrap();
            assert_eq!(manifest.parameters, row);
            assert_eq!(manifest.seed, row["3.seed"].as_i64());
        }

        // Four records, not one record four times.
        let records: std::collections::HashSet<String> = manifests
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect();
        assert_eq!(records.len(), 4, "two takes wrote the same manifest");
    }

    /// A run with nothing typed — every task queued before FR4 — still gets a
    /// manifest, just one with no seed to name.
    #[test]
    fn a_run_that_set_no_parameters_still_carries_a_manifest() {
        let (_dir, root, mut conn) = library_with_a_finished_task();
        let file_id =
            store_output_file(&mut conn, &a_task(), &an_output(), b"generated", &root).unwrap();

        let m = stored_manifest(&mut conn, &file_id);
        assert!(m.parameters.is_empty());
        assert_eq!(m.seed, None);
        assert_eq!("task-1234", m.task_id);
    }
}
