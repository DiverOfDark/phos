//! Batches on disk: writing one, reading it back, and the counts the caps are
//! decided from.
//!
//! Nothing here decides anything — [`super::plan`] does that. This module's job
//! is to hand it honest numbers, and every one of them is *counted* rather than
//! kept in a column. A counter of "tasks opened today" could disagree with the
//! tasks; `COUNT(*)` cannot, which is the same reason FR5c's
//! `reviewed_task_ids` exists and no "advanced" flag does.

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;

use crate::comfyui::line::{keeps_output, RunState, StageDisposition};
use crate::comfyui::runs::{stages_of_line, StageRow};
use crate::models::{BatchChangeset, NewBatch, NewSavedSelection};
use crate::schema::{batches, enhancement_tasks, runs, saved_selections};

use super::plan::{
    Caps, Cursor, Estimate, Pulse, StageCost, GUESS_IMAGE_BYTES, GUESS_IMAGE_SECONDS,
    GUESS_VIDEO_BYTES, GUESS_VIDEO_SECONDS,
};
use super::selection::Selection;

/// What a batch is. `stopped` is the only one a person causes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BatchState {
    #[default]
    Running,
    Paused,
    Stopped,
    Completed,
}

impl BatchState {
    pub fn as_str(self) -> &'static str {
        match self {
            BatchState::Running => "running",
            BatchState::Paused => "paused",
            BatchState::Stopped => "stopped",
            BatchState::Completed => "completed",
        }
    }

    pub fn parse(s: &str) -> BatchState {
        match s {
            "paused" => BatchState::Paused,
            "stopped" => BatchState::Stopped,
            "completed" => BatchState::Completed,
            _ => BatchState::Running,
        }
    }

    /// A batch the feeder still looks at.
    pub fn feeding() -> &'static [&'static str] {
        &["running", "paused"]
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, BatchState::Stopped | BatchState::Completed)
    }
}

/// One batch, read off its row.
#[derive(Debug, Clone)]
pub struct BatchRow {
    pub id: String,
    pub line_id: String,
    pub label: String,
    pub selection: Selection,
    pub stage_values: Option<String>,
    pub state: BatchState,
    pub paused_reason: Option<String>,
    pub skip_if_generated: bool,
    pub cursor: Option<Cursor>,
    pub matched_total: Option<i32>,
    pub skipped_total: Option<i32>,
    pub est_tasks: Option<i32>,
    pub est_gpu_seconds: Option<i32>,
    pub est_disk_bytes: Option<i64>,
    pub caps: Caps,
    pub created_at: Option<String>,
    pub finished_at: Option<String>,
}

type BatchTuple = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    bool,
    Option<String>,
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<i64>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<i64>,
    Option<i32>,
    Option<String>,
    Option<String>,
);

fn columns() -> (
    batches::id,
    batches::line_id,
    batches::label,
    batches::selection_json,
    batches::stage_values,
    batches::status,
    batches::paused_reason,
    batches::skip_if_generated,
    batches::cursor_key,
    batches::cursor_shot_id,
    batches::matched_total,
    batches::skipped_total,
    batches::est_tasks,
    batches::est_gpu_seconds,
    batches::est_disk_bytes,
    batches::daily_task_cap,
    batches::window_start_minute,
    batches::window_end_minute,
    batches::disk_floor_bytes,
    batches::max_outstanding_holds,
    batches::created_at,
    batches::finished_at,
) {
    (
        batches::id,
        batches::line_id,
        batches::label,
        batches::selection_json,
        batches::stage_values,
        batches::status,
        batches::paused_reason,
        batches::skip_if_generated,
        batches::cursor_key,
        batches::cursor_shot_id,
        batches::matched_total,
        batches::skipped_total,
        batches::est_tasks,
        batches::est_gpu_seconds,
        batches::est_disk_bytes,
        batches::daily_task_cap,
        batches::window_start_minute,
        batches::window_end_minute,
        batches::disk_floor_bytes,
        batches::max_outstanding_holds,
        batches::created_at,
        batches::finished_at,
    )
}

fn hydrate(t: BatchTuple) -> BatchRow {
    let selection = serde_json::from_str(&t.3).unwrap_or(Selection::Ids { ids: Vec::new() });
    BatchRow {
        id: t.0,
        line_id: t.1,
        label: t.2,
        selection,
        stage_values: t.4,
        state: BatchState::parse(&t.5),
        paused_reason: t.6,
        skip_if_generated: t.7,
        cursor: Cursor::from_columns(t.8, t.9),
        matched_total: t.10,
        skipped_total: t.11,
        est_tasks: t.12,
        est_gpu_seconds: t.13,
        est_disk_bytes: t.14,
        caps: Caps {
            daily_task_cap: t.15.map(i64::from),
            window: match (t.16, t.17) {
                (Some(start), Some(end)) => Some((start, end)),
                _ => None,
            },
            disk_floor_bytes: t.18,
            max_outstanding_holds: t.19.map(i64::from),
            lead: None,
        },
        created_at: t.20,
        finished_at: t.21,
    }
}

