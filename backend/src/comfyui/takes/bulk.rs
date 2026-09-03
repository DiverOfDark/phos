//! How far one verdict reaches.
//!
//! A lane that decides one run at a time is enough for somebody who sent four
//! seeds at one photograph. It is not enough for the case the farm exists for:
//! three thousand shots through `describe → hold → generate` is three thousand
//! held runs, and reading three thousand sentences one Enter at a time is not
//! curation, it is data entry. So a verdict may be given on the run in front of
//! you and then **applied to the rest of its batch** — you look at a handful,
//! they are all fine, and the rest go the same way.
//!
//! # Why this is a pure function
//!
//! Deciding which runs a verdict covers is the part that can be wrong in a way
//! that costs somebody a thousand clips, and it is the part that needs no
//! database at all. So it lives here, over plain rows, and the tests below are
//! the proof that a bulk verdict touches exactly the runs it claims to.
//!
//! # The rule
//!
//! A bulk verdict reaches another run when **all three** of these agree:
//!
//! * the same batch — and a run with no batch bulks over itself alone, stated
//!   explicitly rather than left to fall out of `NULL = NULL` never matching;
//! * the same line, because two lines holding under the same batch are two
//!   different questions;
//! * the same held stage, compared as an `Option` so a run parked with no stage
//!   recorded is never quietly swept in beside one that has a stage. Widening
//!   across that boundary is how a verdict about four candidate *clips* lands
//!   on four candidate *sentences*.
//!
//! # And what a bulk verdict may say
//!
//! Not everything. `keep` names task ids, and one run's task ids mean nothing
//! in another's, so the takes a person picked here cannot transfer there. What
//! transfers is the *shape* of the decision, which is why [`super::verdicts`]
//! resolves `continue` to "all of that run's waiting takes" and never carries a
//! rejection across a run nobody looked at. Deleting bytes is a thing you do to
//! pictures you have seen.

/// A held run, reduced to what deciding whether a bulk verdict reaches it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldRun {
    pub run_id: String,
    /// FR7's batch, when the run came from one. `None` for every run started a
    /// shot at a time — and, until FR7 lands, for every run there is.
    pub batch_id: Option<String>,
    pub line_id: Option<String>,
    pub held_at_stage: Option<i32>,
}

/// How wide a verdict reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The run it was given on, and nothing else. The default.
    Run,
    /// That run, and every other run of the same batch held at the same stage
    /// of the same line.
    Batch,
}

impl Scope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Scope::Run => "run",
            Scope::Batch => "batch",
        }
    }

    /// `None` for anything that is not a scope, so the API can say what it was
    /// given rather than guessing wide.
    pub(crate) fn parse(s: &str) -> Option<Scope> {
        match s {
            "run" => Some(Scope::Run),
            "batch" => Some(Scope::Batch),
            _ => None,
        }
    }
}

