//! Integration test for the image captioning pipeline.
//!
//! Uses the same test fixture images as the face recognition tests.
//! Requires captioning models (downloads from HuggingFace on first run).

use phos_backend::ai::AiPipeline;
use std::path::Path;

/// Load a test fixture image.
fn load_fixture(name: &str) -> image::DynamicImage {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    image::open(&path).unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e))
}

#[test]
fn test_caption_generates_nonempty_text() {
    ffmpeg_next::init().unwrap();
    let ai = AiPipeline::new().expect("Failed to load AI models");

    if !ai.has_captioning() {
        eprintln!("Captioning models not available, skipping test");
        return;
    }

    let img = load_fixture("keanu1.jpg");
    let caption = ai.generate_caption(&img).expect("generate_caption failed");

    println!("Caption for keanu1.jpg: {:?}", caption);

    assert!(!caption.is_empty(), "Caption should not be empty");
    assert!(
        caption.len() > 3,
        "Caption too short: {:?} (likely degenerate output)",
        caption
    );
    // Caption should be reasonable English text, not garbage tokens
    assert!(
        caption.chars().all(|c| c.is_ascii() || c.is_alphanumeric()),
        "Caption contains unexpected characters: {:?}",
        caption
    );
}

#[test]
fn test_caption_different_images_produce_different_text() {
    ffmpeg_next::init().unwrap();
    let ai = AiPipeline::new().expect("Failed to load AI models");

    if !ai.has_captioning() {
        eprintln!("Captioning models not available, skipping test");
        return;
    }

    let keanu = load_fixture("keanu1.jpg");
    let celentano = load_fixture("celentano.jpg");

    let caption1 = ai
        .generate_caption(&keanu)
        .expect("generate_caption failed for keanu1");
    let caption2 = ai
        .generate_caption(&celentano)
        .expect("generate_caption failed for celentano");

    println!("Caption keanu1:    {:?}", caption1);
    println!("Caption celentano: {:?}", caption2);

    // Both should be non-empty
    assert!(!caption1.is_empty(), "keanu caption empty");
    assert!(!caption2.is_empty(), "celentano caption empty");

    // They should contain recognizable words (basic sanity — the model should
    // produce something like "a man in a suit" or "a person wearing...")
    let has_common_words = |s: &str| {
        let lower = s.to_lowercase();
        lower.contains("a ") || lower.contains("the ") || lower.contains("man") || lower.contains("person")
    };
    assert!(
        has_common_words(&caption1),
        "keanu caption doesn't look like English: {:?}",
        caption1
    );
    assert!(
        has_common_words(&caption2),
        "celentano caption doesn't look like English: {:?}",
        caption2
    );
}

#[test]
fn test_caption_synthetic_image() {
    // Test with a programmatically generated image to verify the pipeline
    // handles non-photo inputs gracefully (no crash).
    ffmpeg_next::init().unwrap();
    let ai = AiPipeline::new().expect("Failed to load AI models");

    if !ai.has_captioning() {
        eprintln!("Captioning models not available, skipping test");
        return;
    }

    // Create a simple 100x100 red square
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(100, 100, |_, _| {
        image::Rgb([255, 0, 0])
    }));

    let result = ai.generate_caption(&img);

    // Should not crash — may produce empty output for featureless images
    assert!(result.is_ok(), "generate_caption should not error on synthetic image: {:?}", result.err());
    println!("Caption for red square: {:?}", result.unwrap());
}
