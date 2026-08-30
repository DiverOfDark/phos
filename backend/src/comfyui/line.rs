//! What a line is, and what happens next — the decisions, with no database in
//! sight.
//!
//! A *line* is a chain of workflows: photo → 5s clip → interpolate → 4K
//! upscale, run as one thing a person asked for, with each stage's output the
//! next stage's input. A *run* is one line applied to one shot.
//!
//! Three questions get asked about that, and all three are pure functions of
//! the stage contracts and the tasks' statuses. They live here, away from
//! [`super::runs`] and [`super::worker`], for the same reason [`super::policy`]
//! does: the interesting behaviour is a state machine, and a state machine you
//! can only exercise through a ComfyUI and a GPU is a state machine nobody
//! tests.
//!
//! * **What crosses this join?** [`carried_into`], and it is the only place
//!   that question is answered. A stage that makes no file is transparent to
//!   the media flow, and a connector that extracts a frame turns a clip into a
//!   still — two rules that were written three times over on three branches
//!   and are written once here.
//! * **Does this chain hold together?** [`validate_chain`] asks
//!   [`Accepts::admits`] of every [`carried_into`]. Not a second compatibility
//!   rule — the same function FR5b's stage picker calls, so a line the editor
//!   offers and a line the validator accepts can never disagree.
//! * **What happens after a stage finishes?** [`advance_after`]. One step, and
//!   deliberately dull for a stage nobody is reviewing: v1 lines are linear and
//!   fan-out propagates by each completed task spawning its own continuation.
//!   A stage marked **hold for review** is where that stops being a function of
//!   position alone — see below.
//! * **Is the run over, and how did it end?** [`tally`]. A run is running while
//!   any of its tasks is still moving, and then it is however its tasks ended.
//!   The stage it is "on" is the earliest one still unfinished, which is the
//!   honest answer when a fan-out has runners at two stages at once.
//!
//! # A hold point is where fan-in happens
//!
//! [`advance_after`] used to be a total, local rule: a completed task that is
//! not at the last stage continues, always. FR5c's hold point breaks that, and
//! it is the only thing in v1 that does. Four takes at a held stage converge on
//! **one verdict** — that is the fan-in — and the verdict then fans back out to
//! the subset a person kept, each of which walks the rest of the line for
//! itself.
//!
//! So the question stopped being "where is this task in the line" and became
//! "where is this task, and what has been decided about it": [`HoldGate`]
//! carries the second half, and [`Advance`] gained a [`Advance::Hold`] answer
//! for "park the run, this take is waiting to be looked at". Everything else
//! about fan-out is untouched, because a verdict that keeps two takes is
//! exactly two ordinary continuations.
//!
//! # Failure is not cancellation
//!
//! A failed stage queues no continuation, so the stages after it are never
//! reached — that is the whole of "a failed stage fails the run". Nothing is
//! cleaned up and nothing is cancelled: the intermediates that did get made
//! stay on disk for inspection, sibling branches of a fan-out run on, and a
//! retry resumes the failed task from the source file it already holds. Which
//! is to say: re-running an hour of upscaling because stage 4 hiccupped is not
//! something this module can do, because there is no code here that could.

use super::contract::{Accepts, MediaType};
use super::source::SourceMode;

// ===== Design time: does this chain hold together? ==========================

/// A stage, reduced to what deciding needs: what it eats, what it hands on,
/// and how it reads what the stage above it made.
#[derive(Debug, Clone, PartialEq)]
pub struct StageTyping {
    pub stage_idx: i32,
    /// The workflow's name, so a refusal can say which stage is the problem in
    /// the words the person used.
    pub name: String,
    pub accepts: Accepts,
    pub produces: MediaType,
    /// What this stage stored about *which part* of its input it reads —
    /// `line_stages.source_mode`, the connector above it. `None` is "whatever
    /// the graph implies", which is what [`SourceMode::resolve`] answers.
    pub source_mode: Option<String>,
    /// Whether this stage's graph has a loader that can read a clip, taken off
    /// the graph the way the dispatcher takes it. A contract corrected by hand
    /// can disagree with the graph, and the graph is what runs.
    pub takes_video: bool,
}

impl StageTyping {
    /// A stage with no connector settings — the picker's view of a candidate,
    /// and what a test writes when the mode is not what it is about.
    #[cfg(test)]
    pub fn bare(stage_idx: i32, name: &str, accepts: Accepts, produces: MediaType) -> Self {
        StageTyping {
            stage_idx,
            name: name.to_string(),
            accepts,
            produces,
            source_mode: None,
            takes_video: false,
        }
    }
}

/// Why a line was refused, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct LineError {
    /// The stage that cannot be reached, 0-based. `-1` when the whole line is
    /// the problem.
    pub stage_idx: i32,
    pub message: String,
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Stage numbers are 0-based in the database and 1-based to a reader.
fn human(stage_idx: i32) -> i32 {
    stage_idx + 1
}

// ----- The one rule about what crosses a join -------------------------------

/// What the connector above one stage carries into it.
///
/// The single answer to "what media type crosses this join", asked by the
/// picker ([`super::editor::stage_options`]), by the validator
/// ([`validate_chain`]), by the line reader that draws the connector, and by
/// the dispatcher the moment before it uploads. Three copies of this idea were
/// written independently and each was right about a different half; there is
/// one now, and "offered", "accepted" and "sent" cannot come apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
    /// The stage whose output travels down this join.
    pub from: i32,
    /// What that stage made — what the connector says on the screen.
    pub produced: MediaType,
    /// What the stage below actually reads. The same thing, unless it asked
    /// for a frame of a clip, because a frame of a clip is a still.
    pub reads: MediaType,
}