/// Write a batch. The runs it will open are not rows yet.
#[allow(clippy::too_many_arguments)]
pub fn create(
    conn: &mut SqliteConnection,
    line_id: &str,
    label: &str,
    selection: &Selection,
    stage_values: Option<&str>,
    skip_if_generated: bool,
    caps: &Caps,
    estimate: &Estimate,
) -> QueryResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let selection_json = serde_json::to_string(selection).unwrap_or_else(|_| "{}".to_string());
    diesel::insert_into(batches::table)
        .values(NewBatch {
            id: &id,
            line_id,
            label,
            selection_json: &selection_json,
            stage_values,
            skip_if_generated,
            matched_total: Some(clamp_i32(estimate.matched)),
            skipped_total: Some(clamp_i32(estimate.skipped)),
            est_tasks: Some(clamp_i32(estimate.tasks)),
            est_gpu_seconds: Some(clamp_i32(estimate.gpu_seconds)),
            est_disk_bytes: Some(estimate.disk_bytes),
            daily_task_cap: caps.daily_task_cap.map(clamp_i32),
            window_start_minute: caps.window.map(|w| w.0),
            window_end_minute: caps.window.map(|w| w.1),
            disk_floor_bytes: caps.disk_floor_bytes,
            max_outstanding_holds: caps.max_outstanding_holds.map(clamp_i32),
        })
        .execute(conn)?;
    Ok(id)
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(0, i32::MAX as i64) as i32
}

pub fn load(conn: &mut SqliteConnection, id: &str) -> QueryResult<Option<BatchRow>> {
    Ok(batches::table
        .filter(batches::id.eq(id))
        .select(columns())
        .first::<BatchTuple>(conn)
        .optional()?
        .map(hydrate))
}

/// Every batch, newest first. There are never many — a batch is an action a
/// person took, not a row per shot.
pub fn list(conn: &mut SqliteConnection) -> QueryResult<Vec<BatchRow>> {
    Ok(batches::table
        .order(batches::created_at.desc())
        .select(columns())
        .load::<BatchTuple>(conn)?
        .into_iter()
        .map(hydrate)
        .collect())
}

/// The batches the feeder still has work to do for.
pub fn feeding(conn: &mut SqliteConnection) -> QueryResult<Vec<BatchRow>> {
    Ok(batches::table
        .filter(batches::status.eq_any(BatchState::feeding()))
        .order(batches::created_at.asc())
        .select(columns())
        .load::<BatchTuple>(conn)?
        .into_iter()
        .map(hydrate)
        .collect())
}

pub fn apply(conn: &mut SqliteConnection, id: &str, change: BatchChangeset<'_>) -> QueryResult<()> {
    diesel::update(batches::table.filter(batches::id.eq(id)))
        .set(change)
        .execute(conn)?;
    Ok(())
}

/// How many runs of this batch are in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunCounts {
    pub running: i64,
    pub held: i64,
    pub completed: i64,
    pub failed: i64,
    pub cancelled: i64,
}

impl RunCounts {
    /// Runs that are not over. Held runs count: a held run is not finished, it
    /// is waiting, and a batch is not done while one of them exists.
    pub fn live(self) -> i64 {
        self.running + self.held
    }

    pub fn opened(self) -> i64 {
        self.running + self.held + self.completed + self.failed + self.cancelled
    }
}

