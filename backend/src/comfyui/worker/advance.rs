//! Moving a run along: queue the next stage, then see whether the run is over.
//!
//! This is the whole of "photo → clip → interpolate → upscale runs as one
//! thing". It is deliberately small, because the interesting parts are
//! elsewhere: [`crate::comfyui::line`] decides, [`crate::comfyui::runs`] writes,
//! and what is left here is a pass over the rows in a fixed order.
//!
//! # The order is the design
//!
//! Every tick, in this sequence:
//!
//! 1. **Continue.** Every completed task that is not at the last stage, and
//!    that nothing yet names as a parent, queues the stage after it — reading
//!    the file that task produced. Fan-out needs no special case: four
//!    completed takes each ask for themselves and each get their own
//!    continuation, so four takes at stage 2 are four independent runners
//!    through stages 3 and 4.
//! 2. **Settle.** Fold each live run's tasks into a state. A run is running
//!    while anything is still moving, and then it is however its tasks ended.
//!
//! Doing them the other way round would be a data-loss bug rather than a
//! cosmetic one: a run whose stage-1 task has just completed but whose stage-2
//! task has not yet been written would settle as *completed*, and the sweep
//! below would delete the intermediate the next stage was about to read. So a
//! run with a continuation still owed is never settled in the same pass —
//! [`queue_continuations`] hands back the ones it could not finish, and they
//! wait a tick.
//!
//! # Intermediates
//!
//! A four-stage line makes three intermediates per take. They live exactly as
//! long as they are useful — the next stage reads them, and a failure wants
//! them for inspection — and are swept when the run *completes*. Not when it
//! fails: a failed run is retried from its failed stage, and that resumption
//! reads the intermediate the stage before it made.

use crate::comfyui::line::{self, RunState, StageDisposition, TaskPhase};
use crate::comfyui::runs::{queue_stage, stage_at};
use crate::comfyui::timestamp::format_ts;
use crate::schema::{enhancement_tasks, faces, files, line_stages, runs, video_keyframes};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::{error, info, warn};

/// One pass: continue what can be continued, then settle what is over.
pub(super) fn advance_runs(conn: &mut SqliteConnection, library_root: &Path) {
    let stalled = match queue_continuations(conn) {
        Ok(stalled) => stalled,
        Err(e) => {
            error!("Failed to queue run continuations: {}", e);
            // Settling anything now would risk calling a run finished while its
            // next stage is still owed. Wait for the next tick.
            return;
        }
    };
    if let Err(e) = settle_runs(conn, library_root, &stalled) {
        error!("Failed to settle runs: {}", e);
    }
}

/// A completed task that has not yet handed its output to the stage after it.
struct Continuation {
    task_id: String,
    run_id: String,
    shot_id: String,
    line_id: Option<String>,
    stage_idx: i32,
    stage_count: i32,
    output_file_id: Option<String>,
    /// What the sender answered for the stages that asked, snapshotted on the
    /// run — the later stages are queued here, hours after the request that
    /// carried them.
    stage_values: Option<String>,
}

type ContinuationRow = (
    String,
    String,
    String,
    Option<String>,
    i32,
    i32,
    Option<String>,
    Option<String>,
);

