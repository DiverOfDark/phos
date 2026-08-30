//! FR8 against a real migrated SQLite file, and against the real feeder and
//! the real advance pass.
//!
//! The pure ordering is tested beside itself in [`crate::comfyui::queue`] and
//! the cap arithmetic in [`crate::comfyui::batch::plan`]. What is here is the
//! set of claims that only a running system can settle, because reading the
//! code has repeatedly disagreed with running it:
//!
//! * a mixed queue really does drain every task of one workflow before any of
//!   the next — asserted as the *sequence of models the GPU would be asked to
//!   load*, which is the strongest form the model-locality claim can take
//!   without a GPU;
//! * a task queued last by a person really is dispatched first;
//! * a run partway down a chain really does finish while a batch is being fed
//!   new shots every single tick.
//!
//! Nothing here talks to ComfyUI. `land` is what a stage looks like when it
//! works — a file in the library and a completed task pointing at it — which is
//! exactly the state the completion path leaves behind, so the advance pass
//! walks runs along for real.

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;

use super::dispatch::{pending_tasks, DISPATCH_CHUNK};
use crate::api::shots::ShotsQuery;
use crate::comfyui::batch::plan::Caps;
use crate::comfyui::batch::selection::Selection;
use crate::comfyui::batch::{feed, store};
use crate::schema::enhancement_tasks;

/// A graph that takes a still and saves a still.
const IMAGE_GRAPH: &str = r#"{
    "3": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 20, "cfg": 8.0}},
    "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
    "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "out", "images": ["3", 0]}}
}"#;

/// Any time at all. Only the ordering is under test, and the drain order does
/// not read the clock.
const NOW: &str = "2099-01-01 00:00:00";

struct Farm {
    dir: tempfile::TempDir,
    conn: SqliteConnection,
    shots: usize,
}

