//! What order the pending queue drains in, and what a task's hurry means.
//!
//! Everything here is pure. The dispatcher's `ORDER BY` is built from the
//! fragments below and the same order is expressed a second time as [`Ord`] on
//! [`DrainKey`], so the claim "the database returns the order this module
//! describes" is a test rather than a reading of two files side by side.
//!
//! # The order
//!
//! ```text
//!   priority   interactive before batch
//!   stage      lower stage index first
//!   workflow   ties grouped by the graph that will be run
//!   created_at oldest first
//!   id         so the order is total
//! ```
//!
//! ## Why stage, and not `created_at`
//!
//! A batch of three thousand shots down `describe → generate video → upscale`
//! queues its tasks in whatever order the runs happened to reach each stage. By
//! `created_at` that interleaves: a description, a 14B video generation, an
//! upscale, another description. On a 24 GB card each of those swaps takes
//! roughly twenty gigabytes of weights off and another twenty on — per task.
//!
//! Sorting by stage drains the batch a **pass at a time**: every description,
//! then every video, then every upscale. The model each pass needs is loaded
//! once for the pass instead of once per task. (The VRAM saving itself is
//! ComfyUI's and a GPU's; nothing in this tree can measure it. What this module
//! can promise, and what its tests assert, is the *dispatch order* that makes
//! it available.)
//!
//! The second reason is the larger one. Three thousand descriptions that all
//! finish is something a person can review in one sitting — which is exactly
//! what FR5c's hold points and FR10b's Takes lane are for. Three thousand runs
//! each halfway down their own chain is not reviewable at all.
//!
//! ## The tradeoff, stated
//!
//! Runs advance **in lockstep by stage** rather than one at a time. A batch
//! therefore trades per-run latency for throughput: no single run finishes
//! early, and the first finished product of a three-stage batch appears only
//! once the whole wave has walked all three stages. That is right for a farm
//! and wrong for a person waiting — which is the entire reason `priority`
//! exists and is the first key. A click never queues behind a batch.
//!
//! ## Why workflow before `created_at`
//!
//! Two lines can hold different workflows at the same stage index — line A's
//! stage 1 is a video model, line B's is an upscaler. Sorting by stage alone
//! would interleave those two, and the model would swap between them. Grouping
//! by `workflow_id` inside a stage keeps each graph's tasks contiguous. Which
//! workflow goes first is arbitrary (it is a uuid); it is a *grouping* key, not
//! a preference.

/// Whether a person is waiting for this task.
///
/// Two values and deliberately no third. The question a dispatcher has to
/// answer is not "how important is this" — it is "is somebody looking at a
/// spinner right now", and that has two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum Priority {
    /// Somebody pressed a button and is waiting: Enhance on one shot, Describe,
    /// a retry, or a verdict given on the run in front of them.
    #[default]
    Interactive,
    /// A batch opened it. There are three thousand more behind it and nobody is
    /// watching this one in particular.
    Batch,
}

impl Priority {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Priority::Interactive => "interactive",
            Priority::Batch => "batch",
        }
    }

    /// Where a stored value sorts. Anything unrecognised — a hand-edited row,
    /// a value from a newer version — reads as `interactive`, which is the same
    /// direction the column's own default fails in: towards the person.
    pub(crate) fn parse(s: &str) -> Priority {
        match s {
            "batch" => Priority::Batch,
            _ => Priority::Interactive,
        }
    }

    /// Smaller goes first.
    pub(crate) fn rank(self) -> i32 {
        match self {
            Priority::Interactive => 0,
            Priority::Batch => 1,
        }
    }

    /// A batch's runs are batch work; every other run is somebody's click.
    pub(crate) fn of_batch(batch_id: Option<&str>) -> Priority {
        match batch_id {
            Some(_) => Priority::Batch,
            None => Priority::Interactive,
        }
    }
}

