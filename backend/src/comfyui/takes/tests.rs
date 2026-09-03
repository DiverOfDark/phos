//! The curation lane against a real database and real bytes.
//!
//! [`bulk`](super::bulk) proves the *rule* a bulk verdict follows with no
//! database at all. What is left is everything that only a disk can answer:
//! that rejecting removes the file rather than the row, that a refused verdict
//! has deleted nothing, that promoting a take moves the shot, and that a batch
//! verdict touches exactly the runs it claims and leaves the others held.
//!
//! # The batch column, from both sides
//!
//! FR7's `runs.batch_id` is being written in parallel and is not in the schema
//! yet, so the seam has two behaviours and both are pinned here: without the
//! column a batch verdict quietly becomes a run verdict, and with it — added by
//! the test with exactly the `ALTER TABLE` FR7 will ship — it reaches the batch
//! and stops at its edges.

use std::path::Path;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;

use super::bulk::Scope;
use super::verdicts::{self, Ask};
use crate::comfyui::holds::HoldError;
use crate::comfyui::line::Verdict;
use crate::schema::{files, runs, shots};

/// A graph that reads a still and saves a still, with the sampler at node 17
/// rather than ComfyUI's example node 3 — the difference that made four takes
/// indistinguishable once already.
const IMAGE_GRAPH: &str = r#"{
    "17": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20, "cfg": 8.0}},
    "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
    "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out", "images": ["17", 0]}}
}"#;

struct Lib {
    dir: tempfile::TempDir,
    conn: SqliteConnection,
}

impl Lib {
    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn sql(&mut self, sql: &str) {
        self.conn.batch_execute(sql).unwrap();
    }

    /// One shot with a photograph in it, three chained workflows, and a
    /// three-stage line that parks at stage 1.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let conn = crate::db::open_diesel_connection(&db_path).unwrap();
        let mut lib = Lib { dir, conn };
        std::fs::write(lib.root().join("original.jpg"), b"jpeg").unwrap();

        let workflows: String = (0..3)
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
        let stages: String = (0..3)
            .map(|i| {
                format!(
                    "INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, \
                     keep_output, hold_for_review) VALUES ('st-{}', 'line-1', {}, 'wf-{}', 0, {});",
                    i,
                    i,
                    i,
                    if i == 1 { 1 } else { 0 }
                )
            })
            .collect();
        lib.sql(&format!(
            "{workflows}
             INSERT INTO production_lines (id, name) VALUES ('line-1', '4K Restore');
             {stages}
             INSERT INTO shots (id, main_file_id) VALUES ('shot-1', 'file-orig');
             INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, file_size) \
             VALUES ('file-orig', 'shot-1', 'original.jpg', 'h0', 'image/jpeg', 1, 4);"
        ));
        lib
    }

    /// A run parked at stage 1 with `n` completed takes, each with real bytes
    /// on disk. Built by hand rather than driven through the worker, because
    /// [`crate::comfyui::worker::advance`] already proves the worker parks it;
    /// what is under test here is the lane over the parked state.
    ///
    /// Returns the task ids, in order.
    fn held_run(&mut self, run_id: &str, n: usize, created_at: &str) -> Vec<String> {
        self.sql(&format!(
            "INSERT INTO runs (id, line_id, shot_id, label, status, stage_count, \
             held_at_stage, created_at) \
             VALUES ('{run_id}', 'line-1', 'shot-1', '4K Restore', 'held', 3, 1, '{created_at}');"
        ));
        let mut ids = Vec::new();
        for i in 0..n {
            let task_id = format!("{run_id}-take-{i}");
            let file_id = format!("{run_id}-file-{i}");
            let name = format!("{file_id}.png");
            // A hundred bytes rather than four, so a byte count that adds up
            // wrong is visible rather than a rounding accident.
            std::fs::write(self.root().join(&name), vec![b'p'; 100]).unwrap();
            self.sql(&format!(
                "INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, \
                 synthetic, file_size) \
                 VALUES ('{file_id}', 'shot-1', '{name}', '{file_id}', 'image/png', 0, 1, 100);
                 INSERT INTO enhancement_tasks (id, shot_id, workflow_id, status, run_id, \
                 stage_idx, source_file_id, output_file_id, parameters, completed_at) \
                 VALUES ('{task_id}', 'shot-1', 'wf-1', 'completed', '{run_id}', 1, \
                 'file-orig', '{file_id}', '{{\"17.seed\":{seed}}}', '2026-08-31 09:0{i}:00');",
                seed = 1000 + i
            ));
            ids.push(task_id);
        }
        ids
    }

    /// `runs.batch_id` used to be added here, because FR7 was being written in
    /// parallel and the seam had to be testable from this side before its
    /// column existed. It ships in the `batches` migration now, so the
    /// behaviour these tests pin is the real one and this is a no-op kept for
    /// the shape of the tests that call it.
    fn with_fr7_batches(&mut self) {}

    fn put_in_batch(&mut self, run_id: &str, batch: &str) {
        self.sql(&format!(
            "UPDATE runs SET batch_id = '{batch}' WHERE id = '{run_id}';"
        ));
    }

    fn ask(&mut self, run_id: &str, ask: Ask<'_>) -> Result<verdicts::Applied, HoldError> {
        let root = self.dir.path().to_path_buf();
        verdicts::apply(&mut self.conn, &root, run_id, ask)
    }

    fn status(&mut self, run_id: &str) -> String {
        runs::table
            .filter(runs::id.eq(run_id))
            .select(runs::status)
            .first(&mut self.conn)
            .unwrap()
    }

    fn file_row_exists(&mut self, file_id: &str) -> bool {
        files::table
            .filter(files::id.eq(file_id))
            .count()
            .get_result::<i64>(&mut self.conn)
            .unwrap()
            > 0
    }

    fn bytes_on_disk(&self, file_id: &str) -> bool {
        self.root().join(format!("{file_id}.png")).exists()
    }

    fn main_file(&mut self) -> Option<String> {
        shots::table
            .filter(shots::id.eq("shot-1"))
            .select(shots::main_file_id)
            .first(&mut self.conn)
            .unwrap()
    }
}

