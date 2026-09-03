//! The line a person has already been running, one workflow at a time.
//!
//! Almost nobody sits down to design a four-stage chain. What actually happens
//! is that somebody restores a photograph, likes it, upscales the result,
//! interpolates *that*, and then does the same three things to eleven more
//! shots. Phos has watched all of it: every generated file records the workflow
//! that made it, and every task records the file it read. So the sequence is
//! already written down, and the only thing missing is somebody noticing.
//!
//! This module notices. It is a pure function of the task rows — no database,
//! no clustering, no model — and it exists so the highest-value way to get a
//! line is also the one that costs the user nothing: *"You ran Restore →
//! Upscale → Interpolate on 12 shots. Save as a line?"*
//!
//! # How a chain is spotted
//!
//! One edge, one rule: **task B follows task A when B read the file A wrote.**
//! `enhancement_tasks.source_file_id` on one side, `output_file_id` on the
//! other. That is the same link the line runtime makes with `parent_task_id`,
//! drawn by hand instead of by the worker, which is exactly what makes it worth
//! promoting.
//!
//! # And how noise is kept out
//!
//! A suggestion a person did not ask for is an interruption, so the bar is
//! deliberately high:
//!
//! * only **completed** tasks are edges — a failed experiment is not a habit;
//! * a sequence has to have happened on [`MIN_SHOTS`] **distinct shots**, so
//!   one afternoon of fiddling with a single photograph suggests nothing;
//! * a sequence that is a prefix or tail of a longer one *with exactly the same
//!   shots* is dropped, because if every time you ran A → B you went on to C,
//!   the line is A → B → C;
//! * tasks that were already stages of a saved line never take part — the
//!   caller filters those out, and a line that suggested itself back would be
//!   a loop.
//!
//! # What the suggestion carries
//!
//! Not just the workflows. For each position it also works out which parameters
//! were **the same every single time** and which ones moved, which is precisely
//! the difference between a value worth pinning into the line and a value worth
//! asking for when the line is sent. A seed that changed on all twelve shots is
//! not a setting; it is a question.

use super::params::ParameterMap;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// How many distinct shots a sequence has to have been run on before it is
/// worth interrupting anybody about.
pub const MIN_SHOTS: usize = 3;

/// The longest sequence worth looking for. A chain longer than this is a
/// session, not a line.
pub const MAX_LENGTH: usize = 8;

/// One completed enhancement, as the detector reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub task_id: String,
    pub shot_id: String,
    pub workflow_id: String,
    /// The file this task read. `None` means it read the shot's original.
    pub source_file_id: Option<String>,
    /// The file this task wrote. `None` means nothing came back.
    pub output_file_id: Option<String>,
    /// Which part of an upstream video it consumed, if it said.
    pub source_mode: Option<String>,
    /// The typed values it ran with.
    pub parameters: ParameterMap,
    /// The prompts and directives it ran with.
    pub text_overrides: BTreeMap<String, String>,
}

/// What one position of a suggested line was actually run with.
#[derive(Debug, Clone, PartialEq)]
pub struct StageEvidence {
    pub workflow_id: String,
    /// Typed values that never changed across the occurrences — worth pinning.
    pub pinned: ParameterMap,
    /// Prompts that never changed — worth pinning too.
    pub pinned_text: BTreeMap<String, String>,
    /// Keys that *did* change. The line asks for these when it is sent rather
    /// than freezing whichever run happened to be last.
    pub exposed: Vec<String>,
    /// The source mode, when every occurrence used the same one.
    pub source_mode: Option<String>,
}

/// A sequence somebody has run more than once, offered as a line.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub stages: Vec<StageEvidence>,
    /// The distinct shots it was run on, in the order they were first seen.
    pub shots: Vec<String>,
}

impl Suggestion {
    pub fn workflow_ids(&self) -> Vec<&str> {
        self.stages.iter().map(|s| s.workflow_id.as_str()).collect()
    }
}

/// How hard to look. Separated out so a test can lower the bar without the
/// production default drifting to match it.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub min_shots: usize,
    pub max_length: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            min_shots: MIN_SHOTS,
            max_length: MAX_LENGTH,
        }
    }
}

/// One workflow sequence and every time it happened: the indices of the tasks
/// that made up each run of it, and the shots those were on.
struct Repeat {
    workflow_ids: Vec<String>,
    occurrences: Vec<Vec<usize>>,
    shots: Vec<String>,
}

