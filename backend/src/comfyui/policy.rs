//! What to do about what ComfyUI said.
//!
//! Pure decisions, no IO: how long a finished-but-silent prompt is given to
//! publish its files, how often to look again while waiting, and whether a
//! failure is worth another attempt or is simply the answer.
//!
//! Splitting failures by *site* is the point of the second half. A rejected
//! graph, a node exception or an unreadable source file will fail identically
//! next time; a dropped connection very likely will not. Before this split
//! every one of them was terminal, which is why a working workflow could need
//! several manual reruns.

use super::workflow;
use serde_json::Value;
use std::time::Duration;

/// Marker in the message of a prompt ComfyUI refused. A refused graph is
/// refused for good, so `classify_failure` reads this as permanent.
pub(crate) const PROMPT_REJECTED: &str = "ComfyUI rejected the prompt";

/// Budget for an image-only workflow. Files are closed by the time the history
/// entry lands; this only covers the write-then-publish gap.
pub(crate) const SETTLE_BUDGET_IMAGE: Duration = Duration::from_secs(60);

/// Budget for a workflow with a video output. `VHS_VideoCombine` shells out to
/// ffmpeg, so a large mp4 can be in history minutes before it is on disk.
pub(crate) const SETTLE_BUDGET_VIDEO: Duration = Duration::from_secs(15 * 60);

/// How long a completed-but-empty prompt is given to publish its files.
pub(crate) fn settle_budget(workflow: &Value) -> Duration {
    if workflow::has_slow_output(workflow) {
        SETTLE_BUDGET_VIDEO
    } else {
        SETTLE_BUDGET_IMAGE
    }
}

/// Re-check spacing while settling: tight at first, because most runs settle in
/// a second or two, then backing off so a 15-minute video wait is not 300 polls.
pub(crate) fn settle_recheck_delay(elapsed: Duration) -> Duration {
    match elapsed.as_secs() {
        0..=9 => Duration::from_secs(2),
        10..=59 => Duration::from_secs(5),
        60..=299 => Duration::from_secs(15),
        _ => Duration::from_secs(30),
    }
}

/// What to do with a task whose prompt finished without naming a file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SettleDecision {
    /// First sighting — start the clock and come back at `recheck_at`.
    Start {
        deadline: chrono::NaiveDateTime,
        recheck_at: chrono::NaiveDateTime,
    },
    /// Inside the budget — probe the deterministic names, then come back.
    Wait { recheck_at: chrono::NaiveDateTime },
    /// The budget is spent. Probe once more, then give up.
    Expired,
}

pub(crate) fn decide_settle(
    now: chrono::NaiveDateTime,
    settle_until: Option<chrono::NaiveDateTime>,
    budget: Duration,
) -> SettleDecision {
    let budget_secs = budget.as_secs() as i64;
    match settle_until {
        None => SettleDecision::Start {
            deadline: now + chrono::Duration::seconds(budget_secs),
            recheck_at: now
                + chrono::Duration::seconds(settle_recheck_delay(Duration::ZERO).as_secs() as i64),
        },
        Some(deadline) if now < deadline => {
            let remaining = (deadline - now).num_seconds().max(0);
            let elapsed = Duration::from_secs((budget_secs - remaining).max(0) as u64);
            SettleDecision::Wait {
                recheck_at: now
                    + chrono::Duration::seconds(settle_recheck_delay(elapsed).as_secs() as i64),
            }
        }
        Some(_) => SettleDecision::Expired,
    }
}

/// Total attempts a transient failure gets, the first one included.
pub(crate) const MAX_ATTEMPTS: i32 = 4;

/// Where a task fell over. The site decides whether trying again can possibly
/// help — a missing source file will still be missing, a dropped connection
/// probably will not be dropped again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureSite {
    /// Could not read/decode the library file being enhanced.
    SourceImage,
    /// Could not push the source image to ComfyUI.
    Upload,
    /// The stored workflow is not valid JSON.
    WorkflowJson,
    /// `/prompt` did not accept the graph.
    Queue,
    /// `/history` could not be read, or the prompt vanished from both history
    /// and queue.
    History,
    /// A node raised during execution.
    Execution,
    /// The settle budget ran out with nothing on disk. This is also where a
    /// file history *named* but `/view` never served ends up: a 404 while the
    /// muxer closes the file is the same "finished, not published" state as an
    /// empty history entry, so it waits out the same budget rather than a fixed
    /// handful of retries.
    Settle,
    /// A stage of a line was handed something its contract says it cannot read.
    /// The line was validated when it was drawn, so this means the workflow has
    /// been re-imported or its contract corrected since. Permanent: it will be
    /// handed the same file next time, and the fix is to the contract or the
    /// line rather than to the run.
    StageMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Permanent,
    Transient,
}