/// Which runs this verdict covers, the one it was given on always first.
///
/// First, because it is the run somebody actually looked at: if the fifth of a
/// thousand siblings fails, the decision that was made deliberately has already
/// been recorded.
pub(crate) fn covered(scope: Scope, decided: &HeldRun, held: &[HeldRun]) -> Vec<String> {
    let mut out = vec![decided.run_id.clone()];
    if scope == Scope::Run {
        return out;
    }
    // A run that belongs to no batch is a batch of one. Said here rather than
    // relied on from SQL's three-valued logic, because the reader of this
    // function is the person who has to believe it.
    let Some(batch) = decided.batch_id.as_deref() else {
        return out;
    };
    for run in held {
        if run.run_id == decided.run_id {
            continue;
        }
        let same_batch = run.batch_id.as_deref() == Some(batch);
        let same_line = run.line_id == decided.line_id;
        let same_stage = run.held_at_stage == decided.held_at_stage;
        if same_batch && same_line && same_stage {
            out.push(run.run_id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, batch: Option<&str>, line: Option<&str>, stage: Option<i32>) -> HeldRun {
        HeldRun {
            run_id: id.to_string(),
            batch_id: batch.map(str::to_string),
            line_id: line.map(str::to_string),
            held_at_stage: stage,
        }
    }

    /// Four runs of one batch, all parked at the same stage of the same line —
    /// the shape a batch-by-query run actually produces.
    fn a_batch() -> Vec<HeldRun> {
        (0..4)
            .map(|i| {
                run(
                    &format!("run-{i}"),
                    Some("batch-a"),
                    Some("line-1"),
                    Some(1),
                )
            })
            .collect()
    }

    #[test]
    fn a_run_scoped_verdict_touches_one_run_however_many_siblings_it_has() {
        let held = a_batch();
        assert_eq!(
            covered(Scope::Run, &held[2], &held),
            vec!["run-2".to_string()],
            "the whole point of the default scope"
        );
    }

    #[test]
    fn a_batch_scoped_verdict_covers_its_siblings_and_itself_first() {
        let held = a_batch();
        assert_eq!(
            covered(Scope::Batch, &held[2], &held),
            vec![
                "run-2".to_string(),
                "run-0".to_string(),
                "run-1".to_string(),
                "run-3".to_string()
            ],
            "the run somebody looked at is decided before the ones they did not"
        );
    }

    #[test]
    fn a_run_with_no_batch_bulks_over_itself_alone() {
        // Every run there is, until FR7 lands. A wide verdict here would be a
        // verdict over the whole library.
        let mut held = a_batch();
        held.push(run("run-lonely", None, Some("line-1"), Some(1)));
        let lonely = held.last().unwrap().clone();
        assert_eq!(
            covered(Scope::Batch, &lonely, &held),
            vec!["run-lonely".to_string()]
        );
    }

    #[test]
    fn a_verdict_never_reaches_another_batch() {
        let mut held = a_batch();
        held.push(run("run-other", Some("batch-b"), Some("line-1"), Some(1)));
        let covered = covered(Scope::Batch, &held[0], &held);
        assert!(
            !covered.contains(&"run-other".to_string()),
            "two batches are two decisions"
        );
        assert_eq!(covered.len(), 4);
    }

    #[test]
    fn a_verdict_never_reaches_another_line_of_the_same_batch() {
        // FR7's batch may fan across lines; a hold in each is a different
        // question, and one answer is not both.
        let mut held = a_batch();
        held.push(run(
            "run-elsewhere",
            Some("batch-a"),
            Some("line-2"),
            Some(1),
        ));
        assert!(!covered(Scope::Batch, &held[0], &held).contains(&"run-elsewhere".to_string()));
    }

    #[test]
    fn a_verdict_never_reaches_a_run_held_at_a_different_stage() {
        // The describe stage's takes are sentences and the generate stage's are
        // clips. "These are fine" said over one is not said over the other.
        let mut held = a_batch();
        held.push(run("run-later", Some("batch-a"), Some("line-1"), Some(3)));
        assert!(!covered(Scope::Batch, &held[0], &held).contains(&"run-later".to_string()));
    }

    #[test]
    fn a_stage_nobody_recorded_is_its_own_group_and_not_everybody_elses() {
        // `held_at_stage` is nullable and a row can be `held` with nothing in
        // it. Comparing as an `Option` keeps such a run out of a real stage's
        // verdict — and keeps it findable rather than dropped.
        let mut held = a_batch();
        held.push(run("run-nostage", Some("batch-a"), Some("line-1"), None));
        assert!(!covered(Scope::Batch, &held[0], &held).contains(&"run-nostage".to_string()));

        let nostage = held.last().unwrap().clone();
        assert_eq!(
            covered(Scope::Batch, &nostage, &held),
            vec!["run-nostage".to_string()],
            "and it does not drag the four real ones along with it either"
        );
    }

    #[test]
    fn a_run_missing_from_the_held_list_is_still_decided() {
        // The list is read a moment after the run is; a sibling that landed in
        // between is simply not there, and the verdict still applies to the run
        // it was given on.
        assert_eq!(
            covered(Scope::Batch, &a_batch()[0], &[]),
            vec!["run-0".to_string()]
        );
    }

    #[test]
    fn a_scope_is_one_of_two_words_and_nothing_else_is() {
        assert_eq!(Scope::parse("run"), Some(Scope::Run));
        assert_eq!(Scope::parse("batch"), Some(Scope::Batch));
        assert_eq!(Scope::parse("everything"), None);
        assert_eq!(Scope::parse("Batch"), None, "not a spelling question");
        assert_eq!(Scope::Batch.as_str(), "batch");
    }
}