/// Queue the stage after every completed task that is owed one.
///
/// Returns the runs that still owe a continuation after this pass — a line
/// edited out from under a live run, say. They are not settled this tick,
/// because "nothing is in flight" would otherwise read as "finished".
fn queue_continuations(
    conn: &mut SqliteConnection,
) -> Result<HashSet<String>, diesel::result::Error> {
    let rows: Vec<ContinuationRow> = enhancement_tasks::table
        .inner_join(runs::table.on(runs::id.nullable().eq(enhancement_tasks::run_id)))
        .filter(
            enhancement_tasks::status
                .eq("completed")
                .and(runs::status.eq(RunState::Running.as_str()))
                .and(enhancement_tasks::stage_idx.is_not_null()),
        )
        .select((
            enhancement_tasks::id,
            runs::id,
            enhancement_tasks::shot_id,
            runs::line_id,
            enhancement_tasks::stage_idx.assume_not_null(),
            runs::stage_count,
            enhancement_tasks::output_file_id,
            runs::stage_values,
        ))
        .load(conn)?;

    let candidates: Vec<Continuation> = rows
        .into_iter()
        .map(|r| Continuation {
            task_id: r.0,
            run_id: r.1,
            shot_id: r.2,
            line_id: r.3,
            stage_idx: r.4,
            stage_count: r.5,
            output_file_id: r.6,
            stage_values: r.7,
        })
        // The last stage owes nothing: its output is the product.
        .filter(|c| {
            matches!(
                line::advance_after(c.stage_idx, c.stage_count),
                line::Advance::Next(_)
            )
        })
        .collect();

    if candidates.is_empty() {
        return Ok(HashSet::new());
    }

    // A continuation exists exactly when some row names this task as its
    // parent. That is the idempotence marker: no "advanced" flag to write, and
    // no way for the marker and the thing it marks to disagree.
    let run_ids: Vec<&str> = candidates.iter().map(|c| c.run_id.as_str()).collect();
    let already_continued: HashSet<String> = enhancement_tasks::table
        .filter(
            enhancement_tasks::parent_task_id
                .is_not_null()
                .and(enhancement_tasks::run_id.eq_any(&run_ids)),
        )
        .select(enhancement_tasks::parent_task_id.assume_not_null())
        .load::<String>(conn)?
        .into_iter()
        .collect();

    let mut stalled = HashSet::new();
    for c in candidates {
        if already_continued.contains(&c.task_id) {
            continue;
        }
        if let Err(reason) = continue_one(conn, &c) {
            warn!(
                "Run {} cannot continue past stage {}: {}",
                c.run_id, c.stage_idx, reason
            );
            fail_run(conn, &c.run_id, &reason);
            stalled.insert(c.run_id.clone());
        }
    }
    Ok(stalled)
}

/// Queue the stage after one completed task, reading what that task produced.
fn continue_one(conn: &mut SqliteConnection, c: &Continuation) -> Result<(), String> {
    let line::Advance::Next(next_idx) = line::advance_after(c.stage_idx, c.stage_count) else {
        return Ok(());
    };
    let line_id = c
        .line_id
        .as_deref()
        .ok_or_else(|| "the run has more than one stage but no line".to_string())?;

    // `mark_completed` is only ever reached once a file has landed, so this is
    // a can't-happen — but a continuation with no source would silently read
    // the shot's original instead of the clip the stage before it made, which
    // is exactly the kind of wrong that looks right.
    let source_file_id = c
        .output_file_id
        .as_deref()
        .ok_or_else(|| format!("stage {} completed without an output file", c.stage_idx + 1))?;

    let stage = stage_at(conn, line_id, next_idx)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("stage {} is no longer part of the line", next_idx + 1))?;

    let mut plan = stage.plan();
    // The answers this run was started with. A stage that asked for a value at
    // send time gets it here, whichever hour of the run it is queued in.
    let supplied = crate::comfyui::runs::supplied_for(c.stage_values.as_deref(), next_idx);
    plan.accept(&stage.exposed, &supplied)
        .map_err(|e| e.message)?;
    let queued = queue_stage(
        conn,
        &c.run_id,
        &c.shot_id,
        &plan,
        Some(source_file_id),
        Some(&c.task_id),
    )?;
    info!(
        "Run {}: stage {}/{} queued as {} task(s) from {}",
        c.run_id,
        next_idx + 1,
        c.stage_count,
        queued.len(),
        c.task_id
    );
    Ok(())
}