impl Farm {
    /// A library with `stages` workflows chained into one line, and `shots`
    /// photographs to point it at.
    fn new(stages: usize, shots: usize) -> Farm {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).unwrap();
        let conn = crate::db::open_diesel_connection(&db_path).unwrap();
        let mut farm = Farm {
            dir,
            conn,
            shots: 0,
        };
        let workflows: String = (0..stages)
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
        let line_stages: String = (0..stages)
            .map(|i| {
                format!(
                    "INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, keep_output) \
                     VALUES ('st-{}', 'line-1', {}, 'wf-{}', 1);",
                    i, i, i
                )
            })
            .collect();
        farm.sql(&format!(
            "{workflows}
             INSERT INTO production_lines (id, name) VALUES ('line-1', 'Restore');
             {line_stages}"
        ));
        farm.import(shots);
        farm
    }

    fn sql(&mut self, sql: &str) {
        self.conn.batch_execute(sql).unwrap();
    }

    fn root(&self) -> std::path::PathBuf {
        self.dir.path().to_path_buf()
    }

    /// `n` more photographs, sorting after everything already in the library —
    /// which is what makes a query-selected batch pick them up.
    fn import(&mut self, n: usize) {
        for _ in 0..n {
            let i = self.shots;
            self.shots += 1;
            self.sql(&format!(
                "INSERT INTO shots (id, timestamp) \
                 VALUES ('shot-{i:04}', '{:04}-01-01 00:00:00');
                 INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, synthetic) \
                 VALUES ('file-{i:04}', 'shot-{i:04}', 'p{i:04}.jpg', 'h{i:04}', \
                         'image/jpeg', 1, 0);
                 UPDATE shots SET main_file_id = 'file-{i:04}' WHERE id = 'shot-{i:04}';",
                2000 + i,
                i = i
            ));
        }
    }

    /// Send the whole library to the line.
    fn send(&mut self, label: &str) -> String {
        let (estimate, _) = store::estimate_for(&mut self.conn, "line-1", 0, 0).unwrap();
        store::create(
            &mut self.conn,
            "line-1",
            label,
            &Selection::Query {
                query: ShotsQuery::default(),
            },
            None,
            false,
            &Caps::default(),
            &estimate,
        )
        .unwrap()
    }

    /// One feeder tick for one batch, with the lead handed in — the same way
    /// FR7's own tests do it, because the lead is a property of the system
    /// rather than a per-batch column.
    fn feed(&mut self, batch_id: &str, lead: i64) -> feed::FedBatch {
        let mut batch = store::load(&mut self.conn, batch_id).unwrap().unwrap();
        batch.caps.lead = Some(lead);
        let now = chrono::NaiveDate::from_ymd_opt(2026, 8, 31)
            .unwrap()
            .and_hms_opt(3, 0, 0)
            .unwrap();
        let root = self.root();
        feed::feed_one(&mut self.conn, &root, &batch, now).unwrap()
    }

    /// The next pass's worth of tasks, in the order the dispatcher would take
    /// them. This is the production query, not a paraphrase of it.
    fn next_pass(&mut self, limit: i64) -> Vec<crate::comfyui::queue::DrainKey> {
        pending_tasks(&mut self.conn, NOW, limit)
            .unwrap()
            .iter()
            .map(|t| t.drain_key())
            .collect()
    }

    /// What a stage looks like when it works: a file in the library, and the
    /// task pointing at it.
    fn land(&mut self, task_id: &str) {
        let shot_id: String = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(task_id))
            .select(enhancement_tasks::shot_id)
            .first(&mut self.conn)
            .unwrap();
        let file_id = format!("out-{}", &task_id[..8]);
        let name = format!("{}.png", file_id);
        std::fs::write(self.dir.path().join(&name), b"png").unwrap();
        self.sql(&format!(
            "INSERT INTO files (id, shot_id, path, hash, mime_type, is_original, synthetic) \
             VALUES ('{file_id}', '{shot_id}', '{name}', '{file_id}', 'image/png', 0, 1);
             UPDATE enhancement_tasks \
             SET status = 'completed', output_file_id = '{file_id}', \
                 completed_at = '2026-08-31 03:00:00' \
             WHERE id = '{task_id}';"
        ));
    }

    fn advance(&mut self) {
        let root = self.root();
        super::advance::advance_runs(&mut self.conn, &root);
    }

    fn pending_count(&mut self) -> i64 {
        enhancement_tasks::table
            .filter(enhancement_tasks::status.eq("pending"))
            .count()
            .get_result(&mut self.conn)
            .unwrap()
    }
}

/// Collapse a sequence of workflow ids into `(workflow, run length)` blocks.
///
/// This is the model-locality claim in the only form this tree can assert it:
/// how many times the GPU would be asked to swap weights. The saving itself is
/// ComfyUI's and a card's, and nothing here measures it.
fn blocks(workflows: &[String]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for wf in workflows {
        match out.last_mut() {
            Some((last, n)) if last == wf => *n += 1,
            _ => out.push((wf.clone(), 1)),
        }
    }
    out
}

// ── The core claim ──

#[test]
fn a_batch_drains_every_task_of_one_workflow_before_any_of_the_next() {
    // Six shots down a three-stage line, driven by the real feeder, the real
    // drain order and the real advance pass. Eighteen tasks, and the question
    // is what order the GPU is asked for them in.
    let mut farm = Farm::new(3, 6);
    let batch = farm.send("Restore · whole library");

    let mut dispatched: Vec<String> = Vec::new();
    for _ in 0..200 {
        farm.feed(&batch, 6);
        let pass = farm.next_pass(DISPATCH_CHUNK);
        if pass.is_empty() && farm.pending_count() == 0 {
            break;
        }
        for key in pass {
            dispatched.push(key.workflow_id.clone());
            farm.land(&key.id);
        }
        farm.advance();
    }

    assert_eq!(dispatched.len(), 18, "every task ran exactly once");
    // One block per stage, and the model is asked for twice — not eighteen
    // times. By `created_at` this same batch interleaves: the runs reach stage
    // 2 one at a time, so the queue alternates wf-1, wf-0, wf-1, wf-0…
    assert_eq!(
        blocks(&dispatched),
        vec![
            ("wf-0".to_string(), 6),
            ("wf-1".to_string(), 6),
            ("wf-2".to_string(), 6),
        ]
    );
}