fn keep(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn plain(verdict: Verdict, keeping: &[String], rejecting: &[String]) -> Ask<'static> {
    // The slices outlive the call; leaking here keeps the fixtures readable and
    // costs a test process a few dozen bytes.
    Ask {
        verdict,
        keep: Box::leak(keeping.to_vec().into_boxed_slice()),
        reject: Box::leak(rejecting.to_vec().into_boxed_slice()),
        note: None,
        scope: Scope::Run,
    }
}

// ── The sheet ────────────────────────────────────────────────────────────────

#[test]
fn the_lane_reads_every_held_run_in_one_request_oldest_first() {
    // The screen is a backlog somebody is draining. The run that has been
    // waiting longest is the one holding up a batch's outstanding-hold cap, so
    // it is the one they should be looking at.
    let mut lib = Lib::new();
    lib.held_run("run-b", 2, "2026-08-31 10:00:00");
    lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    lib.held_run("run-c", 1, "2026-08-31 11:00:00");

    let (sheets, next) = super::list_sheets(&mut lib.conn, 24, None).unwrap();
    let ids: Vec<&str> = sheets.iter().map(|s| s.hold.run_id.as_str()).collect();
    assert_eq!(ids, vec!["run-a", "run-b", "run-c"]);
    assert_eq!(next, None, "three runs is not three pages");
    assert_eq!(sheets[0].hold.takes.len(), 4, "a four-seed fan-out");
}

#[test]
fn a_sheet_carries_the_original_the_takes_are_variations_of() {
    // Compare mode is the case the whole hold mechanism exists for, and it must
    // not cost a request per run.
    let mut lib = Lib::new();
    lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    let (sheets, _) = super::list_sheets(&mut lib.conn, 24, None).unwrap();
    assert_eq!(sheets[0].source_file_id.as_deref(), Some("file-orig"));
    assert_eq!(sheets[0].main_file_id.as_deref(), Some("file-orig"));
    assert_eq!(
        sheets[0].details["run-a-file-0"].file_size,
        Some(100),
        "and how many bytes rejecting it would free"
    );
    assert_eq!(
        sheets[0].details["run-a-file-0"].mime_type.as_deref(),
        Some("image/png"),
        "so the card knows whether space plays anything"
    );
}

