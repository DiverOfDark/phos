//! What the compiler must get right, with no ComfyUI, no GPU and no database.

use super::compile::{compile_prompt, StringOrList};
use super::*;
use crate::comfyui::contract::{Accepts, MediaType, PromptSlot, StageContract};

fn facts() -> ShotFacts {
    ShotFacts {
        people: vec!["Anna".to_string(), "Bjorn".to_string()],
        taken_at: Some("2019-07-14 19:12:03".to_string()),
        place: Some((59.3293, 18.0686)),
        caption: Some("a woman sitting on a wooden jetty".to_string()),
    }
}

fn intent() -> Intent {
    Intent {
        intent: Some("a slow push-in as the light fades".to_string()),
        style: Some("35mm film, muted palette".to_string()),
        do_not: vec!["change face".to_string(), "add people".to_string()],
    }
}

fn contract_with(slots: Vec<(&str, &str, &str)>) -> StageContract {
    StageContract {
        version: 1,
        accepts: Accepts::Image,
        produces: MediaType::Video,
        roles: Vec::new(),
        slots: slots
            .into_iter()
            .map(|(name, node_id, field)| PromptSlot {
                name: name.to_string(),
                node_id: node_id.to_string(),
                field: field.to_string(),
                node_title: None,
                multiline: true,
                default: None,
            })
            .collect(),
        params: Vec::new(),
        warnings: Vec::new(),
        corrections: Default::default(),
    }
}

// === Phos supplies what Qwen cannot see =====================================

#[test]
fn the_instruction_carries_the_names_clustering_found() {
    let text = describe_instruction(&facts(), &Intent::default());
    assert!(text.contains("Anna"), "{}", text);
    assert!(text.contains("Bjorn"), "{}", text);
    // And says not to invent others, which is the reason for naming them.
    assert!(text.contains("Do not invent names"), "{}", text);
}

#[test]
fn the_instruction_carries_the_exif_date_and_place() {
    let text = describe_instruction(&facts(), &Intent::default());
    assert!(text.contains("2019-07-14 19:12:03"), "{}", text);
    assert!(text.contains("59.3293"), "{}", text);
    assert!(text.contains("18.0686"), "{}", text);
}

#[test]
fn the_instruction_carries_the_library_caption_without_being_it() {
    let text = describe_instruction(&facts(), &Intent::default());
    assert!(
        text.contains("a woman sitting on a wooden jetty"),
        "{}",
        text
    );
    // Florence-2's caption is an input, not the answer: the model is still
    // asked for the structured form.
    assert!(text.contains("\"motion_affordance\""), "{}", text);
}

#[test]
fn the_instruction_carries_the_intent_the_style_and_the_constraints() {
    let text = describe_instruction(&facts(), &intent());
    assert!(
        text.contains("a slow push-in as the light fades"),
        "{}",
        text
    );
    assert!(text.contains("35mm film, muted palette"), "{}", text);
    assert!(text.contains("Must not: change face"), "{}", text);
    assert!(text.contains("Must not: add people"), "{}", text);
}

#[test]
fn a_shot_phos_knows_nothing_about_still_gets_an_instruction() {
    let text = describe_instruction(&ShotFacts::default(), &Intent::default());
    assert!(!text.contains("What the library already knows"), "{}", text);
    assert!(text.contains("Answer with one JSON object"), "{}", text);
}

#[test]
fn directives_ride_in_the_override_map_the_stage_already_has() {
    let mut overrides = std::collections::HashMap::new();
    intent().to_overrides(&mut overrides);
    assert_eq!(Intent::from_overrides(&overrides), intent());
    // One rule per line, so a textarea and a semicolon-separated field agree.
    assert_eq!(
        overrides.get(DO_NOT_KEY).unwrap(),
        "change face\nadd people"
    );
    assert!(overrides.keys().all(|k| k.starts_with(DIRECTIVE_PREFIX)));
}

#[test]
fn constraints_split_on_semicolons_as_well_as_newlines() {
    let mut overrides = std::collections::HashMap::new();
    overrides.insert(
        DO_NOT_KEY.to_string(),
        "change face; add people".to_string(),
    );
    assert_eq!(
        Intent::from_overrides(&overrides).do_not,
        vec!["change face".to_string(), "add people".to_string()]
    );
}

// === Reading what comes back ================================================

const GOOD_ANSWER: &str = r#"{
  "subject": "Anna, seated on a weathered jetty, looking out over the water",
  "setting": "a still lake at dusk, pines on the far shore",
  "lighting": "low warm sun from camera left, long shadows",
  "camera": "35mm, waist-up, slightly below eye level",
  "motion_affordance": "hair and water could move; the subject is seated",
  "do_not": ["change face", "add people"]
}"#;

#[test]
fn a_clean_json_answer_parses() {
    let a = parse_analysis(GOOD_ANSWER).expect("should parse");
    assert!(a.subject.starts_with("Anna, seated"));
    assert_eq!(
        a.motion_affordance,
        "hair and water could move; the subject is seated"
    );
    assert_eq!(a.do_not.items(), vec!["change face", "add people"]);
}

