//! Giving a verdict from the lane: rejecting takes, and reaching a batch.
//!
//! [`holds::give_verdict`] is the runtime and stays exactly as FR5c wrote it —
//! one run, three words, one transaction. Two things the curation lane needs
//! sit *around* it rather than inside it, and this is where they sit.
//!
//! # Reject deletes the bytes
//!
//! Generated video is enormous and a farm fills a disk in days, so the lane's
//! `X` key is not a tidy-up: it removes the file. What it is **not** is a
//! separate action that happens the instant the key is pressed. `X` arms a take
//! and the bytes go when the verdict is given, which buys three things at once:
//!
//! * pressing `X` costs one keystroke and no dialog, so the lane stays fast;
//! * pressing it again disarms, so the fast thing is also the reversible thing
//!   right up until the moment it is not;
//! * and the screen can say *how many megabytes* the next Enter is about to
//!   free, before it is pressed rather than after. That is the whole of
//!   "obvious enough that nobody does it by accident": not a confirmation
//!   nobody reads, a number that is always on screen.
//!
//! Rejecting is deliberately narrower than not-keeping. A take that is simply
//! passed over is disposed of by its stage's `keep_output` policy, which is the
//! line author's decision; a rejected take goes regardless, which is the
//! reviewer's. Both are recorded on the same `run_holds` row as *reviewed*.
//!
//! # A verdict may cover a batch
//!
//! At batch scale, deciding one run at a time is not enough — see
//! [`super::bulk`] for which runs a verdict reaches and why. What it may *say*
//! is narrower than what it says here:
//!
//! | Verdict | What a sibling gets |
//! |---|---|
//! | `cancel` | abandoned, same as the run in front of you |
//! | `regenerate` | fresh seeds, same as the run in front of you |
//! | `continue` | **all of its own waiting takes** — "I looked at a sample, let them through" |
//! | reject | *nothing*. Deleting bytes is something you do to pictures you have seen. |
//!
//! `continue` cannot carry a selection because task ids are per run: the four
//! takes somebody chose between here do not exist there. Resolving it to "all
//! of that run's takes" is the only meaning that is both well defined and
//! useful, and it is exactly the describe-stage case FR8 describes — every
//! description in the batch finishes, you read a handful, the rest go on.
//!
//! # One run's failure is not the batch's
//!
//! A sibling that stopped holding between the read and the write is skipped
//! with its reason kept, not rolled back over the nine hundred that worked. The
//! run the person actually looked at is always decided first, so if anything
//! goes wrong the deliberate decision is the one that survived.

use std::collections::HashSet;
use std::path::Path;

use diesel::sqlite::SqliteConnection;

use super::bulk::{self, Scope};
use super::held_runs;
use crate::comfyui::holds::{self, HoldError, Outcome};
use crate::comfyui::line::Verdict;

/// What the lane is asking for.
#[derive(Debug, Clone)]
pub(crate) struct Ask<'a> {
    pub verdict: Verdict,
    /// Takes that go on, by task id. Meaningful for `continue` only.
    pub keep: &'a [String],
    /// Takes whose bytes go, by task id. Never carried to another run.
    pub reject: &'a [String],
    pub note: Option<&'a str>,
    pub scope: Scope,
}

/// What giving it did.
#[derive(Debug)]
pub(crate) struct Applied {
    pub scope: Scope,
    /// The run the person was looking at, and what its verdict did.
    pub outcome: Outcome,
    /// Takes whose output files were removed from the library.
    pub rejected: Vec<String>,
    /// Bytes those files were, totalled before they went.
    pub freed_bytes: i64,
    /// Sibling runs the same verdict reached.
    pub also_applied: Vec<String>,
    /// Siblings it could not reach, and why. One of these is not a failure of
    /// the whole request.
    pub failed: Vec<(String, String)>,
}