/// The priority key, as SQL.
///
/// A `CASE` rather than the column itself: `'batch' < 'interactive'` in text
/// order, so a plain `ORDER BY priority` would drain the farm first. Two values
/// that happen to sort the right way round is not a thing to build an ordering
/// on, and this one does not.
pub(crate) const PRIORITY_RANK_SQL: &str =
    "CASE enhancement_tasks.priority WHEN 'batch' THEN 1 ELSE 0 END";

/// The stage key, as SQL.
///
/// `COALESCE`, because `stage_idx` is nullable: a row written before runs
/// existed has none, and NULL sorts before every integer in SQLite — which
/// would put the oldest rows in the library permanently at the head of the
/// queue. Reading them as stage 0 is what they are: a one-workflow enhance.
pub(crate) const STAGE_RANK_SQL: &str = "COALESCE(enhancement_tasks.stage_idx, 0)";

/// One pending task, reduced to what deciding its place in the queue needs.
///
/// The [`Ord`] below is the whole of FR8's ordering, expressed without a
/// database so it can be read, and so the database can be checked against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrainKey {
    pub priority: Priority,
    /// Which step of a line this is. A row with no stage counts as the first.
    pub stage_idx: i32,
    pub workflow_id: String,
    pub created_at: String,
    pub id: String,
}

impl DrainKey {
    fn tuple(&self) -> (i32, i32, &str, &str, &str) {
        (
            self.priority.rank(),
            self.stage_idx,
            &self.workflow_id,
            &self.created_at,
            &self.id,
        )
    }
}

impl std::fmt::Display for DrainKey {
    /// One task's place in the queue, for the line the dispatcher logs each
    /// pass. On a farm this is the answer to "why is it doing *that* one next",
    /// and it is worth being able to read without a debugger.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} stage {} {} ({})",
            self.priority.as_str(),
            self.stage_idx + 1,
            self.workflow_id,
            self.id
        )
    }
}

impl Ord for DrainKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.tuple().cmp(&other.tuple())
    }
}

