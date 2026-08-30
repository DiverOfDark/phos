//! What a batch does next, decided without touching a database.
//!
//! Three kinds of arithmetic live here, and all three are pure because all
//! three are the parts that go wrong quietly:
//!
//! * **The cursor.** A keyset over `(COALESCE(shots.timestamp,''), shots.id)`
//!   ascending. An OFFSET would drift the moment an import lands mid-batch and
//!   silently re-run or skip shots; a keyset cannot.
//! * **The caps.** Whether to open runs this tick, and how many. Every "no" has
//!   a name, because a batch that has stopped feeding must be able to say why.
//! * **The estimate.** What the confirm sheet promises before anything is
//!   queued. Its inputs are measured where the library has history and guessed
//!   where it does not, and it says which it used.

use serde::Serialize;
use utoipa::ToSchema;

// ── The cursor ──

/// How far a batch has materialised.
///
/// `key` is the shot's `COALESCE(timestamp, '')` and `shot_id` breaks the tie.
/// Together they are a total order, because `shots.id` is unique — which is the
/// whole reason the cursor is a pair and not a timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Cursor {
    pub key: String,
    pub shot_id: String,
}

impl Cursor {
    pub fn new(key: impl Into<String>, shot_id: impl Into<String>) -> Self {
        Cursor {
            key: key.into(),
            shot_id: shot_id.into(),
        }
    }

    /// Read a cursor back off a batch row. Either column missing means "not
    /// started" rather than a half-cursor: the two are only ever written
    /// together.
    pub fn from_columns(key: Option<String>, shot_id: Option<String>) -> Option<Cursor> {
        match (key, shot_id) {
            (Some(key), Some(shot_id)) => Some(Cursor { key, shot_id }),
            _ => None,
        }
    }
}

/// The cursor after materialising `page`, which must be in cursor order.
///
/// An empty page leaves the cursor alone — the batch has caught up with its own
/// query, and moving the cursor on nothing would skip whatever arrives next.
pub fn advance_cursor(current: Option<Cursor>, page: &[Cursor]) -> Option<Cursor> {
    page.last().cloned().or(current)
}

/// The `WHERE` fragment that resumes from a cursor, with `?{a}`/`?{b}` for its
/// two binds. Kept here rather than beside the query so it can be read against
/// [`advance_cursor`] — the two have to agree on strictness or a batch either
/// re-runs one shot per tick or skips one.
///
/// Strictly greater, so the shot the cursor names is not run twice.
pub fn cursor_predicate(bind_key: usize, bind_id: usize) -> String {
    format!(
        "(COALESCE(s.timestamp,'') > ?{a} \
          OR (COALESCE(s.timestamp,'') = ?{a} AND s.id > ?{b}))",
        a = bind_key,
        b = bind_id
    )
}

// ── The caps ──

/// How far ahead of the queue a batch may open runs.
///
/// Without a lead the feeder would open its chunk every three seconds whether
/// or not anything finished, and fifty thousand runs would be rows within the
/// hour — the exact thing lazy materialisation exists to avoid. With it, a
/// batch keeps the queue fed and nothing more.
pub const DEFAULT_LEAD: i64 = 64;

/// How many shots one tick may turn into runs. Small on purpose: the tick is
/// three seconds, so this is 500 runs a minute at full tilt, and a STOP is
/// never waiting on a long write.
pub const DEFAULT_CHUNK: i64 = 25;

/// The limits a batch was sent with. `None` everywhere is a batch with no caps,
/// which is allowed and is what a small selection gets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caps {
    /// Tasks this batch may open per calendar day.
    pub daily_task_cap: Option<i64>,
    /// Minutes from local midnight, `[start, end)`. May wrap midnight.
    pub window: Option<(i32, i32)>,
    /// Pause while free space on the library volume is below this.
    pub disk_floor_bytes: Option<i64>,
    /// Pause while more than this many of the batch's runs are held.
    pub max_outstanding_holds: Option<i64>,
    /// How many live runs this batch may have open at once.
    pub lead: Option<i64>,
}

/// What the world looks like at this tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pulse {
    /// Minutes since local midnight.
    pub minute_of_day: i32,
    /// Tasks this batch has opened since local midnight.
    pub tasks_today: i64,
    /// Free bytes on the library volume, or `None` when it could not be read.
    /// Unreadable is treated as "no objection": a disk floor that cannot be
    /// measured must not stop a farm, only fail to protect it.
    pub free_disk_bytes: Option<i64>,
    /// Runs of this batch sitting at a hold point, waiting on a person.
    pub outstanding_holds: i64,
    /// Runs of this batch that are not finished — running *and* held.
    pub live_runs: i64,
}

