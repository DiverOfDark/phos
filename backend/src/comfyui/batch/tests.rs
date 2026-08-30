//! The batch runtime against a real migrated SQLite file.
//!
//! Everything here is proven by running it. The pure arithmetic is tested
//! beside itself in [`super::plan`]; this module is for the claims that only a
//! database can settle — that the cursor really does walk, that
//! `skip_if_generated` really does exclude, that a cap really does stop the
//! feed, and that STOP really does leave nothing half-queued.

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;

use crate::api::shots::ShotsQuery;
use crate::schema::{enhancement_tasks, runs};

use super::plan::{Caps, Cursor};
use super::selection::{count, next_page, Narrowing, Selection};
use super::store::{self, BatchState};

/// A graph that takes a still and saves a still.
const IMAGE_GRAPH: &str = r#"{
    "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20, "cfg": 8.0}},
    "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
    "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out", "images": ["3", 0]}}
}"#;

fn library() -> (tempfile::TempDir, SqliteConnection) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(".phos.db");
    crate::db::init_and_migrate(&db_path).unwrap();
    let conn = crate::db::open_diesel_connection(&db_path).unwrap();
    (dir, conn)
}

/// A one-stage line over `IMAGE_GRAPH`, plus `n` shots each with one file.
///
/// Shot timestamps run `1900-01-01` upward, one year apart, so cursor order is
/// the same as the order the ids were made in and the assertions read as a
/// list. Four-digit years matter: the cursor compares timestamps as *text*, and
/// a five-digit year would sort before every four-digit one.
fn seed(conn: &mut SqliteConnection, shots: usize) {
    conn.batch_execute(&format!(
        "INSERT INTO comfyui_workflows (id, name, workflow_json) \
         VALUES ('wf-1', 'Restore', '{}');
         INSERT INTO production_lines (id, name) VALUES ('line-1', 'Restore & upscale');
         INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, keep_output) \
         VALUES ('st-1', 'line-1', 0, 'wf-1', 1);",
        IMAGE_GRAPH.replace('\'', "''")
    ))
    .unwrap();
    for i in 0..shots {
        conn.batch_execute(&format!(
            "INSERT INTO shots (id, timestamp, main_file_id) \
             VALUES ('shot-{i:03}', '{:04}-01-01 00:00:00', 'file-{i:03}');
             INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, synthetic) \
             VALUES ('file-{i:03}', 'shot-{i:03}', 'p{i:03}.jpg', 'h{i:03}', 'image/jpeg', 1, 0);
             UPDATE shots SET main_file_id = 'file-{i:03}' WHERE id = 'shot-{i:03}';",
            1900 + i,
            i = i
        ))
        .unwrap();
    }
}

fn whole_library() -> Selection {
    Selection::Query {
        query: ShotsQuery::default(),
    }
}

fn make_batch(conn: &mut SqliteConnection, caps: Caps, skip: bool) -> String {
    let (estimate, _) = store::estimate_for(conn, "line-1", 0, 0).unwrap();
    store::create(
        conn,
        "line-1",
        "Restore & upscale · whole library",
        &whole_library(),
        None,
        skip,
        &caps,
        &estimate,
    )
    .unwrap()
}

/// One tick, at 03:00 on a fixed day.
///
/// `lead` is handed in rather than read back off the row on purpose: how far
/// ahead of the queue a batch may run is a property of the system
/// ([`super::plan::DEFAULT_LEAD`]), not a per-batch preference, so it is the
/// one cap with no column. Setting it to a handful here is what makes "opens a
/// few at a time, not all at once" a thing a test can watch happen rather than
/// a claim about a constant.
fn tick_lead(
    conn: &mut SqliteConnection,
    root: &std::path::Path,
    id: &str,
    lead: Option<i64>,
) -> super::feed::FedBatch {
    tick_full(conn, root, id, 3, lead)
}

fn tick(conn: &mut SqliteConnection, root: &std::path::Path, id: &str) -> super::feed::FedBatch {
    tick_full(conn, root, id, 3, None)
}

fn tick_at(
    conn: &mut SqliteConnection,
    root: &std::path::Path,
    id: &str,
    hour: u32,
) -> super::feed::FedBatch {
    tick_full(conn, root, id, hour, None)
}

/// A fixed local wall-clock time, so nothing here depends on when it is run.
fn fake_now(hour: u32) -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2026, 8, 31)
        .unwrap()
        .and_hms_opt(hour, 0, 0)
        .unwrap()
}

/// Stamp every task as created on the fake day the ticks happen on, so the
/// daily cap counts them.
///
/// It asks [`store::day_boundary_utc`] for the stamp rather than writing a
/// literal, because `enhancement_tasks.created_at` is SQLite's UTC clock while
/// the cap's day is the *user's*. A test that assumed the two agree would pass
/// in London and fail in Berlin, which is exactly the bug this is here to hold
/// down.
fn stamp_tasks_today(conn: &mut SqliteConnection) {
    let boundary = store::day_boundary_utc(fake_now(3));
    conn.batch_execute(&format!(
        "UPDATE enhancement_tasks SET created_at = '{}';",
        boundary
    ))
    .unwrap();
}