/// Give one verdict, and let it reach as far as it was asked to.
pub(crate) fn apply(
    conn: &mut SqliteConnection,
    library_root: &Path,
    run_id: &str,
    ask: Ask<'_>,
) -> Result<Applied, HoldError> {
    let Some(hold) = holds::read_hold(conn, run_id)? else {
        return Err(HoldError::NotHeld);
    };

    let waiting: HashSet<&str> = hold.takes.iter().map(|t| t.task_id.as_str()).collect();
    let keeping: HashSet<&str> = ask.keep.iter().map(String::as_str).collect();
    for take in ask.reject {
        if !waiting.contains(take.as_str()) {
            return Err(HoldError::Refused(format!(
                "{} is not one of the takes this run is holding.",
                take
            )));
        }
        if keeping.contains(take.as_str()) {
            return Err(HoldError::Refused(format!(
                "{} is named as both kept and rejected. One of those deletes it.",
                take
            )));
        }
    }

    // Measured before the verdict, because after it the rows are gone. A
    // number nobody can check afterwards is worth reporting at the moment it
    // is still true.
    let freed_bytes = bytes_of(conn, ask.reject);

    // Which runs this reaches, worked out before anything is written: the
    // sibling list is a question about `held` rows, and the first verdict
    // changes them.
    let covered = match ask.scope {
        Scope::Run => vec![run_id.to_string()],
        Scope::Batch => {
            let held = held_runs(conn)?;
            let decided = held
                .iter()
                .find(|r| r.run_id == run_id)
                .cloned()
                // Held a moment ago and not in the list now is a race, not a
                // reason to widen: decide the one run and say so.
                .unwrap_or(bulk::HeldRun {
                    run_id: run_id.to_string(),
                    batch_id: None,
                    line_id: None,
                    held_at_stage: None,
                });
            bulk::covered(Scope::Batch, &decided, &held)
        }
    };

    // The run somebody actually looked at. Its failure is the request's.
    let outcome = holds::give_verdict(conn, library_root, run_id, ask.verdict, ask.keep, ask.note)?;

    // And only now the bytes, so a refused verdict has deleted nothing.
    let rejected = ask.reject.to_vec();
    if !rejected.is_empty() {
        crate::comfyui::discard_outputs(conn, library_root, &rejected);
    }

    let mut also_applied = Vec::new();
    let mut failed = Vec::new();
    for sibling in covered.into_iter().filter(|id| id != run_id) {
        match sibling_verdict(conn, library_root, &sibling, ask.verdict, ask.note) {
            Ok(()) => also_applied.push(sibling),
            Err(e) => failed.push((sibling, e.to_string())),
        }
    }

    Ok(Applied {
        scope: ask.scope,
        outcome,
        rejected,
        freed_bytes,
        also_applied,
        failed,
    })
}

/// The same verdict on a run nobody opened.
///
/// `continue` resolves to all of that run's waiting takes; the other two name
/// no takes anywhere, so they carry unchanged.
fn sibling_verdict(
    conn: &mut SqliteConnection,
    library_root: &Path,
    run_id: &str,
    verdict: Verdict,
    note: Option<&str>,
) -> Result<(), HoldError> {
    let keep: Vec<String> = match verdict {
        Verdict::Continue => match holds::read_hold(conn, run_id)? {
            Some(hold) => hold.takes.into_iter().map(|t| t.task_id).collect(),
            None => return Err(HoldError::NotHeld),
        },
        Verdict::Regenerate | Verdict::Cancel => Vec::new(),
    };
    holds::give_verdict(conn, library_root, run_id, verdict, &keep, note)?;
    Ok(())
}

/// How much disk these takes are, before it is freed.
fn bytes_of(conn: &mut SqliteConnection, task_ids: &[String]) -> i64 {
    use crate::schema::{enhancement_tasks, files};
    use diesel::prelude::*;

    if task_ids.is_empty() {
        return 0;
    }
    let file_ids: Vec<String> = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq_any(task_ids))
        .select(enhancement_tasks::output_file_id)
        .load::<Option<String>>(conn)
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();
    files::table
        .filter(files::id.eq_any(&file_ids))
        .select(files::file_size)
        .load::<Option<i32>>(conn)
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .map(i64::from)
        .sum()
}
