//! What the line editor is allowed to offer, and what it has to ask.
//!
//! The editor is a vertical list, not a canvas — Phos does not draw graphs,
//! ComfyUI does. What makes a list enough is that it cannot be used to build
//! something broken: `Add stage` only offers workflows that fit where they are
//! going, so an invalid line is not something a person can draw in the first
//! place and then be told about.
//!
//! Two questions, both pure functions of the stage contracts:
//!
//! * **Which workflows may go here?** [`stage_options`], which builds the line
//!   that *would* result from each candidate and hands it to
//!   [`super::line::validate_chain`]. Not "asks the same rule" — asks the same
//!   *function*, on the whole chain, so the picker cannot drift from the
//!   validator even by one clause. It is also how the picker keeps up with a
//!   rule it does not know about: when validation learns that a text-producing
//!   stage is transparent to the media flow (a describe stage makes no file, so
//!   the stage after it still reads the shot), the picker learns it in the same
//!   commit, without a line changing here.
//! * **What travels along a join, and is there anything to decide about it?**
//!   [`handoff`]. Usually nothing: an image is an image, and a clip going into
//!   a graph with a video loader goes in whole. It becomes a question when the
//!   next stage can read the handoff in more than one way — a graph with both a
//!   video loader and an image loader can take the clip *or* a frame of it —
//!   or when it has more than one slot to put it in, which is FR2's
//!   `SourceRole` and is answered with FR2's own `role` directive rather than
//!   with a second mechanism.
//!
//! Nothing here touches the database or the network, for the same reason
//! [`super::line`] does not: the interesting behaviour is a set of rules, and
//! rules you can only exercise through a ComfyUI and a GPU are rules nobody
//! tests.

use super::contract::{Accepts, MediaType, StageContract};
use super::line::{validate_chain, StageTyping};
use super::loaders::{LoaderKind, SourceRole};
use super::source::SourceMode;

// ===== Which workflows may go here? =========================================

/// A workflow the picker could offer, reduced to what decides it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub workflow_id: String,
    pub name: String,
    pub accepts: Accepts,
    pub produces: MediaType,
}

/// One row of the picker: a workflow, and whether it may go in this slot.
#[derive(Debug, Clone, PartialEq)]
pub struct StageOption {
    pub candidate: Candidate,
    /// `None` when the stage is offered. The reason it is not, otherwise — in
    /// the same words the validator would use if it were saved anyway.
    pub refused: Option<String>,
}

impl StageOption {
    pub fn offered(&self) -> bool {
        self.refused.is_none()
    }
}

/// What the editor is doing to the position it named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Put the candidate at `at`, pushing whatever is there down.
    Insert,
    /// Put the candidate at `at` in place of what is there.
    Replace,
}

/// The line that would result from putting `candidate` at `at`.
fn hypothetical(
    existing: &[StageTyping],
    at: usize,
    placement: Placement,
    candidate: &Candidate,
) -> Vec<StageTyping> {
    let mut chain: Vec<StageTyping> = existing.to_vec();
    let at = at.min(chain.len());
    let stage = StageTyping {
        stage_idx: at as i32,
        name: candidate.name.clone(),
        accepts: candidate.accepts,
        produces: candidate.produces,
    };
    match placement {
        Placement::Insert => chain.insert(at, stage),
        Placement::Replace if at < chain.len() => chain[at] = stage,
        Placement::Replace => chain.push(stage),
    }
    // `validate_chain` reads `stage_idx` to name the stage it refuses, so the
    // whole list is renumbered rather than only the part that moved.
    for (idx, s) in chain.iter_mut().enumerate() {
        s.stage_idx = idx as i32;
    }
    chain
}

/// May this workflow go in this slot?
///
/// `Ok(())`, or the sentence to show against the greyed-out row — and the
/// sentence is [`validate_chain`]'s own, because the question asked is
/// literally "would this line be refused". There is no second rule here to
/// disagree with the first one, and nothing to keep in step when the first one
/// changes.
pub fn fits(
    existing: &[StageTyping],
    at: usize,
    placement: Placement,
    candidate: &Candidate,
) -> Result<(), String> {
    validate_chain(&hypothetical(existing, at, placement, candidate)).map_err(|e| e.message)
}

