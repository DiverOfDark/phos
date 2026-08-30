//! Writing down what somebody decided about a hold, and acting on it.
//!
//! The command half of [`super`], whose other half only reads. The two are
//! split the way `contract/` and `worker/` are: not because a file grew, but
//! because they are asked by different callers for different reasons — the
//! board and FR10b's curation lane read a hold every few seconds, and a verdict
//! is given once and changes the world.
//!
//! Nothing here decides anything. What a verdict may say is
//! [`super::super::line::settle_verdict`], which is pure and needs no database;
//! this puts its answer on disk and queues what follows from it.

use super::{read_hold, Hold, HoldError, Outcome};
use crate::comfyui::line::{self, RunState, Verdict};
use crate::comfyui::params;
use crate::comfyui::runs::{stages_of_line, supplied_for};
use crate::models::NewRunHold;
use crate::schema::{enhancement_tasks, line_stages, run_holds, runs};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::collections::HashMap;
use std::path::Path;

/// A take's own row, as [`regenerate`] reads it: id, parent, source,
/// directives, and the hurry it was queued with.
type SourceRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

/// Where one take came from: the task it continued, the file it read, the
/// directives that were compiled into it, and its priority. Two takes agreeing
/// on the first two are the same generation, and regenerate re-runs the stage
/// once per group.
type TakeSource = (Option<String>, Option<String>, Option<String>, String);

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
                crate::comfyui::worker::advance::cancel_run(conn, run_id)?;
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
                crate::comfyui::worker::advance::discard_outputs(conn, library_root, &waiting);
            }
        }
        Verdict::Cancel => {
            crate::comfyui::worker::advance::sweep_abandoned(conn, library_root, run_id);
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

    // What each group of takes read, and who made it.
    //
    // Walked in the order [`read_hold`] offered the takes — oldest first — and
    // not in whatever order the rows come back in, because `eq_any` promises
    // none. The groups are what the fresh generation is queued from, so an
    // unordered read would give a regeneration a different task order every
    // time it ran, and the board would shuffle for no reason a person can see.
    let take_ids: Vec<&str> = hold.takes.iter().map(|t| t.task_id.as_str()).collect();
    let rows: Vec<SourceRow> = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq_any(&take_ids))
        .select((
            enhancement_tasks::id,
            enhancement_tasks::parent_task_id,
            enhancement_tasks::source_file_id,
            enhancement_tasks::text_overrides,
            enhancement_tasks::priority,
        ))
        .load(conn)?;
    let by_id: HashMap<String, TakeSource> = rows
        .into_iter()
        .map(|(id, parent, source, overrides, priority)| {
            (id, (parent, source, overrides, priority))
        })
        .collect();
    let mut groups: Vec<TakeSource> = Vec::new();
    for take_id in &take_ids {
        let Some(group) = by_id.get(*take_id) else {
            continue;
        };
        if !groups.iter().any(|g| g.0 == group.0 && g.1 == group.1) {
            groups.push(group.clone());
        }
    }

    let supplied = supplied_for(stage_values.as_deref(), hold.stage_idx);
    let mut queued = Vec::new();
    for (parent, source, text_overrides, priority) in &groups {
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
        let ids = crate::comfyui::runs::queue_stage(
            conn,
            run_id,
            &hold.shot_id,
            &plan,
            source.as_deref(),
            parent.as_deref(),
            crate::comfyui::queue::Priority::parse(priority),
        )
        .map_err(HoldError::Refused)?;
        queued.extend(ids);
    }
    Ok(queued)
}