fn tick_full(
    conn: &mut SqliteConnection,
    root: &std::path::Path,
    id: &str,
    hour: u32,
    lead: Option<i64>,
) -> super::feed::FedBatch {
    let mut batch = store::load(conn, id).unwrap().unwrap();
    batch.caps.lead = lead;
    super::feed::feed_one(conn, root, &batch, fake_now(hour)).unwrap()
}

fn opened_shot_ids(conn: &mut SqliteConnection, batch_id: &str) -> Vec<String> {
    runs::table
        .filter(runs::batch_id.eq(batch_id))
        .order(runs::created_at.asc())
        .select(runs::shot_id)
        .load(conn)
        .unwrap()
}

// ── The core claim: lazy materialisation in cursor order ──

#[test]
fn a_query_selected_batch_materialises_across_ticks_and_never_all_at_once() {
    let (dir, mut conn) = library();
    seed(&mut conn, 40);
    // A lead of 5 forces the feeder to open five at a time, which is what makes
    // "not all at once" observable in a test rather than an assertion of faith.
    let caps = Caps::default();
    let id = make_batch(&mut conn, caps, false);

    // Nothing is a row until a tick runs. Forty shots matched, zero runs.
    assert_eq!(
        count(&mut conn, &whole_library(), &Narrowing::default()).unwrap(),
        40
    );
    let all_runs: i64 = runs::table.count().get_result(&mut conn).unwrap();
    assert_eq!(all_runs, 0, "sending a batch must insert no runs at all");

    let first = tick_lead(&mut conn, dir.path(), &id, Some(5));
    assert_eq!(first.opened, 5);
    assert_eq!(opened_shot_ids(&mut conn, &id).len(), 5);

    // The five are the *oldest* five, in cursor order.
    assert_eq!(
        opened_shot_ids(&mut conn, &id),
        vec!["shot-000", "shot-001", "shot-002", "shot-003", "shot-004"]
    );

    // The lead is full, so the next tick opens nothing rather than running away.
    let blocked = tick_lead(&mut conn, dir.path(), &id, Some(5));
    assert_eq!(blocked.opened, 0);
    assert_eq!(opened_shot_ids(&mut conn, &id).len(), 5);

    // Finish those five and the next tick continues from the cursor, not from
    // the beginning — no shot is opened twice.
    diesel::update(runs::table.filter(runs::batch_id.eq(&id)))
        .set(runs::status.eq("completed"))
        .execute(&mut conn)
        .unwrap();
    let second = tick_lead(&mut conn, dir.path(), &id, Some(5));
    assert_eq!(second.opened, 5);
    let so_far = opened_shot_ids(&mut conn, &id);
    assert_eq!(so_far.len(), 10);
    assert_eq!(&so_far[5..], &["shot-005", "shot-006", "shot-007", "shot-008", "shot-009"]);

    let mut unique = so_far.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 10, "a shot must never be opened twice");
}

#[test]
fn the_cursor_is_written_to_the_row_and_resumes_from_there() {
    let (dir, mut conn) = library();
    seed(&mut conn, 10);
    let id = make_batch(
        &mut conn,
        Caps::default(),
        false,
    );

    assert!(store::load(&mut conn, &id).unwrap().unwrap().cursor.is_none());
    tick_lead(&mut conn, dir.path(), &id, Some(3));

    let cursor = store::load(&mut conn, &id).unwrap().unwrap().cursor.unwrap();
    assert_eq!(cursor.shot_id, "shot-002");
    assert_eq!(cursor.key, "1902-01-01 00:00:00");

    // Ask the selection directly with that cursor: the next page starts after it.
    let page = next_page(
        &mut conn,
        &whole_library(),
        &Narrowing {
            after: Some(&cursor),
            limit: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        page.iter().map(|c| c.shot_id.as_str()).collect::<Vec<_>>(),
        vec!["shot-003", "shot-004"]
    );
}

#[test]
fn a_batch_that_runs_out_of_shots_completes_once_nothing_is_live() {
    let (dir, mut conn) = library();
    seed(&mut conn, 3);
    let id = make_batch(&mut conn, Caps::default(), false);

    let first = tick(&mut conn, dir.path(), &id);
    assert_eq!(first.opened, 3);

    // Query exhausted, but three runs are still going: not done.
    let still = tick(&mut conn, dir.path(), &id);
    assert_eq!(still.state, BatchState::Running);

    diesel::update(runs::table.filter(runs::batch_id.eq(&id)))
        .set(runs::status.eq("completed"))
        .execute(&mut conn)
        .unwrap();
    let done = tick(&mut conn, dir.path(), &id);
    assert_eq!(done.state, BatchState::Completed);
    let row = store::load(&mut conn, &id).unwrap().unwrap();
    assert_eq!(row.state, BatchState::Completed);
    assert!(row.finished_at.is_some());
}

#[test]
fn a_shot_imported_mid_batch_is_picked_up_if_it_sorts_after_the_cursor() {
    let (dir, mut conn) = library();
    seed(&mut conn, 3);
    let id = make_batch(
        &mut conn,
        Caps::default(),
        false,
    );
    tick_lead(&mut conn, dir.path(), &id, Some(2));
    assert_eq!(opened_shot_ids(&mut conn, &id).len(), 2);

    // A shot arrives with a later timestamp than the cursor.
    conn.batch_execute(
        "INSERT INTO shots (id, timestamp) VALUES ('shot-new', '1999-01-01 00:00:00');
         INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, synthetic) \
         VALUES ('file-new', 'shot-new', 'new.jpg', 'hnew', 'image/jpeg', 1, 0);
         UPDATE shots SET main_file_id = 'file-new' WHERE id = 'shot-new';",
    )
    .unwrap();

    diesel::update(runs::table.filter(runs::batch_id.eq(&id)))
        .set(runs::status.eq("completed"))
        .execute(&mut conn)
        .unwrap();
    tick_lead(&mut conn, dir.path(), &id, Some(2));
    assert!(opened_shot_ids(&mut conn, &id).contains(&"shot-new".to_string()));
}