#[test]
fn a_run_that_is_not_holding_is_not_on_the_sheet() {
    let mut lib = Lib::new();
    lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    lib.held_run("run-b", 2, "2026-08-31 10:00:00");
    lib.sql("UPDATE runs SET status = 'running', held_at_stage = NULL WHERE id = 'run-b';");
    let (sheets, _) = super::list_sheets(&mut lib.conn, 24, None).unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].hold.run_id, "run-a");
}

#[test]
fn a_page_is_two_dozen_and_never_more_than_a_hundred() {
    // Each row re-prices its own continuation by walking its line's stages, so
    // "give me everything" has to mean something bounded.
    let mut lib = Lib::new();
    for i in 0..3 {
        lib.held_run(&format!("run-{i}"), 1, &format!("2026-08-31 0{i}:00:00"));
    }
    assert_eq!(
        super::list_sheets(&mut lib.conn, 10_000, None)
            .unwrap()
            .0
            .len(),
        3,
        "a limit above the cap is the cap, not an error"
    );
    assert_eq!(
        super::list_sheets(&mut lib.conn, 0, None).unwrap().0.len(),
        1,
        "and zero is one, because a page of nothing never ends"
    );
    assert_eq!(super::DEFAULT_PAGE, 24);
    assert_eq!(super::MAX_PAGE, 100);
}

#[test]
fn a_page_ends_with_the_cursor_the_next_one_starts_from() {
    let mut lib = Lib::new();
    for i in 0..5 {
        lib.held_run(&format!("run-{i}"), 1, &format!("2026-08-31 0{i}:00:00"));
    }
    let (first, cursor) = super::list_sheets(&mut lib.conn, 2, None).unwrap();
    assert_eq!(first.len(), 2);
    let cursor = cursor.expect("three more are waiting");
    let (second, _) = super::list_sheets(&mut lib.conn, 10, Some(&cursor)).unwrap();
    let ids: Vec<&str> = second.iter().map(|s| s.hold.run_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["run-2", "run-3", "run-4"],
        "no row twice, none lost"
    );
}

// ── Reject deletes the bytes ─────────────────────────────────────────────────

#[test]
fn rejecting_a_take_removes_the_file_and_not_only_the_row() {
    // The requirement the lane exists to make safe: generated video is enormous
    // and a farm fills a disk in days, so `X` has to actually free space.
    let mut lib = Lib::new();
    let takes = lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    assert!(lib.bytes_on_disk("run-a-file-1"));

    let applied = lib
        .ask(
            "run-a",
            plain(
                Verdict::Continue,
                &keep(&[&takes[0]]),
                &keep(&[&takes[1], &takes[2]]),
            ),
        )
        .unwrap();

    assert_eq!(applied.rejected.len(), 2);
    assert_eq!(
        applied.freed_bytes, 200,
        "two hundred-byte takes, counted before they went"
    );
    for gone in ["run-a-file-1", "run-a-file-2"] {
        assert!(!lib.bytes_on_disk(gone), "{gone} is still on disk");
        assert!(!lib.file_row_exists(gone), "{gone} still has a row");
    }
    assert!(
        lib.bytes_on_disk("run-a-file-0"),
        "the take that was kept is the one that goes on to the next stage"
    );
    assert!(
        lib.bytes_on_disk("run-a-file-3"),
        "and a take merely passed over is the stage's keep policy to dispose of, not the reviewer's"
    );
    assert_eq!(
        lib.status("run-a"),
        "running",
        "the verdict released the run"
    );
}

#[test]
fn a_take_this_run_is_not_holding_cannot_be_rejected_and_nothing_is_deleted() {
    let mut lib = Lib::new();
    let takes = lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    lib.held_run("run-b", 2, "2026-08-31 10:00:00");

    let err = lib
        .ask(
            "run-a",
            plain(
                Verdict::Continue,
                &keep(&[&takes[0]]),
                &keep(&["run-b-take-0"]),
            ),
        )
        .unwrap_err();
    assert!(matches!(err, HoldError::Refused(_)), "{err:?}");

    assert!(
        lib.bytes_on_disk("run-b-file-0"),
        "another run's take is safe"
    );
    assert!(lib.bytes_on_disk("run-a-file-0"), "and so is this run's");
    assert_eq!(
        lib.status("run-a"),
        "held",
        "a refused verdict leaves the run exactly where it was"
    );
}