/// The sequences worth offering, best first.
///
/// "Best" is the count of distinct shots, then length: twelve shots beats four,
/// and at the same popularity the longer chain is the more useful suggestion.
pub fn suggest(tasks: &[TaskRow], limits: Limits) -> Vec<Suggestion> {
    let chains = chains_of(tasks);

    // Every contiguous window of every chain, grouped by the workflows it names.
    let mut windows: BTreeMap<Vec<String>, Vec<Vec<usize>>> = BTreeMap::new();
    for chain in &chains {
        let max = limits.max_length.min(chain.len());
        for len in 2..=max {
            for start in 0..=(chain.len() - len) {
                let run = &chain[start..start + len];
                let key: Vec<String> = run.iter().map(|&i| tasks[i].workflow_id.clone()).collect();
                windows.entry(key).or_default().push(run.to_vec());
            }
        }
    }

    let mut kept: Vec<Repeat> = windows
        .into_iter()
        .filter_map(|(workflow_ids, occurrences)| {
            let mut shots: Vec<String> = Vec::new();
            for occurrence in &occurrences {
                let shot = &tasks[occurrence[0]].shot_id;
                if !shots.iter().any(|s| s == shot) {
                    shots.push(shot.clone());
                }
            }
            (shots.len() >= limits.min_shots).then_some(Repeat {
                workflow_ids,
                occurrences,
                shots,
            })
        })
        .collect();

    // If every time you ran A → B you went on to C, the line is A → B → C.
    // Judged on the shot *set*, not the count: a prefix that is genuinely more
    // popular than the chain it starts is its own suggestion.
    let shot_sets: Vec<BTreeSet<&str>> = kept
        .iter()
        .map(|r| r.shots.iter().map(String::as_str).collect())
        .collect();
    let subsumed: Vec<bool> = kept
        .iter()
        .enumerate()
        .map(|(i, r)| {
            kept.iter().enumerate().any(|(j, other)| {
                j != i
                    && other.workflow_ids.len() > r.workflow_ids.len()
                    && contains_window(&other.workflow_ids, &r.workflow_ids)
                    && shot_sets[j] == shot_sets[i]
            })
        })
        .collect();
    let mut index = 0;
    kept.retain(|_| {
        let keep = !subsumed[index];
        index += 1;
        keep
    });

    let mut out: Vec<Suggestion> = kept
        .into_iter()
        .filter_map(|r| {
            let mut stages = Vec::with_capacity(r.workflow_ids.len());
            for (position, workflow_id) in r.workflow_ids.iter().enumerate() {
                let (stage, mode_disagrees) = evidence(
                    workflow_id,
                    r.occurrences.iter().map(|o| &tasks[o[position]]),
                );
                // Occurrences that disagreed on how this stage reads its input
                // were runs of two habits, not one. Offering the chain with the
                // mode silently dropped would save a line whose joins mean
                // something none of the observed runs meant, so it is not
                // offered at all.
                if mode_disagrees {
                    return None;
                }
                stages.push(stage);
            }
            Some(Suggestion {
                stages,
                shots: r.shots,
            })
        })
        .collect();

    out.sort_by(|a, b| {
        b.shots
            .len()
            .cmp(&a.shots.len())
            .then(b.stages.len().cmp(&a.stages.len()))
            .then(a.workflow_ids().cmp(&b.workflow_ids()))
    });
    out
}

/// Every maximal hand-run chain: a walk backwards from each task nothing
/// followed, through the file each one read.
fn chains_of(tasks: &[TaskRow]) -> Vec<Vec<usize>> {
    let by_output: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.output_file_id.as_deref().map(|f| (f, i)))
        .collect();

    let parent: Vec<Option<usize>> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            t.source_file_id
                .as_deref()
                .and_then(|f| by_output.get(f).copied())
                .filter(|&p| p != i)
        })
        .collect();

    let mut has_child = vec![false; tasks.len()];
    for p in parent.iter().flatten() {
        has_child[*p] = true;
    }

    let mut chains = Vec::new();
    for (leaf, followed) in has_child.iter().enumerate() {
        if *followed {
            continue;
        }
        let mut chain = vec![leaf];
        let mut seen: BTreeSet<usize> = BTreeSet::from([leaf]);
        let mut at = leaf;
        // The length cap doubles as the cycle guard's belt: a hand-edited
        // database that made a loop settles instead of spinning.
        while let Some(up) = parent[at] {
            if !seen.insert(up) || chain.len() >= MAX_LENGTH {
                break;
            }
            chain.push(up);
            at = up;
        }
        if chain.len() >= 2 {
            chain.reverse();
            chains.push(chain);
        }
    }
    chains
}

