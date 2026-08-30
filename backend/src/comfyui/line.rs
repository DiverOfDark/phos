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
//! * **Does this chain hold together?** [`validate_chain`] asks
//!   [`Accepts::admits`] of every join. Not a second compatibility rule — the
//!   same function FR5b's stage picker will call, so a line the editor offers
//!   and a line the validator accepts can never disagree.
//! * **What happens after a stage finishes?** [`advance_after`]. One step, and
//!   deliberately dull: v1 lines are linear, fan-out propagates by each
//!   completed task spawning its own continuation, and fan-*in* waits for FR5c.
//! * **Is the run over, and how did it end?** [`tally`]. A run is running while
//!   any of its tasks is still moving, and then it is however its tasks ended.
//!   The stage it is "on" is the earliest one still unfinished, which is the
//!   honest answer when a fan-out has runners at two stages at once.
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

// ===== Design time: does this chain hold together? ==========================

/// A stage, reduced to what deciding needs: what it eats and what it hands on.
#[derive(Debug, Clone, PartialEq)]
pub struct StageTyping {
    pub stage_idx: i32,
    /// The workflow's name, so a refusal can say which stage is the problem in
    /// the words the person used.
    pub name: String,
    pub accepts: Accepts,
    pub produces: MediaType,
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

/// Can every stage eat what the one before it produces?
///
/// Rejected at design time, so a four-hour chain is refused when it is drawn
/// rather than after its third stage has run. The rule is
/// [`Accepts::admits`] and nothing else — including the case where a stage
/// consumes nothing at all, which `admits` answers `false` to, and which reads
/// as "that can only be the first stage of a line".
///
/// # A text stage is transparent
///
/// A describe stage makes no file: FR5a's `produces: text` writes no `files`
/// row, and there is nothing for the stage after it to load. What flows down
/// the line is therefore *unchanged* by it — the photograph the describe stage
/// read is the photograph the generation stage after it reads — and its
/// sentence binds into that stage's prompt slot instead.
///
/// Which is why the line the whole feature exists for validates at all:
///
/// ```text
///   [1]  DESCRIBE (QWEN-VL)     image → text
///    │   text → positive
///   [2]  PHOTO → 5S CLIP        image + text → video
/// ```
///
/// Stage 2 accepts `image`, and asking it to admit stage 1's `text` would
/// refuse a line that is obviously correct. So each join is checked against the
/// last stage that actually made a file, and a line of nothing but text stages
/// leaves every join reading the run's own source.
pub fn validate_chain(stages: &[StageTyping]) -> Result<(), LineError> {
    if stages.is_empty() {
        return Err(LineError {
            stage_idx: -1,
            message: "A line needs at least one stage.".to_string(),
        });
    }

    // The last stage that made a file, and what it made. `None` until one has:
    // every stage until then reads the run's source, which is checked when the
    // run starts and the shot is known.
    let mut carried: Option<&StageTyping> = None;

    for (position, down) in stages.iter().enumerate() {
        match carried {
            _ if position == 0 => {}
            Some(up) if down.accepts.admits(up.produces) => {}
            Some(up) => {
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
                            human(up.stage_idx),
                            up.name,
                            up.produces.as_str()
                        )
                    },
                });
            }
            // Only describe stages so far, so this one reads the run's source
            // like the first stage does. Nothing to check here — except that a
            // stage consuming nothing at all still cannot follow anything.
            None if down.accepts.starts_a_line() => {
                return Err(LineError {
                    stage_idx: down.stage_idx,
                    message: starts_a_line_message(down),
                });
            }
            None => {}
        }

        if down.produces != MediaType::Text {
            carried = Some(down);
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
/// every one of them has to fit the shot the run is against.
pub fn source_readers(stages: &[StageTyping]) -> Vec<&StageTyping> {
    let mut readers = Vec::new();
    for stage in stages {
        if reads_source(stage.accepts) {
            readers.push(stage);
        }
        if stage.produces != MediaType::Text {
            break;
        }
    }
    readers
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
pub fn admits_upstream_output(next: &StageTyping, produced: MediaType) -> Result<(), LineError> {
    if next.accepts.admits(produced) {
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
    /// That was the last stage: this branch of the run is done.
    Finished,
}

/// One step along the line. Fan-out needs nothing here — four completed tasks
/// at stage 2 each ask this question for themselves and each get `Next(3)`, so
/// four takes become four independent continuations without the runtime ever
/// holding the idea of a branch.
pub fn advance_after(stage_idx: i32, stage_count: i32) -> Advance {
    if stage_idx + 1 < stage_count {
        Advance::Next(stage_idx + 1)
    } else {
        Advance::Finished
    }
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
    Completed,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        self != RunState::Running
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageDisposition {
    /// The user ticked "keep" on this stage.
    pub keep_flag: bool,
    /// This is the last stage: its output is the thing that was asked for.
    pub is_final: bool,
    // FR5c adds `feeds_hold`: a stage whose output a person is going to be
    // shown must survive whatever the flag says. It is a third `||` term here
    // and nothing else, which is why this is a function rather than a column
    // read at the call site.
}

/// Does this stage's output stay in the library once the run is over?
///
/// The default is no, and that is the point: a four-stage line makes three
/// intermediates per take, a fan-out of four makes twelve, and a library that
/// keeps all of them is a library nobody can find a photograph in. They live
/// exactly as long as they are useful — the next stage needs them, and a
/// failure wants them for inspection — and are swept when the run lands.
pub fn keeps_output(d: StageDisposition) -> bool {
    d.is_final || d.keep_flag
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(idx: i32, name: &str, accepts: Accepts, produces: MediaType) -> StageTyping {
        StageTyping {
            stage_idx: idx,
            name: name.to_string(),
            accepts,
            produces,
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
        // Somebody dropped a still-image upscaler in after the clip.
        line[2] = stage(2, "Upscale 4K", Accepts::Image, MediaType::Image);
        let err = validate_chain(&line).unwrap_err();
        assert_eq!(err.stage_idx, 2);
        assert_eq!(
            err.message,
            "Stage 3 (Upscale 4K) takes image, but stage 2 (Interpolate) produces video."
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
            stage(0, "Image to Video", Accepts::Image, MediaType::Video),
            stage(1, "Describe", Accepts::Video, MediaType::Text),
            stage(2, "Upscale", Accepts::Image, MediaType::Image),
        ];
        let err = validate_chain(&line).unwrap_err();
        assert_eq!(err.stage_idx, 2);
        // Stage 2 made nothing, so stage 1 is what stage 3 is refused against.
        assert_eq!(
            err.message,
            "Stage 3 (Upscale) takes image, but stage 1 (Image to Video) produces video."
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
        // somebody corrected stage 3's contract to say it takes stills.
        let next = stage(2, "Upscale 4K", Accepts::Image, MediaType::Image);
        let err = admits_upstream_output(&next, MediaType::Video).unwrap_err();
        assert!(err.message.contains("handed it video"), "{}", err.message);
        assert!(
            err.message.contains("contract has changed"),
            "{}",
            err.message
        );
        assert_eq!(admits_upstream_output(&next, MediaType::Image), Ok(()));
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
        assert_eq!(advance_after(0, 4), Advance::Next(1));
        assert_eq!(advance_after(2, 4), Advance::Next(3));
        assert_eq!(advance_after(3, 4), Advance::Finished);
        // A single-workflow run is a one-stage line, and finishes at once.
        assert_eq!(advance_after(0, 1), Advance::Finished);
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
        let discard = StageDisposition {
            keep_flag: false,
            is_final: false,
        };
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
    }
}
