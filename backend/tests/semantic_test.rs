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

/// Manual calibration for "same photo" grouping: SigLIP cosine and dHash
/// distance for edited copies vs. distinct photos.
/// `PHOS_SEMANTIC_EVAL_DIR=/path cargo test --release --test semantic_test variation_eval -- --ignored --nocapture`
#[test]
#[ignore]
fn variation_eval() {
    use image::imageops::FilterType;
    use phos_backend::scanner::{compute_dhash, hamming_distance};
    use phos_backend::semantic::{simhash, SAME_PHOTO_SIMILARITY};
    let dir = std::env::var("PHOS_SEMANTIC_EVAL_DIR").expect("set PHOS_SEMANTIC_EVAL_DIR");
    let model = SemanticModel::default();
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    paths.sort();
    let originals: Vec<image::DynamicImage> = paths.iter().filter_map(|p| image::open(p).ok()).collect();
    let jpeg = |img: &image::DynamicImage, q: u8| {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q).encode_image(&img.to_rgb8()).unwrap();
        image::load_from_memory(&buf.into_inner()).unwrap()
    };
    type Edit = (&'static str, fn(&image::DynamicImage) -> image::DynamicImage);
    let edits: Vec<Edit> = vec![
        ("resize25", |i| i.resize(i.width() / 4, i.height() / 4, FilterType::Triangle)),
        ("crop10", |i| i.crop_imm(i.width() / 20, i.height() / 20, i.width() * 9 / 10, i.height() * 9 / 10)),
        ("crop30", |i| i.crop_imm(i.width() * 3 / 20, i.height() * 3 / 20, i.width() * 7 / 10, i.height() * 7 / 10)),
        ("bright", |i| i.brighten(40)),
        ("contrast", |i| i.adjust_contrast(30.0)),
        ("gray", |i| image::DynamicImage::ImageLuma8(i.to_luma8())),
        ("hue", |i| i.huerotate(25)),
        ("flip", |i| i.fliph()),
        ("shift20", |i| i.crop_imm(i.width() / 5, 0, i.width() * 4 / 5, i.height())),
    ];
    let emb: Vec<Vec<f32>> = model.embed_images(&originals).unwrap();
    let dh: Vec<[u8; 8]> = originals.iter().map(compute_dhash).collect();
    for (name, f) in &edits {
        let mut cos = Vec::new();
        let mut ham = Vec::new();
        let mut bits = Vec::new();
        for (i, o) in originals.iter().enumerate() {
            let v = if *name == "resize25" { f(o) } else { jpeg(&f(o), 85) };
            let e = model.embed_image(&v).unwrap();
            cos.push(dot(&emb[i], &e));
            bits.push((simhash(&emb[i]) ^ simhash(&e)).count_ones());
            ham.push(hamming_distance(&dh[i], &compute_dhash(&v)));
        }
        let same = cos.iter().filter(|&&c| c >= SAME_PHOTO_SIMILARITY).count();
        println!("          simhash bits max={} ; grouped by SigLIP: {same}/{} ; by either: {}/{}",
            bits.iter().max().unwrap(), cos.len(),
            cos.iter().zip(&ham).filter(|(c, h)| **c >= SAME_PHOTO_SIMILARITY || **h <= 10).count(), cos.len());
        cos.sort_by(f32::total_cmp);
        ham.sort();
        println!("{name:<9} cos min={:.3} med={:.3} | dhash max={} med={} (<=10: {}/{})",
            cos[0], cos[cos.len() / 2], ham[ham.len() - 1], ham[ham.len() / 2],
            ham.iter().filter(|&&h| h <= 10).count(), ham.len());
    }
    let mut pairs: Vec<(f32, u32, usize, usize)> = Vec::new();
    for i in 0..originals.len() {
        for j in i + 1..originals.len() {
            pairs.push((dot(&emb[i], &emb[j]), hamming_distance(&dh[i], &dh[j]), i, j));
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("distinct pairs: {} ; dhash<=10: {}", pairs.len(), pairs.iter().filter(|p| p.1 <= 10).count());
    for (c, h, i, j) in pairs.iter().take(6) {
        let bits = (simhash(&emb[*i]) ^ simhash(&emb[*j])).count_ones();
        println!("  cos={c:.3} dhash={h} simhash={bits} {:?} {:?}", paths[*i].file_name().unwrap(), paths[*j].file_name().unwrap());
    }
}