pub(crate) fn classify_failure(site: FailureSite, message: &str) -> FailureKind {
    match site {
        // Bad input, bad graph, or a node that raised: identical next time.
        FailureSite::SourceImage
        | FailureSite::WorkflowJson
        | FailureSite::Execution
        | FailureSite::Settle
        | FailureSite::StageMismatch => FailureKind::Permanent,
        // A rejected prompt is a validation failure; anything else on /prompt is
        // transport.
        FailureSite::Queue => {
            if message.contains(PROMPT_REJECTED) {
                FailureKind::Permanent
            } else {
                FailureKind::Transient
            }
        }
        FailureSite::Upload | FailureSite::History => FailureKind::Transient,
    }
}

/// What to do about a failure, given how many attempts this task has already had.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FailureAction {
    /// Give up, recording this message.
    Fail(String),
    /// Queue attempt `attempt` of [`MAX_ATTEMPTS`] after `delay`.
    Retry {
        attempt: i32,
        delay: Duration,
        message: String,
    },
}

/// After a transient failure, is the ComfyUI prompt still worth going back to?
///
/// A prompt that already ran does not need to run again — re-polling it is
/// cheaper and far likelier to work than re-executing the graph. Only failures
/// that happened before the prompt was accepted start over.
pub(crate) fn retry_resumes_prompt(site: FailureSite) -> bool {
    matches!(site, FailureSite::History)
}

/// Backoff before the next attempt: 5s, 15s, 45s.
pub(crate) fn retry_backoff(retry_count: i32) -> Duration {
    let step = retry_count.clamp(0, 4) as u32;
    Duration::from_secs(5 * 3u64.pow(step))
}