/// What flows into the stage at `position`, or `None` when nothing does.
///
/// `None` means that stage reads the **run's own source**: it is the first, or
/// everything above it produced text. That is not a hole in the check — it is
/// the one question design time cannot answer, because a line is drawn once and
/// run against many shots, and [`admits_source`] asks it when a run starts.
///
/// # A text stage is transparent
///
/// A describe stage makes no file: FR5a's `produces: text` writes no `files`
/// row, and there is nothing for the stage after it to load. What flows down
/// the line is therefore *unchanged* by it — the photograph the describe stage
/// read is the photograph the generation stage after it reads — and its
/// sentence binds into that stage's prompt slot instead. So the join is taken
/// from the last stage that actually made a file.
///
/// # A frame of a clip is a still
///
/// The other half, and the one that was missing. [`Accepts::admits`] is a pure
/// media-type match and stays one; whether a clip arrives as a clip is not a
/// question about media types but about the connector, and FR2 built
/// `first_frame` / `last_frame` / `at_time` / `keyframe` precisely so a video
/// can feed a still stage. [`reads_as`] applies that here, once.
pub fn carried_into(stages: &[StageTyping], position: usize) -> Option<Carried> {
    let down = stages.get(position)?;
    let up = upstream_of(stages, position)?;
    Some(Carried {
        from: up.stage_idx,
        produced: up.produces,
        reads: reads_as(up.produces, down),
    })
}

/// The last stage before `position` that actually made a file.
fn upstream_of(stages: &[StageTyping], position: usize) -> Option<&StageTyping> {
    stages
        .get(..position)?
        .iter()
        .rev()
        .find(|s| s.produces != MediaType::Text)
}

/// What `down` will actually be handed, given what came out of the stage above
/// it and what `down` said about reading it.
///
/// The dispatcher's own rule, borrowed rather than restated: [`SourceMode`] is
/// what decides, and this asks it. A clip going into a stage that takes clips
/// stays a clip, and which of that graph's loaders the file lands in is the
/// stage's own business. A clip going into a stage that does *not* take clips
/// is whatever the source mode makes of it — by default frame zero, which is
/// what the dispatcher would send anyway.
pub fn reads_as(produced: MediaType, down: &StageTyping) -> MediaType {
    if produced != MediaType::Video || down.accepts.admits(MediaType::Video) {
        return produced;
    }
    match SourceMode::resolve(down.source_mode.as_deref(), down.takes_video, true) {
        SourceMode::WholeVideo => MediaType::Video,
        _ => MediaType::Image,
    }
}

/// Can every stage eat what the one before it produces?
///
/// Rejected at design time, so a four-hour chain is refused when it is drawn
/// rather than after its third stage has run. Every join is [`carried_into`]
/// and then [`Accepts::admits`], and nothing else — including the case where a
/// stage consumes nothing at all, which `admits` answers `false` to, and which
/// reads as "that can only be the first stage of a line".
///
/// Which is why both of the lines the last two features exist for validate:
///
/// ```text
///   [1]  DESCRIBE (QWEN-VL)     image → text
///    │   text → positive
///   [2]  PHOTO → 5S CLIP        image + text → video
///
///   [1]  PHOTO → 5S CLIP        image → video
///    │   last frame
///   [2]  RESTORE PORTRAIT       image → image
/// ```
///
/// Stage 2 of the first accepts `image` and stage 1 produced `text`; stage 2 of
/// the second accepts `image` and stage 1 produced `video`. Both are obviously
/// correct lines, and both are correct for a reason that lives in
/// [`carried_into`] rather than in a second compatibility rule here.
pub fn validate_chain(stages: &[StageTyping]) -> Result<(), LineError> {
    if stages.is_empty() {
        return Err(LineError {
            stage_idx: -1,
            message: "A line needs at least one stage.".to_string(),
        });
    }

    for (position, down) in stages.iter().enumerate() {
        match carried_into(stages, position) {
            // Nothing above has made a file, so this stage reads the run's own
            // source like the first one does. Nothing to check here — except
            // that a stage consuming nothing at all still cannot follow one.
            None => {
                if position > 0 && down.accepts.starts_a_line() {
                    return Err(LineError {
                        stage_idx: down.stage_idx,
                        message: starts_a_line_message(down),
                    });
                }
            }
            Some(carried) if down.accepts.admits(carried.reads) => {}
            Some(carried) => {
                let up = upstream_of(stages, position).expect("carried_into found one");
                return Err(LineError {
                    stage_idx: down.stage_idx,
                    message: if down.accepts.starts_a_line() {
                        starts_a_line_message(down)
                    } else {
                        format!(
                            "Stage {} ({}) takes {}, but stage {} ({}) produces {}.",
                            human(down.stage_idx),
                            down.name,
                            down.accepts.as_str(),
                            human(carried.from),
                            up.name,
                            carried.produced.as_str()
                        )
                    },
                });
            }
        }
    }

    Ok(())
}

fn starts_a_line_message(stage: &StageTyping) -> String {
    format!(
        "Stage {} ({}) consumes nothing, so it can only be the first stage of a line.",
        human(stage.stage_idx),
        stage.name
    )
}

/// The stages that read the run's own source rather than an upstream output.
///
/// Normally just the first. A line that opens with one or more describe stages
/// has several, because none of them made a file for the next one to read, and
/// every one of them has to fit the shot the run is against. Which stages those
/// are is [`carried_into`]'s answer, not a second walk of the same list.
pub fn source_readers(stages: &[StageTyping]) -> Vec<&StageTyping> {
    stages
        .iter()
        .enumerate()
        .take_while(|(position, _)| carried_into(stages, *position).is_none())
        .map(|(_, stage)| stage)
        .filter(|stage| reads_source(stage.accepts))
        .collect()
}

/// Does this stage read the run's source at all?
///
/// A text-to-image graph and a describe-then-generate first stage both begin a
/// line rather than continuing one: they have no loader, so the shot a run is
/// filed under is where their output lands rather than what they consume. Only
/// a stage that does read a file has to match the shot.
pub fn reads_source(accepts: Accepts) -> bool {
    matches!(accepts, Accepts::Image | Accepts::Video)
}

/// Can this line be started against a source of this type?
///
/// The one check design-time validation cannot make, because a line is drawn
/// once and run against many shots. Asked again at run creation, where the shot
/// is known.
pub fn admits_source(first: &StageTyping, source: MediaType) -> Result<(), LineError> {
    if !reads_source(first.accepts) || first.accepts.admits(source) {
        return Ok(());
    }
    Err(LineError {
        stage_idx: first.stage_idx,
        message: format!(
            "Stage {} ({}) takes {}, and this shot is {}.",
            human(first.stage_idx),
            first.name,
            first.accepts.as_str(),
            source.as_str()
        ),
    })
}