/// Why a batch is not feeding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    /// Outside the hours it was given.
    OutsideWindow,
    /// It has opened its day's tasks.
    DailyCap,
    /// The volume is at or below the floor.
    DiskFloor,
    /// More runs are waiting on a person than the batch allows.
    HoldCap,
}

impl PauseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            PauseReason::OutsideWindow => "window",
            PauseReason::DailyCap => "daily_cap",
            PauseReason::DiskFloor => "disk_floor",
            PauseReason::HoldCap => "holds",
        }
    }

    /// What every screen says when a batch has stopped feeding.
    ///
    /// This is the answer to "why has nothing happened for six hours", and
    /// there is deliberately **one** of it. A note written here and a sentence
    /// synthesised on some other screen would paraphrase each other right up
    /// until the day they disagreed about why something stopped.
    ///
    /// That is the obvious reason. The one worth writing down is not: when this
    /// string grew from 62 characters to 111, it broke the layout of *both*
    /// screens that render it — truncated into a 196px cell on the batch board,
    /// wrapped and shunted a button onto a third line in the Takes lane — and
    /// neither author noticed until one of them measured. Sharing the string is
    /// what gave that bug a single place to surface. Two hand-written sentences
    /// would have cost two subtly different wordings *and* two layout bugs, and
    /// the second would have been found by a user rather than by us. So: if you
    /// are about to write a second sentence for a paused batch somewhere else,
    /// don't. Render this one, and re-measure the screens that show it.
    ///
    /// It carries the numbers *and* who can act on them: the reader learns what
    /// stopped the batch, by how much, and whether they are the one who
    /// unsticks it. The hold cap is the only pause a person lifts by working,
    /// so it is the only one that says so — a sentence implying a window or a
    /// full disk could be cleared by reviewing would send somebody off to fight
    /// a problem they cannot solve from where they are standing.
    ///
    /// It names the *action* rather than the place, because the same string is
    /// rendered on the batch board — where holds cannot be cleared — and on the
    /// Takes lane, where they can.
    pub fn note(self, caps: &Caps, held: i64) -> String {
        match self {
            PauseReason::OutsideWindow => match caps.window {
                Some((start, end)) => format!(
                    "Paused: outside this batch's window, {}–{}. It picks up again then.",
                    clock(start),
                    clock(end)
                ),
                None => "Paused: outside this batch's window.".to_string(),
            },
            PauseReason::DailyCap => match caps.daily_task_cap {
                Some(cap) => format!(
                    "Paused: this batch has opened its {} tasks for today. \
                     It carries on after midnight.",
                    cap
                ),
                None => "Paused: this batch has opened its tasks for today.".to_string(),
            },
            PauseReason::DiskFloor => match caps.disk_floor_bytes {
                Some(floor) => format!(
                    "Paused: free space is down to this batch's floor of {}. \
                     Only freeing disk lifts this.",
                    gigabytes(floor)
                ),
                None => "Paused: free space is at this batch's floor.".to_string(),
            },
            PauseReason::HoldCap => match caps.max_outstanding_holds {
                Some(cap) => format!(
                    "Paused: {} runs are waiting on a verdict, and the cap is {}. \
                     Giving verdicts on some of them lets it feed again.",
                    held, cap
                ),
                None => format!(
                    "Paused: {} runs are waiting on a verdict. \
                     Giving verdicts on some of them lets it feed again.",
                    held
                ),
            },
        }
    }
}

/// Minutes from midnight as `HH:MM`, for a note that names a window.
fn clock(minutes: i32) -> String {
    let wrapped = minutes.rem_euclid(1440);
    format!("{:02}:{:02}", wrapped / 60, wrapped % 60)
}

/// Bytes as whole gigabytes, for a note that names a disk floor. Rounded down,
/// so a floor of 50 GB never reads as 51 and sends somebody hunting for a
/// gigabyte that was never there.
fn gigabytes(bytes: i64) -> String {
    format!("{} GB", bytes / 1024i64.pow(3))
}

