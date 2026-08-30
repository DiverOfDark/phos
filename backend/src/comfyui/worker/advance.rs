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
//!    through stages 3 and 4. A take at a **hold point** is the one exception:
//!    it queues nothing and parks its run instead.
//! 2. **Settle.** Fold each live run's tasks into a state. A run is running
//!    while anything is still moving, and then it is however its tasks ended —
//!    or, if it is holding takes nobody has looked at, it is *held*.
//!
//! Doing them the other way round would be a data-loss bug rather than a
//! cosmetic one: a run whose stage-1 task has just completed but whose stage-2
//! task has not yet been written would settle as *completed*, and the sweep
//! below would delete the intermediate the next stage was about to read. So a
//! run with a continuation still owed is never settled in the same pass —
//! [`queue_continuations`] hands back the ones it could not finish, and they
//! wait a tick. A run that is holding is never settled either, for the same
//! reason turned up a notch: its takes are the whole point.
//!
//! # Holds park, they do not block
//!
//! A held run stops being this pass's business entirely — both halves filter on
//! `status = running`, so a held run is read by nothing here until a verdict
//! puts it back. That is what makes holds safe at scale: 3,329 shots through
//! `×4 extend → hold → upscale` park 3,329 runs and the queue keeps feeding the
//! GPU from everything else. Held runs accumulate; they do not block. (Capping
//! how many may accumulate is FR7's job, and belongs where batches are fed.)
//!
//! # Intermediates
//!
//! A four-stage line makes three intermediates per take. They live exactly as
//! long as they are useful — the next stage reads them, and a failure wants
//! them for inspection — and are swept when the run *completes*. Not when it
//! fails: a failed run is retried from its failed stage, and that resumption
//! reads the intermediate the stage before it made. A hold stage's takes are
//! never swept on completion at all: they are what somebody chose between.

use crate::comfyui::line::{self, Advance, HoldGate, RunState, StageDisposition, TaskPhase};
use crate::comfyui::prompt;
use crate::comfyui::runs::{queue_stage, stage_at};
use crate::comfyui::timestamp::format_ts;
use crate::schema::{
    enhancement_tasks, faces, files, line_stages, run_holds, runs, video_keyframes,
};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::{error, info, warn};

/// One pass: continue what can be continued, then settle what is over.
pub(super) fn advance_runs(conn: &mut SqliteConnection, library_root: &Path) {
    let pass = match queue_continuations(conn) {
        Ok(pass) => pass,
        Err(e) => {
            error!("Failed to queue run continuations: {}", e);
            // Settling anything now would risk calling a run finished while its
            // next stage is still owed. Wait for the next tick.
            return;
        }
    };
    if let Err(e) = settle_runs(conn, library_root, &pass) {
        error!("Failed to settle runs: {}", e);
    }
}

/// What the continue half learned, and the settle half needs.
#[derive(Default)]
struct Pass {
    /// Runs that still owe a continuation — a line edited out from under one,
    /// say. Not settled this tick, because "nothing is in flight" would
    /// otherwise read as "finished".
    stalled: HashSet<String>,
    /// Runs with takes waiting at a hold point, and the earliest stage each is
    /// waiting at. Settled as *held* rather than as completed.
    holding: HashMap<String, i32>,
}

impl Pass {
    fn hold(&mut self, run_id: &str, stage_idx: i32) {
        self.holding
            .entry(run_id.to_string())
            .and_modify(|at| *at = (*at).min(stage_idx))
            .or_insert(stage_idx);
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
    /// What this task read. A describe stage makes no file, so the stage after
    /// it reads the same photograph this one did.
    source_file_id: Option<String>,
    /// The sentence a describe stage produced, if this was one.
    text_output: Option<String>,
    /// The describe stage's own directives, so the stage after it inherits the
    /// intent and the constraints a person typed once.
    text_overrides: Option<String>,
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
    Option<String>,
    Option<String>,
    Option<String>,
);

/// What a completed stage hands the one after it.
enum Handoff {
    /// A file. The ordinary case: the next stage loads it.
    File(String),
    /// A sentence, and no file at all. What the describe stage read is still
    /// what the next stage reads — the photograph is unchanged by having been
    /// described — and the sentence goes into the next stage's prompt slot.
    Description {
        text: String,
        source_file_id: Option<String>,
    },
}

/// Queue the stage after every completed task that is owed one, and park the
/// runs whose takes are waiting to be looked at.
fn queue_continuations(conn: &mut SqliteConnection) -> Result<Pass, diesel::result::Error> {
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
            enhancement_tasks::source_file_id,
            enhancement_tasks::text_output,
            enhancement_tasks::text_overrides,
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
            source_file_id: r.7,
            text_output: r.8,
            text_overrides: r.9,
            stage_values: r.10,
        })
        .collect();

    if candidates.is_empty() {
        return Ok(Pass::default());
    }

    // A continuation exists exactly when some row names this task as its
    // parent. That is the idempotence marker: no "advanced" flag to write, and
    // no way for the marker and the thing it marks to disagree. It holds
    // unchanged when one verdict continues several takes at once — each keeps
    // naming its own parent, and one row per kept take is what a verdict is.
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

    let hold_stages = hold_stages_of(conn, &candidates)?;
    let (reviewed, kept) = verdicts_over(conn, &run_ids)?;

    let mut pass = Pass::default();
    for c in candidates {
        if already_continued.contains(&c.task_id) {
            continue;
        }
        let asks = c
            .line_id
            .as_deref()
            .is_some_and(|line| hold_stages.contains(&(line.to_string(), c.stage_idx)));
        let gate = if asks {
            HoldGate {
                holds: true,
                kept: kept.contains(&c.task_id),
                reviewed: reviewed.contains(&c.task_id),
            }
        } else {
            // A stage nobody is reviewing, which is every stage of every line
            // that has no hold point in it.
            HoldGate::open()
        };
        let next_idx = match line::advance_after(c.stage_idx, c.stage_count, gate) {
            // The last stage owes nothing: its output is the product. Neither
            // does a take somebody looked at and did not choose.
            Advance::Finished => continue,
            Advance::Hold(at) => {
                pass.hold(&c.run_id, at);
                continue;
            }
            Advance::Next(next_idx) => next_idx,
        };
        if let Err(reason) = continue_one(conn, &c, next_idx) {
            warn!(
                "Run {} cannot continue past stage {}: {}",
                c.run_id, c.stage_idx, reason
            );
            fail_run(conn, &c.run_id, &reason);
            pass.stalled.insert(c.run_id.clone());
        }
    }
    Ok(pass)
}

/// Which `(line_id, stage_idx)` pairs among these candidates ask for a verdict.
///
/// One query for the whole tick rather than one per task: a page of runs of one
/// line is the ordinary case, and asking the same row fifty times is how a
/// three-second loop becomes a hot loop.
fn hold_stages_of(
    conn: &mut SqliteConnection,
    candidates: &[Continuation],
) -> Result<HashSet<(String, i32)>, diesel::result::Error> {
    let line_ids: Vec<&str> = candidates
        .iter()
        .filter_map(|c| c.line_id.as_deref())
        .collect();
    if line_ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(line_stages::table
        .filter(
            line_stages::line_id
                .eq_any(&line_ids)
                .and(line_stages::hold_for_review.eq(true)),
        )
        .select((line_stages::line_id, line_stages::stage_idx))
        .load::<(String, i32)>(conn)?
        .into_iter()
        .collect())
}