/// Decide between another attempt and a final failure. `retry_count` is what the
/// row already records, so the first failure arrives with 0.
pub(crate) fn plan_failure(site: FailureSite, message: &str, retry_count: i32) -> FailureAction {
    let attempts_made = retry_count + 1;
    if classify_failure(site, message) == FailureKind::Transient && attempts_made < MAX_ATTEMPTS {
        FailureAction::Retry {
            attempt: attempts_made + 1,
            delay: retry_backoff(retry_count),
            message: message.to_string(),
        }
    } else if attempts_made > 1 {
        FailureAction::Fail(format!("{} (after {} attempts)", message, attempts_made))
    } else {
        FailureAction::Fail(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::timestamp::parse_ts;
    use serde_json::json;

    fn dt(s: &str) -> chrono::NaiveDateTime {
        parse_ts(s).expect("fixture timestamp")
    }

    #[test]
    fn defect_2_video_workflows_get_the_long_budget() {
        let video = json!({
            "12": { "class_type": "VHS_VideoCombine",
                    "inputs": { "filename_prefix": "AnimateDiff" } }
        });
        let images = json!({
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "ComfyUI" } }
        });
        assert_eq!(settle_budget(&video), SETTLE_BUDGET_VIDEO);
        assert_eq!(settle_budget(&images), SETTLE_BUDGET_IMAGE);
        // 10s of budget was never going to cover an ffmpeg mux.
        assert!(SETTLE_BUDGET_VIDEO > Duration::from_secs(10 * 60));
    }

    #[test]
    fn defect_2_settle_starts_a_clock_then_expires() {
        let now = dt("2026-08-30 12:00:00");
        let budget = Duration::from_secs(60);

        // First sighting: start the clock, come back shortly.
        let SettleDecision::Start {
            deadline,
            recheck_at,
        } = decide_settle(now, None, budget)
        else {
            panic!("first sighting should start the clock");
        };
        assert_eq!(deadline, dt("2026-08-30 12:01:00"));
        assert_eq!(recheck_at, dt("2026-08-30 12:00:02"));

        // Inside the budget: keep waiting, with a widening gap.
        assert_eq!(
            decide_settle(dt("2026-08-30 12:00:30"), Some(deadline), budget),
            SettleDecision::Wait {
                recheck_at: dt("2026-08-30 12:00:35")
            }
        );

        // Past the deadline: only now is it a failure.
        assert_eq!(
            decide_settle(dt("2026-08-30 12:01:01"), Some(deadline), budget),
            SettleDecision::Expired
        );
    }

    #[test]
    fn defect_2_settle_backoff_widens_with_elapsed_time() {
        assert_eq!(settle_recheck_delay(Duration::ZERO), Duration::from_secs(2));
        assert_eq!(
            settle_recheck_delay(Duration::from_secs(30)),
            Duration::from_secs(5)
        );
        assert_eq!(
            settle_recheck_delay(Duration::from_secs(120)),
            Duration::from_secs(15)
        );
        assert_eq!(
            settle_recheck_delay(Duration::from_secs(600)),
            Duration::from_secs(30)
        );
    }

    // === Defect 3 / scope D — transient vs permanent =========================
    #[test]
    fn defect_3_transient_failures_are_retried() {
        for site in [FailureSite::Upload, FailureSite::History] {
            assert_eq!(
                classify_failure(site, "connection reset"),
                FailureKind::Transient,
                "{:?} should be retryable",
                site
            );
        }
    }

    #[test]
    fn defect_3_permanent_failures_are_not_retried() {
        for site in [
            FailureSite::SourceImage,
            FailureSite::WorkflowJson,
            FailureSite::Execution,
            FailureSite::Settle,
        ] {
            assert_eq!(
                classify_failure(site, "whatever"),
                FailureKind::Permanent,
                "{:?} should be terminal",
                site
            );
        }
    }

    #[test]
    fn defect_3_a_rejected_prompt_is_permanent_but_a_dropped_connection_is_not() {
        let rejected = format!(
            "Queue failed: {}: required input is missing",
            PROMPT_REJECTED
        );
        assert_eq!(
            classify_failure(FailureSite::Queue, &rejected),
            FailureKind::Permanent
        );
        assert_eq!(
            classify_failure(FailureSite::Queue, "Queue failed: connection refused"),
            FailureKind::Transient
        );
    }

    #[test]
    fn defect_3_retry_count_is_spent_then_the_real_error_stands() {
        let msg = "History fetch failed for prompt abc: connection reset";
        // Attempts 1..3 come back for another go, with a widening delay.
        let mut delays = Vec::new();
        for retry_count in 0..MAX_ATTEMPTS - 1 {
            match plan_failure(FailureSite::History, msg, retry_count) {
                FailureAction::Retry {
                    attempt,
                    delay,
                    message,
                } => {
                    assert_eq!(attempt, retry_count + 2);
                    assert_eq!(message, msg);
                    delays.push(delay);
                }
                other => panic!("attempt {} should retry, got {:?}", retry_count, other),
            }
        }
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(5),
                Duration::from_secs(15),
                Duration::from_secs(45)
            ]
        );

        // The last one keeps the real error rather than inventing a new one.
        match plan_failure(FailureSite::History, msg, MAX_ATTEMPTS - 1) {
            FailureAction::Fail(text) => {
                assert!(text.starts_with(msg), "lost the real error: {}", text);
                assert!(text.contains("after 4 attempts"), "{}", text);
            }
            other => panic!("budget was spent, expected a failure, got {:?}", other),
        }
    }

    #[test]
    fn a_timed_out_call_retries_rather_than_killing_the_task() {
        // A ComfyUI that stopped answering is the definition of a transient
        // problem, and every HTTP call the worker makes lands on one of these
        // sites. The Queue case is the one worth pinning: its permanence turns
        // on a substring match, so a timeout must not trip it.
        let timeout = "Queue prompt failed: timeout in recv_response after 30s";
        for site in [
            FailureSite::Upload,
            FailureSite::History,
            FailureSite::Queue,
        ] {
            assert_eq!(
                classify_failure(site, timeout),
                FailureKind::Transient,
                "a timeout at {:?} should be retried, not fatal",
                site
            );
        }
    }

    #[test]
    fn defect_3_a_retry_after_queueing_resumes_the_prompt() {
        // A prompt that already ran should be re-polled, not re-executed —
        // re-running it is expensive and can duplicate the output.
        assert!(retry_resumes_prompt(FailureSite::History));
        // Nothing reached ComfyUI yet, so these start over.
        assert!(!retry_resumes_prompt(FailureSite::Upload));
        assert!(!retry_resumes_prompt(FailureSite::Queue));
    }

    #[test]
    fn defect_3_a_permanent_failure_does_not_burn_attempts() {
        match plan_failure(FailureSite::Execution, "node blew up", 0) {
            FailureAction::Fail(text) => assert_eq!(text, "node blew up"),
            other => panic!("expected an immediate failure, got {:?}", other),
        }
    }
}