#[test]
fn a_take_named_both_kept_and_rejected_is_refused_rather_than_guessed_at() {
    // One of those two words deletes the file. Picking for somebody is the
    // wrong answer whichever way it is picked.
    let mut lib = Lib::new();
    let takes = lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    let err = lib
        .ask(
            "run-a",
            plain(Verdict::Continue, &keep(&[&takes[0]]), &keep(&[&takes[0]])),
        )
        .unwrap_err();
    assert!(matches!(err, HoldError::Refused(_)), "{err:?}");
    assert!(lib.bytes_on_disk("run-a-file-0"));
    assert_eq!(lib.status("run-a"), "held");
}

#[test]
fn rejecting_every_take_is_a_cancel_and_still_frees_the_disk() {
    // Four bad takes: there is nothing to continue with, so the honest verdict
    // is the one that abandons the run — and the disk should come back anyway.
    let mut lib = Lib::new();
    let takes = lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    let applied = lib
        .ask("run-a", plain(Verdict::Cancel, &[], &takes))
        .unwrap();
    assert_eq!(applied.freed_bytes, 400);
    for i in 0..4 {
        assert!(!lib.bytes_on_disk(&format!("run-a-file-{i}")));
    }
    assert_eq!(lib.status("run-a"), "cancelled");
}

// ── Promote ──────────────────────────────────────────────────────────────────

#[test]
fn promoting_a_take_makes_it_the_shots_main_file_and_demotes_the_photograph() {
    // `P`. The take a person just chose becomes the picture the library shows,
    // and both halves of "main file" have to move together or the gallery draws
    // one thing and opens another.
    let mut lib = Lib::new();
    lib.held_run("run-a", 4, "2026-08-31 09:00:00");
    assert_eq!(lib.main_file().as_deref(), Some("file-orig"));

    let shot = crate::api::files::set_main_file(&mut lib.conn, "run-a-file-2").unwrap();
    assert_eq!(shot, "shot-1");
    assert_eq!(lib.main_file().as_deref(), Some("run-a-file-2"));

    let originals: Vec<String> = files::table
        .filter(files::is_original.eq(true))
        .select(files::id)
        .load(&mut lib.conn)
        .unwrap();
    assert_eq!(
        originals,
        vec!["run-a-file-2".to_string()],
        "exactly one file is the original, and it is the promoted take"
    );
}

#[test]
fn promoting_a_file_that_is_not_there_says_so_rather_than_moving_the_shot() {
    let mut lib = Lib::new();
    lib.held_run("run-a", 1, "2026-08-31 09:00:00");
    let err = crate::api::files::set_main_file(&mut lib.conn, "no-such-file").unwrap_err();
    assert!(matches!(err, diesel::result::Error::NotFound));
    assert_eq!(lib.main_file().as_deref(), Some("file-orig"));
}

// ── Rating ───────────────────────────────────────────────────────────────────

#[test]
fn a_rating_is_one_to_five_and_clearing_it_is_not_zero() {
    let mut lib = Lib::new();
    lib.held_run("run-a", 1, "2026-08-31 09:00:00");
    assert_eq!(
        super::rate_file(&mut lib.conn, "run-a-file-0", Some(4)).unwrap(),
        Some(4)
    );
    assert_eq!(
        super::rate_file(&mut lib.conn, "run-a-file-0", Some(9)).unwrap(),
        Some(5),
        "clamped, because losing a keystroke over a caller's bug helps nobody"
    );
    assert_eq!(
        super::rate_file(&mut lib.conn, "run-a-file-0", None).unwrap(),
        None,
        "and not rated is a different answer from rated zero"
    );
    let stored: Option<i32> = files::table
        .filter(files::id.eq("run-a-file-0"))
        .select(files::rating)
        .first(&mut lib.conn)
        .unwrap();
    assert_eq!(stored, None);
}

#[test]
fn rating_a_file_that_is_not_there_is_a_not_found_rather_than_a_silent_nothing() {
    let mut lib = Lib::new();
    let err = super::rate_file(&mut lib.conn, "no-such-file", Some(3)).unwrap_err();
    assert!(matches!(err, diesel::result::Error::NotFound));
}

// ── Bulk ─────────────────────────────────────────────────────────────────────