// ── skip_if_generated ──

#[test]
fn skip_if_generated_excludes_shots_that_already_have_output_from_this_line() {
    let (_dir, mut conn) = library();
    seed(&mut conn, 6);

    // shot-000 has a completed run of this line. shot-001 has a file made by
    // the line's workflow but no run row — a one-off enhance from before lines
    // existed. Both count as already done; neither should be offered.
    conn.batch_execute(
        "INSERT INTO runs (id, line_id, shot_id, label, status, stage_count) \
         VALUES ('r-done', 'line-1', 'shot-000', 'x', 'completed', 1);
         INSERT INTO files (id, shot_id, path, hash, source_workflow_id, synthetic) \
         VALUES ('f-gen', 'shot-001', 'gen.png', 'hgen', 'wf-1', 1);",
    )
    .unwrap();

    let narrow = Narrowing {
        skip_line_id: Some("line-1"),
        skip_workflow_id: Some("wf-1"),
        ..Default::default()
    };
    assert_eq!(
        count(&mut conn, &whole_library(), &Narrowing::default()).unwrap(),
        6
    );
    assert_eq!(count(&mut conn, &whole_library(), &narrow).unwrap(), 4);

    let page = next_page(
        &mut conn,
        &whole_library(),
        &Narrowing {
            limit: Some(10),
            ..narrow
        },
    )
    .unwrap();
    let ids: Vec<&str> = page.iter().map(|c| c.shot_id.as_str()).collect();
    assert!(!ids.contains(&"shot-000"));
    assert!(!ids.contains(&"shot-001"));
    assert_eq!(ids.len(), 4);
}

#[test]
fn a_failed_run_does_not_count_as_already_generated() {
    // Only a *completed* run is output. A batch that skipped shots whose run
    // failed would make a retry-everything impossible.
    let (_dir, mut conn) = library();
    seed(&mut conn, 3);
    conn.batch_execute(
        "INSERT INTO runs (id, line_id, shot_id, label, status, stage_count) \
         VALUES ('r-bad', 'line-1', 'shot-000', 'x', 'failed', 1);",
    )
    .unwrap();
    assert_eq!(
        count(
            &mut conn,
            &whole_library(),
            &Narrowing {
                skip_line_id: Some("line-1"),
                skip_workflow_id: Some("wf-1"),
                ..Default::default()
            }
        )
        .unwrap(),
        3
    );
}

#[test]
fn a_feeding_batch_honours_skip_if_generated_and_redo_ignores_it() {
    let (dir, mut conn) = library();
    seed(&mut conn, 4);
    conn.batch_execute(
        "INSERT INTO runs (id, line_id, shot_id, label, status, stage_count) \
         VALUES ('r-done', 'line-1', 'shot-000', 'x', 'completed', 1);",
    )
    .unwrap();

    let skipping = make_batch(&mut conn, Caps::default(), true);
    tick(&mut conn, dir.path(), &skipping);
    let ids = opened_shot_ids(&mut conn, &skipping);
    assert_eq!(ids.len(), 3);
    assert!(!ids.contains(&"shot-000".to_string()));

    let redo = make_batch(&mut conn, Caps::default(), false);
    tick(&mut conn, dir.path(), &redo);
    let ids = opened_shot_ids(&mut conn, &redo);
    assert_eq!(ids.len(), 4);
    assert!(ids.contains(&"shot-000".to_string()));
}

