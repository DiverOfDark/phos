//! Integration tests for the SigLIP 2 semantic embeddings.
//!
//! Downloads the SigLIP 2 towers from Hugging Face on first run.

use phos_backend::semantic::{match_probability, SemanticModel};
use std::path::Path;

fn load_fixture(name: &str) -> image::DynamicImage {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    image::open(&path).unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn solid(rgb: [u8; 3]) -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb(rgb)))
}

#[test]
fn embeddings_are_unit_vectors_of_the_same_width() {
    let model = SemanticModel::default();
    let image = model.embed_image(&load_fixture("keanu1.jpg")).unwrap();
    let text = model.embed_text("a man").unwrap();
    assert_eq!(image.len(), 768);
    assert_eq!(text.len(), 768);
    assert!((dot(&image, &image) - 1.0).abs() < 1e-4);
    assert!((dot(&text, &text) - 1.0).abs() < 1e-4);
}

#[test]
fn query_ranks_the_matching_image_first() {
    let model = SemanticModel::default();
    let red = model.embed_image(&solid([220, 20, 20])).unwrap();
    let blue = model.embed_image(&solid([20, 40, 220])).unwrap();
    let portrait = model.embed_image(&load_fixture("keanu1.jpg")).unwrap();

    let q_red = model.embed_text("red").unwrap();
    let q_blue = model.embed_text("blue").unwrap();
    let q_man = model.embed_text("a photo of a man").unwrap();

    assert!(dot(&q_red, &red) > dot(&q_red, &blue));
    assert!(dot(&q_blue, &blue) > dot(&q_blue, &red));
    assert!(dot(&q_man, &portrait) > dot(&q_man, &red));
    assert!(dot(&q_man, &portrait) > dot(&q_man, &blue));
    println!(
        "p(man|portrait)={:.4} p(man|red)={:.6}",
        match_probability(dot(&q_man, &portrait)),
        match_probability(dot(&q_man, &red)),
    );
}

#[test]
fn a_resized_copy_is_closer_than_a_different_photo() {
    let model = SemanticModel::default();
    let original = load_fixture("keanu1.jpg");
    let (w, h) = (original.width(), original.height());
    let copy = original
        .crop_imm(w / 20, h / 20, w - w / 10, h - h / 10)
        .resize(w / 4, h / 4, image::imageops::FilterType::Triangle);

    let a = model.embed_image(&original).unwrap();
    let b = model.embed_image(&copy).unwrap();
    let other = model.embed_image(&load_fixture("celentano.jpg")).unwrap();
    println!("copy={:.4} other={:.4}", dot(&a, &b), dot(&a, &other));
    assert!(dot(&a, &b) > 0.9);
    assert!(dot(&a, &b) > dot(&a, &other));
}

/// Manual calibration aid: `PHOS_SEMANTIC_EVAL_DIR=/path cargo test --test
/// semantic_test eval -- --ignored --nocapture` prints, per query, every image
/// in the directory ranked by match probability.
#[test]
#[ignore]
fn eval() {
    let dir = std::env::var("PHOS_SEMANTIC_EVAL_DIR").expect("set PHOS_SEMANTIC_EVAL_DIR");
    let queries: Vec<String> = std::env::var("PHOS_SEMANTIC_EVAL_QUERIES")
        .map(|q| q.split('|').map(str::to_string).collect())
        .unwrap_or_else(|_| vec!["cat".into(), "dog".into()]);
    let model = SemanticModel::default();
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    paths.sort();
    let loaded: Vec<(String, image::DynamicImage)> = paths
        .iter()
        .filter_map(|p| Some((p.file_name()?.to_string_lossy().into_owned(), image::open(p).ok()?)))
        .collect();
    let t = std::time::Instant::now();
    let mut images: Vec<(String, Vec<f32>)> = Vec::new();
    for chunk in loaded.chunks(phos_backend::semantic::IMAGE_BATCH) {
        let imgs: Vec<_> = chunk.iter().map(|(_, i)| i.clone()).collect();
        let vecs = model.embed_images(&imgs).unwrap();
        images.extend(chunk.iter().map(|(n, _)| n.clone()).zip(vecs));
    }
    println!("embedded {} images in {:?} (incl. model load)", images.len(), t.elapsed());
    let t = std::time::Instant::now();
    for chunk in loaded.chunks(phos_backend::semantic::IMAGE_BATCH) {
        let imgs: Vec<_> = chunk.iter().map(|(_, i)| i.clone()).collect();
        model.embed_images(&imgs).unwrap();
    }
    println!("re-embedded {} images in {:?} (warm)", images.len(), t.elapsed());
    for q in &queries {
        let t = std::time::Instant::now();
        let qe = model.embed_text(q).unwrap();
        let mut scored: Vec<(f32, &str)> =
            images.iter().map(|(n, e)| (dot(&qe, e), n.as_str())).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        println!("\n== {q:?} ({:?})", t.elapsed());
        for (c, n) in scored.iter().take(5) {
            println!("  {n:<20} cos={c:.4} p={:.4}", match_probability(*c));
        }
        let (c, n) = scored.last().unwrap();
        println!("  … last {n:<14} cos={c:.4} p={:.6}", match_probability(*c));
    }
}
