//! Every held run at once — what the Takes curation lane reads.
//!
//! [`super::holds`] answers *"what is this one run holding?"*, which is the
//! right question from the queue board, where a person has already picked a
//! row. The curation lane asks the other one: **"what is waiting for me?"** It
//! is a contact sheet over the whole backlog, worked through from the keyboard,
//! and the number it has to survive is two hundred takes in ten minutes.
//!
//! # What this adds to a hold
//!
//! Nothing about the hold itself — [`holds::read_hold`] stays the one
//! definition of what a run is holding, and this calls it. What is added is the
//! handful of facts a *picture* needs that a *task* does not:
//!
//! * whether the take is a clip or a still, so `space` means something and the
//!   card knows whether to draw a `<video>`;
//! * how many bytes it is, because rejecting is deleting and the lane says how
//!   much before the key is pressed rather than after;
//! * the rating somebody gave it, and whether it is already the shot's main
//!   file, so `P` can say *promoted* instead of asking;
//! * the file the held stage **read**, which is the original a take is compared
//!   against — the left-hand side of compare mode;
//! * FR7's `batch_id`, so a verdict can reach the rest of its batch.
//!
//! # Batched, because the backlog is the point
//!
//! A library that put three thousand shots through a hold point has three
//! thousand held runs. Every decoration here is one query for the whole page,
//! never one per row, for the same reason [`holds::held_take_counts`] is: a
//! screen that costs a query per card is a screen nobody opens twice.
//!
//! The exception is [`holds::read_hold`] itself, which is called once per run
//! and re-reads its line's stages to price the continuation. That is the honest
//! cost of the estimate being right, and it is why the page size here is two
//! dozen rather than two hundred.

pub(crate) mod bulk;
pub(crate) mod verdicts;

#[cfg(test)]
mod tests;

use super::holds::{self, Hold, HoldError};
use super::line::RunState;
use crate::schema::{enhancement_tasks, files, runs, shots};
use bulk::HeldRun;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::collections::HashMap;

/// How many held runs a page carries when the caller does not say.
pub(crate) const DEFAULT_PAGE: i64 = 24;
/// And the most it will carry however loudly they ask. Each row re-prices its
/// own continuation, so a page is a real amount of work.
pub(crate) const MAX_PAGE: i64 = 100;

/// What a take's picture is, beyond the task that made it.
#[derive(Debug, Clone, Default)]
pub(crate) struct TakeDetail {
    /// `image/png`, `video/mp4` — what decides whether `space` does anything.
    pub mime_type: Option<String>,
    /// Bytes. The lane totals these into what rejecting is about to free.
    pub file_size: Option<i64>,
    /// One to five, or nothing at all. Nothing is not zero.
    pub rating: Option<i32>,
    /// Already this shot's main file, so `P` has nothing to do.
    pub is_main_file: bool,
}

/// One held run as the lane draws it: the hold, plus what a picture needs.
#[derive(Debug, Clone)]
pub(crate) struct Sheet {
    pub hold: Hold,
    /// FR7's batch. `None` for a run started a shot at a time — and for every
    /// run that exists until FR7 lands. See [`batch_ids`].
    pub batch_id: Option<String>,
    /// The file the held stage was given to work from: the original these takes
    /// are variations of, and the left-hand side of compare mode. `None` for a
    /// stage that read no file, such as a describe stage fed by a prompt.
    pub source_file_id: Option<String>,
    /// The shot's main file as it stands, which is what `P` would replace.
    pub main_file_id: Option<String>,
    /// Keyed by the take's `output_file_id`. A take with no file — a describe
    /// stage's sentence — has no entry, which is why this is a map.
    pub details: HashMap<String, TakeDetail>,
    /// When the run was started. The lane's cursor.
    pub created_at: Option<String>,
}