// ── Caps ──

#[test]
fn the_daily_cap_stops_the_feed_and_names_itself() {
    let (dir, mut conn) = library();
    seed(&mut conn, 30);
    // Two tasks a day, one task a shot: two shots and then a pause.
    let id = make_batch(
        &mut conn,
        Caps {
            daily_task_cap: Some(2),
            ..Default::default()
        },
        false,
    );

    let first = tick(&mut conn, dir.path(), &id);
    assert_eq!(first.opened, 2);
    stamp_tasks_today(&mut conn);

    let second = tick(&mut conn, dir.path(), &id);
    assert_eq!(second.opened, 0);
    assert_eq!(second.state, BatchState::Paused);
    assert_eq!(second.paused_reason, Some("daily_cap"));

    let row = store::load(&mut conn, &id).unwrap().unwrap();
    assert_eq!(row.state, BatchState::Paused);
    assert_eq!(row.paused_reason.as_deref(), Some("daily_cap"));
    // And it is a *pause*: nothing is finished and the cursor is intact.
    assert!(row.finished_at.is_none());
    assert_eq!(row.cursor.unwrap().shot_id, "shot-001");
}

#[test]
fn a_paused_batch_resumes_by_itself_when_the_cap_stops_biting() {
    let (dir, mut conn) = library();
    seed(&mut conn, 30);
    let id = make_batch(
        &mut conn,
        Caps {
            daily_task_cap: Some(2),
            ..Default::default()
        },
        false,
    );
    tick(&mut conn, dir.path(), &id);
    stamp_tasks_today(&mut conn);
    assert_eq!(tick(&mut conn, dir.path(), &id).state, BatchState::Paused);

    // Tomorrow. The cap counts tasks created since local midnight, so backdating
    // yesterday's tasks is the same thing as the clock moving on.
    conn.batch_execute("UPDATE enhancement_tasks SET created_at = '2020-01-01 00:00:00';")
        .unwrap();

    let resumed = tick(&mut conn, dir.path(), &id);
    assert_eq!(resumed.state, BatchState::Running);
    assert_eq!(resumed.opened, 2);
    let row = store::load(&mut conn, &id).unwrap().unwrap();
    assert_eq!(row.paused_reason, None);
}

#[test]
fn a_window_paces_the_batch_without_ever_starting_it() {
    let (dir, mut conn) = library();
    seed(&mut conn, 10);
    // Midnight to seven.
    let id = make_batch(
        &mut conn,
        Caps {
            window: Some((0, 7 * 60)),
            ..Default::default()
        },
        false,
    );

    let midday = tick_full(&mut conn, dir.path(), &id, 12, Some(2));
    assert_eq!(midday.opened, 0);
    assert_eq!(midday.paused_reason, Some("window"));

    let overnight = tick_full(&mut conn, dir.path(), &id, 3, Some(2));
    assert_eq!(overnight.opened, 2);
    assert_eq!(overnight.state, BatchState::Running);
}

#[test]
fn the_disk_floor_pauses_the_feed_before_the_volume_fills() {
    let (dir, mut conn) = library();
    seed(&mut conn, 10);
    // A floor of i64::MAX is above any real volume, so the floor always bites —
    // which is the only way to test this without filling a disk.
    let id = make_batch(
        &mut conn,
        Caps {
            disk_floor_bytes: Some(i64::MAX),
            ..Default::default()
        },
        false,
    );

    let fed = tick(&mut conn, dir.path(), &id);
    assert_eq!(fed.opened, 0);
    assert_eq!(fed.paused_reason, Some("disk_floor"));
    assert_eq!(
        runs::table
            .filter(runs::batch_id.eq(&id))
            .count()
            .get_result::<i64>(&mut conn)
            .unwrap(),
        0
    );

    // And the reading itself is real: the temp dir's volume has *some* space.
    let free = store::free_disk_bytes(dir.path());
    assert!(free.is_some(), "statvfs should read a tempdir's volume");
    assert!(free.unwrap() > 0);
}

// ── The outstanding-hold cap ──

#[test]
fn too_many_held_runs_pauses_the_feed() {
    let (dir, mut conn) = library();
    seed(&mut conn, 30);
    let id = make_batch(
        &mut conn,
        Caps {
            max_outstanding_holds: Some(3),
            ..Default::default()
        },
        false,
    );

    let first = tick_lead(&mut conn, dir.path(), &id, Some(4));
    assert_eq!(first.opened, 4);

    // Three of them park at a hold point.
    let opened: Vec<String> = runs::table
        .filter(runs::batch_id.eq(&id))
        .select(runs::id)
        .load(&mut conn)
        .unwrap();
    diesel::update(runs::table.filter(runs::id.eq_any(&opened[..3])))
        .set((runs::status.eq("held"), runs::held_at_stage.eq(0)))
        .execute(&mut conn)
        .unwrap();

    let paused = tick_lead(&mut conn, dir.path(), &id, Some(4));
    assert_eq!(paused.opened, 0);
    assert_eq!(paused.paused_reason, Some("holds"));

    // A verdict on one of them, and the feed comes back on its own.
    diesel::update(runs::table.filter(runs::id.eq(&opened[0])))
        .set((
            runs::status.eq("completed"),
            runs::held_at_stage.eq(None::<i32>),
        ))
        .execute(&mut conn)
        .unwrap();
    let resumed = tick_lead(&mut conn, dir.path(), &id, Some(4));
    assert_eq!(resumed.state, BatchState::Running);
    assert_eq!(resumed.opened, 1);
}