/// Every workflow, marked with whether it may go in this slot.
///
/// The refused ones come back too rather than being filtered away, so the
/// editor can say *how many* it is not offering and why — a picker that
/// silently omits half a library reads as a library that has lost half its
/// workflows.
pub fn stage_options(
    candidates: &[Candidate],
    existing: &[StageTyping],
    at: usize,
    placement: Placement,
) -> Vec<StageOption> {
    candidates
        .iter()
        .map(|c| StageOption {
            candidate: c.clone(),
            refused: fits(existing, at, placement, c).err(),
        })
        .collect()
}

/// What is flowing into the slot at `at` — what the connector above it carries.
///
/// A stage that produces no file is **transparent** to the media flow: a
/// describe stage reads the photograph and hands on a sentence, so the stage
/// after it is still reading the photograph. Skipping those is the difference
/// between a connector that says `image` and one that says `text` and then
/// cannot explain why the next stage eats a picture.
///
/// **Integration note.** FR9 (`feat/comfyui-prompt-compiler`, PR #117) owns
/// this rule — it is the same one that makes `validate_chain` accept
/// `describe → photo-to-clip` — and expresses it in [`super::line`]. This
/// function exists so that this branch stands on its own, and is the one place
/// to delete when the two are merged: every caller here goes through it, and
/// nothing else in this module reasons about the media flow at all.
pub fn carried_into(above: &[MediaType]) -> Option<MediaType> {
    above
        .iter()
        .rev()
        .find(|&&produces| produces != MediaType::Text)
        .copied()
}

// ===== What travels along a join? ===========================================

/// The choices the connector between two stages offers, in the order it lists
/// them. Strings rather than [`SourceMode`] values because `at_time` needs a
/// number the editor collects; they are the same spellings
/// [`SourceMode::from_str`] reads.
pub const WHOLE_VIDEO: &str = "whole_video";
pub const FIRST_FRAME: &str = "first_frame";
pub const LAST_FRAME: &str = "last_frame";
pub const AT_TIME: &str = "at_time";

/// What one join of a line carries, and what there is to decide about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    /// What the stage above hands down.
    pub carries: MediaType,
    /// What the stage below will actually read, given what it stored — the
    /// answer [`SourceMode::resolve`] gives, computed here so the connector
    /// states the truth rather than a plausible default.
    pub resolved: String,
    /// The modes worth offering. **Empty means there is nothing to choose**:
    /// the connector states the handoff and is not clickable.
    pub modes: Vec<&'static str>,
    /// The slots the incoming file could fill, lowest-numbered loader first.
    /// More than one is the only other question a connector asks.
    pub roles: Vec<SourceRole>,
}

impl Handoff {
    /// Is there anything here for a person to answer?
    pub fn is_a_question(&self) -> bool {
        !self.modes.is_empty() || self.roles.len() > 1
    }
}

/// What the join into `downstream` carries, and what it may be asked.
///
/// The rule mirrors the dispatcher exactly, because disagreeing with it is the
/// only way this can be wrong:
///
/// * a still is a still — there is no frame two of a JPEG, and the connector
///   says `image` and offers nothing;
/// * a clip going into a graph with a video loader goes in whole, and there is
///   nothing to choose either;
/// * a clip going into a graph that has **both** a video loader and an image
///   loader is the real question: the clip itself, or a frame of it. That is
///   what FR2's source modes are for, and what `bind_targets` then puts in the
///   loader of the matching kind.
///
/// `takes_video` comes from the caller rather than off `downstream.roles`,
/// because that is where the dispatcher gets it: straight from the graph, with
/// [`super::loaders::takes_video`]. A contract stored before loaders were
/// recorded carries none, and a connector reading the contract alone would then
/// promise a frame where the run is going to send the whole clip.
pub fn handoff(
    carries: MediaType,
    downstream: &StageContract,
    takes_video: bool,
    stored: Option<&str>,
) -> Handoff {
    let takes_image = downstream.roles.iter().any(|r| r.kind == LoaderKind::Image);
    let carries_video = carries == MediaType::Video;

    // The same call the dispatcher makes, so what the editor shows is what the
    // run will do.
    let resolved = SourceMode::resolve(stored, takes_video, carries_video);

    let modes: Vec<&'static str> = match (carries_video, takes_video, takes_image) {
        // A clip, and two ways in. The only genuinely ambiguous join there is.
        (true, true, true) => vec![WHOLE_VIDEO, FIRST_FRAME, LAST_FRAME, AT_TIME],
        // A clip, and only frames have a home — which happens when somebody
        // corrected a contract to say "this takes video" over a graph that
        // loads stills. Their correction, so their choice of frame.
        (true, false, true) => vec![FIRST_FRAME, LAST_FRAME, AT_TIME],
        // Everything else resolves itself.
        _ => Vec::new(),
    };

    // Which loaders the upload can land in, and so which slots it could fill.
    // The same pool `bind_targets` builds: loaders of the kind being uploaded,
    // or all of them when the graph has none of that kind.
    let kind = if resolved == SourceMode::WholeVideo {
        LoaderKind::Video
    } else {
        LoaderKind::Image
    };
    let mut pool: Vec<&super::contract::RoleSlot> =
        downstream.roles.iter().filter(|r| r.kind == kind).collect();
    if pool.is_empty() {
        pool = downstream.roles.iter().collect();
    }
    let mut roles = Vec::new();
    for slot in pool {
        if !roles.contains(&slot.role) {
            roles.push(slot.role);
        }
    }

    Handoff {
        carries,
        resolved: resolved.to_string(),
        modes,
        roles,
    }
}