/// Count this batch's runs by status.
///
/// The held count is deliberately taken from `status = 'held'` alone and never
/// joined to `held_at_stage`. FR5c's author left a note about exactly this: the
/// two are written together everywhere, but `held_at_stage` is the one marker
/// in that feature that is not self-evidencing, and a cap that silently ignored
/// a held run with a NULL stage would let the mountain grow past the cap it was
/// supposed to enforce. The cap counts parked runs; where they are parked is
/// not its business.
pub fn run_counts(conn: &mut SqliteConnection, batch_id: &str) -> QueryResult<RunCounts> {
    #[derive(QueryableByName)]
    struct StatusCount {
        #[diesel(sql_type = diesel::sql_types::Text)]
        status: String,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let rows: Vec<StatusCount> = diesel::sql_query(
        "SELECT status, COUNT(*) AS n FROM runs WHERE batch_id = ?1 GROUP BY status",
    )
    .bind::<diesel::sql_types::Text, _>(batch_id)
    .load(conn)?;

    let mut counts = RunCounts::default();
    for StatusCount { status, n } in rows {
        match status.as_str() {
            s if s == RunState::Running.as_str() => counts.running = n,
            s if s == RunState::Held.as_str() => counts.held = n,
            s if s == RunState::Completed.as_str() => counts.completed = n,
            s if s == RunState::Failed.as_str() => counts.failed = n,
            s if s == RunState::Cancelled.as_str() => counts.cancelled = n,
            _ => {}
        }
    }
    Ok(counts)
}

/// Tasks this batch has opened since `since` (a `YYYY-MM-DD HH:MM:SS` stamp).
///
/// Counted from the tasks themselves rather than from a per-day column, so a
/// restart, a crash or a hand-edited row cannot make the daily cap believe a
/// day is fresh when it is not.
pub fn tasks_since(
    conn: &mut SqliteConnection,
    batch_id: &str,
    since: &str,
) -> QueryResult<i64> {
    #[derive(QueryableByName)]
    struct N {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let row: N = diesel::sql_query(
        "SELECT COUNT(*) AS n FROM enhancement_tasks t \
         JOIN runs r ON t.run_id = r.id \
         WHERE r.batch_id = ?1 AND t.created_at >= ?2",
    )
    .bind::<diesel::sql_types::Text, _>(batch_id)
    .bind::<diesel::sql_types::Text, _>(since)
    .get_result(conn)?;
    Ok(row.n)
}

/// The prompt ids a STOP has to purge from ComfyUI's own queue.
pub fn live_prompt_ids(conn: &mut SqliteConnection, batch_id: &str) -> QueryResult<Vec<String>> {
    enhancement_tasks::table
        .inner_join(runs::table.on(enhancement_tasks::run_id.eq(runs::id.nullable())))
        .filter(runs::batch_id.eq(batch_id))
        .filter(enhancement_tasks::comfyui_prompt_id.is_not_null())
        .filter(enhancement_tasks::status.ne_all(&[
            "completed",
            "failed",
            crate::comfyui::STATUS_CANCELLED,
        ]))
        .select(enhancement_tasks::comfyui_prompt_id.assume_not_null())
        .load(conn)
}

/// The run ids of a batch that are not finished — what a STOP cancels.
pub fn live_run_ids(conn: &mut SqliteConnection, batch_id: &str) -> QueryResult<Vec<String>> {
    runs::table
        .filter(runs::batch_id.eq(batch_id))
        .filter(runs::status.eq_any(RunState::live()))
        .select(runs::id)
        .load(conn)
}

// ── Saved selections ──

#[derive(Debug, Clone)]
pub struct SavedSelectionRow {
    pub id: String,
    pub name: String,
    pub line_id: Option<String>,
    pub selection: Selection,
    pub caps_json: Option<String>,
    pub skip_if_generated: bool,
    pub created_at: Option<String>,
}

pub fn save_selection(
    conn: &mut SqliteConnection,
    name: &str,
    line_id: Option<&str>,
    selection: &Selection,
    caps_json: Option<&str>,
    skip_if_generated: bool,
) -> QueryResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let selection_json = serde_json::to_string(selection).unwrap_or_else(|_| "{}".to_string());
    diesel::insert_into(saved_selections::table)
        .values(NewSavedSelection {
            id: &id,
            name,
            line_id,
            selection_json: &selection_json,
            caps_json,
            skip_if_generated,
        })
        .execute(conn)?;
    Ok(id)
}

pub fn list_selections(conn: &mut SqliteConnection) -> QueryResult<Vec<SavedSelectionRow>> {
    let rows: Vec<(
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        bool,
        Option<String>,
    )> = saved_selections::table
        .order(saved_selections::created_at.desc())
        .select((
            saved_selections::id,
            saved_selections::name,
            saved_selections::line_id,
            saved_selections::selection_json,
            saved_selections::caps_json,
            saved_selections::skip_if_generated,
            saved_selections::created_at,
        ))
        .load(conn)?;
    Ok(rows
        .into_iter()
        .map(|r| SavedSelectionRow {
            id: r.0,
            name: r.1,
            line_id: r.2,
            selection: serde_json::from_str(&r.3)
                .unwrap_or(Selection::Ids { ids: Vec::new() }),
            caps_json: r.4,
            skip_if_generated: r.5,
            created_at: r.6,
        })
        .collect())
}

pub fn delete_selection(conn: &mut SqliteConnection, id: &str) -> QueryResult<usize> {
    diesel::delete(saved_selections::table.filter(saved_selections::id.eq(id))).execute(conn)
}

// ── Costing a line ──

/// What each stage of a line costs, measured where the library has history.
///
/// "Measured" means this library has actually run that workflow to completion:
/// the median of `completed_at - started_at` over its completed tasks, and the
/// median size of the files it produced. Median rather than mean because one
/// task that sat in `awaiting_output` for a quarter of an hour would otherwise
/// double every estimate on the sheet.
///
/// A workflow this library has never run is *guessed*, from constants that are
/// invented — see [`GUESS_IMAGE_SECONDS`]. The estimate carries which is which
/// so the sheet can say so.
pub fn stage_costs(conn: &mut SqliteConnection, line_id: &str) -> QueryResult<Vec<StageCost>> {
    let stages = stages_of_line(conn, line_id)?;
    let last = stages.len().saturating_sub(1);
    let mut out = Vec::with_capacity(stages.len());
    for (idx, stage) in stages.iter().enumerate() {
        let (measured_seconds, measured_bytes) = measured_cost(conn, &stage.workflow_id)?;
        let video = stage.contract.produces == crate::comfyui::MediaType::Video;
        let is_final = idx == last;
        out.push(StageCost {
            fanout: fanout_of(stage),
            seconds: measured_seconds.unwrap_or(if video {
                GUESS_VIDEO_SECONDS
            } else {
                GUESS_IMAGE_SECONDS
            }),
            bytes: measured_bytes.unwrap_or(if video {
                GUESS_VIDEO_BYTES
            } else {
                GUESS_IMAGE_BYTES
            }),
            keeps_output: keeps_output(StageDisposition {
                keep_flag: stage.keep_output,
                is_final,
                feeds_hold: stage.hold_for_review,
            }),
            seconds_measured: measured_seconds.is_some(),
            bytes_measured: measured_bytes.is_some(),
            holds: stage.hold_for_review,
        });
    }
    Ok(out)
}

/// How many tasks one upstream task of this stage becomes.
///
/// Asked of `params::expand`, the same function the runtime uses, so a `×4
/// seeds` sweep is counted the way it will actually be queued rather than by a
/// second reading of the same JSON.
fn fanout_of(stage: &StageRow) -> i64 {
    let plan = stage.plan();
    match crate::comfyui::params::expand(&plan.parameters, &plan.vary) {
        Ok(combos) => combos.len().max(1) as i64,
        // A sweep this wide is refused at start time with its own message. For
        // an estimate, one is the honest floor.
        Err(_) => 1,
    }
}

#[derive(QueryableByName)]
struct MedianRow {
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Double>)]
    seconds: Option<f64>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Double>)]
    bytes: Option<f64>,
}