#[test]
fn a_held_run_with_a_null_stage_still_counts_against_the_cap() {
    // FR5c's author flagged `status='held'` and `held_at_stage` as the one pair
    // of markers in that feature that could in principle disagree. Nothing
    // writes such a row — but if one existed, a cap that quietly ignored it
    // would let the mountain grow past the limit it exists to enforce. So the
    // cap counts parked runs and never asks where they are parked.
    let (dir, mut conn) = library();
    seed(&mut conn, 30);
    let id = make_batch(
        &mut conn,
        Caps {
            max_outstanding_holds: Some(2),
            ..Default::default()
        },
        false,
    );
    tick_lead(&mut conn, dir.path(), &id, Some(3));

    let opened: Vec<String> = runs::table
        .filter(runs::batch_id.eq(&id))
        .select(runs::id)
        .load(&mut conn)
        .unwrap();
    // One ordinary hold, one with no stage on it at all.
    diesel::update(runs::table.filter(runs::id.eq(&opened[0])))
        .set((runs::status.eq("held"), runs::held_at_stage.eq(1)))
        .execute(&mut conn)
        .unwrap();
    diesel::update(runs::table.filter(runs::id.eq(&opened[1])))
        .set((
            runs::status.eq("held"),
            runs::held_at_stage.eq(None::<i32>),
        ))
        .execute(&mut conn)
        .unwrap();

    let counts = store::run_counts(&mut conn, &id).unwrap();
    assert_eq!(counts.held, 2, "a NULL stage is still a held run");

    let paused = tick_lead(&mut conn, dir.path(), &id, Some(3));
    assert_eq!(paused.paused_reason, Some("holds"));
    assert_eq!(paused.opened, 0);
}

#[test]
fn held_runs_count_as_live_so_a_batch_is_not_done_while_one_waits() {
    let (dir, mut conn) = library();
    seed(&mut conn, 2);
    let id = make_batch(&mut conn, Caps::default(), false);
    tick(&mut conn, dir.path(), &id);

    let opened: Vec<String> = runs::table
        .filter(runs::batch_id.eq(&id))
        .select(runs::id)
        .load(&mut conn)
        .unwrap();
    diesel::update(runs::table.filter(runs::id.eq(&opened[0])))
        .set(runs::status.eq("completed"))
        .execute(&mut conn)
        .unwrap();
    diesel::update(runs::table.filter(runs::id.eq(&opened[1])))
        .set((runs::status.eq("held"), runs::held_at_stage.eq(0)))
        .execute(&mut conn)
        .unwrap();

    let fed = tick(&mut conn, dir.path(), &id);
    assert_eq!(
        fed.state,
        BatchState::Running,
        "a batch with a run still waiting on a person is not finished"
    );
}

// ── STOP ──

#[test]
fn stop_halts_mid_batch_and_leaves_no_half_queued_run() {
    let (dir, mut conn) = library();
    seed(&mut conn, 50);
    let id = make_batch(
        &mut conn,
        Caps::default(),
        false,
    );
    tick_lead(&mut conn, dir.path(), &id, Some(6));
    assert_eq!(opened_shot_ids(&mut conn, &id).len(), 6);

    let stopped = super::feed::stop(&mut conn, dir.path(), &id).unwrap();
    assert_eq!(stopped.cancelled_runs, 6);

    let row = store::load(&mut conn, &id).unwrap().unwrap();
    assert_eq!(row.state, BatchState::Stopped);
    assert!(row.finished_at.is_some());

    // Every run of the batch is terminal, and every task of them too. Nothing
    // is left half-queued.
    let live: i64 = runs::table
        .filter(runs::batch_id.eq(&id))
        .filter(runs::status.eq_any(crate::comfyui::RunState::live()))
        .count()
        .get_result(&mut conn)
        .unwrap();
    assert_eq!(live, 0);

    let unfinished: i64 = enhancement_tasks::table
        .inner_join(runs::table.on(enhancement_tasks::run_id.eq(runs::id.nullable())))
        .filter(runs::batch_id.eq(&id))
        .filter(enhancement_tasks::status.ne_all(&["completed", "failed", "cancelled"]))
        .count()
        .get_result(&mut conn)
        .unwrap();
    assert_eq!(unfinished, 0, "STOP must leave no task mid-flight");

    // The 44 shots it never got to were never rows, and never will be.
    let all: i64 = runs::table
        .filter(runs::batch_id.eq(&id))
        .count()
        .get_result(&mut conn)
        .unwrap();
    assert_eq!(all, 6);

    // And a tick after STOP opens nothing: the batch is out of `feeding`.
    let feeding = store::feeding(&mut conn).unwrap();
    assert!(feeding.iter().all(|b| b.id != id));
}