#[test]
fn an_answer_wrapped_in_prose_and_fences_still_parses() {
    // Which is what a model actually does, however firmly it was told not to.
    let wrapped = format!(
        "Sure! Here is the description:\n\n```json\n{}\n```\nLet me know if you \
         want me to adjust it.",
        GOOD_ANSWER
    );
    let a = parse_analysis(&wrapped).expect("should parse");
    assert_eq!(a.setting, "a still lake at dusk, pines on the far shore");
}

#[test]
fn a_brace_inside_a_string_does_not_end_the_object() {
    let answer = r#"{"subject": "a sign reading {OPEN}", "setting": "a shopfront"}"#;
    let a = parse_analysis(answer).expect("should parse");
    assert_eq!(a.subject, "a sign reading {OPEN}");
    assert_eq!(a.setting, "a shopfront");
}

#[test]
fn do_not_survives_a_model_that_answers_with_one_string() {
    let a = parse_analysis(r#"{"subject": "a cat", "do_not": "change face; add people"}"#)
        .expect("should parse");
    assert_eq!(a.do_not.items(), vec!["change face", "add people"]);
}

#[test]
fn prose_with_no_object_in_it_is_not_an_analysis() {
    assert_eq!(parse_analysis("A woman sits on a jetty at dusk."), None);
    assert_eq!(parse_analysis("{}"), None);
}

// === Compiling ==============================================================

#[test]
fn the_prompt_reads_general_to_specific_and_ends_with_what_was_asked_for() {
    let a = parse_analysis(GOOD_ANSWER).unwrap();
    let compiled = compile_prompt(&a, &intent());
    assert_eq!(
        compiled.positive,
        "Anna, seated on a weathered jetty, looking out over the water. \
         a still lake at dusk, pines on the far shore. \
         low warm sun from camera left, long shadows. \
         35mm, waist-up, slightly below eye level. \
         hair and water could move; the subject is seated. \
         35mm film, muted palette. \
         a slow push-in as the light fades."
    );
}

#[test]
fn constraints_never_reach_the_positive_prompt() {
    // "do not add people" in a positive prompt adds people.
    let compiled = compile_prompt(&parse_analysis(GOOD_ANSWER).unwrap(), &intent());
    assert!(!compiled.positive.contains("do not"));
    assert!(!compiled.positive.contains("add people"));
    assert_eq!(compiled.negative, "change face, add people");
}

#[test]
fn the_model_and_the_person_do_not_repeat_each_other() {
    let a = Analysis {
        subject: "a cat".to_string(),
        do_not: StringOrList::Many(vec!["Change Face".to_string(), "warp hands".to_string()]),
        ..Analysis::default()
    };
    let compiled = compile_prompt(
        &a,
        &Intent {
            do_not: vec!["change face".to_string()],
            ..Intent::default()
        },
    );
    assert_eq!(compiled.negative, "Change Face, warp hands");
}

#[test]
fn empty_fields_leave_no_gaps() {
    let a = Analysis {
        subject: "a cat".to_string(),
        camera: "close".to_string(),
        ..Analysis::default()
    };
    assert_eq!(
        compile_prompt(&a, &Intent::default()).positive,
        "a cat. close."
    );
}

#[test]
fn a_model_that_answered_in_prose_is_still_used() {
    let compiled = compile_from_text(
        "A woman sits on a jetty at dusk, the water still.",
        &Intent {
            style: Some("35mm film".to_string()),
            ..Intent::default()
        },
    );
    assert_eq!(
        compiled.positive,
        "A woman sits on a jetty at dusk, the water still. 35mm film."
    );
}

// === Binding ================================================================

#[test]
fn a_description_binds_into_the_next_stage_by_its_override_key() {
    let contract = contract_with(vec![("positive", "6", "text")]);
    let mut overrides = std::collections::HashMap::new();
    bind_description(
        &contract,
        &mut overrides,
        &CompiledPrompt {
            positive: "a cat.".to_string(),
            negative: String::new(),
        },
    )
    .unwrap();
    // The key `prepare_workflow` substitutes on, and nothing else.
    assert_eq!(overrides.get("6.text").map(String::as_str), Some("a cat."));
    assert_eq!(contract.slot("positive").unwrap().override_key(), "6.text");
}

#[test]
fn the_description_beats_whatever_was_left_in_the_box() {
    let contract = contract_with(vec![("positive", "6", "text")]);
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("6.text".to_string(), "a leftover default".to_string());
    bind_description(
        &contract,
        &mut overrides,
        &CompiledPrompt {
            positive: "a cat.".to_string(),
            negative: String::new(),
        },
    )
    .unwrap();
    assert_eq!(overrides.get("6.text").map(String::as_str), Some("a cat."));
}

#[test]
fn constraints_are_appended_to_a_negative_prompt_somebody_tuned() {
    let contract = contract_with(vec![("positive", "6", "text"), ("negative", "7", "text")]);
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("7.text".to_string(), "blurry, watermark".to_string());
    bind_description(
        &contract,
        &mut overrides,
        &CompiledPrompt {
            positive: "a cat.".to_string(),
            negative: "change face, add people".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        overrides.get("7.text").map(String::as_str),
        Some("blurry, watermark, change face, add people")
    );
}

#[test]
fn a_stage_can_name_the_slot_it_wants_the_description_in() {
    let contract = contract_with(vec![("positive", "6", "text"), ("scene", "9", "prompt")]);
    let mut overrides = std::collections::HashMap::new();
    overrides.insert(SLOT_KEY.to_string(), "scene".to_string());
    bind_description(
        &contract,
        &mut overrides,
        &CompiledPrompt {
            positive: "a cat.".to_string(),
            negative: String::new(),
        },
    )
    .unwrap();
    assert_eq!(
        overrides.get("9.prompt").map(String::as_str),
        Some("a cat.")
    );
    assert_eq!(overrides.get("6.text"), None);
}

#[test]
fn one_unnamed_text_box_is_unambiguous_whatever_it_is_called() {
    let contract = contract_with(vec![("prompt_11", "11", "value")]);
    let mut overrides = std::collections::HashMap::new();
    bind_description(
        &contract,
        &mut overrides,
        &CompiledPrompt {
            positive: "a cat.".to_string(),
            negative: String::new(),
        },
    )
    .unwrap();
    assert_eq!(
        overrides.get("11.value").map(String::as_str),
        Some("a cat.")
    );
}

#[test]
fn a_stage_with_nowhere_to_put_a_description_says_so() {
    let contract = contract_with(vec![]);
    let err = bind_description(
        &contract,
        &mut std::collections::HashMap::new(),
        &CompiledPrompt {
            positive: "a cat.".to_string(),
            negative: String::new(),
        },
    )
    .unwrap_err();
    assert!(err.message.contains("no prompt box"), "{}", err.message);
}

#[test]
fn a_named_slot_that_is_not_there_lists_the_ones_that_are() {
    let contract = contract_with(vec![("positive", "6", "text"), ("negative", "7", "text")]);
    let mut overrides = std::collections::HashMap::new();
    overrides.insert(SLOT_KEY.to_string(), "scene".to_string());
    let err = bind_description(&contract, &mut overrides, &CompiledPrompt::default()).unwrap_err();
    assert!(
        err.message.contains("positive, negative"),
        "{}",
        err.message
    );
}

#[test]
fn the_instruction_goes_into_the_describe_stage_the_same_way() {
    let contract = contract_with(vec![("positive", "4", "prompt")]);
    let mut overrides = std::collections::HashMap::new();
    bind_instruction(&contract, &mut overrides, "describe this").unwrap();
    assert_eq!(
        overrides.get("4.prompt").map(String::as_str),
        Some("describe this")
    );
}

#[test]
fn an_instruction_a_person_typed_is_left_alone() {
    // The Enhance dialog sends every text box's current value, so "untouched"
    // has to mean "still the graph's own default" rather than "absent".
    let mut contract = contract_with(vec![("positive", "4", "prompt")]);
    contract.slots[0].default = Some("describe this photograph".to_string());

    let mut untouched = std::collections::HashMap::new();
    untouched.insert(
        "4.prompt".to_string(),
        "describe this photograph".to_string(),
    );
    bind_instruction(&contract, &mut untouched, "PHOS INSTRUCTION").unwrap();
    assert_eq!(untouched["4.prompt"], "PHOS INSTRUCTION");

    let mut typed = std::collections::HashMap::new();
    typed.insert("4.prompt".to_string(), "just say the colours".to_string());
    bind_instruction(&contract, &mut typed, "PHOS INSTRUCTION").unwrap();
    assert_eq!(
        typed["4.prompt"], "just say the colours",
        "a person testing their own instruction keeps it"
    );
}

#[test]
fn a_stage_inherits_the_words_typed_once_on_the_describe_stage() {
    let stage_said_nothing = Intent::default().inherit(intent());
    assert_eq!(stage_said_nothing, intent());

    let stage_said_its_own = Intent {
        style: Some("charcoal sketch".to_string()),
        ..Intent::default()
    }
    .inherit(intent());
    assert_eq!(stage_said_its_own.style.as_deref(), Some("charcoal sketch"));
    assert_eq!(
        stage_said_its_own.intent.as_deref(),
        Some("a slow push-in as the light fades")
    );
}

#[test]
fn a_run_can_ask_for_a_fresh_description() {
    let mut overrides = std::collections::HashMap::new();
    assert!(!wants_refresh(&overrides));
    overrides.insert(REFRESH_KEY.to_string(), "1".to_string());
    assert!(wants_refresh(&overrides));
}
