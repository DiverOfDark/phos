//! Reading a hold, and writing the verdict somebody gave on it.
//!
//! [`super::line`] decides what a verdict may say; this puts it on disk, beside
//! [`super::runs`] and for the same reason — the API and the worker both need
//! it, and neither can reach the other's private modules.
//!
//! # What a hold is, from here
//!
//! A run parked at stage *k* is holding every completed task at stage *k* that
//! no verdict has yet been given over. Those are its **takes**: four seeds,
//! four candidate continuations, and a person about to say which of them are
//! worth an hour of upscaling. Nothing here expires, nothing here times out and
//! nothing here picks for them.
//!
//! # Why a verdict names tasks
//!
//! A take *is* a task. It is what `enhancement_tasks.parent_task_id` points at,
//! so continuing from one is the write the advance pass already makes; it keeps
//! meaning something after its output file has been swept, which a file id does
//! not; and a describe stage's takes — sentences, with no file anywhere — are
//! namable for free. The screen shows a picture, and the picture's task id is
//! what comes back.
//!
//! # The verdict does not queue anything
//!
//! `continue` writes a row and puts the run back to `running`; the advance pass
//! queues the continuations on its next tick, through exactly the code every
//! other continuation goes through. So there is one place a stage is queued
//! from, and a verdict given a moment before the process died is still applied
//! when it comes back.
//!
//! # Two files, and where the line between them is
//!
//! This one **reads**: what a run is holding, which of its takes still want a
//! verdict, and what continuing from one would cost. [`verdict`] **writes**:
//! the row that records what somebody said, and the queueing or the sweeping
//! that follows from it. The reading half is asked every few seconds by the
//! board, and will be asked again by FR10b's curation lane; the writing half is
//! asked once per hold.
//!
//! Neither half decides anything. What a verdict may say is
//! [`super::line::settle_verdict`], which is pure and needs no database at all
//! — the same seam [`super::line`] and [`super::runs`] are cut along.

mod verdict;

pub(crate) use verdict::give_verdict;

use super::line::{RunState, Verdict, VerdictError};
use super::params;
use super::runs::{stages_of_line, supplied_for, StageRow};
use crate::schema::{enhancement_tasks, run_holds, runs};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::collections::{HashMap, HashSet};

/// One candidate a person is choosing between.
#[derive(Debug, Clone)]
pub(crate) struct Take {
    pub task_id: String,
    /// What it made. `None` for a describe stage's take, which is a sentence.
    pub output_file_id: Option<String>,
    /// The sentence, when that is what this stage produces.
    pub text_output: Option<String>,
    /// This take's fully resolved parameters — the seed that made it, which is
    /// what tells four otherwise identical cards apart.
    pub parameters: Option<String>,
    pub completed_at: Option<String>,
}

/// A run parked at a hold point, and everything a person needs to decide.
#[derive(Debug, Clone)]
pub(crate) struct Hold {
    pub run_id: String,
    pub shot_id: String,
    pub label: String,
    pub stage_idx: i32,
    pub stage_count: i32,
    /// The workflow whose takes these are.
    pub stage_label: Option<String>,
    pub takes: Vec<Take>,
    /// How many tasks each stage after the hold would queue for **one** kept
    /// take. Multiplied out by [`line::continuation_tasks`] for a selection.
    pub fanouts: Vec<usize>,
}

/// Why a verdict could not be given.
#[derive(Debug)]
pub(crate) enum HoldError {
    /// No such run.
    NotFound,
    /// The run is not parked at a hold point.
    NotHeld,
    /// The verdict itself is the problem — an empty selection, or a take this
    /// hold is not offering.
    Refused(String),
    Db(String),
}

impl std::fmt::Display for HoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HoldError::NotFound => f.write_str("Run not found"),
            HoldError::NotHeld => f.write_str("This run is not holding anything for review."),
            HoldError::Refused(m) => f.write_str(m),
            HoldError::Db(e) => f.write_str(e),
        }
    }
}

impl From<VerdictError> for HoldError {
    fn from(e: VerdictError) -> Self {
        HoldError::Refused(e.message)
    }
}