/// Can this stage eat what the stage before it actually made?
///
/// The design-time check asked the same question of the workflow's *declared*
/// contract. This one asks it of the file on disk, at the moment before the
/// next stage is queued — a workflow can be re-imported, or its contract
/// corrected, long after a line was drawn, and a run should not discover that
/// by uploading a video into an image loader.
///
/// Through [`reads_as`], so the dispatcher answers with the same rule the
/// picker offered on and the validator accepted on. A clip whose connector says
/// `last_frame` is a still by the time this stage sees it, here as everywhere.
pub fn admits_upstream_output(next: &StageTyping, produced: MediaType) -> Result<(), LineError> {
    if next.accepts.admits(reads_as(produced, next)) {
        return Ok(());
    }
    Err(LineError {
        stage_idx: next.stage_idx,
        message: format!(
            "Stage {} ({}) takes {}, but stage {} handed it {}. The workflow's \
             contract has changed since this line was built.",
            human(next.stage_idx),
            next.name,
            next.accepts.as_str(),
            human(next.stage_idx - 1),
            produced.as_str()
        ),
    })
}

/// What a file's recorded MIME type says it is.
///
/// `None` for anything a line cannot carry — an audio track, a JSON sidecar —
/// which is a refusal rather than a guess.
pub fn media_type_of_mime(mime: &str) -> Option<MediaType> {
    if mime.starts_with("image/") {
        Some(MediaType::Image)
    } else if mime.starts_with("video/") {
        Some(MediaType::Video)
    } else if mime.starts_with("text/") {
        Some(MediaType::Text)
    } else {
        None
    }
}

// ===== Runtime: what happens after a stage finishes? ========================

/// What to do with a task that just completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// Queue this stage next, reading the completed task's output.
    Next(i32),
    /// The stage it completed at asks for a verdict. The run parks at this
    /// stage and the take waits to be looked at.
    Hold(i32),
    /// This branch of the run is done — either because that was the last stage
    /// and its output is the product, or because a person looked at this take
    /// and chose another one. Both mean the same thing to the pass: queue
    /// nothing, and stop owing this task anything.
    Finished,
}

/// What is known about one completed take, beyond where it sits in the line.
///
/// The three fields answer one question between them — *may this take go on?* —
/// and they are separate because the run's behaviour differs in all three ways:
/// a take at an ordinary stage continues, a kept take continues, a passed-over
/// take stops, and a take nobody has looked at yet parks the whole run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HoldGate {
    /// The stage it completed at is marked `hold_for_review`.
    pub holds: bool,
    /// A verdict named this take among the ones that continue.
    pub kept: bool,
    /// A verdict was given over this take at all — kept, or passed over in
    /// favour of another. Set on every take a verdict covered, which is what
    /// stops a passed-over take from parking its run forever.
    pub reviewed: bool,
}

impl HoldGate {
    /// The gate at a stage nobody is reviewing, which is every stage of a line
    /// with no hold point in it.
    pub fn open() -> Self {
        HoldGate::default()
    }
}

/// One step along the line.
///
/// Fan-out needs nothing here — four completed tasks at stage 2 each ask this
/// question for themselves and each get `Next(3)`, so four takes become four
/// independent continuations without the runtime ever holding the idea of a
/// branch. A hold point does not change that; it changes *which* of the four
/// get to ask.
///
/// The last stage wins over everything, hold flag included: its output is the
/// product, so there is nothing after it to hold for. (A line is refused when
/// it is drawn if it marks its last stage, so this is a belt on top of braces —
/// but a line edited by hand should not be able to park a run at a stage no
/// verdict could ever release.)
pub fn advance_after(stage_idx: i32, stage_count: i32, gate: HoldGate) -> Advance {
    if stage_idx + 1 >= stage_count {
        return Advance::Finished;
    }
    if !gate.holds || gate.kept {
        return Advance::Next(stage_idx + 1);
    }
    if gate.reviewed {
        // Looked at, and another take was chosen. This branch ends here, and
        // its output stays in the library as one of the takes somebody was
        // shown.
        return Advance::Finished;
    }
    Advance::Hold(stage_idx)
}

// ===== Runtime: what a person said about a hold =============================

/// The three things a person can say about a held run.
///
/// Three, and only three, on purpose. "Continue but with a different prompt" is
/// an edit and a new run; folding it in here would make the verdict a form
/// rather than a button, and the whole value of a hold point is that looking at
/// four clips and picking two is one gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Proceed with the takes named. More than one is ordinary: keep two of
    /// four and both run the remainder of the line, independently.
    Continue,
    /// Run the held stage again with fresh seeds and nothing else changed. The
    /// run stays alive and holds again on the new takes.
    Regenerate,
    /// Abandon the run.
    Cancel,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Continue => "continue",
            Verdict::Regenerate => "regenerate",
            Verdict::Cancel => "cancel",
        }
    }

    pub fn parse(s: &str) -> Option<Verdict> {
        match s {
            "continue" => Some(Verdict::Continue),
            "regenerate" => Some(Verdict::Regenerate),
            "cancel" => Some(Verdict::Cancel),
            _ => None,
        }
    }
}

/// Why a verdict was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictError {
    pub message: String,
}

/// Which of a hold's takes a verdict actually applies to.
///
/// Pure, so the rules about what a verdict may name are testable without a
/// database: `waiting` is what the hold is offering, `named` is what the caller
/// asked to keep. A verdict is given over **all** the waiting takes, whether or
/// not it keeps them — that is what makes the passed-over ones stop holding the
/// run — so `reviewed` is always the whole set.
pub fn settle_verdict(
    verdict: Verdict,
    waiting: &[String],
    named: &[String],
) -> Result<Vec<String>, VerdictError> {
    // Abandoning is never refused. A run somebody wants rid of should not need
    // its takes to still exist to be got rid of — that is the one verdict that
    // has to work on a run in any state at all, or a library ends up with rows
    // nothing can clear.
    if verdict == Verdict::Cancel {
        return Ok(Vec::new());
    }
    if waiting.is_empty() {
        return Err(VerdictError {
            message: "This run is not holding anything for review.".to_string(),
        });
    }
    if verdict == Verdict::Regenerate {
        // Regenerate is about the hold, not about a selection. Naming takes for
        // it would read as though it changed something.
        return Ok(Vec::new());
    }
    if named.is_empty() {
        return Err(VerdictError {
            message: "Continuing needs at least one take. To drop them all, \
                      regenerate or cancel."
                .to_string(),
        });
    }
    let mut kept = Vec::with_capacity(named.len());
    for take in named {
        if !waiting.contains(take) {
            return Err(VerdictError {
                message: format!("{} is not one of the takes this run is holding.", take),
            });
        }
        if !kept.contains(take) {
            kept.push(take.clone());
        }
    }
    Ok(kept)
}