/// The takes these runs already have a verdict over, and the ones that verdict
/// let through.
///
/// `reviewed` is the marker that matters: without it a passed-over take looks
/// exactly like one nobody has seen, and the run parks again on it forever.
fn verdicts_over(
    conn: &mut SqliteConnection,
    run_ids: &[&str],
) -> Result<(HashSet<String>, HashSet<String>), diesel::result::Error> {
    let rows: Vec<(String, String)> = run_holds::table
        .filter(run_holds::run_id.eq_any(run_ids))
        .select((run_holds::reviewed_task_ids, run_holds::kept_task_ids))
        .load(conn)?;
    let mut reviewed = HashSet::new();
    let mut kept = HashSet::new();
    for (r, k) in rows {
        reviewed.extend(serde_json::from_str::<Vec<String>>(&r).unwrap_or_default());
        kept.extend(serde_json::from_str::<Vec<String>>(&k).unwrap_or_default());
    }
    Ok((reviewed, kept))
}

/// Queue the stage after one completed task, reading what that task produced.
fn continue_one(
    conn: &mut SqliteConnection,
    c: &Continuation,
    next_idx: i32,
) -> Result<(), String> {
    let line_id = c
        .line_id
        .as_deref()
        .ok_or_else(|| "the run has more than one stage but no line".to_string())?;

    // `mark_completed` is only ever reached once a file has landed *or* a
    // description has, so the third arm is a can't-happen — but a continuation
    // with no source would silently read the shot's original instead of the
    // clip the stage before it made, which is exactly the kind of wrong that
    // looks right.
    let handoff = match (c.output_file_id.as_deref(), c.text_output.as_deref()) {
        (Some(file_id), _) => Handoff::File(file_id.to_string()),
        (None, Some(text)) if !text.trim().is_empty() => Handoff::Description {
            text: text.to_string(),
            source_file_id: c.source_file_id.clone(),
        },
        _ => {
            return Err(format!(
                "stage {} completed without an output",
                c.stage_idx + 1
            ))
        }
    };

    let stage = stage_at(conn, line_id, next_idx)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("stage {} is no longer part of the line", next_idx + 1))?;

    let mut plan = stage.plan_for(conn, &c.shot_id);
    let source_file_id = match &handoff {
        Handoff::File(file_id) => Some(file_id.as_str()),
        Handoff::Description {
            text,
            source_file_id,
        } => {
            // The one binding this whole feature is about, and it is one entry
            // in a map the dispatcher already reads.
            let upstream: std::collections::HashMap<String, String> = c
                .text_overrides
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let intent = prompt::Intent::from_overrides(&plan.text_overrides)
                .inherit(prompt::Intent::from_overrides(&upstream));
            // `phos:slot` is written on the describe stage — "put my answer in
            // *that* box of the stage after me" — but `bind_description` reads
            // the map it binds into, so the directive has to ride down with the
            // description. The downstream stage's own say still wins.
            if !plan.text_overrides.contains_key(prompt::SLOT_KEY) {
                if let Some(slot) = upstream
                    .get(prompt::SLOT_KEY)
                    .filter(|s| !s.trim().is_empty())
                {
                    plan.text_overrides
                        .insert(prompt::SLOT_KEY.to_string(), slot.clone());
                }
            }
            let compiled = prompt::compile_from_text(text, &intent);
            prompt::bind_description(&stage.contract, &mut plan.text_overrides, &compiled)
                .map_err(|e| e.message)?;
            source_file_id.as_deref()
        }
    };

    // The answers this run was started with. A stage that asked for a value at
    // send time gets it here, whichever hour of the run it is queued in. Applied
    // after the description binds, so an explicit answer from the sender wins.
    let supplied = crate::comfyui::runs::supplied_for(c.stage_values.as_deref(), next_idx);
    plan.accept(&stage.exposed, &supplied)
        .map_err(|e| e.message)?;
    let queued = queue_stage(
        conn,
        &c.run_id,
        &c.shot_id,
        &plan,
        source_file_id,
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
    pass: &Pass,
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
        if pass.stalled.contains(run_id) {
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

        // Nothing is moving, and there are takes at a hold point nobody has
        // looked at: the run is not over, it is waiting. Parked rather than
        // finished, so the sweep below never runs and the takes stay.
        //
        // Only when everything landed. A failure outranks a hold — the run
        // failed, retrying it re-runs the stage that broke, and the hold is
        // what the retry arrives at.
        if t.state == RunState::Completed {
            if let Some(&at) = pass.holding.get(run_id.as_str()) {
                park_run(conn, run_id, at)?;
                info!(
                    "Run {} held at stage {}/{} for review",
                    run_id,
                    at + 1,
                    stage_count
                );
                continue;
            }
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
            discard_intermediates(
                conn,
                library_root,
                run_id,
                line_id.as_deref(),
                *stage_count,
                Sweep::Landed,
            );
        }
    }
    Ok(())
}

/// Park a run at a hold point.
///
/// Not `finished_at`: a held run has not finished, and stamping it would make
/// the board's clock stop on a run that is still going to spend GPU time.
fn park_run(
    conn: &mut SqliteConnection,
    run_id: &str,
    stage_idx: i32,
) -> Result<(), diesel::result::Error> {
    diesel::update(runs::table.filter(runs::id.eq(run_id)))
        .set((
            runs::status.eq(RunState::Held.as_str()),
            runs::held_at_stage.eq(stage_idx),
            runs::error_message.eq(None::<String>),
        ))
        .execute(conn)?;
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

/// Why a run's intermediates are being swept.
///
/// The two differ in exactly one place, and only for a hold stage: whether the
/// takes it made are still something a person might choose between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sweep {
    /// The run landed. A hold stage's takes are the alternatives somebody was
    /// shown, and a run that finished does not throw away what it was picked
    /// out of.
    Landed,
    /// The run was abandoned at its hold. Nobody is going to choose one of
    /// those takes now, so only the stage's own keep flag saves them — which is
    /// what "cancel removes intermediates unless the stage says keep" means.
    Abandoned,
}