impl From<diesel::result::Error> for HoldError {
    fn from(e: diesel::result::Error) -> Self {
        HoldError::Db(e.to_string())
    }
}

/// What giving a verdict did.
#[derive(Debug)]
pub(crate) struct Outcome {
    pub verdict: Verdict,
    /// The takes that will walk the rest of the line.
    pub kept: Vec<String>,
    /// Every take the verdict was given over.
    pub reviewed: Vec<String>,
    /// The status the run is now in.
    pub status: RunState,
    /// Tasks queued right now — a fresh generation, for a regenerate. A
    /// continue queues nothing here: the advance pass does that on its next
    /// tick, through the code every other continuation goes through.
    pub queued: Vec<String>,
}

/// A `runs` row, reduced to what deciding whether it is holding needs: shot,
/// label, status, stage count, held stage, line.
type RunRow = (String, String, String, i32, Option<i32>, Option<String>);

/// One completed task, as a take reads out of the database: id, output file,
/// sentence, resolved parameters, when it landed.
type TakeRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// What a run is holding, or `None` if it is not holding anything.
pub(crate) fn read_hold(
    conn: &mut SqliteConnection,
    run_id: &str,
) -> Result<Option<Hold>, HoldError> {
    let row: Option<RunRow> = runs::table
        .filter(runs::id.eq(run_id))
        .select((
            runs::shot_id,
            runs::label,
            runs::status,
            runs::stage_count,
            runs::held_at_stage,
            runs::line_id,
        ))
        .first(conn)
        .optional()?;
    let Some((shot_id, label, status, stage_count, held_at, line_id)) = row else {
        return Err(HoldError::NotFound);
    };
    if status != RunState::Held.as_str() {
        return Ok(None);
    }
    let Some(stage_idx) = held_at else {
        // `held` with no stage is a row nobody should be able to write. Saying
        // so beats guessing which stage the takes belong to.
        return Ok(None);
    };

    let takes = waiting_takes(conn, run_id, stage_idx)?;
    let (stage_label, fanouts) = match line_id.as_deref() {
        Some(line_id) => tail_of(conn, line_id, &shot_id, run_id, stage_idx)?,
        None => (None, Vec::new()),
    };

    Ok(Some(Hold {
        run_id: run_id.to_string(),
        shot_id,
        label,
        stage_idx,
        stage_count,
        stage_label,
        takes,
        fanouts,
    }))
}

/// The completed tasks at this stage that no verdict has been given over.
///
/// A take a verdict already covered — kept, or passed over in favour of another
/// — is history, and is deliberately not offered again: its output may have
/// been swept, and re-deciding a decision is not what regenerate means.
pub(crate) fn waiting_takes(
    conn: &mut SqliteConnection,
    run_id: &str,
    stage_idx: i32,
) -> Result<Vec<Take>, diesel::result::Error> {
    let reviewed = reviewed_task_ids(conn, &[run_id])?;
    let rows: Vec<TakeRow> = enhancement_tasks::table
        .filter(
            enhancement_tasks::run_id
                .eq(run_id)
                .and(enhancement_tasks::stage_idx.eq(stage_idx))
                .and(enhancement_tasks::status.eq("completed")),
        )
        .order((
            enhancement_tasks::completed_at.asc(),
            enhancement_tasks::id.asc(),
        ))
        .select((
            enhancement_tasks::id,
            enhancement_tasks::output_file_id,
            enhancement_tasks::text_output,
            enhancement_tasks::parameters,
            enhancement_tasks::completed_at,
        ))
        .load(conn)?;
    Ok(rows
        .into_iter()
        .filter(|r| !reviewed.contains(&r.0))
        .map(|r| Take {
            task_id: r.0,
            output_file_id: r.1,
            text_output: r.2,
            parameters: r.3,
            completed_at: r.4,
        })
        .collect())
}