#[test]
fn a_run_scoped_verdict_leaves_every_sibling_held() {
    let mut lib = Lib::new();
    lib.with_fr7_batches();
    for i in 0..3 {
        lib.held_run(&format!("run-{i}"), 2, &format!("2026-08-31 0{i}:00:00"));
        lib.put_in_batch(&format!("run-{i}"), "batch-a");
    }
    let applied = lib.ask("run-0", plain(Verdict::Cancel, &[], &[])).unwrap();
    assert!(applied.also_applied.is_empty());
    assert_eq!(lib.status("run-0"), "cancelled");
    assert_eq!(lib.status("run-1"), "held");
    assert_eq!(lib.status("run-2"), "held");
}

#[test]
fn a_batch_scoped_cancel_reaches_the_batch_and_stops_at_its_edge() {
    let mut lib = Lib::new();
    lib.with_fr7_batches();
    for i in 0..3 {
        lib.held_run(&format!("run-{i}"), 2, &format!("2026-08-31 0{i}:00:00"));
        lib.put_in_batch(&format!("run-{i}"), "batch-a");
    }
    lib.held_run("run-other", 2, "2026-08-31 04:00:00");
    lib.put_in_batch("run-other", "batch-b");
    lib.held_run("run-lonely", 2, "2026-08-31 05:00:00");

    let applied = lib
        .ask(
            "run-1",
            Ask {
                scope: Scope::Batch,
                ..plain(Verdict::Cancel, &[], &[])
            },
        )
        .unwrap();

    let mut reached = applied.also_applied.clone();
    reached.sort();
    assert_eq!(reached, vec!["run-0".to_string(), "run-2".to_string()]);
    assert!(applied.failed.is_empty(), "{:?}", applied.failed);
    for cancelled in ["run-0", "run-1", "run-2"] {
        assert_eq!(lib.status(cancelled), "cancelled");
    }
    assert_eq!(
        lib.status("run-other"),
        "held",
        "another batch is another decision"
    );
    assert_eq!(
        lib.status("run-lonely"),
        "held",
        "and no batch is a batch of one"
    );
}

#[test]
fn a_batch_scoped_continue_keeps_every_take_of_a_run_nobody_opened() {
    // "I read a sample of the descriptions, they are fine, let the rest
    // through." Task ids are per run, so the only meaning a selection can carry
    // across is *all of them* — and that is the meaning worth having.
    let mut lib = Lib::new();
    lib.with_fr7_batches();
    let mine = lib.held_run("run-0", 4, "2026-08-31 00:00:00");
    lib.put_in_batch("run-0", "batch-a");
    lib.held_run("run-1", 3, "2026-08-31 01:00:00");
    lib.put_in_batch("run-1", "batch-a");

    let applied = lib
        .ask(
            "run-0",
            Ask {
                scope: Scope::Batch,
                ..plain(Verdict::Continue, &keep(&[&mine[0]]), &[])
            },
        )
        .unwrap();

    assert_eq!(
        applied.outcome.kept,
        vec![mine[0].clone()],
        "one of four here"
    );
    assert_eq!(applied.also_applied, vec!["run-1".to_string()]);

    let kept_there: String = crate::schema::run_holds::table
        .filter(crate::schema::run_holds::run_id.eq("run-1"))
        .select(crate::schema::run_holds::kept_task_ids)
        .first(&mut lib.conn)
        .unwrap();
    let kept_there: Vec<String> = serde_json::from_str(&kept_there).unwrap();
    assert_eq!(kept_there.len(), 3, "all three of a run nobody opened");
}

#[test]
fn a_rejection_never_travels_to_a_run_nobody_looked_at() {
    // Deleting bytes is something you do to pictures you have seen.
    let mut lib = Lib::new();
    lib.with_fr7_batches();
    let mine = lib.held_run("run-0", 4, "2026-08-31 00:00:00");
    lib.put_in_batch("run-0", "batch-a");
    lib.held_run("run-1", 3, "2026-08-31 01:00:00");
    lib.put_in_batch("run-1", "batch-a");

    lib.ask(
        "run-0",
        Ask {
            scope: Scope::Batch,
            ..plain(Verdict::Continue, &keep(&[&mine[0]]), &keep(&[&mine[1]]))
        },
    )
    .unwrap();

    assert!(
        !lib.bytes_on_disk("run-0-file-1"),
        "the one that was looked at"
    );
    for i in 0..3 {
        assert!(
            lib.bytes_on_disk(&format!("run-1-file-{i}")),
            "run-1's take {i} was never on anybody's screen"
        );
    }
}