impl PartialOrd for DrainKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(priority: Priority, stage: i32, workflow: &str, created: &str, id: &str) -> DrainKey {
        DrainKey {
            priority,
            stage_idx: stage,
            workflow_id: workflow.to_string(),
            created_at: created.to_string(),
            id: id.to_string(),
        }
    }

    /// The keys of a mixed queue, in drain order.
    fn drained(mut keys: Vec<DrainKey>) -> Vec<String> {
        keys.sort();
        keys.into_iter().map(|k| k.id).collect()
    }

    #[test]
    fn a_whole_stage_drains_before_the_next_one_starts() {
        // The shape a batch actually produces: runs at three different stages,
        // queued in the order they got there.
        let queue = vec![
            key(Priority::Batch, 0, "describe", "12:00:00", "d1"),
            key(Priority::Batch, 1, "video", "12:00:01", "v1"),
            key(Priority::Batch, 2, "upscale", "12:00:02", "u1"),
            key(Priority::Batch, 0, "describe", "12:00:03", "d2"),
            key(Priority::Batch, 1, "video", "12:00:04", "v2"),
            key(Priority::Batch, 0, "describe", "12:00:05", "d3"),
        ];
        assert_eq!(drained(queue), ["d1", "d2", "d3", "v1", "v2", "u1"]);
    }

    #[test]
    fn one_click_never_queues_behind_three_thousand_descriptions() {
        // The inversion FR8 exists to avoid, stated at its own scale: a batch
        // of three thousand, and one task somebody queued *last*.
        let mut queue: Vec<DrainKey> = (0..3_000)
            .map(|i| {
                key(
                    Priority::Batch,
                    0,
                    "describe",
                    &format!("12:00:{:04}", i),
                    &format!("b{}", i),
                )
            })
            .collect();
        queue.push(key(
            Priority::Interactive,
            0,
            "portrait",
            "23:59:59",
            "mine",
        ));
        assert_eq!(drained(queue)[0], "mine");
    }

    #[test]
    fn an_interactive_task_at_a_late_stage_still_beats_the_whole_farm() {
        // Priority is asked before stage, not after. A person's run that is
        // three stages down is still theirs, and still first.
        let queue = vec![
            key(Priority::Batch, 0, "describe", "08:00:00", "batch"),
            key(Priority::Interactive, 3, "upscale", "09:00:00", "mine"),
        ];
        assert_eq!(drained(queue), ["mine", "batch"]);
    }

    #[test]
    fn a_stage_holding_two_lines_is_grouped_by_the_graph_that_will_run() {
        // The model-locality claim, as an assertion about dispatch order:
        // every task of one workflow is contiguous. Interleaved by created_at,
        // this same queue would swap the model five times.
        let queue = vec![
            key(Priority::Batch, 1, "wan-video", "12:00:00", "a1"),
            key(Priority::Batch, 1, "esrgan", "12:00:01", "b1"),
            key(Priority::Batch, 1, "wan-video", "12:00:02", "a2"),
            key(Priority::Batch, 1, "esrgan", "12:00:03", "b2"),
            key(Priority::Batch, 1, "wan-video", "12:00:04", "a3"),
        ];
        let order = drained(queue);
        assert_eq!(order, ["b1", "b2", "a1", "a2", "a3"]);

        // Said as the property rather than the literal list: no workflow is
        // returned to once it has been left.
        let workflows = ["esrgan", "esrgan", "wan-video", "wan-video", "wan-video"];
        let mut seen: Vec<&str> = Vec::new();
        for wf in workflows {
            if seen.last() != Some(&wf) {
                assert!(!seen.contains(&wf), "{} was returned to", wf);
                seen.push(wf);
            }
        }
    }

    #[test]
    fn inside_one_workflow_the_oldest_task_goes_first() {
        let queue = vec![
            key(Priority::Batch, 0, "describe", "12:00:09", "late"),
            key(Priority::Batch, 0, "describe", "12:00:01", "early"),
        ];
        assert_eq!(drained(queue), ["early", "late"]);
    }

    #[test]
    fn two_tasks_alike_in_everything_else_are_still_totally_ordered() {
        // A fan-out writes several rows inside one transaction, so `created_at`
        // ties are ordinary rather than exotic. Without the id the order would
        // be whatever SQLite felt like, and the board would shuffle.
        let queue = vec![
            key(Priority::Batch, 0, "seeded", "12:00:00", "task-b"),
            key(Priority::Batch, 0, "seeded", "12:00:00", "task-a"),
        ];
        assert_eq!(drained(queue), ["task-a", "task-b"]);
    }

    #[test]
    fn an_unreadable_priority_is_read_as_the_persons() {
        // A hand-edited row, or one written by a version that knows a word this
        // one does not. Guessing `batch` would silently park somebody's click
        // behind a farm; guessing `interactive` costs a batch one slot.
        assert_eq!(Priority::parse("interactive"), Priority::Interactive);
        assert_eq!(Priority::parse("batch"), Priority::Batch);
        assert_eq!(Priority::parse("URGENT"), Priority::Interactive);
        assert_eq!(Priority::parse(""), Priority::Interactive);
        assert_eq!(Priority::default(), Priority::Interactive);
    }

    #[test]
    fn a_run_is_batch_work_exactly_when_a_batch_opened_it() {
        assert_eq!(Priority::of_batch(Some("batch-1")), Priority::Batch);
        assert_eq!(Priority::of_batch(None), Priority::Interactive);
    }

    #[test]
    fn the_priority_key_is_a_case_because_the_text_sorts_the_wrong_way() {
        // The bug this constant exists to not have: 'batch' < 'interactive'.
        assert!("batch" < "interactive");
        assert!(Priority::Interactive.rank() < Priority::Batch.rank());
        assert!(PRIORITY_RANK_SQL.contains("CASE"));
        assert!(PRIORITY_RANK_SQL.contains("'batch'"));
    }

    #[test]
    fn a_row_with_no_stage_is_read_as_the_first_one() {
        // NULL sorts before 0 in SQLite, so without the COALESCE every task
        // queued before runs existed would sit permanently at the head.
        assert!(STAGE_RANK_SQL.contains("COALESCE"));
        assert!(STAGE_RANK_SQL.contains("stage_idx"));
    }
}