/// How many tasks continuing from a hold will queue, stage by stage.
///
/// A hold is a fan-out point as much as a filter: `fanouts[i]` is how many
/// takes the stage `held_at + 1 + i` expands to, so keeping two of four does
/// not halve the bill — it doubles whatever one take costs from here on. The
/// estimate is what turns "keep two" from a guess into a decision.
pub fn continuation_tasks(kept: usize, fanouts: &[usize]) -> Vec<usize> {
    let mut running = kept;
    fanouts
        .iter()
        .map(|&width| {
            running = running.saturating_mul(width.max(1));
            running
        })
        .collect()
}

// ===== Runtime: is the run over, and how did it end? ========================

/// Where one task has got to, coarsened to what a run cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhase {
    /// Still moving: anything the worker will look at again.
    InFlight,
    Completed,
    Failed,
    Cancelled,
}

/// Read a task's `status` column. Anything unrecognised is treated as still
/// moving, which is the safe way to be wrong: a run stays open rather than
/// being declared finished over a status nobody here knew about.
pub fn phase_of(status: &str) -> TaskPhase {
    match status {
        "completed" => TaskPhase::Completed,
        "failed" => TaskPhase::Failed,
        "cancelled" => TaskPhase::Cancelled,
        _ => TaskPhase::InFlight,
    }
}

/// How a run ended, or that it has not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    /// Parked at a hold point, waiting for a person. Not a terminal state and
    /// not an expiring one: a hold with no verdict stays held, for as long as
    /// that takes. Nothing sweeps a held run and nothing settles it.
    ///
    /// [`tally`] never answers this — it folds *task* statuses, and every task
    /// of a held run has completed. Holding is a fact about the line and the
    /// verdicts, so it is decided in the advance pass and written on the row.
    Held,
    Completed,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Held => "held",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
        }
    }

    /// Is this run over? A held run is not: it is waiting, and the GPU has
    /// moved on to other work while it does.
    pub fn is_terminal(self) -> bool {
        !matches!(self, RunState::Running | RunState::Held)
    }

    /// The statuses that mean "this run has not finished with its line yet".
    ///
    /// What "a run of this line is still walking it" means for the editor lock:
    /// a held run will read its later stages the moment a verdict lands, so
    /// editing the line under it is exactly the change that was refused for a
    /// running one. Spelled as a slice because the callers are SQL filters.
    pub fn live() -> &'static [&'static str] {
        &["running", "held"]
    }
}

/// What a run looks like from the outside, which is what the board draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTally {
    pub state: RunState,
    /// The stage the run is working on, 0-based — the earliest one not yet
    /// finished. Once it is over this is the stage that ended it: the one that
    /// failed, or `stage_count` when everything landed.
    pub current_stage: i32,
    pub in_flight: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

/// Fold a run's tasks into its state.
///
/// A run is *running* while any task is still moving, however many of its
/// siblings have already failed — a fan-out whose second take broke keeps
/// making the other three, and there is no sense in calling it finished while
/// a GPU is busy on its behalf. Once nothing is moving, a failure outranks a
/// cancellation outranks success, because the run did not deliver what was
/// asked and the reason worth showing is the worst one.
///
/// A run with no tasks reads as running at stage 0: the only moment that
/// happens is inside the transaction that is about to insert them.
pub fn tally(tasks: &[(i32, TaskPhase)], stage_count: i32) -> RunTally {
    let mut in_flight = 0usize;
    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut cancelled = 0usize;
    let mut earliest_unfinished: Option<i32> = None;
    let mut earliest_failed: Option<i32> = None;
    let mut earliest_cancelled: Option<i32> = None;

    for &(stage_idx, phase) in tasks {
        match phase {
            TaskPhase::InFlight => {
                in_flight += 1;
                earliest_unfinished = Some(min_opt(earliest_unfinished, stage_idx));
            }
            TaskPhase::Completed => completed += 1,
            TaskPhase::Failed => {
                failed += 1;
                earliest_failed = Some(min_opt(earliest_failed, stage_idx));
            }
            TaskPhase::Cancelled => {
                cancelled += 1;
                earliest_cancelled = Some(min_opt(earliest_cancelled, stage_idx));
            }
        }
    }

    if tasks.is_empty() {
        return RunTally {
            state: RunState::Running,
            current_stage: 0,
            in_flight,
            completed,
            failed,
            cancelled,
        };
    }

    let (state, current_stage) = if in_flight > 0 {
        (RunState::Running, earliest_unfinished.unwrap_or(0))
    } else if failed > 0 {
        (RunState::Failed, earliest_failed.unwrap_or(0))
    } else if cancelled > 0 {
        (RunState::Cancelled, earliest_cancelled.unwrap_or(0))
    } else {
        (RunState::Completed, stage_count)
    };

    RunTally {
        state,
        current_stage,
        in_flight,
        completed,
        failed,
        cancelled,
    }
}

fn min_opt(current: Option<i32>, candidate: i32) -> i32 {
    match current {
        Some(c) => c.min(candidate),
        None => candidate,
    }
}

// ===== Runtime: does this stage's output survive the run? ===================