#[test]
fn without_fr7s_column_a_batch_verdict_is_quietly_a_run_verdict() {
    // The state of the world until FR7 merges. A lane that guessed wide here
    // would cancel a library.
    let mut lib = Lib::new();
    for i in 0..3 {
        lib.held_run(&format!("run-{i}"), 2, &format!("2026-08-31 0{i}:00:00"));
    }
    let applied = lib
        .ask(
            "run-1",
            Ask {
                scope: Scope::Batch,
                ..plain(Verdict::Cancel, &[], &[])
            },
        )
        .unwrap();
    assert!(applied.also_applied.is_empty());
    assert_eq!(lib.status("run-0"), "held");
    assert_eq!(lib.status("run-2"), "held");
}

#[test]
fn a_sibling_that_landed_between_the_two_queries_is_simply_not_covered() {
    // The board would already show it done. A bulk verdict is a statement about
    // what is *held*, so a run that stopped being held is not a failure to
    // report — it is not part of the question.
    let mut lib = Lib::new();
    lib.with_fr7_batches();
    for i in 0..3 {
        lib.held_run(&format!("run-{i}"), 2, &format!("2026-08-31 0{i}:00:00"));
        lib.put_in_batch(&format!("run-{i}"), "batch-a");
    }
    lib.sql("UPDATE runs SET status = 'completed', held_at_stage = NULL WHERE id = 'run-2';");

    let applied = lib
        .ask(
            "run-0",
            Ask {
                scope: Scope::Batch,
                ..plain(Verdict::Cancel, &[], &[])
            },
        )
        .unwrap();

    assert_eq!(applied.also_applied, vec!["run-1".to_string()]);
    assert!(applied.failed.is_empty(), "{:?}", applied.failed);
    assert_eq!(lib.status("run-2"), "completed", "and it is left alone");
}

#[test]
fn a_sibling_the_verdict_cannot_be_given_on_is_skipped_with_its_reason() {
    // A run marked held with nothing left to look at — every take already
    // covered by an earlier verdict, or none ever completed. `continue` has
    // nothing to continue with there, and that one refusal must not roll back
    // the nine hundred that worked.
    let mut lib = Lib::new();
    lib.with_fr7_batches();
    let mine = lib.held_run("run-0", 2, "2026-08-31 00:00:00");
    lib.put_in_batch("run-0", "batch-a");
    lib.held_run("run-1", 2, "2026-08-31 01:00:00");
    lib.put_in_batch("run-1", "batch-a");
    lib.held_run("run-empty", 0, "2026-08-31 02:00:00");
    lib.put_in_batch("run-empty", "batch-a");

    let applied = lib
        .ask(
            "run-0",
            Ask {
                scope: Scope::Batch,
                ..plain(Verdict::Continue, &keep(&[&mine[0]]), &[])
            },
        )
        .unwrap();

    assert_eq!(applied.also_applied, vec!["run-1".to_string()]);
    assert_eq!(applied.failed.len(), 1);
    assert_eq!(applied.failed[0].0, "run-empty");
    assert!(
        applied.failed[0].1.contains("not holding"),
        "the reason is kept, not swallowed: {}",
        applied.failed[0].1
    );
    assert_eq!(
        lib.status("run-0"),
        "running",
        "the deliberate verdict still landed"
    );
    assert_eq!(lib.status("run-1"), "running");
    assert_eq!(
        lib.status("run-empty"),
        "held",
        "and the odd one is untouched"
    );
}

#[test]
fn a_verdict_on_a_run_that_is_not_holding_is_a_conflict_rather_than_a_no_op() {
    let mut lib = Lib::new();
    lib.held_run("run-a", 2, "2026-08-31 09:00:00");
    lib.sql("UPDATE runs SET status = 'running', held_at_stage = NULL WHERE id = 'run-a';");
    let err = lib
        .ask("run-a", plain(Verdict::Cancel, &[], &[]))
        .unwrap_err();
    assert!(matches!(err, HoldError::NotHeld), "{err:?}");
}