/// What the feeder should do this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Materialise up to this many shots. Never zero.
    Open(i64),
    /// Nothing to do, nothing wrong — the queue is as far ahead as it may be.
    Idle,
    /// A cap says not now. The batch stays alive and asks again next tick.
    Pause(PauseReason),
    /// The query is exhausted and no run of this batch is still live.
    Done,
}

/// Is `minute` inside `[start, end)`, where the window may wrap midnight?
///
/// `00:00–07:00` does not wrap; `22:00–06:00` does, and is the reason this is a
/// function rather than two comparisons at the call site. A window whose ends
/// are equal is "always" rather than "never": a person who set both to the same
/// hour meant a day, not an instant.
pub fn in_window(minute: i32, start: i32, end: i32) -> bool {
    if start == end {
        true
    } else if start < end {
        minute >= start && minute < end
    } else {
        minute >= start || minute < end
    }
}

/// The one decision the feeder makes per batch per tick.
///
/// `exhausted` means the query answered nothing past the cursor. That alone is
/// not done — runs already opened are still walking their lines — so `Done`
/// also wants `live_runs == 0`.
///
/// Order matters, and it is the order of how bad each thing is. The disk floor
/// is asked before the daily cap because filling a volume loses work that has
/// already been paid for, and the hold cap before the daily cap because a
/// mountain of unreviewed takes is what the whole guardrail exists to prevent.
pub fn decide(caps: &Caps, pulse: &Pulse, exhausted: bool, tasks_per_shot: i64) -> Feed {
    if exhausted && pulse.live_runs == 0 {
        return Feed::Done;
    }

    if let Some((start, end)) = caps.window {
        if !in_window(pulse.minute_of_day, start, end) {
            return Feed::Pause(PauseReason::OutsideWindow);
        }
    }

    if let (Some(floor), Some(free)) = (caps.disk_floor_bytes, pulse.free_disk_bytes) {
        if free <= floor {
            return Feed::Pause(PauseReason::DiskFloor);
        }
    }

    if let Some(max_holds) = caps.max_outstanding_holds {
        if pulse.outstanding_holds >= max_holds {
            return Feed::Pause(PauseReason::HoldCap);
        }
    }

    // Everything past here is about *how many*, so an exhausted query stops
    // here: there is nothing to open, but the batch is not over either.
    if exhausted {
        return Feed::Idle;
    }

    let mut room = DEFAULT_CHUNK;

    let lead = caps.lead.unwrap_or(DEFAULT_LEAD);
    room = room.min(lead - pulse.live_runs);

    if let Some(cap) = caps.daily_task_cap {
        let left = cap - pulse.tasks_today;
        if left <= 0 {
            return Feed::Pause(PauseReason::DailyCap);
        }
        // A shot costs `tasks_per_shot` tasks and cannot be opened by halves,
        // so a day with room for three tasks and a four-task line opens
        // nothing rather than overshooting.
        room = room.min(left / tasks_per_shot.max(1));
        if room <= 0 {
            return Feed::Pause(PauseReason::DailyCap);
        }
    }

    if room <= 0 {
        Feed::Idle
    } else {
        Feed::Open(room)
    }
}

// ── The estimate ──

/// What one stage of a line is expected to cost, per task.
#[derive(Debug, Clone, PartialEq)]
pub struct StageCost {
    /// How many tasks each upstream task fans out to. 1 for an ordinary stage,
    /// 4 for `×4 seeds`.
    pub fanout: i64,
    /// Seconds of GPU per task.
    pub seconds: f64,
    /// Bytes of output per task.
    pub bytes: i64,
    /// Whether the output survives the run's sweep. Only kept output is disk
    /// this batch will still be using tomorrow.
    pub keeps_output: bool,
    /// Whether `seconds` came from this library's own completed tasks.
    pub seconds_measured: bool,
    /// Whether `bytes` came from this library's own generated files.
    pub bytes_measured: bool,
    /// Whether this stage parks its run and asks a person.
    pub holds: bool,
}

/// A guess for a stage nothing has ever run here, in seconds and bytes.
///
/// These are the only numbers in this feature that are invented. They are
/// deliberately round, and they stop being used the moment the library has run
/// the workflow once — [`StageCost::seconds_measured`] says which is which, and
/// the confirm sheet says so out loud.
pub const GUESS_IMAGE_SECONDS: f64 = 25.0;
pub const GUESS_IMAGE_BYTES: i64 = 2 * 1024 * 1024;
pub const GUESS_VIDEO_SECONDS: f64 = 240.0;
pub const GUESS_VIDEO_BYTES: i64 = 60 * 1024 * 1024;