/// Fold every live run's tasks into its state, and act on the ones that landed.
fn settle_runs(
    conn: &mut SqliteConnection,
    library_root: &Path,
    stalled: &HashSet<String>,
) -> Result<(), diesel::result::Error> {
    let live: Vec<(String, Option<String>, i32)> = runs::table
        .filter(runs::status.eq(RunState::Running.as_str()))
        .select((runs::id, runs::line_id, runs::stage_count))
        .load(conn)?;
    if live.is_empty() {
        return Ok(());
    }

    let live_ids: Vec<&str> = live.iter().map(|(id, _, _)| id.as_str()).collect();
    let task_rows: Vec<(String, Option<i32>, String, Option<String>)> = enhancement_tasks::table
        .filter(enhancement_tasks::run_id.eq_any(&live_ids))
        .select((
            enhancement_tasks::run_id.assume_not_null(),
            enhancement_tasks::stage_idx,
            enhancement_tasks::status,
            enhancement_tasks::error_message,
        ))
        .load(conn)?;

    let mut by_run: HashMap<&str, Vec<(i32, TaskPhase)>> = HashMap::new();
    let mut first_error: HashMap<&str, &str> = HashMap::new();
    for (run_id, stage_idx, status, error) in &task_rows {
        let phase = line::phase_of(status);
        by_run
            .entry(run_id.as_str())
            .or_default()
            .push((stage_idx.unwrap_or(0), phase));
        if phase == TaskPhase::Failed {
            if let Some(msg) = error.as_deref() {
                first_error.entry(run_id.as_str()).or_insert(msg);
            }
        }
    }

    for (run_id, line_id, stage_count) in &live {
        if stalled.contains(run_id) {
            continue;
        }
        let tasks = by_run
            .get(run_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if tasks.is_empty() {
            // Every task deleted by hand. Nothing to decide from, and nothing
            // will ever move it, but inventing a verdict is worse than waiting.
            continue;
        }
        let t = line::tally(tasks, *stage_count);
        if !t.state.is_terminal() {
            continue;
        }

        let error = first_error.get(run_id.as_str()).copied();
        finish_run(conn, run_id, t.state, error)?;
        info!(
            "Run {} {} at stage {}/{}",
            run_id,
            t.state.as_str(),
            t.current_stage.min(*stage_count),
            stage_count
        );

        // Only a run that landed sweeps its intermediates. A failed one is
        // about to be retried from the stage that broke, and that resumption
        // reads what the stage before it made.
        if t.state == RunState::Completed {
            discard_intermediates(conn, library_root, run_id, line_id.as_deref(), *stage_count);
        }
    }
    Ok(())
}

/// Write a run's fate onto its row. `runs.status` is what the API answers with,
/// so this is the only place a run's outcome is decided.
fn finish_run(
    conn: &mut SqliteConnection,
    run_id: &str,
    state: RunState,
    error: Option<&str>,
) -> Result<(), diesel::result::Error> {
    let now = format_ts(chrono::Utc::now().naive_utc());
    diesel::update(runs::table.filter(runs::id.eq(run_id)))
        .set((
            runs::status.eq(state.as_str()),
            runs::finished_at.eq(&now),
            runs::error_message.eq(error),
        ))
        .execute(conn)?;
    Ok(())
}

/// A run that cannot go on — its line was edited under it, or a completed stage
/// left no file. Recorded as a failure of the run rather than of a task,
/// because no task did anything wrong.
fn fail_run(conn: &mut SqliteConnection, run_id: &str, reason: &str) {
    let _ = finish_run(conn, run_id, RunState::Failed, Some(reason));
}

/// Sweep the outputs of a completed run's non-final, non-kept stages.
fn discard_intermediates(
    conn: &mut SqliteConnection,
    library_root: &Path,
    run_id: &str,
    line_id: Option<&str>,
    stage_count: i32,
) {
    // What the user asked to keep. An ad-hoc run has no line, and its one stage
    // is final, so this stays empty and nothing is swept.
    let keep_flags: HashMap<i32, bool> = match line_id {
        Some(line_id) => line_stages::table
            .filter(line_stages::line_id.eq(line_id))
            .select((line_stages::stage_idx, line_stages::keep_output))
            .load::<(i32, bool)>(conn)
            .unwrap_or_default()
            .into_iter()
            .collect(),
        None => HashMap::new(),
    };

    let outputs: Vec<(String, Option<i32>, Option<String>)> = enhancement_tasks::table
        .filter(
            enhancement_tasks::run_id
                .eq(run_id)
                .and(enhancement_tasks::output_file_id.is_not_null()),
        )
        .select((
            enhancement_tasks::id,
            enhancement_tasks::stage_idx,
            enhancement_tasks::output_file_id,
        ))
        .load(conn)
        .unwrap_or_default();

    for (task_id, stage_idx, file_id) in outputs {
        let stage_idx = stage_idx.unwrap_or(0);
        let disposition = StageDisposition {
            keep_flag: keep_flags.get(&stage_idx).copied().unwrap_or(false),
            is_final: stage_idx + 1 >= stage_count,
        };
        if line::keeps_output(disposition) {
            continue;
        }
        let Some(file_id) = file_id else { continue };
        match discard_file(conn, library_root, &task_id, &file_id) {
            Ok(path) => info!(
                "Run {}: discarded stage {} intermediate {}",
                run_id,
                stage_idx + 1,
                path
            ),
            Err(e) => warn!(
                "Run {}: could not discard stage {} intermediate: {}",
                run_id,
                stage_idx + 1,
                e
            ),
        }
    }
}

/// Take one intermediate out of the library: its row, its bytes, its thumbnail,
/// and anything hanging off it.
///
/// The task keeps pointing at nothing rather than at a row that is gone, which
/// is what the file-delete endpoint does too — a dangling id would show on the
/// board as an output you can click and not open.
fn discard_file(
    conn: &mut SqliteConnection,
    library_root: &Path,
    task_id: &str,
    file_id: &str,
) -> anyhow::Result<String> {
    let path: String = files::table
        .filter(files::id.eq(file_id))
        .select(files::path)
        .first(conn)?;

    diesel::delete(faces::table.filter(faces::file_id.eq(file_id))).execute(conn)?;
    diesel::delete(video_keyframes::table.filter(video_keyframes::video_file_id.eq(file_id)))
        .execute(conn)?;
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
        .set(enhancement_tasks::output_file_id.eq(None::<String>))
        .execute(conn)?;
    diesel::delete(files::table.filter(files::id.eq(file_id))).execute(conn)?;

    let resolved = crate::db::resolve_path(library_root, &path);
    if let Err(e) = std::fs::remove_file(&resolved) {
        warn!(
            "Discarded row for {:?} but could not remove it: {}",
            resolved, e
        );
    }
    let _ = std::fs::remove_file(
        library_root
            .join(".phos_thumbnails")
            .join(format!("{}.jpg", file_id)),
    );
    Ok(path)
}

/// Put every failed task of a run back in the queue, from where it failed.
///
/// The point of the whole exercise: a stage-4 hiccup re-runs stage 4, not the
/// hour of upscaling before it. Each failed task already holds the
/// `source_file_id` of the intermediate it was given, so resuming is a status
/// change and nothing else — no re-derivation, no re-upload of the original.
///
/// Returns how many tasks were re-queued.
pub(crate) fn retry_run(conn: &mut SqliteConnection, run_id: &str) -> QueryResult<usize> {
    let requeued = diesel::update(
        enhancement_tasks::table.filter(
            enhancement_tasks::run_id
                .eq(run_id)
                .and(enhancement_tasks::status.eq_any(&["failed", "cancelled"])),
        ),
    )
    .set((
        enhancement_tasks::status.eq("pending"),
        enhancement_tasks::error_message.eq(None::<String>),
        enhancement_tasks::retry_count.eq(0),
        enhancement_tasks::settle_until.eq(None::<String>),
        enhancement_tasks::next_attempt_at.eq(None::<String>),
        enhancement_tasks::comfyui_prompt_id.eq(None::<String>),
    ))
    .execute(conn)?;

    if requeued > 0 {
        diesel::update(runs::table.filter(runs::id.eq(run_id)))
            .set((
                runs::status.eq(RunState::Running.as_str()),
                runs::error_message.eq(None::<String>),
                runs::finished_at.eq(None::<String>),
            ))
            .execute(conn)?;
    }
    Ok(requeued)
}

/// Stop every task of a run that has not landed yet.
///
/// Only the local rows: interrupting a prompt that is actually executing on
/// ComfyUI is [`crate::api`]'s job, because it needs the HTTP client. A task
/// still `pending` never reached ComfyUI at all.
pub(crate) fn cancel_run(conn: &mut SqliteConnection, run_id: &str) -> QueryResult<usize> {
    let now = format_ts(chrono::Utc::now().naive_utc());
    let stopped = diesel::update(
        enhancement_tasks::table.filter(enhancement_tasks::run_id.eq(run_id).and(
            enhancement_tasks::status.ne_all(&[
                "completed",
                "failed",
                crate::comfyui::STATUS_CANCELLED,
            ]),
        )),
    )
    .set((
        enhancement_tasks::status.eq(crate::comfyui::STATUS_CANCELLED),
        enhancement_tasks::error_message.eq("Run cancelled"),
        enhancement_tasks::completed_at.eq(&now),
        enhancement_tasks::settle_until.eq(None::<String>),
        enhancement_tasks::next_attempt_at.eq(None::<String>),
    ))
    .execute(conn)?;

    diesel::update(runs::table.filter(runs::id.eq(run_id)))
        .set((
            runs::status.eq(RunState::Cancelled.as_str()),
            runs::finished_at.eq(&now),
        ))
        .execute(conn)?;
    Ok(stopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::contract::StageContract;
    use diesel::connection::SimpleConnection;

    /// A graph that reads a still and saves a still.
    const IMAGE_GRAPH: &str = r#"{
        "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20, "cfg": 8.0}},
        "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
        "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out", "images": ["3", 0]}}
    }"#;

    struct Library {
        dir: tempfile::TempDir,
        conn: SqliteConnection,
    }

    impl Library {
        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn sql(&mut self, sql: &str) {
            self.conn.batch_execute(sql).unwrap();
        }

        /// A library with one shot, one photograph in it, and `n` identical
        /// image-to-image workflows to chain.
        fn with_workflows(n: usize) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join(".phos.db");
            crate::db::init_and_migrate(&db_path).unwrap();
            let conn = crate::db::open_diesel_connection(&db_path).unwrap();
            let mut lib = Library { dir, conn };
            std::fs::write(lib.root().join("original.jpg"), b"jpeg").unwrap();
            let workflows: String = (0..n)
                .map(|i| {
                    format!(
                        "INSERT INTO comfyui_workflows (id, name, workflow_json) \
                         VALUES ('wf-{}', 'Stage {}', '{}');",
                        i,
                        i + 1,
                        IMAGE_GRAPH.replace('\'', "''")
                    )
                })
                .collect();
            lib.sql(&format!(
                "{}
                 INSERT INTO shots (id) VALUES ('shot-1');
                 INSERT INTO files (id, shot_id, path, hash, mime_type, is_original) \
                 VALUES ('file-orig', 'shot-1', 'original.jpg', 'h0', 'image/jpeg', 1);",
                workflows
            ));
            lib
        }

        /// A line over the first `n` workflows. `keep` names the stages whose
        /// intermediates the user asked to hold on to.
        fn line(&mut self, n: usize, keep: &[usize]) {
            let stages: String = (0..n)
                .map(|i| {
                    format!(
                        "INSERT INTO line_stages \
                         (id, line_id, stage_idx, workflow_id, keep_output) \
                         VALUES ('st-{}', 'line-1', {}, 'wf-{}', {});",
                        i,
                        i,
                        i,
                        if keep.contains(&i) { 1 } else { 0 }
                    )
                })
                .collect();
            self.sql(&format!(
                "INSERT INTO production_lines (id, name) VALUES ('line-1', '4K Restore');
                 {}",
                stages
            ));
        }

        fn start(&mut self) -> crate::comfyui::runs::RunStart {
            crate::comfyui::runs::start_line_run(
                &mut self.conn,
                "line-1",
                "shot-1",
                &Default::default(),
            )
            .unwrap()
        }

        fn advance(&mut self) {
            let root = self.dir.path().to_path_buf();
            advance_runs(&mut self.conn, &root);
        }

        /// What a stage does when it works: a file lands in the library and the
        /// task points at it. The bytes are real, so the discard pass has
        /// something to delete.
        fn land(&mut self, task_id: &str) -> String {
            let file_id = format!("out-{}", &task_id[..8]);
            let name = format!("{}.png", file_id);
            std::fs::write(self.root().join(&name), b"png").unwrap();
            diesel::insert_into(files::table)
                .values((
                    files::id.eq(&file_id),
                    files::shot_id.eq("shot-1"),
                    files::path.eq(&name),
                    files::hash.eq(&file_id),
                    files::mime_type.eq("image/png"),
                    files::is_original.eq(false),
                    files::synthetic.eq(true),
                ))
                .execute(&mut self.conn)
                .unwrap();
            diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
                .set((
                    enhancement_tasks::status.eq("completed"),
                    enhancement_tasks::output_file_id.eq(&file_id),
                    enhancement_tasks::completed_at.eq("2026-08-30 12:00:00"),
                ))
                .execute(&mut self.conn)
                .unwrap();
            file_id
        }

        fn break_task(&mut self, task_id: &str, message: &str) {
            diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
                .set((
                    enhancement_tasks::status.eq("failed"),
                    enhancement_tasks::error_message.eq(message),
                ))
                .execute(&mut self.conn)
                .unwrap();
        }

        /// Every task of the run, as (stage_idx, status), in stage order.
        fn tasks(&mut self) -> Vec<(i32, String)> {
            enhancement_tasks::table
                .order((
                    enhancement_tasks::stage_idx.asc(),
                    enhancement_tasks::id.asc(),
                ))
                .select((
                    enhancement_tasks::stage_idx.assume_not_null(),
                    enhancement_tasks::status,
                ))
                .load(&mut self.conn)
                .unwrap()
        }

        fn pending_at(&mut self, stage_idx: i32) -> Vec<String> {
            enhancement_tasks::table
                .filter(
                    enhancement_tasks::stage_idx
                        .eq(stage_idx)
                        .and(enhancement_tasks::status.eq("pending")),
                )
                .order(enhancement_tasks::id.asc())
                .select(enhancement_tasks::id)
                .load(&mut self.conn)
                .unwrap()
        }

        fn run_status(&mut self, run_id: &str) -> (String, Option<String>) {
            runs::table
                .filter(runs::id.eq(run_id))
                .select((runs::status, runs::error_message))
                .first(&mut self.conn)
                .unwrap()
        }

        fn file_exists(&mut self, file_id: &str) -> bool {
            files::table
                .filter(files::id.eq(file_id))
                .count()
                .get_result::<i64>(&mut self.conn)
                .unwrap()
                > 0
        }
    }

    #[test]
    fn a_three_stage_run_walks_from_the_photograph_to_the_product() {
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[]);
        let run = lib.start();
        assert_eq!(lib.tasks(), vec![(0, "pending".to_string())]);

        // Stage 1 lands. Stage 2 is queued from what it made, and nothing else.
        let stage1_out = lib.land(&run.task_ids[0]);
        lib.advance();
        let stage2 = lib.pending_at(1);
        assert_eq!(stage2.len(), 1);
        assert_eq!(
            enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(&stage2[0]))
                .select((
                    enhancement_tasks::source_file_id,
                    enhancement_tasks::parent_task_id,
                    enhancement_tasks::workflow_id,
                ))
                .first::<(Option<String>, Option<String>, String)>(&mut lib.conn)
                .unwrap(),
            (
                Some(stage1_out),
                Some(run.task_ids[0].clone()),
                "wf-1".to_string()
            ),
            "stage 2 eats stage 1's output, and knows who made it"
        );
        assert_eq!(lib.run_status(&run.run_id).0, "running");

        // Running the pass again queues nothing new: the parent link is the
        // marker, so a tick that finds the same completed task does nothing.
        lib.advance();
        assert_eq!(lib.pending_at(1).len(), 1, "advanced exactly once");

        lib.land(&stage2[0]);
        lib.advance();
        let stage3 = lib.pending_at(2);
        assert_eq!(stage3.len(), 1);
        assert_eq!(lib.run_status(&run.run_id).0, "running");

        // The last stage lands. Nothing is queued after it, and the run is over.
        lib.land(&stage3[0]);
        lib.advance();
        assert_eq!(lib.pending_at(3), Vec::<String>::new());
        assert_eq!(lib.run_status(&run.run_id).0, "completed");
        assert!(
            runs::table
                .filter(runs::id.eq(&run.run_id))
                .select(runs::finished_at)
                .first::<Option<String>>(&mut lib.conn)
                .unwrap()
                .is_some(),
            "a finished run knows when it finished"
        );
    }

    #[test]
    fn a_stage_two_failure_leaves_three_and_four_unqueued_and_resumes_from_two() {
        let mut lib = Library::with_workflows(4);
        lib.line(4, &[]);
        let run = lib.start();

        let stage1_out = lib.land(&run.task_ids[0]);
        lib.advance();
        let stage2 = lib.pending_at(1)[0].clone();

        // Stage 2 falls over.
        lib.break_task(&stage2, "CUDA out of memory");
        lib.advance();

        assert_eq!(
            lib.tasks(),
            vec![(0, "completed".to_string()), (1, "failed".to_string())],
            "stages 3 and 4 were never queued"
        );
        let (status, error) = lib.run_status(&run.run_id);
        assert_eq!(status, "failed");
        assert_eq!(error.as_deref(), Some("CUDA out of memory"));

        // The intermediate stage 2 was given is still there — the run failed,
        // so nothing was swept, and that file is what the retry resumes from.
        assert!(lib.file_exists(&stage1_out));

        // Retry. Stage 1 is not re-run; stage 2 goes back in the queue holding
        // the same source file it was given the first time.
        let resumed = crate::comfyui::retry_run(&mut lib.conn, &run.run_id).unwrap();
        assert_eq!(resumed, 1, "only the failed stage");
        assert_eq!(
            lib.tasks(),
            vec![(0, "completed".to_string()), (1, "pending".to_string())]
        );
        assert_eq!(
            enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(&stage2))
                .select(enhancement_tasks::source_file_id)
                .first::<Option<String>>(&mut lib.conn)
                .unwrap(),
            Some(stage1_out),
            "resumed from the intermediate, not from the original photograph"
        );
        assert_eq!(lib.run_status(&run.run_id).0, "running");

        // And it goes on from there.
        lib.land(&stage2);
        lib.advance();
        assert_eq!(lib.pending_at(2).len(), 1, "stage 3 finally queued");
    }

    #[test]
    fn a_fan_out_of_four_at_stage_two_becomes_four_independent_continuations() {
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[]);
        // Stage 2 asks for four seeds.
        lib.sql(
            r#"UPDATE line_stages
               SET parameters = '{"3.seed":1000}',
                   vary = '{"3.seed":{"count":4,"mode":"increment"}}'
               WHERE stage_idx = 1;"#,
        );
        let run = lib.start();

        lib.land(&run.task_ids[0]);
        lib.advance();
        let takes = lib.pending_at(1);
        assert_eq!(takes.len(), 4, "one request, four takes");

        // Each take walks stage 3 for itself: four parents, four children.
        for take in &takes {
            lib.land(take);
        }
        lib.advance();
        let stage3 = lib.pending_at(2);
        assert_eq!(stage3.len(), 4, "the fan-out propagated");
        let mut parents: Vec<String> = enhancement_tasks::table
            .filter(enhancement_tasks::stage_idx.eq(2))
            .select(enhancement_tasks::parent_task_id.assume_not_null())
            .load(&mut lib.conn)
            .unwrap();
        parents.sort();
        let mut expected = takes.clone();
        expected.sort();
        assert_eq!(parents, expected, "each continuation has its own parent");

        // One take failing does not stop the other three.
        lib.break_task(&stage3[0], "node raised");
        for t in &stage3[1..] {
            lib.land(t);
        }
        lib.advance();
        let (status, error) = lib.run_status(&run.run_id);
        assert_eq!(status, "failed");
        assert_eq!(error.as_deref(), Some("node raised"));
        assert_eq!(
            lib.tasks()
                .iter()
                .filter(|(s, st)| *s == 2 && st == "completed")
                .count(),
            3,
            "three takes made it to the end"
        );
    }

    #[test]
    fn intermediates_are_swept_on_completion_unless_the_stage_says_keep() {
        let mut lib = Library::with_workflows(3);
        // Stage 2's output is worth keeping; stage 1's is not.
        lib.line(3, &[1]);
        let run = lib.start();

        let stage1_out = lib.land(&run.task_ids[0]);
        lib.advance();
        let stage2 = lib.pending_at(1)[0].clone();
        let stage2_out = lib.land(&stage2);
        lib.advance();
        let stage3 = lib.pending_at(2)[0].clone();

        // While the run is live, everything is still there: the next stage
        // reads it, and a failure would want it.
        assert!(lib.file_exists(&stage1_out));
        assert!(lib.file_exists(&stage2_out));

        let stage3_out = lib.land(&stage3);
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "completed");

        assert!(
            !lib.file_exists(&stage1_out),
            "an unkept intermediate is swept when the run lands"
        );
        assert!(
            !lib.root().join(format!("{}.png", stage1_out)).exists(),
            "and its bytes go with it"
        );
        assert!(
            lib.file_exists(&stage2_out),
            "the stage the user marked keep survives"
        );
        assert!(
            lib.file_exists(&stage3_out),
            "the last stage's output is the product, whatever its flag says"
        );

        // The task no longer points at a row that is gone.
        assert_eq!(
            enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(&run.task_ids[0]))
                .select(enhancement_tasks::output_file_id)
                .first::<Option<String>>(&mut lib.conn)
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_line_edited_out_from_under_a_live_run_fails_it_rather_than_stalling() {
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[]);
        let run = lib.start();
        lib.land(&run.task_ids[0]);

        // Somebody deleted the rest of the line while stage 1 was running.
        lib.sql("DELETE FROM line_stages WHERE stage_idx > 0;");
        lib.advance();

        let (status, error) = lib.run_status(&run.run_id);
        assert_eq!(status, "failed");
        assert!(
            error.unwrap().contains("no longer part of the line"),
            "and says why"
        );
    }

    #[test]
    fn cancelling_a_run_stops_what_has_not_landed_and_leaves_what_has() {
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[]);
        let run = lib.start();
        lib.land(&run.task_ids[0]);
        lib.advance();

        let stopped = crate::comfyui::cancel_run(&mut lib.conn, &run.run_id).unwrap();
        assert_eq!(stopped, 1, "only the stage that was still queued");
        assert_eq!(
            lib.tasks(),
            vec![(0, "completed".to_string()), (1, "cancelled".to_string())]
        );
        assert_eq!(lib.run_status(&run.run_id).0, "cancelled");
    }

    #[test]
    fn a_completed_runs_tasks_are_swept_and_a_live_runs_are_not() {
        let mut lib = Library::with_workflows(2);
        lib.line(2, &[]);
        let run = lib.start();
        lib.land(&run.task_ids[0]);
        lib.advance();

        // The stage-1 task completed long ago, which is well past the
        // five-minute sweep — but its run is still walking, and the next stage
        // reads what it made.
        lib.sql(
            "UPDATE enhancement_tasks SET completed_at = '2020-01-01 00:00:00' \
             WHERE status = 'completed';",
        );
        super::super::cleanup_completed_tasks(&mut lib.conn);
        assert_eq!(
            lib.tasks().len(),
            2,
            "a live run keeps the step the next one is reading"
        );

        // Once the run lands, the ordinary five-minute rule applies again.
        let stage2 = lib.pending_at(1)[0].clone();
        lib.land(&stage2);
        lib.advance();
        lib.sql("UPDATE enhancement_tasks SET completed_at = '2020-01-01 00:00:00';");
        super::super::cleanup_completed_tasks(&mut lib.conn);
        assert_eq!(lib.tasks().len(), 0);
        assert_eq!(
            lib.run_status(&run.run_id).0,
            "completed",
            "and the run itself still reads on the board"
        );
    }

    #[test]
    fn a_contract_corrected_mid_run_is_caught_before_the_next_stage_uploads() {
        let mut lib = Library::with_workflows(2);
        lib.line(2, &[]);
        let run = lib.start();

        // Stage 2's workflow is re-contracted to say it takes video. The line
        // was valid when it was drawn; it is not any more.
        let mut contract = StageContract::derive(
            &serde_json::from_str::<serde_json::Value>(IMAGE_GRAPH).unwrap(),
            None,
        );
        contract.accepts = crate::comfyui::Accepts::Video;
        let json = serde_json::to_string(&contract).unwrap();
        diesel::update(
            crate::schema::comfyui_workflows::table
                .filter(crate::schema::comfyui_workflows::id.eq("wf-1")),
        )
        .set(crate::schema::comfyui_workflows::contract_json.eq(&json))
        .execute(&mut lib.conn)
        .unwrap();

        lib.land(&run.task_ids[0]);
        lib.advance();

        // The continuation is queued — the advance pass does not second-guess
        // the line — and dispatch is where it is refused, with the real source
        // in hand rather than a declared one.
        let stage2 = lib.pending_at(1);
        assert_eq!(stage2.len(), 1);
        let root = lib.dir.path().to_path_buf();
        super::super::dispatch::process_pending_tasks(
            &mut lib.conn,
            &crate::comfyui::ComfyUiClient::new("http://127.0.0.1:1"),
            &root,
        );
        let (status, error): (String, Option<String>) = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&stage2[0]))
            .select((enhancement_tasks::status, enhancement_tasks::error_message))
            .first(&mut lib.conn)
            .unwrap();
        assert_eq!(status, "failed", "and not after four retries, either");
        let error = error.unwrap();
        assert!(error.contains("takes video"), "{}", error);
        assert!(error.contains("handed it image"), "{}", error);
    }
}