/// The median duration and output size this library has seen for a workflow.
///
/// `NULL` for either means "never measured here". SQLite has no median, so this
/// is the middle row of an ordered list, which is what the name promises and
/// what an average of a bimodal distribution is not.
fn measured_cost(
    conn: &mut SqliteConnection,
    workflow_id: &str,
) -> QueryResult<(Option<f64>, Option<i64>)> {
    // The middle row of an ordered list. `LIMIT 2 - (n % 2) OFFSET (n-1)/2`
    // takes one row when the count is odd and the middle two when it is even,
    // which is what a median is; a plain `LIMIT 2` would average the middle
    // with the one above it and report a number that is in neither half.
    let row: MedianRow = diesel::sql_query(
        "SELECT \
           (SELECT AVG(d) FROM (\
              SELECT (julianday(t.completed_at) - julianday(t.started_at)) * 86400.0 AS d \
              FROM enhancement_tasks t \
              WHERE t.workflow_id = ?1 AND t.status = 'completed' \
                AND t.started_at IS NOT NULL AND t.completed_at IS NOT NULL \
              ORDER BY d \
              LIMIT (SELECT 2 - (COUNT(*) % 2) FROM enhancement_tasks t3 \
                     WHERE t3.workflow_id = ?1 AND t3.status = 'completed' \
                       AND t3.started_at IS NOT NULL AND t3.completed_at IS NOT NULL) \
              OFFSET (\
                SELECT (COUNT(*) - 1) / 2 FROM enhancement_tasks t2 \
                WHERE t2.workflow_id = ?1 AND t2.status = 'completed' \
                  AND t2.started_at IS NOT NULL AND t2.completed_at IS NOT NULL))) AS seconds, \
           (SELECT AVG(b) FROM (\
              SELECT f.file_size AS b FROM files f \
              WHERE f.source_workflow_id = ?2 AND f.file_size IS NOT NULL \
              ORDER BY b \
              LIMIT (SELECT 2 - (COUNT(*) % 2) FROM files f3 \
                     WHERE f3.source_workflow_id = ?2 AND f3.file_size IS NOT NULL) \
              OFFSET (\
                SELECT (COUNT(*) - 1) / 2 FROM files f2 \
                WHERE f2.source_workflow_id = ?2 AND f2.file_size IS NOT NULL))) AS bytes",
    )
    .bind::<diesel::sql_types::Text, _>(workflow_id)
    .bind::<diesel::sql_types::Text, _>(workflow_id)
    .get_result(conn)?;

    // A duration of zero is a clock that did not tick, not a free workflow.
    let seconds = row.seconds.filter(|s| *s > 0.0);
    let bytes = row.bytes.filter(|b| *b > 0.0).map(|b| b as i64);
    Ok((seconds, bytes))
}