#[test]
fn stop_names_the_prompts_comfyui_still_has_queued() {
    let (dir, mut conn) = library();
    seed(&mut conn, 5);
    let id = make_batch(&mut conn, Caps::default(), false);
    tick(&mut conn, dir.path(), &id);

    // Two of the batch's tasks reached ComfyUI; one already finished.
    let tasks: Vec<String> = enhancement_tasks::table
        .inner_join(runs::table.on(enhancement_tasks::run_id.eq(runs::id.nullable())))
        .filter(runs::batch_id.eq(&id))
        .select(enhancement_tasks::id)
        .load(&mut conn)
        .unwrap();
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&tasks[0])))
        .set((
            enhancement_tasks::comfyui_prompt_id.eq("prompt-live"),
            enhancement_tasks::status.eq("processing"),
        ))
        .execute(&mut conn)
        .unwrap();
    diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&tasks[1])))
        .set((
            enhancement_tasks::comfyui_prompt_id.eq("prompt-done"),
            enhancement_tasks::status.eq("completed"),
        ))
        .execute(&mut conn)
        .unwrap();

    let stopped = super::feed::stop(&mut conn, dir.path(), &id).unwrap();
    assert_eq!(stopped.prompt_ids, vec!["prompt-live".to_string()]);
}

#[test]
fn stop_cancels_a_held_run_through_its_hold() {
    let (dir, mut conn) = library();
    seed(&mut conn, 2);
    let id = make_batch(&mut conn, Caps::default(), false);
    tick(&mut conn, dir.path(), &id);

    let opened: Vec<String> = runs::table
        .filter(runs::batch_id.eq(&id))
        .select(runs::id)
        .load(&mut conn)
        .unwrap();
    diesel::update(runs::table.filter(runs::id.eq(&opened[0])))
        .set((runs::status.eq("held"), runs::held_at_stage.eq(0)))
        .execute(&mut conn)
        .unwrap();

    let stopped = super::feed::stop(&mut conn, dir.path(), &id).unwrap();
    assert_eq!(stopped.cancelled_runs, 2);
    let statuses: Vec<String> = runs::table
        .filter(runs::batch_id.eq(&id))
        .select(runs::status)
        .load(&mut conn)
        .unwrap();
    assert!(statuses.iter().all(|s| s == "cancelled"));
}

#[test]
fn stop_survives_a_held_run_with_no_stage_to_hold() {
    // The same should-never-happen row as above, on the path where refusing is
    // worst: stopping a run is the thing that must always work.
    let (dir, mut conn) = library();
    seed(&mut conn, 1);
    let id = make_batch(&mut conn, Caps::default(), false);
    tick(&mut conn, dir.path(), &id);

    diesel::update(runs::table.filter(runs::batch_id.eq(&id)))
        .set((
            runs::status.eq("held"),
            runs::held_at_stage.eq(None::<i32>),
        ))
        .execute(&mut conn)
        .unwrap();

    let stopped = super::feed::stop(&mut conn, dir.path(), &id).unwrap();
    assert_eq!(stopped.cancelled_runs, 1);
    let status: String = runs::table
        .filter(runs::batch_id.eq(&id))
        .select(runs::status)
        .first(&mut conn)
        .unwrap();
    assert_eq!(status, "cancelled");
}

// ── Attribution and estimates ──

#[test]
fn a_batchs_runs_carry_its_id_and_a_hand_started_run_carries_none() {
    let (dir, mut conn) = library();
    seed(&mut conn, 2);
    let id = make_batch(&mut conn, Caps::default(), false);
    tick(&mut conn, dir.path(), &id);

    crate::comfyui::runs::start_line_run(
        &mut conn,
        "line-1",
        "shot-000",
        &Default::default(),
    )
    .unwrap();

    let unattributed: i64 = runs::table
        .filter(runs::batch_id.is_null())
        .count()
        .get_result(&mut conn)
        .unwrap();
    assert_eq!(unattributed, 1);
    assert_eq!(store::run_counts(&mut conn, &id).unwrap().opened(), 2);
}