/// Every take of these runs some verdict has already covered.
///
/// The one definition of *reviewed*, asked for one run when a hold is read and
/// for a page of them when the board counts what each is waiting on. Written
/// once because the two answers have to agree: a take the hold does not offer
/// and the board still counts is `HELD · 8 TAKES` over a regeneration of four.
fn reviewed_task_ids(
    conn: &mut SqliteConnection,
    run_ids: &[&str],
) -> Result<HashSet<String>, diesel::result::Error> {
    if run_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let rows: Vec<String> = run_holds::table
        .filter(run_holds::run_id.eq_any(run_ids))
        .select(run_holds::reviewed_task_ids)
        .load(conn)?;
    Ok(rows
        .into_iter()
        .flat_map(|r| serde_json::from_str::<Vec<String>>(&r).unwrap_or_default())
        .collect())
}

/// How many takes each of these runs is holding, keyed by run id.
///
/// Two queries for a whole page rather than two per row — a library that put
/// three thousand shots through a hold point has three thousand held runs, and
/// a board that costs a query each is a board nobody opens twice. Beside
/// [`waiting_takes`] rather than in the handler that draws the board, because
/// the two have to be counting the same takes.
pub(crate) fn held_take_counts(
    conn: &mut SqliteConnection,
    held: &[(&str, i32)],
) -> HashMap<String, usize> {
    if held.is_empty() {
        return HashMap::new();
    }
    let run_ids: Vec<&str> = held.iter().map(|(id, _)| *id).collect();
    let reviewed = reviewed_task_ids(conn, &run_ids).unwrap_or_default();

    let tasks: Vec<(String, String, Option<i32>)> = enhancement_tasks::table
        .filter(
            enhancement_tasks::run_id
                .eq_any(&run_ids)
                .and(enhancement_tasks::status.eq("completed")),
        )
        .select((
            enhancement_tasks::id,
            enhancement_tasks::run_id.assume_not_null(),
            enhancement_tasks::stage_idx,
        ))
        .load(conn)
        .unwrap_or_default();

    let at: HashMap<&str, i32> = held.iter().copied().collect();
    let mut counts: HashMap<String, usize> =
        run_ids.iter().map(|id| (id.to_string(), 0usize)).collect();
    for (task_id, run_id, stage_idx) in tasks {
        if at.get(run_id.as_str()).copied() != stage_idx || reviewed.contains(&task_id) {
            continue;
        }
        *counts.entry(run_id).or_default() += 1;
    }
    counts
}

/// The name of the held stage, and how wide each stage after it would fan out.
///
/// The fan-out widths are what the estimate is made of, and they are read the
/// way the queue will read them: the stage's own values with this run's answers
/// folded in, expanded by the same [`params::expand`] that will queue them.
fn tail_of(
    conn: &mut SqliteConnection,
    line_id: &str,
    shot_id: &str,
    run_id: &str,
    stage_idx: i32,
) -> Result<(Option<String>, Vec<usize>), HoldError> {
    let stage_values: Option<String> = runs::table
        .filter(runs::id.eq(run_id))
        .select(runs::stage_values)
        .first(conn)?;
    let stages = stages_of_line(conn, line_id)?;
    let label = stages
        .iter()
        .find(|s| s.stage_idx == stage_idx)
        .map(|s| s.workflow_name.clone());

    let mut fanouts = Vec::new();
    for stage in stages.iter().filter(|s| s.stage_idx > stage_idx) {
        fanouts.push(width_of(conn, stage, shot_id, stage_values.as_deref()));
    }
    Ok((label, fanouts))
}

/// How many tasks one continuation into this stage becomes.
///
/// A stage whose sweep cannot be read counts as one: an estimate that refuses
/// to be shown is worse than an estimate that is low, and the sweep is refused
/// for real when the stage is actually queued.
fn width_of(
    conn: &mut SqliteConnection,
    stage: &StageRow,
    shot_id: &str,
    stage_values: Option<&str>,
) -> usize {
    let supplied = supplied_for(stage_values, stage.stage_idx);
    stage
        .plan_for(conn, shot_id, &supplied)
        .ok()
        .and_then(|plan| params::expand(&plan.parameters, &plan.vary).ok())
        .map(|tasks| tasks.len())
        .unwrap_or(1)
}