// ── The pulse ──

/// Free bytes on the volume holding `path`.
///
/// `None` when it cannot be read, and [`super::plan::decide`] treats that as no
/// objection: a floor that cannot be measured must fail to protect, never fail
/// to run.
pub fn free_disk_bytes(path: &std::path::Path) -> Option<i64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statvfs` writes into `buf` and reads a NUL-terminated path.
        // Both are satisfied; the return value says whether `buf` was filled.
        unsafe {
            let mut buf: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c_path.as_ptr(), &mut buf) != 0 {
                return None;
            }
            // `f_bavail` is what a non-root process may actually use, which is
            // the number a disk floor is about — `f_bfree` includes the
            // reserved blocks nothing will ever hand us.
            let frsize = if buf.f_frsize > 0 {
                buf.f_frsize as u64
            } else {
                buf.f_bsize as u64
            };
            (buf.f_bavail as u64).checked_mul(frsize).map(|b| b as i64)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// The most recent local midnight before `now`, expressed on the clock
/// `enhancement_tasks.created_at` is stamped with.
///
/// Two clocks meet here and they are not the same one. A person's "400 tasks a
/// day" and "only overnight" both mean *their* day, so the boundary is local
/// midnight — but task rows carry SQLite's `CURRENT_TIMESTAMP`, which is UTC.
/// Comparing the two without converting makes the cap reset hours early or late
/// depending on which side of Greenwich the library is on, and in summer it
/// moves. So the local midnight is converted before it is compared.
pub fn day_boundary_utc(now_local: chrono::NaiveDateTime) -> String {
    use chrono::TimeZone;
    let midnight = now_local.date().and_hms_opt(0, 0, 0).unwrap_or(now_local);
    let offset = chrono::Local
        .offset_from_local_datetime(&midnight)
        .earliest()
        .or_else(|| chrono::Local.offset_from_local_datetime(&midnight).latest());
    let utc = match offset {
        Some(off) => {
            use chrono::Offset;
            midnight - chrono::Duration::seconds(off.fix().local_minus_utc() as i64)
        }
        // A local midnight that does not exist (the hour a DST jump skips).
        // Falling back to the naive value is an hour out at worst, once a year.
        None => midnight,
    };
    utc.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Everything [`super::plan::decide`] needs, read in one place.
///
/// `now` is *local* time: both the window and the daily cap are about a
/// person's day, not Greenwich's.
pub fn pulse(
    conn: &mut SqliteConnection,
    batch: &BatchRow,
    library_root: &std::path::Path,
    now: chrono::NaiveDateTime,
) -> QueryResult<Pulse> {
    use chrono::Timelike;
    let counts = run_counts(conn, &batch.id)?;
    let tasks_today = tasks_since(conn, &batch.id, &day_boundary_utc(now))?;
    Ok(Pulse {
        minute_of_day: now.hour() as i32 * 60 + now.minute() as i32,
        tasks_today,
        // Only read when a floor was actually set: `statvfs` is a syscall per
        // batch per tick otherwise, for a number nothing would look at.
        free_disk_bytes: batch
            .caps
            .disk_floor_bytes
            .and_then(|_| free_disk_bytes(library_root)),
        outstanding_holds: counts.held,
        live_runs: counts.live(),
    })
}

/// Build the estimate a confirm sheet shows, and the one a batch is created
/// with. One function, so the sheet and the row can never disagree.
pub fn estimate_for(
    conn: &mut SqliteConnection,
    line_id: &str,
    matched: i64,
    skipped: i64,
) -> QueryResult<(Estimate, Vec<StageCost>)> {
    let costs = stage_costs(conn, line_id)?;
    Ok((super::plan::estimate(matched, skipped, &costs), costs))
}