/// Sweep the outputs of a run's non-final, non-kept stages.
pub(crate) fn discard_intermediates(
    conn: &mut SqliteConnection,
    library_root: &Path,
    run_id: &str,
    line_id: Option<&str>,
    stage_count: i32,
    sweep: Sweep,
) {
    // What the user asked to keep, and which stages asked for a verdict. An
    // ad-hoc run has no line, and its one stage is final, so this stays empty
    // and nothing is swept.
    let flags: HashMap<i32, (bool, bool)> = match line_id {
        Some(line_id) => line_stages::table
            .filter(line_stages::line_id.eq(line_id))
            .select((
                line_stages::stage_idx,
                line_stages::keep_output,
                line_stages::hold_for_review,
            ))
            .load::<(i32, bool, bool)>(conn)
            .unwrap_or_default()
            .into_iter()
            .map(|(idx, keep, holds)| (idx, (keep, holds)))
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
        let (keep_flag, holds) = flags.get(&stage_idx).copied().unwrap_or((false, false));
        let disposition = StageDisposition {
            keep_flag,
            is_final: stage_idx + 1 >= stage_count,
            feeds_hold: holds && sweep == Sweep::Landed,
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

/// Take the outputs of named tasks out of the library.
///
/// What regenerating does with the generation it replaced, when the stage did
/// not ask to keep it: those takes were looked at and a fresh set was asked
/// for, so the same rule applies as to an abandoned run's.
pub(crate) fn discard_outputs(
    conn: &mut SqliteConnection,
    library_root: &Path,
    task_ids: &[String],
) {
    if task_ids.is_empty() {
        return;
    }
    let outputs: Vec<(String, Option<String>)> = enhancement_tasks::table
        .filter(
            enhancement_tasks::id
                .eq_any(task_ids)
                .and(enhancement_tasks::output_file_id.is_not_null()),
        )
        .select((enhancement_tasks::id, enhancement_tasks::output_file_id))
        .load(conn)
        .unwrap_or_default();
    for (task_id, file_id) in outputs {
        let Some(file_id) = file_id else { continue };
        if let Err(e) = discard_file(conn, library_root, &task_id, &file_id) {
            warn!("Could not discard take {}: {}", task_id, e);
        }
    }
}

/// Sweep what a run abandoned at a hold point made.
///
/// The difference from what [`cancel_run`] does alone is deliberate, and is the
/// reason this is a second step rather than a flag on that one. Cancelling a
/// *running* run leaves its intermediates alone, because a cancelled task goes
/// back in the queue on retry and reads the one before it. A run abandoned at a
/// hold has nothing to resume — every one of its tasks completed — so its
/// intermediates are exactly the disk somebody asked to be rid of.
pub(crate) fn sweep_abandoned(conn: &mut SqliteConnection, library_root: &Path, run_id: &str) {
    let run: Option<(Option<String>, i32)> = runs::table
        .filter(runs::id.eq(run_id))
        .select((runs::line_id, runs::stage_count))
        .first(conn)
        .optional()
        .unwrap_or(None);
    if let Some((line_id, stage_count)) = run {
        discard_intermediates(
            conn,
            library_root,
            run_id,
            line_id.as_deref(),
            stage_count,
            Sweep::Abandoned,
        );
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

    // Only a run still in flight — running or parked at a hold — can be
    // cancelled. A cancel that arrives after the run settled, or races the
    // worker past its last landing, must not overwrite a terminal verdict.
    diesel::update(
        runs::table.filter(
            runs::id.eq(run_id).and(
                runs::status.eq_any([RunState::Running.as_str(), RunState::Held.as_str()]),
            ),
        ),
    )
    .set((
        runs::status.eq(RunState::Cancelled.as_str()),
        runs::finished_at.eq(&now),
        // A cancelled run is holding nothing: the takes are still there to
        // look at, but there is no verdict left to give about them.
        runs::held_at_stage.eq(None::<i32>),
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

        /// Mark a stage of the line as one that parks the run and asks.
        fn hold_at(&mut self, stage_idx: usize) {
            self.sql(&format!(
                "UPDATE line_stages SET hold_for_review = 1 WHERE stage_idx = {};",
                stage_idx
            ));
        }

        /// Sweep four seeds at one stage — the fan-out a hold point exists for.
        fn four_seeds_at(&mut self, stage_idx: usize) {
            self.sql(&format!(
                r#"UPDATE line_stages
                   SET parameters = '{{"3.seed":1000}}',
                       vary = '{{"3.seed":{{"count":4,"mode":"increment"}}}}'
                   WHERE stage_idx = {};"#,
                stage_idx
            ));
        }

        /// What a run is holding, if anything.
        fn hold(&mut self, run_id: &str) -> Option<crate::comfyui::holds::Hold> {
            crate::comfyui::holds::read_hold(&mut self.conn, run_id).unwrap()
        }

        /// A verdict, given the way the endpoint gives it.
        fn verdict(
            &mut self,
            run_id: &str,
            verdict: crate::comfyui::Verdict,
            keep: &[String],
        ) -> crate::comfyui::holds::Outcome {
            let root = self.dir.path().to_path_buf();
            crate::comfyui::holds::give_verdict(&mut self.conn, &root, run_id, verdict, keep, None)
                .unwrap()
        }

        /// Every task of the run at one stage, with its seed and its parent.
        fn takes_at(&mut self, stage_idx: i32) -> Vec<(String, i64, Option<String>, String)> {
            enhancement_tasks::table
                .filter(enhancement_tasks::stage_idx.eq(stage_idx))
                .order(enhancement_tasks::id.asc())
                .select((
                    enhancement_tasks::id,
                    enhancement_tasks::parameters,
                    enhancement_tasks::parent_task_id,
                    enhancement_tasks::status,
                ))
                .load::<(String, Option<String>, Option<String>, String)>(&mut self.conn)
                .unwrap()
                .into_iter()
                .map(|(id, params, parent, status)| {
                    let seed =
                        serde_json::from_str::<serde_json::Value>(&params.unwrap_or_default())
                            .ok()
                            .and_then(|p| p["3.seed"].as_i64())
                            .unwrap_or(-1);
                    (id, seed, parent, status)
                })
                .collect()
        }

        fn verdict_rows(&mut self) -> Vec<(String, i32, Vec<String>, Vec<String>)> {
            crate::schema::run_holds::table
                .order(crate::schema::run_holds::id.asc())
                .select((
                    crate::schema::run_holds::verdict,
                    crate::schema::run_holds::stage_idx,
                    crate::schema::run_holds::reviewed_task_ids,
                    crate::schema::run_holds::kept_task_ids,
                ))
                .load::<(String, i32, String, String)>(&mut self.conn)
                .unwrap()
                .into_iter()
                .map(|(v, idx, r, k)| {
                    (
                        v,
                        idx,
                        serde_json::from_str(&r).unwrap_or_default(),
                        serde_json::from_str(&k).unwrap_or_default(),
                    )
                })
                .collect()
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
    fn cancelling_a_run_that_already_landed_does_not_rewrite_its_verdict() {
        let mut lib = Library::with_workflows(1);
        lib.line(1, &[]);
        let run = lib.start();
        lib.land(&run.task_ids[0]);
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "completed");

        // A cancel that arrives after the fact stops nothing and changes
        // nothing: completed is a verdict, not a phase.
        let stopped = crate::comfyui::cancel_run(&mut lib.conn, &run.run_id).unwrap();
        assert_eq!(stopped, 0);
        assert_eq!(lib.run_status(&run.run_id).0, "completed");
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

    // === FR9 — a describe stage, and the prompt it writes ====================

    /// A graph that reads a photograph and previews a sentence: no saver, no
    /// file, `produces: text`.
    const DESCRIBE_GRAPH: &str = r#"{
        "1": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
        "2": {"class_type": "QwenVLRun",
              "inputs": {"image": ["1", 0], "prompt": "describe this photograph"}},
        "8": {"class_type": "PreviewText", "inputs": {"text": ["2", 0]}}
    }"#;

    /// The contract a server that can be asked derives for it.
    ///
    /// Stored rather than derived on the spot, because a custom VL node is
    /// invisible to the offline heuristics — they only surface string fields on
    /// classes named like text nodes — which is the whole reason
    /// `comfyui_workflows.contract_json` exists and the worker backfills it
    /// from `/object_info`.
    const DESCRIBE_CONTRACT: &str = r#"{
        "accepts": "image", "produces": "text",
        "slots": [{"name": "positive", "node_id": "2", "field": "prompt",
                   "multiline": true}]
    }"#;

    /// A graph with a prompt and a negative prompt, told apart by which
    /// sampler socket each is wired into.
    const GENERATE_GRAPH: &str = r#"{
        "3": {"class_type": "KSampler",
              "inputs": {"seed": 1, "steps": 20, "cfg": 8.0,
                         "positive": ["6", 0], "negative": ["7", 0]}},
        "4": {"class_type": "LoadImage", "inputs": {"image": "example.png"}},
        "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a photograph"}},
        "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "blurry"}},
        "9": {"class_type": "SaveImage",
              "inputs": {"filename_prefix": "out", "images": ["3", 0]}}
    }"#;

    impl Library {
        /// The line FR9 is about: describe, then generate from the description.
        ///
        /// `directives` are the compiler's `phos:` keys, set on the describe
        /// stage — which is where a person setting up a line would type them.
        fn describe_then_generate(directives: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join(".phos.db");
            crate::db::init_and_migrate(&db_path).unwrap();
            let conn = crate::db::open_diesel_connection(&db_path).unwrap();
            let mut lib = Library { dir, conn };
            std::fs::write(lib.root().join("original.jpg"), b"jpeg").unwrap();
            lib.sql(&format!(
                "INSERT INTO comfyui_workflows (id, name, workflow_json, contract_json) \
                   VALUES ('wf-describe', 'Describe', '{describe}', '{contract}');
                 INSERT INTO comfyui_workflows (id, name, workflow_json) \
                   VALUES ('wf-gen', 'Photo to clip', '{generate}');
                 INSERT INTO people (id, name) VALUES ('p-anna', 'Anna');
                 INSERT INTO shots (id, timestamp, latitude, longitude, description, \
                                    primary_person_id) \
                   VALUES ('shot-1', '2019-07-14 19:12:03', 59.3293, 18.0686, \
                           'a woman sitting on a wooden jetty', 'p-anna');
                 INSERT INTO files (id, shot_id, path, hash, mime_type, is_original) \
                   VALUES ('file-orig', 'shot-1', 'original.jpg', 'h0', 'image/jpeg', 1);
                 INSERT INTO faces (id, file_id, person_id) \
                   VALUES ('face-1', 'file-orig', 'p-anna');
                 INSERT INTO production_lines (id, name) VALUES ('line-1', 'Describe then clip');
                 INSERT INTO line_stages (id, line_id, stage_idx, workflow_id, text_overrides) \
                   VALUES ('st-0', 'line-1', 0, 'wf-describe', '{directives}');
                 INSERT INTO line_stages (id, line_id, stage_idx, workflow_id) \
                   VALUES ('st-1', 'line-1', 1, 'wf-gen');",
                describe = DESCRIBE_GRAPH.replace('\'', "''"),
                contract = DESCRIBE_CONTRACT.replace('\'', "''"),
                generate = GENERATE_GRAPH.replace('\'', "''"),
                directives = directives.replace('\'', "''"),
            ));
            lib
        }

        /// What a describe stage does when it works: a sentence on the task and
        /// no file anywhere.
        fn describe(&mut self, task_id: &str, text: &str) {
            diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(task_id)))
                .set((
                    enhancement_tasks::status.eq("completed"),
                    enhancement_tasks::text_output.eq(text),
                    enhancement_tasks::completed_at.eq("2026-08-30 12:00:00"),
                ))
                .execute(&mut self.conn)
                .unwrap();
        }

        fn overrides_of(&mut self, task_id: &str) -> HashMap<String, String> {
            let raw: Option<String> = enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(task_id))
                .select(enhancement_tasks::text_overrides)
                .first(&mut self.conn)
                .unwrap();
            serde_json::from_str(&raw.unwrap_or_default()).unwrap_or_default()
        }
    }

    const ANSWER: &str = r#"{
        "subject": "Anna, seated on a weathered jetty, looking out over the water",
        "setting": "a still lake at dusk",
        "lighting": "low warm sun from camera left",
        "camera": "35mm, waist-up",
        "motion_affordance": "hair and water could move; the subject is seated",
        "do_not": ["warp hands"]
    }"#;

    #[test]
    fn a_describe_stage_writes_the_next_stages_prompt() {
        let mut lib = Library::describe_then_generate(
            r#"{"phos:intent": "a slow push-in as the light fades",
                 "phos:style": "35mm film, muted palette",
                 "phos:do_not": "change face"}"#,
        );
        let run = lib.start();

        // Stage 1 is the describe stage, and Phos wrote its instruction: the
        // names clustering found, the EXIF date and place, the caption. Not
        // the person's intent or constraints — the answer is cached per shot
        // and reused by lines with different ones, so the description must be
        // about the photograph alone.
        let instruction = lib.overrides_of(&run.task_ids[0])["2.prompt"].clone();
        assert!(instruction.contains("Anna"), "{}", instruction);
        assert!(instruction.contains("2019-07-14"), "{}", instruction);
        assert!(instruction.contains("59.3293"), "{}", instruction);
        assert!(
            instruction.contains("a woman sitting on a wooden jetty"),
            "{}",
            instruction
        );
        assert!(!instruction.contains("change face"), "{}", instruction);
        assert!(
            !instruction.contains("a slow push-in as the light fades"),
            "{}",
            instruction
        );

        // It answers, and makes no file at all.
        lib.describe(&run.task_ids[0], ANSWER);
        assert_eq!(
            files::table
                .filter(files::is_original.eq(false))
                .count()
                .get_result::<i64>(&mut lib.conn)
                .unwrap(),
            0,
            "a text stage writes no files row"
        );

        lib.advance();

        // Stage 2 is queued, reading the same photograph — the description
        // changed nothing about what the line is carrying — with the compiled
        // prompt in the slot the contract names.
        let stage2 = lib.pending_at(1);
        assert_eq!(stage2.len(), 1);
        let (source, parent): (Option<String>, Option<String>) = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&stage2[0]))
            .select((
                enhancement_tasks::source_file_id,
                enhancement_tasks::parent_task_id,
            ))
            .first(&mut lib.conn)
            .unwrap();
        assert_eq!(source, None, "still the shot's own photograph");
        assert_eq!(parent, Some(run.task_ids[0].clone()));

        let overrides = lib.overrides_of(&stage2[0]);
        // `6.text` is `StageContract::slot("positive").override_key()` — the key
        // `prepare_workflow` substitutes on, and no new plumbing anywhere.
        let positive = &overrides["6.text"];
        assert!(
            positive.starts_with("Anna, seated on a weathered jetty"),
            "{}",
            positive
        );
        assert!(
            positive.contains("hair and water could move"),
            "{}",
            positive
        );
        // The style and the intent were typed once, on the describe stage, and
        // the stage after it inherited them.
        assert!(
            positive.contains("35mm film, muted palette"),
            "{}",
            positive
        );
        assert!(
            positive.contains("a slow push-in as the light fades"),
            "{}",
            positive
        );
        // Constraints never reach the positive prompt; they join the negative
        // one the workflow's author already wrote.
        assert!(!positive.contains("change face"), "{}", positive);
        assert_eq!(overrides["7.text"], "blurry, warp hands, change face");
    }

    #[test]
    fn the_slot_directive_on_the_describe_stage_names_the_next_stages_box() {
        // `phos:slot` is typed on the describe stage — "put my answer in that
        // box of the stage after me" — and has to survive the trip down, since
        // `bind_description` reads the map it binds into.
        let mut lib = Library::describe_then_generate(r#"{"phos:slot": "negative"}"#);
        let run = lib.start();
        lib.describe(&run.task_ids[0], "A woman sits on a jetty at dusk.");
        lib.advance();
        let stage2 = lib.pending_at(1);
        let overrides = lib.overrides_of(&stage2[0]);
        assert!(
            overrides["7.text"].contains("A woman sits on a jetty at dusk"),
            "{:?}",
            overrides
        );
        assert!(!overrides.contains_key("6.text"), "{:?}", overrides);
    }

    #[test]
    fn a_description_the_model_wrote_as_prose_is_still_used() {
        let mut lib = Library::describe_then_generate("{}");
        let run = lib.start();
        lib.describe(&run.task_ids[0], "A woman sits on a jetty at dusk.");
        lib.advance();
        let stage2 = lib.pending_at(1);
        assert_eq!(
            lib.overrides_of(&stage2[0])["6.text"],
            "A woman sits on a jetty at dusk."
        );
    }

    // === FR5c — hold points ==================================================

    use crate::comfyui::Verdict;

    /// The line the whole feature exists for: make four candidates cheaply,
    /// stop, and only spend the expensive stage on what somebody kept.
    fn extend_then_upscale() -> (Library, crate::comfyui::runs::RunStart) {
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[]);
        lib.four_seeds_at(1);
        lib.hold_at(1);
        let run = lib.start();
        (lib, run)
    }

    /// Get a held run all the way to its hold: stage 1 lands, four takes land.
    fn park(lib: &mut Library, run: &crate::comfyui::runs::RunStart) -> Vec<String> {
        lib.land(&run.task_ids[0]);
        lib.advance();
        let takes = lib.pending_at(1);
        for take in &takes {
            lib.land(take);
        }
        lib.advance();
        takes
    }

    #[test]
    fn four_takes_at_a_hold_point_park_the_run_instead_of_upscaling_all_four() {
        let (mut lib, run) = extend_then_upscale();

        lib.land(&run.task_ids[0]);
        lib.advance();
        let takes = lib.pending_at(1);
        assert_eq!(takes.len(), 4, "one request, four candidates");

        // Three landed, one still rendering: nothing to review yet, and
        // certainly nothing to park on — a person shown one of four takes is
        // not the feature.
        for take in &takes[..3] {
            lib.land(take);
        }
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "running");
        assert!(lib.hold(&run.run_id).is_none());

        // The fourth lands. Now the run parks.
        lib.land(&takes[3]);
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "held");
        assert_eq!(
            lib.pending_at(2),
            Vec::<String>::new(),
            "and not one second of upscaling was spent"
        );

        let hold = lib.hold(&run.run_id).expect("the run is holding");
        assert_eq!(hold.stage_idx, 1);
        assert_eq!(hold.takes.len(), 4);
        assert_eq!(
            hold.fanouts,
            vec![1],
            "one upscale per take, which is what continuing costs"
        );
        assert_eq!(
            runs::table
                .filter(runs::id.eq(&run.run_id))
                .select(runs::held_at_stage)
                .first::<Option<i32>>(&mut lib.conn)
                .unwrap(),
            Some(1)
        );

        // A hold with no verdict stays held, tick after tick, for as long as it
        // takes. Nothing here expires.
        lib.advance();
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "held");
        assert_eq!(lib.hold(&run.run_id).unwrap().takes.len(), 4);
    }

    #[test]
    fn continuing_with_two_of_four_upscales_exactly_those_two() {
        let (mut lib, run) = extend_then_upscale();
        let _takes = park(&mut lib, &run);
        let hold = lib.hold(&run.run_id).unwrap();
        let offered: Vec<String> = hold.takes.iter().map(|t| t.task_id.clone()).collect();

        // Keep the first and the third.
        let keep = vec![offered[0].clone(), offered[2].clone()];
        let outcome = lib.verdict(&run.run_id, Verdict::Continue, &keep);
        assert_eq!(outcome.kept, keep);
        assert_eq!(
            outcome.reviewed.len(),
            4,
            "the verdict was given over all four, not only over the two it kept"
        );
        assert_eq!(lib.run_status(&run.run_id).0, "running");

        // The worker queues the continuations, through the same code every
        // other continuation goes through.
        lib.advance();
        let upscales = lib.pending_at(2);
        assert_eq!(upscales.len(), 2, "two takes, two upscales");
        let mut parents: Vec<String> = enhancement_tasks::table
            .filter(enhancement_tasks::stage_idx.eq(2))
            .select(enhancement_tasks::parent_task_id.assume_not_null())
            .load(&mut lib.conn)
            .unwrap();
        parents.sort();
        let mut expected = keep.clone();
        expected.sort();
        assert_eq!(parents, expected, "and each reads the take it was kept for");

        // The two nobody chose have no children, and never get any.
        for passed in [&offered[1], &offered[3]] {
            assert_eq!(
                enhancement_tasks::table
                    .filter(enhancement_tasks::parent_task_id.eq(passed))
                    .count()
                    .get_result::<i64>(&mut lib.conn)
                    .unwrap(),
                0,
                "a passed-over take does not run the rest of the line"
            );
        }

        // And the run does not park again on them: they were reviewed.
        lib.advance();
        assert_eq!(lib.pending_at(2).len(), 2, "still two, not four");
        assert_eq!(lib.run_status(&run.run_id).0, "running");

        // It walks to the end like any other run.
        for task in &upscales {
            lib.land(task);
        }
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "completed");
    }

    #[test]
    fn one_verdict_keeping_three_takes_queues_each_child_exactly_once() {
        // The idempotence property, under the case that could break it: one
        // verdict, several continuations. `parent_task_id` is still the only
        // marker, and every extra tick has to be a no-op.
        let (mut lib, run) = extend_then_upscale();
        park(&mut lib, &run);
        let offered: Vec<String> = lib
            .hold(&run.run_id)
            .unwrap()
            .takes
            .iter()
            .map(|t| t.task_id.clone())
            .collect();

        lib.verdict(&run.run_id, Verdict::Continue, &offered[..3]);
        for _ in 0..5 {
            lib.advance();
        }
        let children = lib.takes_at(2);
        assert_eq!(children.len(), 3, "three takes, three children, five ticks");
        let mut parents: Vec<String> = children.iter().filter_map(|c| c.2.clone()).collect();
        parents.sort();
        parents.dedup();
        assert_eq!(parents.len(), 3, "one child each, and no child twice");
    }

    #[test]
    fn regenerating_gives_fresh_seeds_changes_nothing_else_and_holds_again() {
        let (mut lib, run) = extend_then_upscale();
        let first = park(&mut lib, &run);
        let before = lib.takes_at(1);
        let mut seeds: Vec<i64> = before.iter().map(|t| t.1).collect();
        seeds.sort();
        assert_eq!(seeds, [1000, 1001, 1002, 1003]);
        let parent = before[0].2.clone();
        let source: Option<String> = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&first[0]))
            .select(enhancement_tasks::source_file_id)
            .first(&mut lib.conn)
            .unwrap();

        let outcome = lib.verdict(&run.run_id, Verdict::Regenerate, &[]);
        assert_eq!(
            outcome.queued.len(),
            4,
            "another four takes, queued at once"
        );
        assert!(outcome.kept.is_empty());

        // Fresh seeds, and the sweep kept the shape somebody asked for.
        let after = lib.takes_at(1);
        assert_eq!(after.len(), 8, "the old generation is still on the row");
        let mut fresh: Vec<i64> = after
            .iter()
            .filter(|t| outcome.queued.contains(&t.0))
            .map(|t| t.1)
            .collect();
        fresh.sort();
        assert_eq!(fresh, [1004, 1005, 1006, 1007]);

        // Nothing else moved. Said as strongly as it can be said: take one of
        // the old rows and one of the new, drop the seed from each, and the two
        // must be the same map. Same workflow, same source, same parent.
        let without_seed = |lib: &mut Library, id: &str| -> serde_json::Value {
            let raw: Option<String> = enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(id))
                .select(enhancement_tasks::parameters)
                .first(&mut lib.conn)
                .unwrap();
            let mut params: serde_json::Value =
                serde_json::from_str(&raw.unwrap_or_default()).unwrap();
            params.as_object_mut().unwrap().remove("3.seed");
            params
        };
        let was = without_seed(&mut lib, &first[0]);
        for (id, _, new_parent, _) in after
            .iter()
            .filter(|t| outcome.queued.contains(&t.0))
            .cloned()
            .collect::<Vec<_>>()
        {
            assert_eq!(new_parent, parent, "read by the same stage-1 output");
            let (wf, src): (String, Option<String>) = enhancement_tasks::table
                .filter(enhancement_tasks::id.eq(&id))
                .select((
                    enhancement_tasks::workflow_id,
                    enhancement_tasks::source_file_id,
                ))
                .first(&mut lib.conn)
                .unwrap();
            assert_eq!(wf, "wf-1");
            assert_eq!(src, source);
            assert_eq!(without_seed(&mut lib, &id), was, "only the seed moved");
        }

        // The run is alive again, and holds on the *new* takes only — a verdict
        // was already given over the old ones.
        assert_eq!(lib.run_status(&run.run_id).0, "running");
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "running", "still rendering");
        for id in &outcome.queued {
            lib.land(id);
        }
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "held");
        let hold = lib.hold(&run.run_id).unwrap();
        assert_eq!(hold.takes.len(), 4, "the new generation, not eight");
        let offered: Vec<&String> = hold.takes.iter().map(|t| &t.task_id).collect();
        for old in &first {
            assert!(!offered.contains(&old), "a decided take is not re-offered");
        }

        // And every verdict is on the record, in the order they were given.
        let rows = lib.verdict_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "regenerate");
        assert_eq!(rows[0].1, 1);
        assert_eq!(rows[0].2.len(), 4, "given over the four it replaced");
        assert!(rows[0].3.is_empty(), "and kept none of them");
    }

    #[test]
    fn regenerating_discards_the_takes_it_replaced_unless_the_stage_keeps_them() {
        let (mut lib, run) = extend_then_upscale();
        park(&mut lib, &run);
        let outputs: Vec<String> = enhancement_tasks::table
            .filter(enhancement_tasks::stage_idx.eq(1))
            .select(enhancement_tasks::output_file_id.assume_not_null())
            .load(&mut lib.conn)
            .unwrap();
        lib.verdict(&run.run_id, Verdict::Regenerate, &[]);
        for file_id in &outputs {
            assert!(
                !lib.file_exists(file_id),
                "a generation somebody replaced is not worth the disk"
            );
        }

        // With `keep` on the held stage, every generation stays.
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[1]);
        lib.four_seeds_at(1);
        lib.hold_at(1);
        let run = lib.start();
        park(&mut lib, &run);
        let outputs: Vec<String> = enhancement_tasks::table
            .filter(enhancement_tasks::stage_idx.eq(1))
            .select(enhancement_tasks::output_file_id.assume_not_null())
            .load(&mut lib.conn)
            .unwrap();
        lib.verdict(&run.run_id, Verdict::Regenerate, &[]);
        for file_id in &outputs {
            assert!(lib.file_exists(file_id), "the stage asked to keep them");
        }
    }

    #[test]
    fn cancelling_a_hold_abandons_the_run_and_removes_what_no_stage_keeps() {
        // Stage 1's output is worth keeping; the takes are not.
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[0]);
        lib.four_seeds_at(1);
        lib.hold_at(1);
        let run = lib.start();
        let stage1_out: String = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&run.task_ids[0]))
            .select(enhancement_tasks::id)
            .first(&mut lib.conn)
            .unwrap();
        let _ = stage1_out;
        lib.land(&run.task_ids[0]);
        lib.advance();
        let kept_output: String = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&run.task_ids[0]))
            .select(enhancement_tasks::output_file_id.assume_not_null())
            .first(&mut lib.conn)
            .unwrap();
        for take in lib.pending_at(1) {
            lib.land(&take);
        }
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "held");
        let take_outputs: Vec<String> = enhancement_tasks::table
            .filter(enhancement_tasks::stage_idx.eq(1))
            .select(enhancement_tasks::output_file_id.assume_not_null())
            .load(&mut lib.conn)
            .unwrap();

        let outcome = lib.verdict(&run.run_id, Verdict::Cancel, &[]);
        assert_eq!(outcome.reviewed.len(), 4);
        assert_eq!(lib.run_status(&run.run_id).0, "cancelled");

        for file_id in &take_outputs {
            assert!(
                !lib.file_exists(file_id),
                "nobody is going to choose one of these now"
            );
            assert!(!lib.root().join(format!("{}.png", file_id)).exists());
        }
        assert!(
            lib.file_exists(&kept_output),
            "the stage that said keep still means it"
        );
        // And there is nothing left to resume: every task of the run completed.
        assert_eq!(
            crate::comfyui::retry_run(&mut lib.conn, &run.run_id).unwrap(),
            0
        );
    }

    #[test]
    fn a_run_that_lands_keeps_the_takes_it_was_picked_out_of() {
        let (mut lib, run) = extend_then_upscale();
        park(&mut lib, &run);
        let offered: Vec<String> = lib
            .hold(&run.run_id)
            .unwrap()
            .takes
            .iter()
            .map(|t| t.task_id.clone())
            .collect();
        let take_outputs: Vec<String> = enhancement_tasks::table
            .filter(enhancement_tasks::stage_idx.eq(1))
            .select(enhancement_tasks::output_file_id.assume_not_null())
            .load(&mut lib.conn)
            .unwrap();
        let stage1_out: String = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&run.task_ids[0]))
            .select(enhancement_tasks::output_file_id.assume_not_null())
            .first(&mut lib.conn)
            .unwrap();

        lib.verdict(&run.run_id, Verdict::Continue, &[offered[0].clone()]);
        lib.advance();
        let upscale = lib.pending_at(2)[0].clone();
        lib.land(&upscale);
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "completed");

        assert!(
            !lib.file_exists(&stage1_out),
            "an ordinary intermediate is swept as it always was"
        );
        for file_id in &take_outputs {
            assert!(
                lib.file_exists(file_id),
                "but the takes are what somebody chose between, and they stay"
            );
        }
    }

    #[test]
    fn a_held_run_survives_a_restart_and_is_never_swept_or_settled() {
        let (mut lib, run) = extend_then_upscale();
        park(&mut lib, &run);
        assert_eq!(lib.run_status(&run.run_id).0, "held");

        // The data-loss bug the pass's order exists to prevent, in the shape a
        // hold gives it. Every task of this run has completed, so a settle that
        // did not know about the hold would call it *finished* — and the sweep
        // that follows a finished run would delete the stage-1 clip that the
        // upscale a person is about to ask for reads. It is still there.
        let feeds_the_takes: String = enhancement_tasks::table
            .filter(enhancement_tasks::id.eq(&run.task_ids[0]))
            .select(enhancement_tasks::output_file_id.assume_not_null())
            .first(&mut lib.conn)
            .unwrap();
        assert!(
            lib.file_exists(&feeds_the_takes),
            "a held run is not settled, so nothing sweeps what its next stage will read"
        );

        // The process comes back. Recovery touches tasks by status, and a held
        // run's are all completed, so it has nothing to say about them.
        super::super::recover_interrupted_tasks(&mut lib.conn);
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "held");
        assert_eq!(lib.hold(&run.run_id).unwrap().takes.len(), 4);

        // Nor is it settled: `finished_at` is what a run that is over has, and
        // a run waiting on a person has not finished.
        assert_eq!(
            runs::table
                .filter(runs::id.eq(&run.run_id))
                .select(runs::finished_at)
                .first::<Option<String>>(&mut lib.conn)
                .unwrap(),
            None
        );

        // And the five-minute sweep leaves its takes alone, however long the
        // person takes to look: the tasks are the takes.
        lib.sql("UPDATE enhancement_tasks SET completed_at = '2020-01-01 00:00:00';");
        super::super::cleanup_completed_tasks(&mut lib.conn);
        assert_eq!(lib.tasks().len(), 5, "one stage-1 task and four takes");
        assert_eq!(lib.hold(&run.run_id).unwrap().takes.len(), 4);
    }

    #[test]
    fn a_held_run_does_not_stop_the_queue_moving() {
        // The deadlock this feature could obviously cause, and does not: a held
        // run is out of the advance pass entirely, so everything else keeps
        // going. FR7's cap on outstanding holds is the other half, and belongs
        // where batches are fed rather than here.
        let (mut lib, held) = extend_then_upscale();
        park(&mut lib, &held);
        assert_eq!(lib.run_status(&held.run_id).0, "held");

        let other = lib.start();
        lib.land(&other.task_ids[0]);
        lib.advance();
        assert_eq!(
            enhancement_tasks::table
                .filter(
                    enhancement_tasks::run_id
                        .eq(&other.run_id)
                        .and(enhancement_tasks::stage_idx.eq(1))
                )
                .count()
                .get_result::<i64>(&mut lib.conn)
                .unwrap(),
            4,
            "the second run walked straight past the first one's hold"
        );
        assert_eq!(
            lib.run_status(&held.run_id).0,
            "held",
            "and it is still held"
        );
    }

    #[test]
    fn a_take_that_broke_fails_the_run_and_the_hold_is_what_the_retry_arrives_at() {
        let (mut lib, run) = extend_then_upscale();
        lib.land(&run.task_ids[0]);
        lib.advance();
        let takes = lib.pending_at(1);
        lib.break_task(&takes[0], "CUDA out of memory");
        for take in &takes[1..] {
            lib.land(take);
        }
        lib.advance();

        // A failure outranks a hold: showing three of four takes and calling it
        // a choice would be the wrong answer to a broken GPU.
        let (status, error) = lib.run_status(&run.run_id);
        assert_eq!(status, "failed");
        assert_eq!(error.as_deref(), Some("CUDA out of memory"));
        assert!(lib.hold(&run.run_id).is_none());

        // Retry, and it lands where it was always going to.
        crate::comfyui::retry_run(&mut lib.conn, &run.run_id).unwrap();
        lib.land(&takes[0]);
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "held");
        assert_eq!(lib.hold(&run.run_id).unwrap().takes.len(), 4);
    }

    #[test]
    fn a_verdict_is_refused_when_there_is_nothing_to_give_one_on() {
        let (mut lib, run) = extend_then_upscale();
        let root = lib.dir.path().to_path_buf();

        // Not held yet.
        let err = crate::comfyui::holds::give_verdict(
            &mut lib.conn,
            &root,
            &run.run_id,
            Verdict::Continue,
            &["whatever".to_string()],
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, crate::comfyui::holds::HoldError::NotHeld),
            "{}",
            err
        );

        // No such run.
        let err = crate::comfyui::holds::give_verdict(
            &mut lib.conn,
            &root,
            "nobody",
            Verdict::Cancel,
            &[],
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, crate::comfyui::holds::HoldError::NotFound),
            "{}",
            err
        );

        // Held, but the verdict names a take from somewhere else.
        park(&mut lib, &run);
        let err = crate::comfyui::holds::give_verdict(
            &mut lib.conn,
            &root,
            &run.run_id,
            Verdict::Continue,
            &["a-take-from-another-run".to_string()],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not one of the takes"), "{}", err);
        assert_eq!(
            lib.run_status(&run.run_id).0,
            "held",
            "and a refused verdict changes nothing"
        );
        assert!(lib.verdict_rows().is_empty());
    }

    #[test]
    fn a_held_run_with_nothing_left_to_look_at_can_still_be_abandoned() {
        // The verdict's writes are one transaction precisely so this state
        // cannot arise from a crash — but a take deleted by hand can still
        // produce it, and a run nothing can clear is worse than a run that
        // stopped. Abandoning is the verdict that is never refused.
        let (mut lib, run) = extend_then_upscale();
        park(&mut lib, &run);
        lib.sql("DELETE FROM enhancement_tasks WHERE stage_idx = 1;");
        assert_eq!(lib.hold(&run.run_id).unwrap().takes.len(), 0);

        let root = lib.dir.path().to_path_buf();
        assert!(crate::comfyui::holds::give_verdict(
            &mut lib.conn,
            &root,
            &run.run_id,
            Verdict::Regenerate,
            &[],
            None,
        )
        .is_err());

        lib.verdict(&run.run_id, Verdict::Cancel, &[]);
        assert_eq!(lib.run_status(&run.run_id).0, "cancelled");
    }

    #[test]
    fn a_held_row_with_no_stage_on_it_is_still_stoppable() {
        // The fall-through in the API's cancel handler, at the level the
        // handler decides on. Cancelling a held run goes through the hold's own
        // Cancel verdict, so an abandoned hold is recorded like every other;
        // but if the run turns out not to be holding anything the handler must
        // go on to the ordinary cancel rather than refusing, because stopping a
        // run is the one thing that has to work in every state.
        //
        // Nothing writes this row: `park_run` and `release` set the status and
        // the stage together, and the verdict's writes are one transaction. A
        // hand on the database can, which is the whole reason the branch exists.
        let (mut lib, run) = extend_then_upscale();
        park(&mut lib, &run);
        lib.sql("UPDATE runs SET held_at_stage = NULL;");

        // Still `held`, so the handler tries the verdict — and gets back the
        // one error that means "go on", rather than a refusal to report.
        assert_eq!(lib.run_status(&run.run_id).0, "held");
        assert!(
            lib.hold(&run.run_id).is_none(),
            "a held row with no stage is holding nothing, and says so"
        );
        let root = lib.dir.path().to_path_buf();
        let err = crate::comfyui::holds::give_verdict(
            &mut lib.conn,
            &root,
            &run.run_id,
            Verdict::Cancel,
            &[],
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, crate::comfyui::holds::HoldError::NotHeld),
            "{}",
            err
        );
        assert!(
            lib.verdict_rows().is_empty(),
            "and it recorded no verdict on the way out"
        );

        // The ordinary cancel finishes the job, and takes the stale marker with
        // it: a cancelled run is holding nothing.
        crate::comfyui::cancel_run(&mut lib.conn, &run.run_id).unwrap();
        assert_eq!(lib.run_status(&run.run_id).0, "cancelled");
        assert_eq!(
            runs::table
                .filter(runs::id.eq(&run.run_id))
                .select(runs::held_at_stage)
                .first::<Option<i32>>(&mut lib.conn)
                .unwrap(),
            None
        );

        // And it stays stopped: the advance pass reads running runs only, so
        // nothing re-parks it and nothing upscales its takes.
        lib.advance();
        assert_eq!(lib.run_status(&run.run_id).0, "cancelled");
        assert_eq!(lib.pending_at(2), Vec::<String>::new());
    }

    #[test]
    fn a_hold_under_a_fan_out_reviews_every_branch_together() {
        // Two takes at stage 1, four seeds each at stage 2, holding at stage 2:
        // eight takes in two groups, one verdict over all of them, and a
        // regenerate that re-runs both groups.
        let mut lib = Library::with_workflows(3);
        lib.line(3, &[]);
        lib.sql(
            r#"UPDATE line_stages
               SET parameters = '{"3.seed":1}', vary = '{"3.seed":{"count":2,"mode":"increment"}}'
               WHERE stage_idx = 0;"#,
        );
        lib.four_seeds_at(1);
        lib.hold_at(1);
        let run = lib.start();
        assert_eq!(run.task_ids.len(), 2);
        for task in &run.task_ids {
            lib.land(task);
        }
        lib.advance();
        let takes = lib.pending_at(1);
        assert_eq!(takes.len(), 8, "two branches, four takes each");
        for take in &takes {
            lib.land(take);
        }
        lib.advance();

        let hold = lib.hold(&run.run_id).unwrap();
        assert_eq!(hold.takes.len(), 8, "one verdict over both branches");

        let outcome = lib.verdict(&run.run_id, Verdict::Regenerate, &[]);
        assert_eq!(outcome.queued.len(), 8, "and both branches run again");
        let parents: std::collections::HashSet<Option<String>> = lib
            .takes_at(1)
            .into_iter()
            .filter(|t| outcome.queued.contains(&t.0))
            .map(|t| t.2)
            .collect();
        assert_eq!(parents.len(), 2, "each fresh take reads its own branch");
    }

    #[test]
    fn a_describe_stage_can_hold_because_a_take_is_a_task() {
        // A hold does not need a picture. Four sentences are four takes, and
        // naming them by task id rather than by file id is what makes that fall
        // out rather than needing a case.
        let mut lib = Library::describe_then_generate("{}");
        lib.sql(
            r#"UPDATE line_stages SET hold_for_review = 1, vary = '{"2.x":{"values":[1,2]}}'
               WHERE stage_idx = 0;"#,
        );
        let run = lib.start();
        assert_eq!(run.task_ids.len(), 2);
        lib.describe(&run.task_ids[0], "A woman sits on a jetty at dusk.");
        lib.describe(&run.task_ids[1], "A jetty at dusk, empty.");
        lib.advance();

        assert_eq!(lib.run_status(&run.run_id).0, "held");
        let hold = lib.hold(&run.run_id).unwrap();
        assert_eq!(hold.takes.len(), 2);
        let second = hold
            .takes
            .iter()
            .find(|t| t.task_id == run.task_ids[1])
            .expect("both sentences are on offer");
        assert_eq!(
            second.text_output.as_deref(),
            Some("A jetty at dusk, empty.")
        );
        assert!(second.output_file_id.is_none(), "no file anywhere");

        lib.verdict(
            &run.run_id,
            Verdict::Continue,
            std::slice::from_ref(&second.task_id),
        );
        lib.advance();
        let stage2 = lib.pending_at(1);
        assert_eq!(stage2.len(), 1);
        assert_eq!(
            lib.overrides_of(&stage2[0])["6.text"],
            "A jetty at dusk, empty.",
            "the sentence that was chosen is the one that was compiled"
        );
    }

    #[test]
    fn a_describe_stage_that_said_nothing_stops_the_run() {
        let mut lib = Library::describe_then_generate("{}");
        let run = lib.start();
        // Completed, but with neither a file nor a sentence: there is nothing
        // to hand on, and inventing an empty prompt would be worse than saying
        // so.
        diesel::update(enhancement_tasks::table.filter(enhancement_tasks::id.eq(&run.task_ids[0])))
            .set((
                enhancement_tasks::status.eq("completed"),
                enhancement_tasks::completed_at.eq("2026-08-30 12:00:00"),
            ))
            .execute(&mut lib.conn)
            .unwrap();
        lib.advance();
        let (status, error) = lib.run_status(&run.run_id);
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("without an output"));
    }
}
