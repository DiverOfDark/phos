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
//! * **Which workflows may go here?** [`stage_options`], which asks
//!   [`Accepts::admits`] of both joins — the one above the slot and the one
//!   below it. Not a second compatibility rule: [`super::line::validate_chain`]
//!   and [`super::worker::dispatch`] ask the same function, so a stage the
//!   picker offers can never be a stage the validator refuses.
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

/// Where in a line a stage is being put.
///
/// Both ends are optional, and both optionals mean the same thing: there is
/// nothing on that side to fit. `after: None` is the first stage of a line —
/// what it eats is the shot, and only [`super::line::admits_source`] at run
/// creation can know whether that fits. `before: None` is the last stage, whose
/// output is the product and has nothing after it to satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Slot {
    /// What the stage above this slot produces.
    pub after: Option<MediaType>,
    /// What the stage below this slot accepts.
    pub before: Option<Accepts>,
}

/// May this workflow go in this slot?
///
/// `Ok(())` or the sentence to show against the greyed-out row. Both joins are
/// [`Accepts::admits`] and nothing else.
pub fn fits(candidate: &Candidate, slot: Slot) -> Result<(), String> {
    if let Some(upstream) = slot.after {
        if !candidate.accepts.admits(upstream) {
            return Err(if candidate.accepts.starts_a_line() {
                format!(
                    "{} consumes nothing, so it can only be the first stage of a line.",
                    candidate.name
                )
            } else {
                format!(
                    "{} takes {}, and the stage before it produces {}.",
                    candidate.name,
                    candidate.accepts.as_str(),
                    upstream.as_str()
                )
            });
        }
    }
    if let Some(downstream) = slot.before {
        if !downstream.admits(candidate.produces) {
            return Err(format!(
                "{} produces {}, and the stage after it takes {}.",
                candidate.name,
                candidate.produces.as_str(),
                downstream.as_str()
            ));
        }
    }
    Ok(())
}

/// Every workflow, marked with whether it may go in this slot.
///
/// The refused ones come back too rather than being filtered away, so the
/// editor can say *how many* it is not offering and why — "only video-accepting
/// stages offered" is a fact about the line, and a picker that silently omits
/// half a library reads as a library that has lost half its workflows.
pub fn stage_options(candidates: &[Candidate], slot: Slot) -> Vec<StageOption> {
    candidates
        .iter()
        .map(|c| StageOption {
            candidate: c.clone(),
            refused: fits(c, slot).err(),
        })
        .collect()
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
pub fn handoff(carries: MediaType, downstream: &StageContract, stored: Option<&str>) -> Handoff {
    let takes_video = downstream.roles.iter().any(|r| r.kind == LoaderKind::Video);
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

    #[test]
    fn after_a_stage_that_makes_a_clip_only_video_stages_are_offered() {
        let options = stage_options(
            &library(),
            Slot {
                after: Some(MediaType::Video),
                before: None,
            },
        );
        assert_eq!(offered(&options), ["Interpolate 60fps", "Upscale 4K"]);
        // And the ones that are not offered say why, in the validator's words.
        let restore = options
            .iter()
            .find(|o| o.candidate.name == "Restore Portrait");
        assert_eq!(
            restore.unwrap().refused.as_deref(),
            Some("Restore Portrait takes image, and the stage before it produces video.")
        );
    }

    #[test]
    fn a_stage_that_consumes_nothing_is_offered_only_at_the_top() {
        // First slot: nothing above it, so everything fits.
        let first = stage_options(&library(), Slot::default());
        assert!(offered(&first).contains(&"Text to Image"));
        // Anywhere else: it could never be handed the thing above it.
        let later = stage_options(
            &library(),
            Slot {
                after: Some(MediaType::Image),
                before: None,
            },
        );
        let t2i = later
            .iter()
            .find(|o| o.candidate.name == "Text to Image")
            .unwrap();
        assert_eq!(
            t2i.refused.as_deref(),
            Some("Text to Image consumes nothing, so it can only be the first stage of a line.")
        );
    }

    #[test]
    fn swapping_a_stage_in_the_middle_has_to_satisfy_both_sides() {
        // Replacing stage 2 of image → video → video: it must eat an image and
        // hand on something the 4K upscaler can read.
        let options = stage_options(
            &library(),
            Slot {
                after: Some(MediaType::Image),
                before: Some(Accepts::Video),
            },
        );
        assert_eq!(offered(&options), ["Photo to 5s Clip"]);
        // "Restore Portrait" eats the image happily and then hands on a still.
        let restore = options
            .iter()
            .find(|o| o.candidate.name == "Restore Portrait")
            .unwrap();
        assert_eq!(
            restore.refused.as_deref(),
            Some("Restore Portrait produces image, and the stage after it takes video.")
        );
    }

    #[test]
    fn the_picker_and_the_validator_cannot_disagree() {
        // Every stage the picker offers after a given upstream is a stage
        // `validate_chain` then accepts, and every one it refuses is one
        // `validate_chain` refuses. Asked of every pair in the library, because
        // "they call the same function" is only worth saying if it is checked.
        use crate::comfyui::line::{validate_chain, StageTyping};
        for up in &library() {
            let slot = Slot {
                after: Some(up.produces),
                before: None,
            };
            for option in stage_options(&library(), slot) {
                let chain = [
                    StageTyping {
                        stage_idx: 0,
                        name: up.name.clone(),
                        accepts: up.accepts,
                        produces: up.produces,
                    },
                    StageTyping {
                        stage_idx: 1,
                        name: option.candidate.name.clone(),
                        accepts: option.candidate.accepts,
                        produces: option.candidate.produces,
                    },
                ];
                assert_eq!(
                    validate_chain(&chain).is_ok(),
                    option.offered(),
                    "{} after {}: the picker and the validator disagree",
                    option.candidate.name,
                    up.name
                );
            }
        }
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
        let h = handoff(MediaType::Image, &down, None);
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
        let h = handoff(MediaType::Video, &down, None);
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
        let h = handoff(MediaType::Video, &down, None);
        assert_eq!(h.resolved, "whole_video", "the dispatcher's own default");
        assert_eq!(h.modes, [WHOLE_VIDEO, FIRST_FRAME, LAST_FRAME, AT_TIME]);
        assert!(h.is_a_question());
        // Choosing a frame moves the upload into the image loader, so the slot
        // it could fill changes with it.
        assert_eq!(h.roles, [SourceRole::Start], "the video loader's slot");
        let framed = handoff(MediaType::Video, &down, Some("last_frame"));
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
        let h = handoff(MediaType::Image, &down, None);
        assert!(h.modes.is_empty(), "a still is a still");
        assert_eq!(h.roles, [SourceRole::Start, SourceRole::End]);
        assert!(h.is_a_question(), "which slot is a real choice");
    }

    #[test]
    fn a_graph_with_no_loader_at_all_still_answers() {
        let down = contract(Accepts::None, MediaType::Image, Vec::new());
        let h = handoff(MediaType::Image, &down, None);
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
        for mode in handoff(MediaType::Video, &down, None).modes {
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