#[test]
fn a_stage_this_library_has_never_run_is_costed_from_a_guess() {
    let (_dir, mut conn) = library();
    seed(&mut conn, 1);
    let costs = store::stage_costs(&mut conn, "line-1").unwrap();
    assert_eq!(costs.len(), 1);
    assert!(!costs[0].seconds_measured);
    assert!(!costs[0].bytes_measured);
    assert_eq!(costs[0].seconds, super::plan::GUESS_IMAGE_SECONDS);
    // The only stage of a line is its last, so its output is the product.
    assert!(costs[0].keeps_output);
}

#[test]
fn a_stage_this_library_has_run_is_costed_from_what_it_actually_took() {
    let (_dir, mut conn) = library();
    seed(&mut conn, 1);
    // Three completed tasks at 10s, 30s and 200s, and three output files.
    // The median is 30s, not the 80s an average would report.
    conn.batch_execute(
        "INSERT INTO enhancement_tasks (id, shot_id, workflow_id, status, started_at, completed_at) \
         VALUES ('t1','shot-000','wf-1','completed','2026-01-01 00:00:00','2026-01-01 00:00:10'),
                ('t2','shot-000','wf-1','completed','2026-01-01 00:00:00','2026-01-01 00:00:30'),
                ('t3','shot-000','wf-1','completed','2026-01-01 00:00:00','2026-01-01 00:03:20');
         INSERT INTO files (id, shot_id, path, hash, source_workflow_id, file_size, synthetic) \
         VALUES ('g1','shot-000','g1.png','h1','wf-1',1000,1),
                ('g2','shot-000','g2.png','h2','wf-1',3000,1),
                ('g3','shot-000','g3.png','h3','wf-1',90000,1);",
    )
    .unwrap();

    let costs = store::stage_costs(&mut conn, "line-1").unwrap();
    assert!(costs[0].seconds_measured);
    assert!(costs[0].bytes_measured);
    assert!(
        (costs[0].seconds - 30.0).abs() < 0.5,
        "expected the median 30s, got {}",
        costs[0].seconds
    );
    assert_eq!(costs[0].bytes, 3000);
}

#[test]
fn the_estimate_a_batch_is_created_with_is_the_one_it_reads_back() {
    let (_dir, mut conn) = library();
    seed(&mut conn, 12);
    let (estimate, _) = store::estimate_for(&mut conn, "line-1", 12, 4).unwrap();
    assert_eq!(estimate.to_run, 8);
    assert_eq!(estimate.tasks, 8);

    let id = store::create(
        &mut conn,
        "line-1",
        "x",
        &whole_library(),
        None,
        true,
        &Caps::default(),
        &estimate,
    )
    .unwrap();
    let row = store::load(&mut conn, &id).unwrap().unwrap();
    assert_eq!(row.matched_total, Some(12));
    assert_eq!(row.skipped_total, Some(4));
    assert_eq!(row.est_tasks, Some(8));
}

#[test]
fn a_batch_round_trips_its_selection_and_caps_through_the_row() {
    let (_dir, mut conn) = library();
    seed(&mut conn, 1);
    let selection = Selection::Query {
        query: ShotsQuery {
            person_id: Some("p-1".into()),
            to: Some("1990-01-01".into()),
            ..Default::default()
        },
    };
    let caps = Caps {
        daily_task_cap: Some(400),
        window: Some((0, 7 * 60)),
        disk_floor_bytes: Some(50_000_000_000),
        max_outstanding_holds: Some(200),
        lead: None,
    };
    let (estimate, _) = store::estimate_for(&mut conn, "line-1", 0, 0).unwrap();
    let id = store::create(
        &mut conn,
        "line-1",
        "Grandma",
        &selection,
        Some(r#"{"0":{"parameters":{},"text_overrides":{}}}"#),
        true,
        &caps,
        &estimate,
    )
    .unwrap();

    let row = store::load(&mut conn, &id).unwrap().unwrap();
    assert_eq!(row.selection, selection);
    assert_eq!(row.caps.daily_task_cap, Some(400));
    assert_eq!(row.caps.window, Some((0, 420)));
    assert_eq!(row.caps.disk_floor_bytes, Some(50_000_000_000));
    assert_eq!(row.caps.max_outstanding_holds, Some(200));
    assert!(row.stage_values.is_some());
    assert!(row.skip_if_generated);
}

#[test]
fn a_saved_selection_is_a_query_and_a_line_and_nothing_that_fires() {
    let (_dir, mut conn) = library();
    seed(&mut conn, 1);
    let selection = Selection::Query {
        query: ShotsQuery {
            q: Some("grandma".into()),
            ..Default::default()
        },
    };
    let id = store::save_selection(
        &mut conn,
        "Grandma, pre-1990",
        Some("line-1"),
        &selection,
        Some(r#"{"daily_task_cap":400}"#),
        true,
    )
    .unwrap();

    let saved = store::list_selections(&mut conn).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].selection, selection);
    assert_eq!(saved[0].line_id.as_deref(), Some("line-1"));

    // Saving one starts nothing.
    let batches: i64 = crate::schema::batches::table
        .count()
        .get_result(&mut conn)
        .unwrap();
    assert_eq!(batches, 0);

    assert_eq!(store::delete_selection(&mut conn, &id).unwrap(), 1);
    assert!(store::list_selections(&mut conn).unwrap().is_empty());
}