#[test]
fn a_queue_holding_two_stages_at_once_drains_the_lower_one_first() {
    // The order on its own, with the wave gate deliberately out of the way. Two
    // batches over the same line: A is parked at its last stage, B has only
    // just started. A's tasks were written *first* and are strictly older, so
    // by `created_at` — the order this replaced — they would go first and the
    // model would swap between wf-2 and wf-0 for the rest of the batch.
    let mut farm = Farm::new(3, 8);
    let a = farm.send("Batch A");
    farm.feed(&a, 4);
    for _ in 0..2 {
        for key in farm.next_pass(1000) {
            farm.land(&key.id);
        }
        farm.advance();
    }
    let older = farm.next_pass(1000);
    assert_eq!(older.len(), 4);
    assert!(older.iter().all(|k| k.stage_idx == 2));
    // Strictly older, so this is a claim about stage and not about a tie.
    farm.sql(
        "UPDATE enhancement_tasks SET created_at = '2020-01-01 00:00:00' \
         WHERE status = 'pending';",
    );

    let b = farm.send("Batch B");
    farm.feed(&b, 4);

    let pass = farm.next_pass(1000);
    let stages: Vec<i32> = pass.iter().map(|k| k.stage_idx).collect();
    assert_eq!(
        stages,
        vec![0, 0, 0, 0, 2, 2, 2, 2],
        "every first-stage task before any last-stage one"
    );
    assert_eq!(
        blocks(
            &pass
                .iter()
                .map(|k| k.workflow_id.clone())
                .collect::<Vec<_>>()
        ),
        vec![("wf-0".to_string(), 4), ("wf-2".to_string(), 4)],
        "two model loads, not eight"
    );
}

#[test]
fn a_second_wave_is_a_second_pass_and_not_an_interleaving() {
    // Twelve shots and a lead of four: three waves, each walking all three
    // stages before the next wave opens. Nine model loads for thirty-six tasks.
    let mut farm = Farm::new(3, 12);
    let batch = farm.send("Restore · whole library");

    let mut dispatched: Vec<String> = Vec::new();
    for _ in 0..400 {
        farm.feed(&batch, 4);
        let pass = farm.next_pass(DISPATCH_CHUNK);
        if pass.is_empty() && farm.pending_count() == 0 {
            break;
        }
        for key in pass {
            dispatched.push(key.workflow_id.clone());
            farm.land(&key.id);
        }
        farm.advance();
    }

    assert_eq!(dispatched.len(), 36);
    let shape = blocks(&dispatched);
    assert_eq!(
        shape,
        vec![
            ("wf-0".to_string(), 4),
            ("wf-1".to_string(), 4),
            ("wf-2".to_string(), 4),
            ("wf-0".to_string(), 4),
            ("wf-1".to_string(), 4),
            ("wf-2".to_string(), 4),
            ("wf-0".to_string(), 4),
            ("wf-1".to_string(), 4),
            ("wf-2".to_string(), 4),
        ],
        "each wave walks the line as a unit"
    );
    // The number that matters: nine loads rather than one per task.
    assert_eq!(shape.len(), 9);
}

// ── Priority ──

