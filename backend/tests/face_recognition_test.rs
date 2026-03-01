//! Integration test for face detection and recognition pipeline.
//!
//! Uses real photos from Wikimedia Commons:
//! - keanu1.jpg: Keanu Reeves at TIFF 2025 (CC BY-SA 4.0)
//! - keanu2.jpg: Keanu Reeves 2019 (CC BY 2.0)
//! - celentano.jpg: Adriano Celentano 1961 (public domain)
//!
//! Requires AI models (downloads from HuggingFace on first run).

use phos_backend::ai::{cosine_similarity, AiPipeline};
use std::path::Path;

/// Load a test fixture image.
fn load_fixture(name: &str) -> image::DynamicImage {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    image::open(&path).unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e))
}

/// Detect the largest face in an image and return its embedding.
fn detect_largest_face_embedding(ai: &AiPipeline, img: &image::DynamicImage) -> Vec<f32> {
    use image::GenericImageView;

    let detections = ai.detect_faces(img).expect("detect_faces failed");
    assert!(!detections.is_empty(), "No faces detected in image");

    // Find largest face by bounding box area
    let largest = detections
        .iter()
        .max_by(|a, b| {
            let area_a = (a.box_x2 - a.box_x1) * (a.box_y2 - a.box_y1);
            let area_b = (b.box_x2 - b.box_x1) * (b.box_y2 - b.box_y1);
            area_a
                .partial_cmp(&area_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();

    let (img_w, img_h) = img.dimensions();
    let x1 = (largest.box_x1.max(0.0) as u32).min(img_w.saturating_sub(1));
    let y1 = (largest.box_y1.max(0.0) as u32).min(img_h.saturating_sub(1));
    let x2 = (largest.box_x2.max(0.0) as u32).min(img_w);
    let y2 = (largest.box_y2.max(0.0) as u32).min(img_h);
    let face_w = x2.saturating_sub(x1);
    let face_h = y2.saturating_sub(y1);
    assert!(
        face_w >= 10 && face_h >= 10,
        "Largest face too small: {}x{} (detection: ({},{}) -> ({},{}))",
        face_w,
        face_h,
        largest.box_x1,
        largest.box_y1,
        largest.box_x2,
        largest.box_y2,
    );

    let bbox = (
        largest.box_x1,
        largest.box_y1,
        largest.box_x2,
        largest.box_y2,
    );
    let embedding = ai
        .extract_embedding(img, largest.landmarks.as_deref(), bbox)
        .expect("extract_embedding failed");
    assert!(!embedding.is_empty(), "Empty embedding returned");
    embedding
}

#[test]
fn test_face_detection_finds_faces() {
    ffmpeg_next::init().unwrap();
    let ai = AiPipeline::new().expect("Failed to load AI models");

    let keanu1 = load_fixture("keanu1.jpg");
    let keanu2 = load_fixture("keanu2.jpg");
    let celentano = load_fixture("celentano.jpg");

    let det1 = ai.detect_faces(&keanu1).unwrap();
    let det2 = ai.detect_faces(&keanu2).unwrap();
    let det3 = ai.detect_faces(&celentano).unwrap();

    println!("keanu1: {} faces detected", det1.len());
    println!("keanu2: {} faces detected", det2.len());
    println!("celentano: {} faces detected", det3.len());

    // Each portrait should have at least 1 face, and not hundreds of garbage detections
    assert!(!det1.is_empty(), "No faces in keanu1");
    assert!(!det2.is_empty(), "No faces in keanu2");
    assert!(!det3.is_empty(), "No faces in celentano");
    assert!(
        det1.len() < 20,
        "Too many detections in keanu1: {} (likely parsing bug)",
        det1.len()
    );
    assert!(
        det2.len() < 20,
        "Too many detections in keanu2: {} (likely parsing bug)",
        det2.len()
    );
    assert!(
        det3.len() < 20,
        "Too many detections in celentano: {} (likely parsing bug)",
        det3.len()
    );
}

#[test]
fn test_same_person_high_similarity() {
    ffmpeg_next::init().unwrap();
    let ai = AiPipeline::new().expect("Failed to load AI models");

    let keanu1 = load_fixture("keanu1.jpg");
    let keanu2 = load_fixture("keanu2.jpg");

    let emb1 = detect_largest_face_embedding(&ai, &keanu1);
    let emb2 = detect_largest_face_embedding(&ai, &keanu2);

    let similarity = cosine_similarity(&emb1, &emb2);
    println!(
        "Keanu vs Keanu similarity: {:.4} (threshold: 0.4)",
        similarity
    );
    assert!(
        similarity > 0.4,
        "Two photos of Keanu Reeves should match (similarity={:.4}, threshold=0.4)",
        similarity
    );
}

#[test]
fn test_different_people_low_similarity() {
    ffmpeg_next::init().unwrap();
    let ai = AiPipeline::new().expect("Failed to load AI models");

    let keanu1 = load_fixture("keanu1.jpg");
    let celentano = load_fixture("celentano.jpg");

    let emb_keanu = detect_largest_face_embedding(&ai, &keanu1);
    let emb_celentano = detect_largest_face_embedding(&ai, &celentano);

    let similarity = cosine_similarity(&emb_keanu, &emb_celentano);
    println!(
        "Keanu vs Celentano similarity: {:.4} (threshold: 0.4)",
        similarity
    );
    assert!(
        similarity < 0.4,
        "Keanu vs Celentano should NOT match (similarity={:.4}, threshold=0.4)",
        similarity
    );
}