/// What is known about a stage when the run it belongs to has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageDisposition {
    /// The user ticked "keep" on this stage.
    pub keep_flag: bool,
    /// This is the last stage: its output is the thing that was asked for.
    pub is_final: bool,
    /// This stage feeds a hold point, and the choosing still stands: its
    /// outputs are the takes a person was shown and decided between. Choosing
    /// among them is the entire point of the stage, so they survive whatever
    /// the keep flag says.
    ///
    /// False on the two paths where the choosing does **not** stand — a run
    /// abandoned at its hold, and the previous generation of a regenerate.
    /// Nobody is going to pick one of those now, so only the stage's own flag
    /// saves them.
    pub feeds_hold: bool,
}

/// Does this stage's output stay in the library once the run is over?
///
/// The default is no, and that is the point: a four-stage line makes three
/// intermediates per take, a fan-out of four makes twelve, and a library that
/// keeps all of them is a library nobody can find a photograph in. They live
/// exactly as long as they are useful — the next stage needs them, and a
/// failure wants them for inspection — and are swept when the run lands.
pub fn keeps_output(d: StageDisposition) -> bool {
    d.is_final || d.keep_flag || d.feeds_hold
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(idx: i32, name: &str, accepts: Accepts, produces: MediaType) -> StageTyping {
        StageTyping::bare(idx, name, accepts, produces)
    }

    /// The same, with the connector set: what this stage reads out of whatever
    /// the stage above it made.
    fn stage_reading(
        idx: i32,
        name: &str,
        accepts: Accepts,
        produces: MediaType,
        mode: &str,
    ) -> StageTyping {
        StageTyping {
            source_mode: Some(mode.to_string()),
            ..StageTyping::bare(idx, name, accepts, produces)
        }
    }

    /// The line this whole PR exists for.
    fn restore_4k() -> Vec<StageTyping> {
        vec![
            stage(0, "Image to Video", Accepts::Image, MediaType::Video),
            stage(1, "Interpolate", Accepts::Video, MediaType::Video),
            stage(2, "Upscale 4K", Accepts::Video, MediaType::Video),
        ]
    }

    #[test]
    fn a_line_whose_joins_all_fit_is_accepted() {
        assert_eq!(validate_chain(&restore_4k()), Ok(()));
    }

    #[test]
    fn a_stage_that_cannot_eat_what_the_one_before_it_makes_is_refused() {
        let mut line = restore_4k();
        // Somebody dropped a still-image restorer in at the top, so the clip
        // maker below it is handed a still it cannot animate into a clip —
        // and there is no frame of a still to fall back on.
        line[1] = stage(1, "Restore Portrait", Accepts::Image, MediaType::Image);
        let err = validate_chain(&line).unwrap_err();
        assert_eq!(err.stage_idx, 2);
        assert_eq!(
            err.message,
            "Stage 3 (Upscale 4K) takes video, but stage 2 (Restore Portrait) produces image."
        );
    }

    #[test]
    fn a_stage_that_consumes_nothing_can_only_come_first() {
        let t2i = stage(0, "Text to Image", Accepts::None, MediaType::Image);
        // First: fine.
        assert_eq!(
            validate_chain(&[
                t2i.clone(),
                stage(1, "Upscale", Accepts::Image, MediaType::Image)
            ]),
            Ok(())
        );
        // Second: nothing could ever be handed to it.
        let err = validate_chain(&[
            stage(0, "Upscale", Accepts::Image, MediaType::Image),
            stage(1, "Text to Image", Accepts::None, MediaType::Image),
        ])
        .unwrap_err();
        assert_eq!(err.stage_idx, 1);
        assert!(
            err.message.contains("only be the first stage"),
            "{}",
            err.message
        );
    }

    // === FR9 — a describe stage is transparent to the media flow ============

    /// The line the whole prompt compiler exists for.
    fn describe_then_clip() -> Vec<StageTyping> {
        vec![
            stage(0, "Describe", Accepts::Image, MediaType::Text),
            stage(1, "Photo to clip", Accepts::Image, MediaType::Video),
        ]
    }

    #[test]
    fn a_describe_stage_does_not_break_the_chain_after_it() {
        // Stage 2 takes an image and stage 1 produced text, and yet this line
        // is obviously correct: a describe stage makes no file, so the
        // photograph it read is the photograph the stage after it reads.
        assert_eq!(validate_chain(&describe_then_clip()), Ok(()));
    }

    #[test]
    fn a_join_is_checked_against_the_last_stage_that_made_a_file() {
        let line = vec![
            stage(0, "Restore Portrait", Accepts::Image, MediaType::Image),
            stage(1, "Describe", Accepts::Image, MediaType::Text),
            stage(2, "Upscale 4K", Accepts::Video, MediaType::Video),
        ];
        let err = validate_chain(&line).unwrap_err();
        assert_eq!(err.stage_idx, 2);
        // Stage 2 made nothing, so stage 1 is what stage 3 is refused against.
        assert_eq!(
            err.message,
            "Stage 3 (Upscale 4K) takes video, but stage 1 (Restore Portrait) produces image."
        );
    }

    #[test]
    fn every_stage_before_the_first_file_reads_the_shot() {
        let line = describe_then_clip();
        assert_eq!(
            source_readers(&line).len(),
            2,
            "both stages read the photograph itself"
        );
        // And once something has made a file, nothing after it does.
        let line = restore_4k();
        let readers = source_readers(&line);
        assert_eq!(readers.len(), 1);
        assert_eq!(readers[0].stage_idx, 0);
    }

    #[test]
    fn a_shot_the_describe_stage_cannot_read_is_still_refused() {
        // The describe stage takes a still; so does the clip stage after it.
        // A video shot fits neither, and both say so.
        let line = describe_then_clip();
        for reader in source_readers(&line) {
            assert!(admits_source(reader, MediaType::Video).is_err());
        }
    }

    #[test]
    fn a_stage_that_consumes_nothing_cannot_hide_behind_a_describe_stage() {
        let line = vec![
            stage(0, "Describe", Accepts::Image, MediaType::Text),
            stage(1, "Text to Image", Accepts::None, MediaType::Image),
        ];
        let err = validate_chain(&line).unwrap_err();
        assert_eq!(err.stage_idx, 1);
        assert!(
            err.message.contains("only be the first stage"),
            "{}",
            err.message
        );
    }

    // === What one join carries, which is one rule and not three ===========

    #[test]
    fn a_stage_that_makes_no_file_does_not_interrupt_the_media_flow() {
        // The picker used to ask this of its own copy of the rule. There is
        // one copy now, and this is it: a describe stage reads the photograph
        // and hands on a sentence, so the stage after it is still reading the
        // photograph, and the join says `image` rather than `text`.
        let line = vec![
            stage(0, "Restore Portrait", Accepts::Image, MediaType::Image),
            stage(1, "Describe", Accepts::Image, MediaType::Text),
            stage(2, "Describe Again", Accepts::Image, MediaType::Text),
            stage(3, "Restore Again", Accepts::Image, MediaType::Image),
        ];
        assert_eq!(carried_into(&line, 0), None, "the first reads the shot");
        assert_eq!(carried_into(&line, 1).unwrap().produced, MediaType::Image);
        assert_eq!(carried_into(&line, 2).unwrap().from, 0, "past one describe");
        let last = carried_into(&line, 3).unwrap();
        assert_eq!(last.from, 0, "past two of them");
        assert_eq!(last.produced, MediaType::Image);
        // A line that has so far only described things is still reading the
        // shot, so there is no join above the next stage at all.
        let only_text = vec![
            stage(0, "Describe", Accepts::Image, MediaType::Text),
            stage(1, "Photo to clip", Accepts::Image, MediaType::Video),
        ];
        assert_eq!(carried_into(&only_text, 1), None);
        assert_eq!(carried_into(&only_text, 9), None, "past the end");
    }

    /// The gap FR5b found: `Accepts::admits` refuses `video → image`, so the
    /// editor offered `last_frame` on a connector the validator then refused.
    #[test]
    fn a_still_stage_may_follow_a_clip_stage_because_a_frame_of_a_clip_is_a_still() {
        // No source mode at all. A graph with no video loader gets frame zero
        // — the dispatcher's own default — so this line is buildable, and was
        // not before.
        let line = vec![
            stage(0, "Photo to 5s Clip", Accepts::Image, MediaType::Video),
            stage(1, "Restore Portrait", Accepts::Image, MediaType::Image),
        ];
        assert_eq!(validate_chain(&line), Ok(()));
        let carried = carried_into(&line, 1).unwrap();
        assert_eq!(
            carried.produced,
            MediaType::Video,
            "the connector says clip"
        );
        assert_eq!(carried.reads, MediaType::Image, "the stage reads a still");

        // And the mode FR2 built this for, said out loud.
        let asked = vec![
            stage(0, "Photo to 5s Clip", Accepts::Image, MediaType::Video),
            stage_reading(
                1,
                "Restore Portrait",
                Accepts::Image,
                MediaType::Image,
                "last_frame",
            ),
        ];
        assert_eq!(validate_chain(&asked), Ok(()));
        assert_eq!(carried_into(&asked, 1).unwrap().reads, MediaType::Image);
    }

    #[test]
    fn asking_a_still_stage_for_the_whole_clip_is_still_refused() {
        // `whole_video` is not a frame, and a stage with no video loader
        // cannot read one. Saying so here beats a ComfyUI validation error.
        let line = vec![
            stage(0, "Photo to 5s Clip", Accepts::Image, MediaType::Video),
            stage_reading(
                1,
                "Restore Portrait",
                Accepts::Image,
                MediaType::Image,
                "whole_video",
            ),
        ];
        let err = validate_chain(&line).unwrap_err();
        assert_eq!(err.stage_idx, 1);
        assert_eq!(
            err.message,
            "Stage 2 (Restore Portrait) takes image, but stage 1 (Photo to 5s Clip) produces video."
        );
    }

    #[test]
    fn a_clip_stage_asked_for_a_frame_still_carries_a_clip() {
        // The extend-clip shape: a graph with a video loader *and* an image
        // loader, told to take the last frame. Which of its own loaders the
        // file lands in is the stage's business — the line still carries video,
        // and refusing it would be the reverse of the bug above.
        let line = vec![
            stage(0, "Photo to 5s Clip", Accepts::Image, MediaType::Video),
            StageTyping {
                takes_video: true,
                ..stage_reading(
                    1,
                    "Extend Clip",
                    Accepts::Video,
                    MediaType::Video,
                    "last_frame",
                )
            },
        ];
        assert_eq!(validate_chain(&line), Ok(()));
        assert_eq!(carried_into(&line, 1).unwrap().reads, MediaType::Video);
    }

    #[test]
    fn the_validator_and_the_dispatcher_admit_the_same_joins() {
        // `admits_upstream_output` is the dispatcher's second look, taken
        // against the file that actually turned up. It must agree with the
        // design-time check on every pair, or a line that draws will fail at
        // upload — which is the failure this whole subsystem is arranged to
        // avoid.
        let library = [
            stage(0, "Photo to Clip", Accepts::Image, MediaType::Video),
            stage(0, "Upscale 4K", Accepts::Video, MediaType::Video),
            stage(0, "Restore Portrait", Accepts::Image, MediaType::Image),
            stage(0, "Describe", Accepts::Image, MediaType::Text),
            stage_reading(
                0,
                "Last Frame",
                Accepts::Image,
                MediaType::Image,
                "last_frame",
            ),
            stage_reading(
                0,
                "Whole Clip",
                Accepts::Image,
                MediaType::Image,
                "whole_video",
            ),
        ];
        for up in &library {
            for down in &library {
                if up.produces == MediaType::Text {
                    // A describe stage hands on no file, so there is nothing
                    // for the dispatcher to check against.
                    continue;
                }
                let chain = vec![
                    StageTyping {
                        stage_idx: 0,
                        ..up.clone()
                    },
                    StageTyping {
                        stage_idx: 1,
                        ..down.clone()
                    },
                ];
                assert_eq!(
                    validate_chain(&chain).is_ok(),
                    admits_upstream_output(&chain[1], up.produces).is_ok(),
                    "{} after {}: the validator and the dispatcher disagree",
                    down.name,
                    up.name
                );
            }
        }
    }

    #[test]
    fn an_empty_line_is_not_a_line() {
        assert_eq!(validate_chain(&[]).unwrap_err().stage_idx, -1);
    }

    #[test]
    fn the_first_stage_still_has_to_match_the_shot() {
        let line = restore_4k();
        assert_eq!(admits_source(&line[0], MediaType::Image), Ok(()));
        let err = admits_source(&line[0], MediaType::Video).unwrap_err();
        assert_eq!(
            err.message,
            "Stage 1 (Image to Video) takes image, and this shot is video."
        );
        // A stage that reads no file does not care what the shot is.
        let t2i = stage(0, "Text to Image", Accepts::None, MediaType::Image);
        assert_eq!(admits_source(&t2i, MediaType::Video), Ok(()));
    }

    #[test]
    fn a_contract_corrected_after_the_line_was_built_is_caught_before_dispatch() {
        // The design-time check passed when the line was drawn. Since then
        // somebody corrected stage 3's contract to say it takes clips, and the
        // stage above it makes stills.
        let next = stage(2, "Upscale 4K", Accepts::Video, MediaType::Video);
        let err = admits_upstream_output(&next, MediaType::Image).unwrap_err();
        assert!(err.message.contains("handed it image"), "{}", err.message);
        assert!(
            err.message.contains("contract has changed"),
            "{}",
            err.message
        );
        assert_eq!(admits_upstream_output(&next, MediaType::Video), Ok(()));

        // The other way round is not a mismatch any more: a still stage handed
        // a clip reads a frame of it, here exactly as in the editor.
        let still = stage(2, "Restore Portrait", Accepts::Image, MediaType::Image);
        assert_eq!(admits_upstream_output(&still, MediaType::Video), Ok(()));
        // Unless it asked for the whole thing, which it cannot read.
        let whole = stage_reading(
            2,
            "Restore Portrait",
            Accepts::Image,
            MediaType::Image,
            "whole_video",
        );
        assert!(admits_upstream_output(&whole, MediaType::Video).is_err());
    }

    #[test]
    fn a_files_mime_type_says_what_kind_of_thing_it_is() {
        assert_eq!(media_type_of_mime("image/png"), Some(MediaType::Image));
        assert_eq!(media_type_of_mime("video/mp4"), Some(MediaType::Video));
        assert_eq!(media_type_of_mime("text/plain"), Some(MediaType::Text));
        // A line cannot carry a sound file, and will not pretend to.
        assert_eq!(media_type_of_mime("audio/flac"), None);
    }

    #[test]
    fn a_stage_advances_until_the_line_runs_out() {
        let open = HoldGate::open();
        assert_eq!(advance_after(0, 4, open), Advance::Next(1));
        assert_eq!(advance_after(2, 4, open), Advance::Next(3));
        assert_eq!(advance_after(3, 4, open), Advance::Finished);
        // A single-workflow run is a one-stage line, and finishes at once.
        assert_eq!(advance_after(0, 1, open), Advance::Finished);
    }

    // === FR5c — a hold point ================================================

    #[test]
    fn a_take_at_a_hold_point_parks_its_run_until_somebody_looks_at_it() {
        let holding = HoldGate {
            holds: true,
            ..HoldGate::open()
        };
        assert_eq!(advance_after(1, 4, holding), Advance::Hold(1));
        // The same stage without the flag is the ordinary rule, untouched.
        assert_eq!(advance_after(1, 4, HoldGate::open()), Advance::Next(2));
    }

    #[test]
    fn one_verdict_lets_the_takes_it_kept_through_and_stops_the_rest() {
        // The whole of "continue with two of four": the gate is asked once per
        // take, and answers differently for the two the person kept and the two
        // they passed over. That is the fan-in — four takes, one verdict — and
        // the fan-out back out of it needs no code at all.
        let kept = HoldGate {
            holds: true,
            kept: true,
            reviewed: true,
        };
        let passed = HoldGate {
            holds: true,
            kept: false,
            reviewed: true,
        };
        assert_eq!(advance_after(1, 4, kept), Advance::Next(2));
        assert_eq!(
            advance_after(1, 4, passed),
            Advance::Finished,
            "a take somebody looked at and did not choose is a branch that ends"
        );
    }

    #[test]
    fn the_last_stage_is_the_product_and_cannot_be_held() {
        // A line is refused at draw time for marking its last stage; a line
        // edited by hand around that check must still not park a run at a stage
        // no verdict could release.
        let holding = HoldGate {
            holds: true,
            ..HoldGate::open()
        };
        assert_eq!(advance_after(3, 4, holding), Advance::Finished);
    }

    #[test]
    fn a_verdict_is_given_over_every_take_it_was_shown() {
        let waiting = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // Continue keeps what it names…
        assert_eq!(
            settle_verdict(Verdict::Continue, &waiting, &["a".into(), "c".into()]),
            Ok(vec!["a".to_string(), "c".to_string()])
        );
        // …and the same take twice is one take.
        assert_eq!(
            settle_verdict(Verdict::Continue, &waiting, &["a".into(), "a".into()]),
            Ok(vec!["a".to_string()])
        );
        // Regenerate and cancel keep nothing, and naming takes for them changes
        // nothing rather than reading as though it did.
        assert_eq!(
            settle_verdict(Verdict::Regenerate, &waiting, &["a".into()]),
            Ok(Vec::new())
        );
        assert_eq!(
            settle_verdict(Verdict::Cancel, &waiting, &[]),
            Ok(Vec::new())
        );
    }

    #[test]
    fn abandoning_is_the_one_verdict_that_is_never_refused() {
        // A run whose takes were deleted by hand still has to be clearable, or
        // the library keeps a row nothing can ever move.
        assert_eq!(settle_verdict(Verdict::Cancel, &[], &[]), Ok(Vec::new()));
        assert!(settle_verdict(Verdict::Regenerate, &[], &[]).is_err());
        assert!(settle_verdict(Verdict::Continue, &[], &["a".into()]).is_err());
    }

    #[test]
    fn a_verdict_that_names_nothing_or_names_a_stranger_is_refused() {
        let waiting = vec!["a".to_string(), "b".to_string()];
        let err = settle_verdict(Verdict::Continue, &waiting, &[]).unwrap_err();
        assert!(err.message.contains("at least one take"), "{}", err.message);

        let err = settle_verdict(Verdict::Continue, &waiting, &["z".into()]).unwrap_err();
        assert!(
            err.message.contains("not one of the takes"),
            "{}",
            err.message
        );

        // And a run holding nothing has no verdict to give.
        let err = settle_verdict(Verdict::Continue, &[], &["a".into()]).unwrap_err();
        assert!(err.message.contains("not holding"), "{}", err.message);
    }

    #[test]
    fn a_verdict_reads_and_writes_the_same_word() {
        for v in [Verdict::Continue, Verdict::Regenerate, Verdict::Cancel] {
            assert_eq!(Verdict::parse(v.as_str()), Some(v));
        }
        assert_eq!(Verdict::parse("maybe"), None);
    }

    #[test]
    fn keeping_two_of_four_still_pays_for_every_stage_after_it() {
        // The `×4 extend → hold → upscale` line the feature exists for: two
        // kept takes, one upscale each.
        assert_eq!(continuation_tasks(2, &[1]), vec![2]);
        // And a hold is a fan-out point in its own right — the stage after it
        // sweeping four seeds means eight upscales, not two.
        assert_eq!(continuation_tasks(2, &[4]), vec![8]);
        // Stage by stage down a longer tail.
        assert_eq!(continuation_tasks(1, &[2, 1, 3]), vec![2, 2, 6]);
        // A hold on the last stage has no tail, and costs nothing to continue.
        assert_eq!(continuation_tasks(4, &[]), Vec::<usize>::new());
    }

    #[test]
    fn a_held_run_is_neither_over_nor_idle() {
        assert!(!RunState::Held.is_terminal(), "a hold is not an ending");
        assert_eq!(RunState::Held.as_str(), "held");
        // And the line stays locked: a verdict puts it straight back to
        // reading the stages after the hold.
        assert!(RunState::live().contains(&RunState::Held.as_str()));
        assert!(RunState::live().contains(&RunState::Running.as_str()));
        assert!(!RunState::live().contains(&RunState::Completed.as_str()));
    }

    #[test]
    fn a_run_is_running_while_anything_is_still_moving() {
        let t = tally(
            &[
                (0, TaskPhase::Completed),
                (1, TaskPhase::InFlight),
                (1, TaskPhase::InFlight),
            ],
            4,
        );
        assert_eq!(t.state, RunState::Running);
        assert_eq!(t.current_stage, 1);
        assert_eq!(t.in_flight, 2);
        assert_eq!(t.completed, 1);
    }

    #[test]
    fn the_stage_a_run_is_on_is_the_earliest_one_still_unfinished() {
        // A fan-out where one take is lagging: three at stage 3, one still at 2.
        let t = tally(
            &[
                (2, TaskPhase::InFlight),
                (3, TaskPhase::InFlight),
                (3, TaskPhase::InFlight),
                (3, TaskPhase::InFlight),
            ],
            4,
        );
        assert_eq!(t.current_stage, 2, "the board should show the slow one");
    }

    #[test]
    fn a_run_whose_every_task_landed_is_completed() {
        let t = tally(
            &[
                (0, TaskPhase::Completed),
                (1, TaskPhase::Completed),
                (2, TaskPhase::Completed),
            ],
            3,
        );
        assert_eq!(t.state, RunState::Completed);
        assert_eq!(t.current_stage, 3, "3 of 3");
    }

    #[test]
    fn a_failed_stage_fails_the_run_and_names_itself() {
        let t = tally(&[(0, TaskPhase::Completed), (1, TaskPhase::Failed)], 4);
        assert_eq!(t.state, RunState::Failed);
        assert_eq!(t.current_stage, 1, "the stage to resume from");
        assert_eq!(t.failed, 1);
    }

    #[test]
    fn one_broken_take_does_not_stop_the_other_three() {
        // Stage 2 fanned out four ways; one failed, the rest are upscaling.
        let t = tally(
            &[
                (0, TaskPhase::Completed),
                (1, TaskPhase::Failed),
                (1, TaskPhase::Completed),
                (1, TaskPhase::Completed),
                (1, TaskPhase::Completed),
                (2, TaskPhase::InFlight),
                (2, TaskPhase::InFlight),
                (2, TaskPhase::InFlight),
            ],
            3,
        );
        assert_eq!(
            t.state,
            RunState::Running,
            "a GPU is still busy on this run's behalf"
        );
        assert_eq!(t.failed, 1);
        // And when they land, the failure is what the run is remembered by.
        let t = tally(
            &[
                (1, TaskPhase::Failed),
                (2, TaskPhase::Completed),
                (2, TaskPhase::Completed),
                (2, TaskPhase::Completed),
            ],
            3,
        );
        assert_eq!(t.state, RunState::Failed);
        assert_eq!(t.current_stage, 1);
    }

    #[test]
    fn a_cancelled_run_is_not_a_failed_one() {
        let t = tally(&[(0, TaskPhase::Cancelled)], 3);
        assert_eq!(t.state, RunState::Cancelled);
        // But a failure outranks it: the run broke before anyone stopped it.
        let t = tally(&[(0, TaskPhase::Cancelled), (0, TaskPhase::Failed)], 3);
        assert_eq!(t.state, RunState::Failed);
    }

    #[test]
    fn a_status_nobody_here_knows_leaves_the_run_open() {
        assert_eq!(phase_of("awaiting_output"), TaskPhase::InFlight);
        assert_eq!(phase_of("downloading"), TaskPhase::InFlight);
        assert_eq!(phase_of("something_from_2029"), TaskPhase::InFlight);
        assert_eq!(phase_of("completed"), TaskPhase::Completed);
    }

    #[test]
    fn an_intermediate_is_swept_unless_it_was_asked_for() {
        let discard = StageDisposition::default();
        assert!(!keeps_output(discard));
        assert!(keeps_output(StageDisposition {
            keep_flag: true,
            ..discard
        }));
        // The last stage is the product, whatever the flag says.
        assert!(keeps_output(StageDisposition {
            is_final: true,
            ..discard
        }));
        // And so are the takes somebody chose between: they are what the hold
        // point was for, and a run that landed does not throw away the
        // alternatives it was picked out of.
        assert!(keeps_output(StageDisposition {
            feeds_hold: true,
            ..discard
        }));
    }
}