/// What the confirm sheet shows.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct Estimate {
    /// Shots the selection names.
    pub matched: i64,
    /// Of those, how many already have output from this line.
    pub skipped: i64,
    /// What would actually run.
    pub to_run: i64,
    /// Tasks those shots come to, fan-out included.
    pub tasks: i64,
    /// Seconds of GPU, summed over every task.
    pub gpu_seconds: i64,
    /// Bytes this batch will leave behind, counting only output that survives
    /// the run's own sweep.
    pub disk_bytes: i64,
    /// Tasks one shot comes to. What the daily cap is spent in.
    pub tasks_per_shot: i64,
    /// How many of the line's stages were costed from measured history.
    pub measured_stages: usize,
    /// How many were guessed.
    pub guessed_stages: usize,
    /// Whether the line holds anywhere. If it does, everything above is an
    /// upper bound: a person choosing two takes of four cuts the stages below
    /// it by half, and no estimate can know what they will choose.
    pub has_hold: bool,
}

/// Tasks at each stage, per shot, given each stage's fan-out.
///
/// Stage k's count is the product of every fan-out up to and including k,
/// because each task of stage k-1 continues on its own — which is exactly what
/// `parent_task_id` does at runtime, and why four takes at stage 2 are four
/// independent runners through 3 and 4.
pub fn tasks_by_stage(stages: &[StageCost]) -> Vec<i64> {
    let mut out = Vec::with_capacity(stages.len());
    let mut running = 1i64;
    for stage in stages {
        running = running.saturating_mul(stage.fanout.max(1));
        out.push(running);
    }
    out
}