#[test]
fn an_interactive_task_queued_last_is_dispatched_first() {
    // The inversion FR8 exists to prevent, driven for real: a batch fills the
    // queue, and *then* somebody presses Enhance on one photograph.
    let mut farm = Farm::new(1, 40);
    let batch = farm.send("Restore · whole library");
    // Two ticks, because one tick opens a chunk and a chunk is 25.
    farm.feed(&batch, 40);
    farm.feed(&batch, 40);
    assert_eq!(farm.pending_count(), 40, "the farm is queued and waiting");

    farm.import(1);
    let mine = crate::comfyui::runs::start_line_run(
        &mut farm.conn,
        "line-1",
        &format!("shot-{:04}", farm.shots - 1),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(mine.task_ids.len(), 1);

    let pass = farm.next_pass(DISPATCH_CHUNK);
    assert_eq!(
        pass[0].id, mine.task_ids[0],
        "the click goes first, not forty-first"
    );
    assert_eq!(
        pass[0].priority,
        crate::comfyui::queue::Priority::Interactive
    );
    // And every other task in the pass is the farm's, so priority is a fast
    // lane and not a reordering of everything.
    for key in &pass[1..] {
        assert_eq!(key.priority, crate::comfyui::queue::Priority::Batch);
    }
}

#[test]
fn a_batchs_run_is_batch_work_at_every_stage_of_its_line() {
    // A run's hurry is the run's, not the stage's. Without this a batch's
    // stage-2 task would inherit the column default — `interactive` — and the
    // farm would quietly promote itself as it advanced.
    let mut farm = Farm::new(3, 2);
    let batch = farm.send("Restore · whole library");
    farm.feed(&batch, 2);

    for _ in 0..3 {
        for key in farm.next_pass(DISPATCH_CHUNK) {
            assert_eq!(
                key.priority,
                crate::comfyui::queue::Priority::Batch,
                "{} should still be batch work",
                key
            );
            farm.land(&key.id);
        }
        farm.advance();
    }
}

#[test]
fn the_order_the_database_returns_is_the_order_the_pure_comparator_describes() {
    // The two halves of `comfyui::queue`: the SQL fragments the dispatcher
    // builds its ORDER BY from, and `DrainKey`'s `Ord`. They are written twice
    // so they can be checked against each other, and this is the check.
    let mut farm = Farm::new(3, 8);
    let batch = farm.send("Restore · whole library");
    farm.feed(&batch, 8);

    // Walk the batch halfway so the queue holds a mixture of stages, then add
    // somebody's own run and a legacy row with no stage at all.
    for key in farm.next_pass(4) {
        farm.land(&key.id);
    }
    farm.advance();
    farm.import(1);
    crate::comfyui::runs::start_line_run(
        &mut farm.conn,
        "line-1",
        &format!("shot-{:04}", farm.shots - 1),
        &Default::default(),
    )
    .unwrap();
    farm.sql(
        "INSERT INTO enhancement_tasks (id, shot_id, workflow_id, status, priority) \
         VALUES ('legacy', 'shot-0000', 'wf-2', 'pending', 'batch');",
    );

    let returned = farm.next_pass(1000);
    assert!(returned.len() > 6, "a mixture worth sorting");
    let mut sorted = returned.clone();
    sorted.sort();
    assert_eq!(returned, sorted);
}

// ── Starvation ──

#[test]
fn a_batch_stops_opening_while_its_wave_is_past_the_first_stage() {
    // The `advanced_runs` count, asked of a real database: one run at stage 2
    // is enough to shut the feeder, and finishing the wave opens it again.
    let mut farm = Farm::new(3, 20);
    let batch = farm.send("Restore · whole library");

    assert_eq!(farm.feed(&batch, 8).opened, 8);
    assert_eq!(
        store::advanced_runs(&mut farm.conn, &batch).unwrap(),
        0,
        "a wave still on the first stage has advanced nothing"
    );
    // Even with the lead nowhere near full, nothing new opens once one run has
    // moved on — because a new run would be dispatched in front of it.
    let head = farm.next_pass(1);
    farm.land(&head[0].id);
    farm.advance();
    assert_eq!(store::advanced_runs(&mut farm.conn, &batch).unwrap(), 1);
    assert_eq!(farm.feed(&batch, 64).opened, 0);

    // Land the wave, and the next one opens.
    for _ in 0..40 {
        let pass = farm.next_pass(DISPATCH_CHUNK);
        if pass.is_empty() {
            break;
        }
        for key in pass {
            farm.land(&key.id);
        }
        farm.advance();
    }
    assert_eq!(store::advanced_runs(&mut farm.conn, &batch).unwrap(), 0);
    assert!(farm.feed(&batch, 8).opened > 0, "the next wave opens");
}

#[test]
fn a_straggler_finishes_before_the_next_wave_is_allowed_to_start() {
    // Where the wave gate actually bites, and why it is not just the lead
    // wearing a different hat. Five of six runs complete, which frees five of
    // the six lead slots; the sixth is still on the last stage. Topping the
    // lead back up would put five brand-new *first*-stage tasks in front of it,
    // and the stage order would happily run all five before the run that is one
    // task from done.
    let mut farm = Farm::new(3, 12);
    let batch = farm.send("Restore · whole library");
    farm.feed(&batch, 6);
    for _ in 0..2 {
        for key in farm.next_pass(1000) {
            farm.land(&key.id);
        }
        farm.advance();
    }

    let last_stage = farm.next_pass(1000);
    assert_eq!(last_stage.len(), 6);
    for key in &last_stage[..5] {
        farm.land(&key.id);
    }
    farm.advance();
    let straggler = last_stage[5].id.clone();

    assert_eq!(
        store::advanced_runs(&mut farm.conn, &batch).unwrap(),
        1,
        "one run is still walking"
    );
    assert_eq!(
        farm.feed(&batch, 6).opened,
        0,
        "five free lead slots, and still nothing new opens"
    );
    let pass = farm.next_pass(DISPATCH_CHUNK);
    assert_eq!(pass.len(), 1);
    assert_eq!(pass[0].id, straggler, "the run one task from done goes now");

    // And once it lands, the next wave opens as normal.
    farm.land(&straggler);
    farm.advance();
    assert_eq!(store::advanced_runs(&mut farm.conn, &batch).unwrap(), 0);
    assert_eq!(farm.feed(&batch, 6).opened, 6);
}

#[test]
fn a_run_halfway_down_a_chain_finishes_while_another_batch_is_fed_every_tick() {
    // The starvation case, stated as badly as it can be stated: batch A has
    // runs at the *last* stage of the line, and batch B is fed two freshly
    // imported shots on every single tick — an import watcher that never stops.
    // B's tasks are at stage 1 and outrank A's by the drain order, so if
    // anything starves A it is this.
    //
    // It does not, and the reason is `wave_lead`: while B's own wave is past
    // its first stage B opens nothing, so the supply of stage-1 work that
    // outranks A is finite at every moment and only shrinks.
    let mut farm = Farm::new(3, 4);
    let a = farm.send("Batch A");
    farm.feed(&a, 4);
    // Walk A to its last stage.
    for _ in 0..2 {
        for key in farm.next_pass(1000) {
            farm.land(&key.id);
        }
        farm.advance();
    }
    let stranded: Vec<String> = farm
        .next_pass(1000)
        .into_iter()
        .map(|k| {
            assert_eq!(k.stage_idx, 2, "A is parked at the last stage");
            k.id
        })
        .collect();
    assert_eq!(stranded.len(), 4);

    let b = farm.send("Batch B");
    let mut dispatches = 0usize;
    let mut left = stranded.clone();
    for _ in 0..500 {
        // The import that never stops.
        farm.import(5);
        farm.feed(&b, 4);
        let pass = farm.next_pass(DISPATCH_CHUNK);
        if pass.is_empty() {
            break;
        }
        for key in pass {
            dispatches += 1;
            left.retain(|id| id != &key.id);
            farm.land(&key.id);
        }
        farm.advance();
        if left.is_empty() {
            break;
        }
    }

    assert!(
        left.is_empty(),
        "{} of A's stranded tasks never ran",
        left.len()
    );

    // And "eventually" is a number. B's wave is four runs × three stages, and
    // B cannot open a second one until the first has landed, so A waits behind
    // at most two of B's waves plus its own four tasks. (A's and B's last-stage
    // tasks interleave by id rather than by `created_at`, because SQLite's
    // default stamp has one-second granularity and this test runs faster than
    // that. It does not matter: what is bounded is bounded either way.)
    assert!(
        dispatches <= 2 * (4 * 3) + 4,
        "A waited behind {} dispatches, which is more than two waves of B",
        dispatches
    );

    // The mechanism, said directly. Hundreds of shots were imported while this
    // ran, and B opened a wave and a bit — not one run per shot. That is what
    // stops a continuously-fed batch from producing an endless supply of
    // early-stage work to queue in front of A.
    let b_runs: i64 = crate::schema::runs::table
        .filter(crate::schema::runs::batch_id.eq(&b))
        .count()
        .get_result(&mut farm.conn)
        .unwrap();
    assert!(
        farm.shots >= 15,
        "the import really did keep going: {} shots",
        farm.shots
    );
    assert!(
        b_runs <= 8,
        "B opened {} runs from {} shots; the wave gate is not holding",
        b_runs,
        farm.shots
    );
}

#[test]
fn a_person_who_gives_a_verdict_is_not_queued_behind_the_batch_they_gave_it_on() {
    // FR8's other half of "a person waiting cuts the line": a batch run parked
    // at a hold, continued by somebody looking at it. What the verdict releases
    // is theirs, and goes in front of the farm.
    let mut farm = Farm::new(2, 6);
    farm.sql("UPDATE line_stages SET hold_for_review = 1 WHERE stage_idx = 0;");
    let batch = farm.send("Restore · whole library");
    farm.feed(&batch, 6);

    // Every stage-0 task lands, so every run parks at the hold.
    for key in farm.next_pass(1000) {
        farm.land(&key.id);
    }
    farm.advance();

    let held: Vec<String> = crate::schema::runs::table
        .filter(crate::schema::runs::status.eq("held"))
        .select(crate::schema::runs::id)
        .load(&mut farm.conn)
        .unwrap();
    assert_eq!(held.len(), 6);

    // One verdict on one run, given the way the lane gives it.
    let root = farm.root();
    let hold = crate::comfyui::holds::read_hold(&mut farm.conn, &held[0])
        .unwrap()
        .unwrap();
    let keep: Vec<String> = hold.takes.iter().map(|t| t.task_id.clone()).collect();
    crate::comfyui::takes::verdicts::apply(
        &mut farm.conn,
        &root,
        &held[0],
        crate::comfyui::takes::verdicts::Ask {
            verdict: crate::comfyui::Verdict::Continue,
            keep: &keep,
            reject: &[],
            note: None,
            scope: crate::comfyui::takes::bulk::Scope::Run,
        },
    )
    .unwrap();
    farm.advance();

    // Now let the rest through in bulk — the ordinary batch verdict — and the
    // one somebody actually looked at is still the one that runs first.
    let hold = crate::comfyui::holds::read_hold(&mut farm.conn, &held[1])
        .unwrap()
        .unwrap();
    let keep: Vec<String> = hold.takes.iter().map(|t| t.task_id.clone()).collect();
    crate::comfyui::takes::verdicts::apply(
        &mut farm.conn,
        &root,
        &held[1],
        crate::comfyui::takes::verdicts::Ask {
            verdict: crate::comfyui::Verdict::Continue,
            keep: &keep,
            reject: &[],
            note: None,
            scope: crate::comfyui::takes::bulk::Scope::Batch,
        },
    )
    .unwrap();
    farm.advance();

    let pass = farm.next_pass(1000);
    assert_eq!(pass.len(), 6, "all six runs were released");
    assert_eq!(
        pass[0].priority,
        crate::comfyui::queue::Priority::Interactive,
        "the run somebody opened goes first"
    );
    for key in &pass[1..] {
        assert_eq!(
            key.priority,
            crate::comfyui::queue::Priority::Batch,
            "a bulk verdict is still the farm's work: {}",
            key
        );
    }
}