/// Does `haystack` contain `needle` as a contiguous run?
fn contains_window(haystack: &[String], needle: &[String]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// What one position was run with, folded across every occurrence: what never
/// moved, and what did. The second answer says whether the occurrences
/// disagreed on the stage's `source_mode` — a disagreement the caller cannot
/// fold away, because there is no "asked at send time" for a join.
fn evidence<'a>(
    workflow_id: &str,
    occurrences: impl Iterator<Item = &'a TaskRow>,
) -> (StageEvidence, bool) {
    let mut first_params: Option<ParameterMap> = None;
    let mut first_text: Option<BTreeMap<String, String>> = None;
    let mut first_mode: Option<Option<String>> = None;
    let mut moved: BTreeSet<String> = BTreeSet::new();
    let mut mode_moved = false;

    for task in occurrences {
        match &first_params {
            None => first_params = Some(task.parameters.clone()),
            Some(base) => {
                for key in keys_of(base, &task.parameters) {
                    if base.get(&key) != task.parameters.get(&key) {
                        moved.insert(key);
                    }
                }
            }
        }
        match &first_text {
            None => first_text = Some(task.text_overrides.clone()),
            Some(base) => {
                for key in keys_of(base, &task.text_overrides) {
                    if base.get(&key) != task.text_overrides.get(&key) {
                        moved.insert(key);
                    }
                }
            }
        }
        match &first_mode {
            None => first_mode = Some(task.source_mode.clone()),
            Some(base) => {
                if base != &task.source_mode {
                    mode_moved = true;
                }
            }
        }
    }

    let mut pinned: ParameterMap = first_params.unwrap_or_default();
    let mut pinned_text = first_text.unwrap_or_default();
    for key in &moved {
        pinned.remove(key);
        pinned_text.remove(key);
    }

    (
        StageEvidence {
            workflow_id: workflow_id.to_string(),
            pinned,
            pinned_text,
            exposed: moved.into_iter().collect(),
            source_mode: first_mode.flatten(),
        },
        mode_moved,
    )
}

/// Every key either map holds — a value that appeared in one run and not in
/// another has moved just as surely as one that changed.
fn keys_of<V>(a: &BTreeMap<String, V>, b: &BTreeMap<String, V>) -> BTreeSet<String> {
    a.keys().chain(b.keys()).cloned().collect()
}

/// A convenience for the caller: a parameter map from a stored JSON string.
pub fn parameters_of(json: Option<&str>) -> ParameterMap {
    json.and_then(|s| serde_json::from_str::<ParameterMap>(s).ok())
        .unwrap_or_default()
}

