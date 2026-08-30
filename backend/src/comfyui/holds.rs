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

use super::line::{self, RunState, Verdict, VerdictError};
use super::params;
use super::runs::{stages_of_line, supplied_for, StageRow};
use crate::models::NewRunHold;
use crate::schema::{enhancement_tasks, line_stages, run_holds, runs};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::collections::HashSet;
use std::path::Path;

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
    let reviewed = reviewed_task_ids(conn, run_id)?;
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

/// Every take of this run some verdict has already covered.
fn reviewed_task_ids(
    conn: &mut SqliteConnection,
    run_id: &str,
) -> Result<HashSet<String>, diesel::result::Error> {
    let rows: Vec<String> = run_holds::table
        .filter(run_holds::run_id.eq(run_id))
        .select(run_holds::reviewed_task_ids)
        .load(conn)?;
    Ok(rows
        .into_iter()
        .flat_map(|r| serde_json::from_str::<Vec<String>>(&r).unwrap_or_default())
        .collect())
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

/// Give a verdict on a held run.
///
/// # Every row a verdict writes goes in one transaction
///
/// Not a nicety. The verdict row is what says a take has been decided about, so
/// a process that died between writing it and putting the run back to `running`
/// would leave a run held over takes it can no longer offer — parked forever,
/// with nothing to give a verdict on. The files are swept *after* the commit,
/// because a filesystem is not something a transaction can roll back and a
/// leftover intermediate is a much smaller problem than a stuck run.
pub(crate) fn give_verdict(
    conn: &mut SqliteConnection,
    library_root: &Path,
    run_id: &str,
    verdict: Verdict,
    named: &[String],
    note: Option<&str>,
) -> Result<Outcome, HoldError> {
    let Some(hold) = read_hold(conn, run_id)? else {
        // Distinguishing "no such run" from "not held" matters to the caller:
        // one is a 404 and the other is a 409 they can recover from by
        // reloading the board.
        let exists: i64 = runs::table
            .filter(runs::id.eq(run_id))
            .count()
            .get_result(conn)?;
        return Err(if exists == 0 {
            HoldError::NotFound
        } else {
            HoldError::NotHeld
        });
    };

    let waiting: Vec<String> = hold.takes.iter().map(|t| t.task_id.clone()).collect();
    let kept = line::settle_verdict(verdict, &waiting, named)?;

    let (status, queued) = conn.transaction::<_, HoldError, _>(|conn| {
        record(conn, run_id, hold.stage_idx, verdict, &waiting, &kept, note)?;
        match verdict {
            Verdict::Continue => {
                // Nothing is queued here. The advance pass sees the cleared
                // takes on its next tick and continues them through the code
                // every other continuation goes through — so there is one place
                // a stage is queued from, and a verdict given a moment before
                // the process died is still applied when it comes back.
                release(conn, run_id)?;
                Ok((RunState::Running, Vec::new()))
            }
            Verdict::Regenerate => {
                let queued = regenerate(conn, run_id, &hold)?;
                release(conn, run_id)?;
                Ok((RunState::Running, queued))
            }
            Verdict::Cancel => {
                super::worker::advance::cancel_run(conn, run_id)?;
                Ok((RunState::Cancelled, Vec::new()))
            }
        }
    })?;

    // Committed. Now the disk.
    match verdict {
        // The generation just replaced goes the way the stage's own keep flag
        // says. Nobody will choose one of those takes now — a fresh set was
        // asked for instead — so the hold no longer protects them.
        Verdict::Regenerate => {
            if !keeps_takes(conn, run_id, hold.stage_idx).unwrap_or(false) {
                super::worker::advance::discard_outputs(conn, library_root, &waiting);
            }
        }
        Verdict::Cancel => {
            super::worker::advance::sweep_abandoned(conn, library_root, run_id);
        }
        Verdict::Continue => {}
    }

    Ok(Outcome {
        verdict,
        kept,
        reviewed: waiting,
        status,
        queued,
    })
}

/// Does the held stage ask for its takes to be kept in their own right?
fn keeps_takes(
    conn: &mut SqliteConnection,
    run_id: &str,
    stage_idx: i32,
) -> Result<bool, diesel::result::Error> {
    let line_id: Option<String> = runs::table
        .filter(runs::id.eq(run_id))
        .select(runs::line_id)
        .first(conn)?;
    let Some(line_id) = line_id else {
        return Ok(false);
    };
    Ok(line_stages::table
        .filter(
            line_stages::line_id
                .eq(&line_id)
                .and(line_stages::stage_idx.eq(stage_idx)),
        )
        .select(line_stages::keep_output)
        .first::<bool>(conn)
        .optional()?
        .unwrap_or(false))
}

/// Append the verdict. Never updated, never deleted: a run regenerated three
/// times keeps the record of each.
fn record(
    conn: &mut SqliteConnection,
    run_id: &str,
    stage_idx: i32,
    verdict: Verdict,
    reviewed: &[String],
    kept: &[String],
    note: Option<&str>,
) -> Result<(), diesel::result::Error> {
    let reviewed_json = serde_json::to_string(reviewed).unwrap_or_else(|_| "[]".to_string());
    let kept_json = serde_json::to_string(kept).unwrap_or_else(|_| "[]".to_string());
    diesel::insert_into(run_holds::table)
        .values(NewRunHold {
            id: &uuid::Uuid::new_v4().to_string(),
            run_id,
            stage_idx,
            verdict: verdict.as_str(),
            reviewed_task_ids: &reviewed_json,
            kept_task_ids: &kept_json,
            note: note.filter(|n| !n.trim().is_empty()),
        })
        .execute(conn)?;
    Ok(())
}

/// Put a held run back on the board. The advance pass takes it from here.
fn release(conn: &mut SqliteConnection, run_id: &str) -> Result<(), diesel::result::Error> {
    diesel::update(runs::table.filter(runs::id.eq(run_id)))
        .set((
            runs::status.eq(RunState::Running.as_str()),
            runs::held_at_stage.eq(None::<i32>),
            runs::finished_at.eq(None::<String>),
        ))
        .execute(conn)?;
    Ok(())
}

/// Run the held stage again: fresh seeds, everything else exactly as it was.
///
/// One fresh generation per group of takes that shared a parent, because a hold
/// under a fan-out has several: two takes at stage 1, four seeds each at stage
/// 2, is eight takes in two groups, and regenerating has to re-run both. The
/// new tasks name the same parent the old ones did, which leaves the
/// idempotence marker exactly as it was — the parent is still continued, and
/// still continued once.
fn regenerate(
    conn: &mut SqliteConnection,
    run_id: &str,
    hold: &Hold,
) -> Result<Vec<String>, HoldError> {
    let row: (Option<String>, Option<String>) = runs::table
        .filter(runs::id.eq(run_id))
        .select((runs::line_id, runs::stage_values))
        .first(conn)?;
    let (line_id, stage_values) = row;
    let Some(line_id) = line_id else {
        return Err(HoldError::Refused(
            "This run has no line, so there is no stage to run again.".to_string(),
        ));
    };
    let stages = stages_of_line(conn, &line_id)?;
    let Some(stage) = stages.iter().find(|s| s.stage_idx == hold.stage_idx) else {
        return Err(HoldError::Refused(format!(
            "Stage {} is no longer part of the line.",
            hold.stage_idx + 1
        )));
    };

    // What each group of takes read, and who made it. Order-preserving so a
    // regeneration queues its groups in the order the takes were made.
    let take_ids: Vec<&str> = hold.takes.iter().map(|t| t.task_id.as_str()).collect();
    let sources: Vec<(Option<String>, Option<String>, Option<String>)> = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq_any(&take_ids))
        .select((
            enhancement_tasks::parent_task_id,
            enhancement_tasks::source_file_id,
            enhancement_tasks::text_overrides,
        ))
        .load(conn)?;
    let mut groups: Vec<(Option<String>, Option<String>, Option<String>)> = Vec::new();
    for group in sources {
        if !groups.iter().any(|g| g.0 == group.0 && g.1 == group.1) {
            groups.push(group);
        }
    }

    let supplied = supplied_for(stage_values.as_deref(), hold.stage_idx);
    let mut queued = Vec::new();
    for (parent, source, text_overrides) in &groups {
        let mut plan = stage
            .plan_for(conn, &hold.shot_id, &supplied)
            .map_err(|e| HoldError::Refused(e.message))?;
        // The takes carried the prompt a describe stage compiled for them, and
        // regenerating must not lose it — "fresh seeds and nothing else" means
        // the sentence stays too.
        if let Some(stored) = text_overrides
            .as_deref()
            .and_then(|s| serde_json::from_str::<std::collections::HashMap<String, String>>(s).ok())
        {
            plan.text_overrides = stored;
        }
        params::reseed(&stage.seed_keys, &mut plan.parameters, &mut plan.vary);
        let ids = super::runs::queue_stage(
            conn,
            run_id,
            &hold.shot_id,
            &plan,
            source.as_deref(),
            parent.as_deref(),
        )
        .map_err(HoldError::Refused)?;
        queued.extend(ids);
    }
    Ok(queued)
}