/// Add it all up.
pub fn estimate(matched: i64, skipped: i64, stages: &[StageCost]) -> Estimate {
    let to_run = (matched - skipped).max(0);
    let per_stage = tasks_by_stage(stages);
    let tasks_per_shot: i64 = per_stage.iter().sum();

    let mut gpu_seconds = 0.0f64;
    let mut disk_bytes = 0i64;
    for (stage, count) in stages.iter().zip(&per_stage) {
        gpu_seconds += stage.seconds * (*count as f64) * (to_run as f64);
        if stage.keeps_output {
            disk_bytes = disk_bytes.saturating_add(
                stage
                    .bytes
                    .saturating_mul(*count)
                    .saturating_mul(to_run.max(0)),
            );
        }
    }

    Estimate {
        matched,
        skipped,
        to_run,
        tasks: tasks_per_shot.saturating_mul(to_run),
        gpu_seconds: gpu_seconds.round() as i64,
        disk_bytes,
        tasks_per_shot,
        measured_stages: stages
            .iter()
            .filter(|s| s.seconds_measured && s.bytes_measured)
            .count(),
        guessed_stages: stages
            .iter()
            .filter(|s| !(s.seconds_measured && s.bytes_measured))
            .count(),
        has_hold: stages.iter().any(|s| s.holds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(fanout: i64) -> StageCost {
        StageCost {
            fanout,
            seconds: 10.0,
            bytes: 1000,
            keeps_output: true,
            seconds_measured: true,
            bytes_measured: true,
            holds: false,
        }
    }

    // ── Cursor ──

    #[test]
    fn an_empty_page_leaves_the_cursor_where_it_was() {
        let at = Cursor::new("2020", "s-9");
        assert_eq!(advance_cursor(Some(at.clone()), &[]), Some(at));
        assert_eq!(advance_cursor(None, &[]), None);
    }

    #[test]
    fn the_cursor_lands_on_the_last_shot_of_the_page() {
        let page = vec![Cursor::new("1970", "a"), Cursor::new("1971", "b")];
        assert_eq!(advance_cursor(None, &page), Some(Cursor::new("1971", "b")));
    }

    #[test]
    fn a_half_written_cursor_reads_as_not_started() {
        assert_eq!(Cursor::from_columns(Some("x".into()), None), None);
        assert_eq!(Cursor::from_columns(None, Some("y".into())), None);
        assert!(Cursor::from_columns(Some("x".into()), Some("y".into())).is_some());
    }

    #[test]
    fn the_cursor_predicate_is_strictly_after_where_it_points() {
        // The `>` rather than `>=` is what stops the shot the cursor names
        // being opened once per tick forever.
        let sql = cursor_predicate(3, 4);
        assert!(sql.contains("> ?3"));
        assert!(sql.contains("> ?4"));
        assert!(!sql.contains(">= "));
    }

    // ── Window ──

    #[test]
    fn an_overnight_window_wraps_midnight() {
        let (start, end) = (22 * 60, 6 * 60);
        assert!(in_window(23 * 60, start, end));
        assert!(in_window(2 * 60, start, end));
        assert!(!in_window(12 * 60, start, end));
    }

    #[test]
    fn a_plain_window_does_not() {
        let (start, end) = (0, 7 * 60);
        assert!(in_window(0, start, end));
        assert!(in_window(6 * 60 + 59, start, end));
        assert!(!in_window(7 * 60, start, end)); // end is exclusive
        assert!(!in_window(23 * 60, start, end));
    }

    #[test]
    fn a_window_with_equal_ends_is_all_day() {
        assert!(in_window(0, 60, 60));
        assert!(in_window(13 * 60, 60, 60));
    }

    // ── What a paused batch says ──

    #[test]
    fn the_hold_note_says_what_stopped_it_by_how_much_and_who_lifts_it() {
        let caps = Caps {
            max_outstanding_holds: Some(40),
            ..Default::default()
        };
        let note = PauseReason::HoldCap.note(&caps, 40);
        assert!(
            note.contains("40 runs are waiting on a verdict"),
            "{}",
            note
        );
        assert!(note.contains("the cap is 40"), "{}", note);
        // The agency half: the reader is the one who unsticks this.
        assert!(note.contains("Giving verdicts"), "{}", note);
    }

    #[test]
    fn the_hold_note_names_the_action_and_never_a_place() {
        // The same string is rendered on the batch board, where holds cannot be
        // cleared, and on the Takes lane, where they can. "here" would be right
        // on one screen and a lie on the other.
        let note = PauseReason::HoldCap.note(&Caps::default(), 7);
        assert!(!note.contains(" here"), "{}", note);
        assert!(note.contains("7 runs"), "{}", note);
    }

    #[test]
    fn the_pauses_nobody_can_review_away_promise_nothing() {
        // A window and a full disk are lifted by the clock and by free space,
        // not by working. Telling a reviewer otherwise sends them to fight a
        // problem they cannot solve from where they are standing.
        let caps = Caps {
            window: Some((0, 7 * 60)),
            disk_floor_bytes: Some(50 * 1024i64.pow(3)),
            ..Default::default()
        };
        let window = PauseReason::OutsideWindow.note(&caps, 0);
        assert!(window.contains("00:00–07:00"), "{}", window);
        assert!(!window.contains("verdict"), "{}", window);

        let disk = PauseReason::DiskFloor.note(&caps, 0);
        assert!(disk.contains("50 GB"), "{}", disk);
        assert!(!disk.contains("verdict"), "{}", disk);
        assert!(disk.contains("freeing disk"), "{}", disk);
    }

    #[test]
    fn the_daily_note_names_the_cap_and_when_it_lifts() {
        let caps = Caps {
            daily_task_cap: Some(400),
            ..Default::default()
        };
        let note = PauseReason::DailyCap.note(&caps, 0);
        assert!(note.contains("400 tasks"), "{}", note);
        assert!(note.contains("after midnight"), "{}", note);
    }

    #[test]
    fn a_note_with_no_cap_behind_it_still_reads_as_a_sentence() {
        // `paused_reason` and the caps are separate columns, so a hand-edited
        // row can pause for a reason whose cap is NULL. Every arm still has to
        // produce something a person can read.
        for reason in [
            PauseReason::OutsideWindow,
            PauseReason::DailyCap,
            PauseReason::DiskFloor,
            PauseReason::HoldCap,
        ] {
            let note = reason.note(&Caps::default(), 0);
            assert!(note.starts_with("Paused:"), "{}", note);
            assert!(note.ends_with('.'), "{}", note);
        }
    }

    #[test]
    fn a_disk_floor_rounds_down_so_nobody_hunts_a_gigabyte_that_was_never_there() {
        assert_eq!(gigabytes(50 * 1024i64.pow(3)), "50 GB");
        assert_eq!(gigabytes(50 * 1024i64.pow(3) + 1024i64.pow(3) - 1), "50 GB");
    }

    #[test]
    fn a_window_in_a_note_reads_as_a_clock() {
        assert_eq!(clock(0), "00:00");
        assert_eq!(clock(7 * 60), "07:00");
        assert_eq!(clock(22 * 60 + 30), "22:30");
    }

    // ── Caps ──

    #[test]
    fn with_no_caps_a_batch_feeds_a_chunk() {
        let feed = decide(&Caps::default(), &Pulse::default(), false, 1);
        assert_eq!(feed, Feed::Open(DEFAULT_CHUNK));
    }

    #[test]
    fn the_lead_stops_the_feeder_running_away_from_the_queue() {
        let pulse = Pulse {
            live_runs: DEFAULT_LEAD,
            ..Default::default()
        };
        assert_eq!(decide(&Caps::default(), &pulse, false, 1), Feed::Idle);

        let nearly = Pulse {
            live_runs: DEFAULT_LEAD - 3,
            ..Default::default()
        };
        assert_eq!(decide(&Caps::default(), &nearly, false, 1), Feed::Open(3));
    }

    #[test]
    fn outside_the_window_nothing_is_opened() {
        let caps = Caps {
            window: Some((0, 7 * 60)),
            ..Default::default()
        };
        let pulse = Pulse {
            minute_of_day: 12 * 60,
            ..Default::default()
        };
        assert_eq!(
            decide(&caps, &pulse, false, 1),
            Feed::Pause(PauseReason::OutsideWindow)
        );
    }

    #[test]
    fn the_daily_cap_counts_tasks_not_shots() {
        // Ten tasks left, a four-task line: two shots fit, not two and a half.
        let caps = Caps {
            daily_task_cap: Some(100),
            ..Default::default()
        };
        let pulse = Pulse {
            tasks_today: 90,
            ..Default::default()
        };
        assert_eq!(decide(&caps, &pulse, false, 4), Feed::Open(2));
    }

    #[test]
    fn a_day_with_no_room_for_a_whole_shot_pauses_rather_than_overshooting() {
        let caps = Caps {
            daily_task_cap: Some(100),
            ..Default::default()
        };
        let pulse = Pulse {
            tasks_today: 98,
            ..Default::default()
        };
        assert_eq!(
            decide(&caps, &pulse, false, 4),
            Feed::Pause(PauseReason::DailyCap)
        );
    }

    #[test]
    fn a_spent_day_pauses() {
        let caps = Caps {
            daily_task_cap: Some(400),
            ..Default::default()
        };
        let pulse = Pulse {
            tasks_today: 400,
            ..Default::default()
        };
        assert_eq!(
            decide(&caps, &pulse, false, 1),
            Feed::Pause(PauseReason::DailyCap)
        );
    }

    #[test]
    fn the_disk_floor_pauses_at_or_below_it() {
        let caps = Caps {
            disk_floor_bytes: Some(10_000),
            ..Default::default()
        };
        let at = Pulse {
            free_disk_bytes: Some(10_000),
            ..Default::default()
        };
        assert_eq!(
            decide(&caps, &at, false, 1),
            Feed::Pause(PauseReason::DiskFloor)
        );

        let above = Pulse {
            free_disk_bytes: Some(10_001),
            ..Default::default()
        };
        assert_eq!(decide(&caps, &above, false, 1), Feed::Open(DEFAULT_CHUNK));
    }

    #[test]
    fn an_unreadable_volume_does_not_stop_the_farm() {
        let caps = Caps {
            disk_floor_bytes: Some(10_000),
            ..Default::default()
        };
        let unknown = Pulse {
            free_disk_bytes: None,
            ..Default::default()
        };
        assert_eq!(decide(&caps, &unknown, false, 1), Feed::Open(DEFAULT_CHUNK));
    }

    #[test]
    fn too_many_held_runs_pauses_the_feed() {
        let caps = Caps {
            max_outstanding_holds: Some(50),
            ..Default::default()
        };
        let pulse = Pulse {
            outstanding_holds: 50,
            ..Default::default()
        };
        assert_eq!(
            decide(&caps, &pulse, false, 1),
            Feed::Pause(PauseReason::HoldCap)
        );

        let after_verdicts = Pulse {
            outstanding_holds: 49,
            ..Default::default()
        };
        assert_eq!(
            decide(&caps, &after_verdicts, false, 1),
            Feed::Open(DEFAULT_CHUNK)
        );
    }

    #[test]
    fn the_hold_cap_is_asked_before_the_daily_one() {
        // Both bite. The one that names the mountain wins, because that is the
        // one a person can do something about.
        let caps = Caps {
            daily_task_cap: Some(10),
            max_outstanding_holds: Some(5),
            ..Default::default()
        };
        let pulse = Pulse {
            tasks_today: 10,
            outstanding_holds: 5,
            ..Default::default()
        };
        assert_eq!(
            decide(&caps, &pulse, false, 1),
            Feed::Pause(PauseReason::HoldCap)
        );
    }

    #[test]
    fn an_exhausted_query_with_runs_still_going_is_not_done() {
        let pulse = Pulse {
            live_runs: 3,
            ..Default::default()
        };
        assert_eq!(decide(&Caps::default(), &pulse, true, 1), Feed::Idle);
    }

    #[test]
    fn an_exhausted_query_with_nothing_live_is_done() {
        assert_eq!(
            decide(&Caps::default(), &Pulse::default(), true, 1),
            Feed::Done
        );
    }

    #[test]
    fn done_beats_every_pause() {
        // A batch that finished overnight must not read as "outside window" in
        // the morning; it is over.
        let caps = Caps {
            window: Some((0, 7 * 60)),
            daily_task_cap: Some(1),
            disk_floor_bytes: Some(i64::MAX),
            max_outstanding_holds: Some(0),
            lead: Some(0),
        };
        let pulse = Pulse {
            minute_of_day: 12 * 60,
            tasks_today: 999,
            free_disk_bytes: Some(0),
            outstanding_holds: 0,
            live_runs: 0,
        };
        assert_eq!(decide(&caps, &pulse, true, 1), Feed::Done);
    }

    // ── Estimate ──

    #[test]
    fn fan_out_multiplies_down_the_line() {
        // 1 → ×4 → 1: one task, then four, then four.
        let stages = [stage(1), stage(4), stage(1)];
        assert_eq!(tasks_by_stage(&stages), vec![1, 4, 4]);
        assert_eq!(estimate(10, 0, &stages).tasks_per_shot, 9);
    }

    #[test]
    fn the_sheets_headline_numbers_add_up() {
        // 12,431 matched, 9,102 skipped, 3,329 to run, ×2 seeds = 6,658 tasks.
        let stages = [StageCost {
            fanout: 2,
            ..stage(1)
        }];
        let est = estimate(12_431, 9_102, &stages);
        assert_eq!(est.to_run, 3_329);
        assert_eq!(est.tasks_per_shot, 2);
        assert_eq!(est.tasks, 6_658);
    }

    #[test]
    fn gpu_time_counts_every_task_at_every_stage() {
        let stages = [stage(1), stage(4)];
        // 1 task at 10s + 4 tasks at 10s = 50s a shot, over 3 shots.
        assert_eq!(estimate(3, 0, &stages).gpu_seconds, 150);
    }

    #[test]
    fn swept_intermediates_are_not_counted_as_disk() {
        let intermediate = StageCost {
            keeps_output: false,
            ..stage(1)
        };
        let stages = [intermediate, stage(1)];
        // Only the second stage's 1000 bytes survive, once per shot.
        assert_eq!(estimate(5, 0, &stages).disk_bytes, 5000);
    }

    #[test]
    fn skipping_everything_estimates_nothing() {
        let est = estimate(900, 900, &[stage(1)]);
        assert_eq!(est.to_run, 0);
        assert_eq!(est.tasks, 0);
        assert_eq!(est.gpu_seconds, 0);
        assert_eq!(est.disk_bytes, 0);
    }

    #[test]
    fn a_stage_is_measured_only_when_both_of_its_numbers_are() {
        let half = StageCost {
            bytes_measured: false,
            ..stage(1)
        };
        let est = estimate(1, 0, &[stage(1), half]);
        assert_eq!(est.measured_stages, 1);
        assert_eq!(est.guessed_stages, 1);
    }

    #[test]
    fn a_line_that_holds_says_so_because_its_estimate_is_an_upper_bound() {
        let held = StageCost {
            holds: true,
            ..stage(4)
        };
        assert!(estimate(1, 0, &[held, stage(1)]).has_hold);
        assert!(!estimate(1, 0, &[stage(1)]).has_hold);
    }

    #[test]
    fn more_skipped_than_matched_never_goes_negative() {
        // The two counts are taken by separate queries; a concurrent import
        // between them must not produce a negative headline.
        assert_eq!(estimate(5, 9, &[stage(1)]).to_run, 0);
    }
}