/// The same, for the string-valued override map.
pub fn text_overrides_of(json: Option<&str>) -> BTreeMap<String, String> {
    json.and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(s).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One hand-run enhancement: it read `source`, it wrote `output`.
    fn task(
        id: &str,
        shot: &str,
        workflow: &str,
        source: Option<&str>,
        output: Option<&str>,
    ) -> TaskRow {
        TaskRow {
            task_id: id.to_string(),
            shot_id: shot.to_string(),
            workflow_id: workflow.to_string(),
            source_file_id: source.map(str::to_string),
            output_file_id: output.map(str::to_string),
            source_mode: None,
            parameters: ParameterMap::new(),
            text_overrides: BTreeMap::new(),
        }
    }

    /// Restore → Upscale → Interpolate, run by hand on `shot`.
    fn worked_by_hand(shot: &str) -> Vec<TaskRow> {
        vec![
            task(
                &format!("{}-a", shot),
                shot,
                "wf-restore",
                None,
                Some(&format!("{}-f1", shot)),
            ),
            task(
                &format!("{}-b", shot),
                shot,
                "wf-upscale",
                Some(&format!("{}-f1", shot)),
                Some(&format!("{}-f2", shot)),
            ),
            task(
                &format!("{}-c", shot),
                shot,
                "wf-interp",
                Some(&format!("{}-f2", shot)),
                Some(&format!("{}-f3", shot)),
            ),
        ]
    }

    fn library(shots: &[&str]) -> Vec<TaskRow> {
        shots.iter().flat_map(|s| worked_by_hand(s)).collect()
    }

    #[test]
    fn a_sequence_run_by_hand_on_enough_shots_is_offered_as_a_line() {
        let found = suggest(&library(&["s1", "s2", "s3"]), Limits::default());
        assert_eq!(found.len(), 1, "one suggestion, not three overlapping ones");
        assert_eq!(
            found[0].workflow_ids(),
            ["wf-restore", "wf-upscale", "wf-interp"]
        );
        assert_eq!(found[0].shots, ["s1", "s2", "s3"]);
    }

    #[test]
    fn twice_is_a_coincidence() {
        assert!(suggest(&library(&["s1", "s2"]), Limits::default()).is_empty());
        // And an afternoon on one photograph is not a habit either, however
        // many times the same three workflows were run over it.
        let mut one_shot = worked_by_hand("s1");
        one_shot.extend(worked_by_hand("s1"));
        assert!(suggest(&one_shot, Limits::default()).is_empty());
    }

    #[test]
    fn tasks_that_share_no_file_are_not_a_chain() {
        // Three shots, each run through Restore and then Upscale — but Upscale
        // read the *original* each time rather than what Restore made. Two
        // unrelated enhancements, not a line, however often it happened.
        let mut tasks = Vec::new();
        for shot in ["s1", "s2", "s3"] {
            tasks.push(task(
                &format!("{}-a", shot),
                shot,
                "wf-restore",
                None,
                Some(&format!("{}-f1", shot)),
            ));
            tasks.push(task(
                &format!("{}-b", shot),
                shot,
                "wf-upscale",
                None,
                Some(&format!("{}-f2", shot)),
            ));
        }
        assert!(suggest(&tasks, Limits::default()).is_empty());
    }

    #[test]
    fn a_run_that_produced_nothing_breaks_the_chain_where_it_stopped() {
        // The middle stage produced no file on one shot. Its own third task
        // then reads a file nobody wrote, so that shot's chain stops after two
        // — and the triple, now down to two shots, falls below the bar.
        let mut tasks = library(&["s1", "s2", "s3"]);
        for t in tasks.iter_mut() {
            if t.task_id == "s2-b" {
                t.output_file_id = None;
            }
        }
        let found = suggest(&tasks, Limits::default());
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].workflow_ids(),
            ["wf-restore", "wf-upscale"],
            "the part that worked everywhere is still the part worth offering"
        );
        assert_eq!(found[0].shots, ["s1", "s2", "s3"]);
        // And with one more shot broken the same way, nothing is left to say.
        for t in tasks.iter_mut() {
            if t.task_id == "s3-a" {
                t.output_file_id = None;
            }
        }
        assert!(suggest(&tasks, Limits::default()).is_empty());
    }

    #[test]
    fn the_longest_thing_everybody_did_is_what_gets_offered() {
        // Four shots went Restore → Upscale; three of them went on to
        // Interpolate. Both are real: the pair happened on a shot the triple
        // did not, so the pair is its own suggestion rather than being folded
        // into the longer one.
        let mut tasks = library(&["s1", "s2", "s3"]);
        tasks.push(task("s4-a", "s4", "wf-restore", None, Some("s4-f1")));
        tasks.push(task(
            "s4-b",
            "s4",
            "wf-upscale",
            Some("s4-f1"),
            Some("s4-f2"),
        ));

        let found = suggest(&tasks, Limits::default());
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].workflow_ids(),
            ["wf-restore", "wf-upscale"],
            "four shots outranks three"
        );
        assert_eq!(found[0].shots.len(), 4);
        assert_eq!(
            found[1].workflow_ids(),
            ["wf-restore", "wf-upscale", "wf-interp"]
        );

        // Whereas when the three shots are all there is, the pair is subsumed:
        // every time they ran Restore → Upscale they went on to Interpolate.
        let found = suggest(&library(&["s1", "s2", "s3"]), Limits::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].stages.len(), 3);
    }

    #[test]
    fn a_setting_that_never_moved_is_pinned_and_one_that_did_is_exposed() {
        let mut tasks = library(&["s1", "s2", "s3"]);
        for (i, t) in tasks.iter_mut().enumerate() {
            if t.workflow_id == "wf-upscale" {
                // The scale was 4 every time; the seed never repeated.
                t.parameters.insert("3.scale".to_string(), json!(4));
                t.parameters
                    .insert("3.seed".to_string(), json!(1000 + i as i64));
            }
        }
        let found = suggest(&tasks, Limits::default());
        let upscale = &found[0].stages[1];
        assert_eq!(upscale.workflow_id, "wf-upscale");
        assert_eq!(upscale.pinned.get("3.scale"), Some(&json!(4)));
        assert_eq!(
            upscale.pinned.get("3.seed"),
            None,
            "a value that moved is not a setting"
        );
        assert_eq!(upscale.exposed, ["3.seed"]);
    }

    #[test]
    fn a_prompt_typed_afresh_every_time_is_a_question_not_a_value() {
        let mut tasks = library(&["s1", "s2", "s3"]);
        for t in tasks.iter_mut() {
            if t.workflow_id == "wf-restore" {
                t.text_overrides
                    .insert("6.text".to_string(), format!("a portrait of {}", t.shot_id));
                t.text_overrides
                    .insert("7.text".to_string(), "blurry, jpeg artifacts".to_string());
            }
        }
        let restore = &suggest(&tasks, Limits::default())[0].stages[0];
        assert_eq!(restore.exposed, ["6.text"]);
        assert_eq!(
            restore.pinned_text.get("7.text").map(String::as_str),
            Some("blurry, jpeg artifacts"),
            "the negative prompt never changed, so it is part of the line"
        );
        assert!(!restore.pinned_text.contains_key("6.text"));
    }

    #[test]
    fn a_source_mode_everybody_used_travels_with_the_suggestion() {
        let mut tasks = library(&["s1", "s2", "s3"]);
        for t in tasks.iter_mut() {
            if t.workflow_id == "wf-interp" {
                t.source_mode = Some("whole_video".to_string());
            }
        }
        let found = suggest(&tasks, Limits::default());
        assert_eq!(
            found[0].stages[2].source_mode.as_deref(),
            Some("whole_video")
        );
        assert_eq!(found[0].stages[0].source_mode, None);

        // One that varied is nobody's default — the twelve runs were two
        // habits, not one, and saving the chain with the mode dropped would
        // change what its joins mean. No suggestion is honest here.
        for t in tasks.iter_mut() {
            if t.workflow_id == "wf-interp" && t.shot_id == "s2" {
                t.source_mode = Some("last_frame".to_string());
            }
        }
        assert!(
            suggest(&tasks, Limits::default()).is_empty(),
            "a chain whose occurrences fed a stage differently is not offered"
        );
    }

    #[test]
    fn a_chain_that_loops_back_on_itself_settles() {
        // Two tasks each reading the other's output: impossible through the
        // API, reachable by hand in sqlite3, and not worth hanging over.
        let tasks = vec![
            task("a", "s1", "wf-1", Some("f2"), Some("f1")),
            task("b", "s1", "wf-2", Some("f1"), Some("f2")),
        ];
        // No leaf, so nothing to walk from — and above all, it returns.
        assert!(suggest(&tasks, Limits::default()).is_empty());
    }

    #[test]
    fn nothing_at_all_is_not_a_suggestion() {
        assert!(suggest(&[], Limits::default()).is_empty());
        assert!(suggest(&worked_by_hand("s1")[..1], Limits::default()).is_empty());
    }

    #[test]
    fn a_branch_offers_both_arms_it_is_long_enough_for() {
        // Restore, then upscale *and* interpolate the same restored file — a
        // fork, run the same way on three shots. Two chains, one shared root.
        let mut tasks = Vec::new();
        for shot in ["s1", "s2", "s3"] {
            tasks.push(task(
                &format!("{}-a", shot),
                shot,
                "wf-restore",
                None,
                Some(&format!("{}-f1", shot)),
            ));
            tasks.push(task(
                &format!("{}-b", shot),
                shot,
                "wf-upscale",
                Some(&format!("{}-f1", shot)),
                Some(&format!("{}-f2", shot)),
            ));
            tasks.push(task(
                &format!("{}-c", shot),
                shot,
                "wf-interp",
                Some(&format!("{}-f1", shot)),
                Some(&format!("{}-f3", shot)),
            ));
        }
        let found = suggest(&tasks, Limits::default());
        let names: Vec<Vec<&str>> = found.iter().map(|s| s.workflow_ids()).collect();
        assert!(names.contains(&vec!["wf-restore", "wf-upscale"]));
        assert!(names.contains(&vec!["wf-restore", "wf-interp"]));
        // v1 lines are linear: a fork is offered as the two lines it is, and
        // nothing here invents a branch.
        assert!(found.iter().all(|s| s.stages.len() == 2));
    }
}