/// A page of the backlog, oldest first, and the cursor for the next one.
///
/// Oldest first because this is a queue somebody is draining, not a feed they
/// are browsing: the take that has been waiting longest is the one holding up
/// a batch's outstanding-hold cap. The board, which *is* a feed, orders the
/// other way, and the two disagreeing is deliberate.
pub(crate) fn list_sheets(
    conn: &mut SqliteConnection,
    limit: i64,
    cursor: Option<&str>,
) -> Result<(Vec<Sheet>, Option<String>), HoldError> {
    let limit = limit.clamp(1, MAX_PAGE);

    let mut q = runs::table
        .filter(runs::status.eq(RunState::Held.as_str()))
        .filter(runs::held_at_stage.is_not_null())
        .into_boxed();
    if let Some(after) = cursor {
        q = q.filter(runs::created_at.gt(after));
    }
    let rows: Vec<(String, Option<String>)> = q
        .order((runs::created_at.asc(), runs::id.asc()))
        .limit(limit + 1)
        .select((runs::id, runs::created_at))
        .load(conn)?;

    let has_more = rows.len() as i64 > limit;
    let rows: Vec<(String, Option<String>)> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more {
        rows.last().and_then(|(_, at)| at.clone())
    } else {
        None
    };

    let mut holds_read = Vec::new();
    for (run_id, created_at) in &rows {
        // A run that stopped holding between the two queries simply is not on
        // this page. Nothing here is a transaction: the lane re-reads.
        if let Some(hold) = holds::read_hold(conn, run_id)? {
            holds_read.push((hold, created_at.clone()));
        }
    }

    Ok((decorate(conn, holds_read), next_cursor))
}

/// The facts a page of holds needs that a hold does not carry, in four queries
/// for the whole page.
fn decorate(conn: &mut SqliteConnection, held: Vec<(Hold, Option<String>)>) -> Vec<Sheet> {
    let file_ids: Vec<String> = held
        .iter()
        .flat_map(|(h, _)| h.takes.iter().filter_map(|t| t.output_file_id.clone()))
        .collect();
    let task_ids: Vec<String> = held
        .iter()
        .flat_map(|(h, _)| h.takes.iter().map(|t| t.task_id.clone()))
        .collect();
    let shot_ids: Vec<String> = held.iter().map(|(h, _)| h.shot_id.clone()).collect();
    let run_ids: Vec<String> = held.iter().map(|(h, _)| h.run_id.clone()).collect();

    let mains: HashMap<String, Option<String>> = shots::table
        .filter(shots::id.eq_any(&shot_ids))
        .select((shots::id, shots::main_file_id))
        .load::<(String, Option<String>)>(conn)
        .unwrap_or_default()
        .into_iter()
        .collect();

    // The file each take's task read. Every take of a fan-out read the same
    // one, which is exactly what makes compare mode a comparison.
    let sources: HashMap<String, Option<String>> = enhancement_tasks::table
        .filter(enhancement_tasks::id.eq_any(&task_ids))
        .select((enhancement_tasks::id, enhancement_tasks::source_file_id))
        .load::<(String, Option<String>)>(conn)
        .unwrap_or_default()
        .into_iter()
        .collect();

    let mut details: HashMap<String, TakeDetail> = files::table
        .filter(files::id.eq_any(&file_ids))
        .select((
            files::id,
            files::mime_type,
            files::file_size,
            files::rating,
            files::is_original,
        ))
        .load::<(
            String,
            Option<String>,
            Option<i32>,
            Option<i32>,
            Option<bool>,
        )>(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, mime, size, rating, original)| {
            (
                id,
                TakeDetail {
                    mime_type: mime,
                    file_size: size.map(i64::from),
                    rating,
                    is_main_file: original.unwrap_or(false),
                },
            )
        })
        .collect();

    let batches = batch_ids(conn, &run_ids);

    held.into_iter()
        .map(|(hold, created_at)| {
            let mine: HashMap<String, TakeDetail> = hold
                .takes
                .iter()
                .filter_map(|t| t.output_file_id.as_ref())
                .filter_map(|id| details.remove(id).map(|d| (id.clone(), d)))
                .collect();
            let source_file_id = hold
                .takes
                .iter()
                .find_map(|t| sources.get(&t.task_id).cloned().flatten());
            Sheet {
                batch_id: batches.get(&hold.run_id).cloned(),
                main_file_id: mains.get(&hold.shot_id).cloned().flatten(),
                source_file_id,
                details: mine,
                created_at,
                hold,
            }
        })
        .collect()
}