// ===== What a stage pins, and what it leaves open ===========================

/// Is this key one the stage deliberately did not pin?
///
/// The other half of the editor's promise. A row marked *exposed* is a value
/// the line refuses to decide, so it has to be askable at send time; a row that
/// is *pinned* or *varied* is a decision the line made, and a caller supplying
/// a value for it is not steering the line but rewriting it.
pub fn is_exposed(exposed: &[String], key: &str) -> bool {
    exposed.iter().any(|e| e == key)
}

/// The supplied keys this stage never offered, so the refusal can name them.
///
/// Refused rather than ignored: a caller who sent a seed to a stage that pins
/// its seed has misunderstood the line, and finding that out four hours into a
/// run is finding out too late.
pub fn unasked_keys<'a>(
    exposed: &[String],
    supplied: impl IntoIterator<Item = &'a String>,
) -> Vec<String> {
    let mut out: Vec<String> = supplied
        .into_iter()
        .filter(|k| !is_exposed(exposed, k))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

// The slot a connector picks is written into the stage's `text_overrides`
// under FR2's own bare `role` key — see [`super::loaders::role_directives`],
// which already reads it. A connector that says "this goes into the end-frame
// slot" therefore writes one string into a map that exists: no column, no
// second vocabulary, and nothing in the worker to change.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comfyui::contract::{ContractCorrections, RoleSlot};

    fn candidate(name: &str, accepts: Accepts, produces: MediaType) -> Candidate {
        Candidate {
            workflow_id: format!("wf-{}", name.to_ascii_lowercase().replace(' ', "-")),
            name: name.to_string(),
            accepts,
            produces,
        }
    }

    /// The library the editor is picking out of, in the worked example.
    fn library() -> Vec<Candidate> {
        vec![
            candidate("Photo to 5s Clip", Accepts::Image, MediaType::Video),
            candidate("Interpolate 60fps", Accepts::Video, MediaType::Video),
            candidate("Upscale 4K", Accepts::Video, MediaType::Video),
            candidate("Restore Portrait", Accepts::Image, MediaType::Image),
            candidate("Text to Image", Accepts::None, MediaType::Image),
            candidate("Describe", Accepts::Image, MediaType::Text),
        ]
    }

    fn offered(options: &[StageOption]) -> Vec<&str> {
        options
            .iter()
            .filter(|o| o.offered())
            .map(|o| o.candidate.name.as_str())
            .collect()
    }

    fn typing(idx: i32, c: &Candidate) -> StageTyping {
        StageTyping {
            stage_idx: idx,
            name: c.name.clone(),
            accepts: c.accepts,
            produces: c.produces,
        }
    }

    /// A line already drawn, as the editor holds it while it is being edited.
    fn drawn(names: &[&str]) -> Vec<StageTyping> {
        let shop = library();
        names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                typing(
                    i as i32,
                    shop.iter()
                        .find(|c| &c.name == name)
                        .expect("in the library"),
                )
            })
            .collect()
    }

    #[test]
    fn after_a_stage_that_makes_a_clip_only_video_stages_are_offered() {
        let line = drawn(&["Photo to 5s Clip"]);
        let options = stage_options(&library(), &line, 1, Placement::Insert);
        assert_eq!(offered(&options), ["Interpolate 60fps", "Upscale 4K"]);
        // And the ones that are not offered say why — in the validator's own
        // words, because it is the validator that was asked.
        let restore = options
            .iter()
            .find(|o| o.candidate.name == "Restore Portrait")
            .unwrap();
        assert_eq!(
            restore.refused.as_deref(),
            Some(
                "Stage 2 (Restore Portrait) takes image, but stage 1 (Photo to 5s Clip) produces video."
            )
        );
    }

    #[test]
    fn a_stage_that_consumes_nothing_is_offered_only_at_the_top() {
        // The first slot of an empty line: everything fits, because what a
        // first stage eats is the shot, and only a run can know what that is.
        let first = stage_options(&library(), &[], 0, Placement::Insert);
        assert!(offered(&first).contains(&"Text to Image"));
        // Anywhere else: it could never be handed the thing above it.
        let later = stage_options(
            &library(),
            &drawn(&["Restore Portrait"]),
            1,
            Placement::Insert,
        );
        let t2i = later
            .iter()
            .find(|o| o.candidate.name == "Text to Image")
            .unwrap();
        assert!(
            t2i.refused
                .as_deref()
                .unwrap()
                .contains("only be the first stage"),
            "{:?}",
            t2i.refused
        );
    }

    #[test]
    fn swapping_a_stage_in_the_middle_has_to_satisfy_both_sides() {
        // Replacing the middle of image → image → video → video: what goes in
        // must eat the restored still *and* hand on something the upscaler can
        // read. Exactly one workflow in this library does both.
        let line = drawn(&["Restore Portrait", "Photo to 5s Clip", "Upscale 4K"]);
        let options = stage_options(&library(), &line, 1, Placement::Replace);
        assert_eq!(offered(&options), ["Photo to 5s Clip"]);
        // "Restore Portrait" would eat the still happily and then hand on
        // another one, which the upscaler cannot read.
        let restore = options
            .iter()
            .find(|o| o.candidate.name == "Restore Portrait")
            .unwrap();
        assert_eq!(
            restore.refused.as_deref(),
            Some(
                "Stage 3 (Upscale 4K) takes video, but stage 2 (Restore Portrait) produces image."
            )
        );
    }

    #[test]
    fn the_first_stage_of_a_line_is_only_checked_against_what_follows_it() {
        // Replacing stage 1 puts the candidate at the top, where what it eats
        // is the shot — and only a run knows what that is. So a video-eating
        // stage is offered here: a line that begins by upscaling a clip is a
        // perfectly good line, and refusing it in the picker would be the
        // editor inventing a rule the validator does not have.
        let line = drawn(&["Photo to 5s Clip", "Upscale 4K"]);
        assert_eq!(
            offered(&stage_options(&library(), &line, 0, Placement::Replace)),
            [
                "Photo to 5s Clip",
                "Interpolate 60fps",
                "Upscale 4K",
                "Describe"
            ],
            "everything that hands the upscaler a clip — and the describe stage, \
             which hands it nothing and so leaves it reading the shot"
        );
    }

    #[test]
    fn inserting_and_replacing_at_one_index_are_different_questions() {
        // image → image → video. At index 1, *inserting* has to feed the still
        // stage below it; *replacing* removes that stage, so what goes in has
        // to feed the clip maker instead. Here the answers coincide…
        let line = drawn(&["Restore Portrait", "Restore Portrait", "Photo to 5s Clip"]);
        assert_eq!(
            offered(&stage_options(&library(), &line, 1, Placement::Insert)),
            ["Restore Portrait", "Describe"]
        );
        // …and on a shorter line they come apart, which is the whole reason
        // the two are different questions. image → image, at index 1:
        let two = drawn(&["Restore Portrait", "Restore Portrait"]);
        assert_eq!(
            offered(&stage_options(&library(), &two, 1, Placement::Insert)),
            ["Restore Portrait", "Describe"],
            "inserted above a still stage, it has to hand on a still — or nothing"
        );
        assert_eq!(
            offered(&stage_options(&library(), &two, 1, Placement::Replace)),
            ["Photo to 5s Clip", "Restore Portrait", "Describe"],
            "replacing the last stage, there is nothing below to satisfy"
        );
    }

    /// The one case FR9 changed, pinned here so the change stayed visible.
    ///
    /// A describe stage reads a photograph and produces a sentence. Before FR9
    /// `validate_chain` refused anything after it that wanted a file, and the
    /// picker refused it too — which was the point: they agreed. FR9 made a
    /// text-producing stage transparent to the media flow, and the picker
    /// learned it here without a line of it moving: only the expectation below
    /// changed.
    #[test]
    fn what_may_follow_a_describe_stage_is_whatever_validation_says_may() {
        let line = drawn(&["Describe"]);
        let options = stage_options(&library(), &line, 1, Placement::Insert);
        for option in &options {
            let chain = hypothetical(&line, 1, Placement::Insert, &option.candidate);
            assert_eq!(
                validate_chain(&chain).is_ok(),
                option.offered(),
                "{} after a describe stage",
                option.candidate.name
            );
        }
        // Post-FR9 that is everything but the stage that reads nothing: the
        // describe stage made no file, so the stage after it is still reading
        // the run's own source — and what that is, only a run knows.
        assert_eq!(
            offered(&options),
            [
                "Photo to 5s Clip",
                "Interpolate 60fps",
                "Upscale 4K",
                "Restore Portrait",
                "Describe"
            ]
        );
    }

    #[test]
    fn the_picker_and_the_validator_cannot_disagree() {
        // Every stage the picker offers is one `validate_chain` then accepts,
        // and every one it refuses is one `validate_chain` refuses — asked of
        // every candidate at every position of every one-stage line, because
        // "they call the same function" is only worth saying if it is checked.
        for above in library() {
            let line = vec![typing(0, &above)];
            for placement in [Placement::Insert, Placement::Replace] {
                for at in 0..=line.len() {
                    for option in stage_options(&library(), &line, at, placement) {
                        let chain = hypothetical(&line, at, placement, &option.candidate);
                        assert_eq!(
                            validate_chain(&chain).is_ok(),
                            option.offered(),
                            "{:?} {} at {} under {}: the picker and the validator disagree",
                            placement,
                            option.candidate.name,
                            at,
                            above.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_stage_that_makes_no_file_does_not_interrupt_the_media_flow() {
        // What the connector above a slot carries. A describe stage reads the
        // photograph and hands on a sentence, so the stage after it is still
        // reading the photograph — and the join says `image`, not `text`.
        use MediaType::{Image, Text, Video};
        assert_eq!(carried_into(&[]), None, "nothing above: the shot decides");
        assert_eq!(carried_into(&[Image]), Some(Image));
        assert_eq!(carried_into(&[Image, Text]), Some(Image));
        assert_eq!(carried_into(&[Image, Text, Text]), Some(Image));
        assert_eq!(carried_into(&[Image, Video, Text]), Some(Video));
        // A line that has so far only described things is still reading the
        // shot, so there is no join above the next stage to draw at all.
        assert_eq!(carried_into(&[Text]), None);
    }

    // ----- Handoffs ---------------------------------------------------------

    fn slot(node_id: &str, kind: LoaderKind, role: SourceRole) -> RoleSlot {
        RoleSlot {
            role,
            node_id: node_id.to_string(),
            node_type: match kind {
                LoaderKind::Video => "VHS_LoadVideo".to_string(),
                LoaderKind::Image => "LoadImage".to_string(),
            },
            kind,
            title: None,
        }
    }

    fn contract(accepts: Accepts, produces: MediaType, roles: Vec<RoleSlot>) -> StageContract {
        StageContract {
            version: 1,
            accepts,
            produces,
            roles,
            slots: Vec::new(),
            params: Vec::new(),
            warnings: Vec::new(),
            corrections: ContractCorrections::default(),
        }
    }

    #[test]
    fn a_still_handed_to_a_still_stage_has_nothing_to_decide() {
        let down = contract(
            Accepts::Image,
            MediaType::Image,
            vec![slot("4", LoaderKind::Image, SourceRole::Start)],
        );
        let h = handoff(MediaType::Image, &down, false, None);
        assert_eq!(h.resolved, "first_frame");
        assert!(h.modes.is_empty(), "there is no frame two of a JPEG");
        assert!(!h.is_a_question());
    }

    #[test]
    fn a_clip_handed_to_a_video_loader_goes_in_whole_without_being_asked() {
        let down = contract(
            Accepts::Video,
            MediaType::Video,
            vec![slot("7", LoaderKind::Video, SourceRole::Start)],
        );
        let h = handoff(MediaType::Video, &down, true, None);
        assert_eq!(h.resolved, "whole_video");
        assert!(h.modes.is_empty());
        assert!(!h.is_a_question());
    }

    #[test]
    fn a_clip_handed_to_a_graph_that_can_read_both_is_the_real_question() {
        // A graph that takes a clip *and* a still: extend-the-clip workflows
        // look like this, and "the whole thing or its last frame" is a choice
        // nothing but a person can make.
        let down = contract(
            Accepts::Video,
            MediaType::Video,
            vec![
                slot("7", LoaderKind::Video, SourceRole::Start),
                slot("9", LoaderKind::Image, SourceRole::Reference),
            ],
        );
        let h = handoff(MediaType::Video, &down, true, None);
        assert_eq!(h.resolved, "whole_video", "the dispatcher's own default");
        assert_eq!(h.modes, [WHOLE_VIDEO, FIRST_FRAME, LAST_FRAME, AT_TIME]);
        assert!(h.is_a_question());
        // Choosing a frame moves the upload into the image loader, so the slot
        // it could fill changes with it.
        assert_eq!(h.roles, [SourceRole::Start], "the video loader's slot");
        let framed = handoff(MediaType::Video, &down, true, Some("last_frame"));
        assert_eq!(framed.resolved, "last_frame");
        assert_eq!(framed.roles, [SourceRole::Reference]);
    }

    #[test]
    fn a_join_into_two_slots_asks_which_one() {
        // The interpolator with a start frame and an end frame: FR2's whole
        // reason for existing, and the second thing a connector can ask.
        let down = contract(
            Accepts::Image,
            MediaType::Video,
            vec![
                slot("4", LoaderKind::Image, SourceRole::Start),
                slot("5", LoaderKind::Image, SourceRole::End),
            ],
        );
        let h = handoff(MediaType::Image, &down, false, None);
        assert!(h.modes.is_empty(), "a still is a still");
        assert_eq!(h.roles, [SourceRole::Start, SourceRole::End]);
        assert!(h.is_a_question(), "which slot is a real choice");
    }

    #[test]
    fn the_connector_asks_the_graph_what_it_can_load_not_the_contract() {
        // A contract stored before loaders were recorded: it says it takes
        // video and lists nothing. The graph underneath does have a video
        // loader, and the dispatcher will read it there — so the connector has
        // to say `whole video`, not `first frame`, or it promises a frame the
        // run is never going to send.
        let bare = contract(Accepts::Video, MediaType::Video, Vec::new());
        assert_eq!(
            handoff(MediaType::Video, &bare, true, None).resolved,
            "whole_video"
        );
        // And with no video loader anywhere, the same contract resolves to the
        // frame the dispatcher would actually extract.
        assert_eq!(
            handoff(MediaType::Video, &bare, false, None).resolved,
            "first_frame"
        );
    }

    #[test]
    fn a_graph_with_no_loader_at_all_still_answers() {
        let down = contract(Accepts::None, MediaType::Image, Vec::new());
        let h = handoff(MediaType::Image, &down, false, None);
        assert!(h.roles.is_empty());
        assert!(!h.is_a_question());
    }

    // ----- Dispositions -----------------------------------------------------

    #[test]
    fn a_stage_only_answers_for_what_it_left_open() {
        let exposed = vec!["6.text".to_string(), "3.seed".to_string()];
        assert!(is_exposed(&exposed, "6.text"));
        assert!(!is_exposed(&exposed, "3.cfg"));
        // A caller who sent a pinned key is told which one, by name.
        let sent = vec![
            "6.text".to_string(),
            "3.cfg".to_string(),
            "4.steps".to_string(),
        ];
        assert_eq!(unasked_keys(&exposed, &sent), ["3.cfg", "4.steps"]);
        assert!(unasked_keys(&exposed, &["3.seed".to_string()]).is_empty());
        // A stage that exposes nothing accepts nothing.
        assert_eq!(unasked_keys(&[], &sent), ["3.cfg", "4.steps", "6.text"]);
    }

    #[test]
    fn the_connector_never_offers_a_mode_the_dispatcher_cannot_read() {
        let down = contract(
            Accepts::Video,
            MediaType::Video,
            vec![
                slot("7", LoaderKind::Video, SourceRole::Start),
                slot("9", LoaderKind::Image, SourceRole::Start),
            ],
        );
        for mode in handoff(MediaType::Video, &down, true, None).modes {
            // `at_time` is the one the editor completes with a number.
            let spelling = if mode == AT_TIME {
                "at_time:1500".to_string()
            } else {
                mode.to_string()
            };
            assert!(
                spelling.parse::<SourceMode>().is_ok(),
                "{} is not a source mode",
                spelling
            );
        }
    }
}