#[test]
fn a_shot_the_line_refuses_is_skipped_rather_than_stopping_the_batch() {
    let (dir, mut conn) = library();
    seed(&mut conn, 3);
    // shot-001 has no original file at all, which `admits_source` refuses.
    conn.batch_execute(
        "DELETE FROM files WHERE shot_id = 'shot-001';
         UPDATE shots SET main_file_id = NULL WHERE id = 'shot-001';",
    )
    .unwrap();

    let id = make_batch(&mut conn, Caps::default(), false);
    let fed = tick(&mut conn, dir.path(), &id);
    assert_eq!(fed.opened, 2);
    assert_eq!(fed.refused, 1);
    // And the cursor moved past the refused shot, so it is not re-offered.
    assert_eq!(
        store::load(&mut conn, &id)
            .unwrap()
            .unwrap()
            .cursor
            .unwrap()
            .shot_id,
        "shot-002"
    );
    let again = tick(&mut conn, dir.path(), &id);
    assert_eq!(again.opened, 0);
    assert_eq!(again.refused, 0);
}

#[test]
fn an_explicit_id_list_walks_the_same_cursor_as_a_query() {
    let (dir, mut conn) = library();
    seed(&mut conn, 10);
    let (estimate, _) = store::estimate_for(&mut conn, "line-1", 0, 0).unwrap();
    let selection = Selection::Ids {
        ids: vec![
            "shot-007".into(),
            "shot-002".into(),
            "shot-005".into(),
        ],
    };
    let id = store::create(
        &mut conn,
        "line-1",
        "three of them",
        &selection,
        None,
        false,
        &Caps::default(),
        &estimate,
    )
    .unwrap();

    // Cursor order, not the order they were listed in.
    tick_lead(&mut conn, dir.path(), &id, Some(2));
    assert_eq!(
        opened_shot_ids(&mut conn, &id),
        vec!["shot-002", "shot-005"]
    );

    diesel::update(runs::table.filter(runs::batch_id.eq(&id)))
        .set(runs::status.eq("completed"))
        .execute(&mut conn)
        .unwrap();
    tick_lead(&mut conn, dir.path(), &id, Some(2));
    assert_eq!(
        opened_shot_ids(&mut conn, &id),
        vec!["shot-002", "shot-005", "shot-007"]
    );
}

#[test]
fn a_batch_whose_line_was_deleted_ends_rather_than_sticking() {
    let (dir, mut conn) = library();
    seed(&mut conn, 5);
    let id = make_batch(&mut conn, Caps::default(), false);
    conn.batch_execute("DELETE FROM line_stages WHERE line_id = 'line-1';")
        .unwrap();
    let fed = tick(&mut conn, dir.path(), &id);
    assert_eq!(fed.state, BatchState::Completed);
    assert_eq!(fed.opened, 0);
}

#[test]
fn feed_batches_walks_every_feeding_batch_and_skips_the_stopped_one() {
    let (dir, mut conn) = library();
    seed(&mut conn, 10);
    let a = make_batch(&mut conn, Caps::default(), false);
    let b = make_batch(&mut conn, Caps::default(), false);
    super::feed::stop(&mut conn, dir.path(), &b).unwrap();

    // The production entry point, with the production lead and chunk — the one
    // the worker actually calls each tick.
    let fed = super::feed_batches(&mut conn, dir.path());
    assert_eq!(fed.len(), 1, "a stopped batch must not be fed");
    assert_eq!(fed[0].batch_id, a);
    assert_eq!(fed[0].opened, 10);
    assert_eq!(opened_shot_ids(&mut conn, &b).len(), 0);
}

#[test]
fn the_cursor_is_written_as_a_pair_or_not_at_all() {
    // A half-cursor would resume from the wrong place in silence, so the reader
    // treats one column without the other as "not started".
    let (_dir, mut conn) = library();
    seed(&mut conn, 3);
    let id = make_batch(&mut conn, Caps::default(), false);
    diesel::update(crate::schema::batches::table.filter(crate::schema::batches::id.eq(&id)))
        .set(crate::schema::batches::cursor_key.eq("1975"))
        .execute(&mut conn)
        .unwrap();
    assert!(store::load(&mut conn, &id).unwrap().unwrap().cursor.is_none());

    diesel::update(crate::schema::batches::table.filter(crate::schema::batches::id.eq(&id)))
        .set(crate::schema::batches::cursor_shot_id.eq("shot-001"))
        .execute(&mut conn)
        .unwrap();
    assert_eq!(
        store::load(&mut conn, &id).unwrap().unwrap().cursor,
        Some(Cursor::new("1975", "shot-001"))
    );
}