/// Every held run, reduced to what a bulk verdict has to compare.
///
/// Not paged: a bulk verdict is *about* the runs that are not on the page, and
/// asking "which of these three thousand share my batch" is the one question
/// where reading them all is the answer rather than a shortcut.
pub(crate) fn held_runs(conn: &mut SqliteConnection) -> Result<Vec<HeldRun>, HoldError> {
    let rows: Vec<(String, Option<String>, Option<i32>)> = runs::table
        .filter(runs::status.eq(RunState::Held.as_str()))
        .select((runs::id, runs::line_id, runs::held_at_stage))
        .load(conn)?;
    let ids: Vec<String> = rows.iter().map(|(id, _, _)| id.clone()).collect();
    let batches = batch_ids(conn, &ids);
    Ok(rows
        .into_iter()
        .map(|(run_id, line_id, held_at_stage)| HeldRun {
            batch_id: batches.get(&run_id).cloned(),
            run_id,
            line_id,
            held_at_stage,
        })
        .collect())
}

#[derive(QueryableByName)]
struct BatchRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    batch_id: String,
}

/// Which batch each of these runs came from — **the seam with FR7**.
///
/// `runs.batch_id` is FR7's column, being written in parallel with this lane
/// and not yet in `schema.rs`. Rather than block on it or invent a second
/// notion of a batch, this asks SQL directly and treats the question failing as
/// the answer *"no batches"*: on a database without the column the query errors
/// once per page and every run is a batch of one, which is precisely the
/// behaviour that is correct today.
///
/// Raw SQL for exactly one reason — that a `diesel::table!` entry for a column
/// that does not exist yet would not compile, and a lane that cannot be built
/// until another branch merges is a lane nobody can review. When FR7 lands and
/// `batch_id` is in the schema this becomes an ordinary `select`, and nothing
/// above it changes: the callers already read an `Option<String>`.
fn batch_ids(conn: &mut SqliteConnection, run_ids: &[String]) -> HashMap<String, String> {
    if run_ids.is_empty() {
        return HashMap::new();
    }
    // No binds: the ids are the rows we already hold, and filtering in Rust
    // keeps this one statement whatever the page size is.
    let wanted: std::collections::HashSet<&str> = run_ids.iter().map(String::as_str).collect();
    diesel::sql_query("SELECT id, batch_id FROM runs WHERE batch_id IS NOT NULL")
        .load::<BatchRow>(conn)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| wanted.contains(r.id.as_str()))
        .map(|r| (r.id, r.batch_id))
        .collect()
}

/// Set, clear or clamp the rating on a file.
///
/// One to five, or `None` for "not rated" — which is a different answer from
/// zero and is drawn differently. Out-of-range numbers are clamped rather than
/// refused: the lane's `1`–`5` keys cannot produce one, so a six is a caller
/// bug and losing somebody's keystroke over it helps nobody.
pub(crate) fn rate_file(
    conn: &mut SqliteConnection,
    file_id: &str,
    rating: Option<i32>,
) -> Result<Option<i32>, diesel::result::Error> {
    let rating = rating.map(|r| r.clamp(1, 5));
    let changed = diesel::update(files::table.filter(files::id.eq(file_id)))
        .set(files::rating.eq(rating))
        .execute(conn)?;
    if changed == 0 {
        return Err(diesel::result::Error::NotFound);
    }
    Ok(rating)
}
