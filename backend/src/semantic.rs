//! SigLIP 2 image/text embeddings for semantic search and visual similarity.
//!
//! One vision tower turns a shot's main photo into a unit vector; one text
//! tower turns a search query into a vector in the same space. Cosine between
//! the two ranks shots for a query, and cosine between two image vectors says
//! how alike two shots look — robust to the crops, filters and re-encodes that
//! defeat a 64-bit dHash.
//!
//! The towers are loaded lazily and separately: the vision tower is only needed
//! while a library is being analysed (and is unloaded afterwards), the text
//! tower only once somebody searches.

use anyhow::{Context, Result};
use hf_hub::api::sync::Api;
use image::DynamicImage;
use ort::inputs;
use ort::session::Session;
use ort::value::Value;
use std::path::PathBuf;
use std::sync::Mutex;
use tokenizers::Tokenizer;

const REPO: &str = "onnx-community/siglip2-base-patch16-256-ONNX";
/// The vision tower runs in full precision: quantised vision towers lose
/// retrieval quality, and at 372 MB it is the smaller of the two anyway.
const VISION_FILE: &str = "onnx/vision_model.onnx";
/// The text tower is dominated by its 256k-token embedding table, so int8 saves
/// ~850 MB for a negligible change in the pooled vector.
const TEXT_FILE: &str = "onnx/text_model_quantized.onnx";

/// Identifies the vectors this model produces. Stored next to every embedding
/// (as `<MODEL_ID>:<file id>`) so a model change or a new main photo makes the
/// stored vector stale instead of silently comparable with fresh ones.
pub const MODEL_ID: &str = "siglip2-base-p16-256";

const IMAGE_SIZE: u32 = 256;
/// SigLIP 2 was trained on text padded to exactly 64 tokens.
const TEXT_LEN: usize = 64;
const PAD_ID: i64 = 0;
const EOS_ID: u32 = 1;

/// Learned temperature and bias of `google/siglip2-base-patch16-256`
/// (`logit_scale` is stored as a log). The ONNX towers do not carry them.
#[allow(dead_code)] // match_probability only
const LOGIT_SCALE: f32 = 112.893_7; // exp(4.726_513)
#[allow(dead_code)]
const LOGIT_BIAS: f32 = -16.771_803;

/// Cosine a shot needs to count as a match for a search query.
///
/// SigLIP's sigmoid head is calibrated for full captions; a one- or two-word
/// query scores a correct photo at only 0.09–0.16 cosine (p ≈ 0.002–0.66), so a
/// probability cut would drop most true hits. Calibrated on 18 COCO val2017
/// photos × 19 queries (English and Russian): every correct top hit scored
/// ≥ 0.095, and all but one wrong one stayed below 0.08.
pub const MIN_QUERY_SIMILARITY: f32 = 0.08;

/// Probability that `text` describes the image, from the cosine of their
/// unit embeddings — SigLIP's own sigmoid head.
#[allow(dead_code)] // calibration aid, used by tests/semantic_test.rs
pub fn match_probability(cosine: f32) -> f32 {
    1.0 / (1.0 + (-(cosine * LOGIT_SCALE + LOGIT_BIAS)).exp())
}

fn ensure_file(filename: &str) -> Result<PathBuf> {
    let api = Api::new().context("Failed to initialize Hugging Face API")?;
    api.model(REPO.to_string())
        .get(filename)
        .with_context(|| format!("Failed to download SigLIP 2 file '{}'", filename))
}

fn build_session(path: &std::path::Path, threads: usize) -> Result<Session> {
    Session::builder()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_intra_threads(threads)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_memory_pattern(false)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

/// Extract one unit vector per batch row from a tower's pooled output.
fn pooled(outputs: &ort::session::SessionOutputs) -> Result<Vec<Vec<f32>>> {
    let value = outputs
        .get("pooler_output")
        .ok_or_else(|| anyhow::anyhow!("SigLIP output 'pooler_output' missing"))?;
    let (shape, data) = value.try_extract_tensor::<f32>()?;
    let width = *shape.last().unwrap_or(&0) as usize;
    if width == 0 {
        anyhow::bail!("SigLIP output has no width");
    }
    Ok(data.chunks_exact(width).map(|row| l2_normalize(row.to_vec())).collect())
}

/// Photos per vision-tower call. Batching keeps the intra-op threads busy on
/// large matrix multiplies instead of on per-call overhead.
pub const IMAGE_BATCH: usize = 8;

/// SiglipImageProcessor: bilinear resize to 256×256 without cropping, then
/// scale to [-1, 1].
pub fn preprocess_image(img: &DynamicImage) -> Vec<f32> {
    let rgb = img
        .resize_exact(IMAGE_SIZE, IMAGE_SIZE, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let plane = (IMAGE_SIZE * IMAGE_SIZE) as usize;
    let mut data = vec![0f32; 3 * plane];
    for (x, y, p) in rgb.enumerate_pixels() {
        let i = (y * IMAGE_SIZE + x) as usize;
        for c in 0..3 {
            data[c * plane + i] = p[c] as f32 / 127.5 - 1.0;
        }
    }
    data
}

/// SigLIP 2 tokenisation: lowercased (the model was trained on lowercased
/// text), `<eos>`-terminated, truncated and padded to exactly 64 tokens.
pub fn tokenize(tokenizer: &Tokenizer, text: &str) -> Result<Vec<i64>> {
    let encoding = tokenizer
        .encode(text.trim().to_lowercase(), true)
        .map_err(|e| anyhow::anyhow!("Tokenizer encode failed: {e}"))?;
    let mut ids: Vec<u32> = encoding.get_ids().to_vec();
    if ids.last() != Some(&EOS_ID) {
        ids.push(EOS_ID);
    }
    if ids.len() > TEXT_LEN {
        ids.truncate(TEXT_LEN - 1);
        ids.push(EOS_ID);
    }
    let mut out: Vec<i64> = ids.into_iter().map(i64::from).collect();
    out.resize(TEXT_LEN, PAD_ID);
    Ok(out)
}

struct TextTower {
    session: Session,
    tokenizer: Tokenizer,
}

/// Lazily-loaded SigLIP 2 towers. A failed load is not retried until the
/// tower is unloaded, so an offline machine does not re-download per photo.
#[derive(Default)]
pub struct SemanticModel {
    vision: Mutex<Option<Session>>,
    vision_failed: Mutex<bool>,
    text: Mutex<Option<TextTower>>,
    text_failed: Mutex<bool>,
}

fn dummy_mode() -> bool {
    std::env::var("PHOS_DUMMY_AI").is_ok_and(|v| v == "1")
}

impl SemanticModel {
    /// Embed an image as a unit vector. Loads the vision tower on first use.
    #[allow(dead_code)] // used by tests/semantic_test.rs
    pub fn embed_image(&self, img: &DynamicImage) -> Result<Vec<f32>> {
        self.embed_images(std::slice::from_ref(img))?
            .pop()
            .ok_or_else(|| anyhow::anyhow!("SigLIP returned no embedding"))
    }

    /// Embed a batch of images in one vision-tower call, one unit vector each,
    /// in input order. The tower runs on half the machine's cores.
    pub fn embed_images(&self, imgs: &[DynamicImage]) -> Result<Vec<Vec<f32>>> {
        use rayon::prelude::*;
        if imgs.is_empty() {
            return Ok(Vec::new());
        }
        if dummy_mode() {
            anyhow::bail!("Semantic embeddings are disabled in dummy AI mode");
        }
        let mut guard = self.vision.lock().unwrap();
        if guard.is_none() {
            let mut failed = self.vision_failed.lock().unwrap();
            if *failed {
                anyhow::bail!("SigLIP 2 vision tower unavailable");
            }
            tracing::info!("Loading SigLIP 2 vision tower...");
            let threads = crate::scanner::half_available_threads();
            match ensure_file(VISION_FILE).and_then(|p| build_session(&p, threads)) {
                Ok(s) => *guard = Some(s),
                Err(e) => {
                    *failed = true;
                    return Err(e);
                }
            }
        }
        let session = guard.as_mut().unwrap();
        let s = IMAGE_SIZE as i64;
        let pixels: Vec<f32> = imgs.par_iter().flat_map_iter(preprocess_image).collect();
        let input = Value::from_array((vec![imgs.len() as i64, 3, s, s], pixels))?;
        let outputs = session.run(inputs!["pixel_values" => input])?;
        let vectors = pooled(&outputs)?;
        if vectors.len() != imgs.len() {
            anyhow::bail!("SigLIP returned {} embeddings for {} images", vectors.len(), imgs.len());
        }
        Ok(vectors)
    }

    /// Embed a search query as a unit vector. Loads the text tower on first use.
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        if dummy_mode() {
            anyhow::bail!("Semantic embeddings are disabled in dummy AI mode");
        }
        let mut guard = self.text.lock().unwrap();
        if guard.is_none() {
            let mut failed = self.text_failed.lock().unwrap();
            if *failed {
                anyhow::bail!("SigLIP 2 text tower unavailable");
            }
            tracing::info!("Loading SigLIP 2 text tower...");
            let loaded = (|| -> Result<TextTower> {
                // One query at a time, ~30 ms on one core: more threads would
                // only contend with an analysis pass running beside it.
                let session = build_session(&ensure_file(TEXT_FILE)?, 1)?;
                let tokenizer = Tokenizer::from_file(ensure_file("tokenizer.json")?)
                    .map_err(|e| anyhow::anyhow!("Failed to load SigLIP tokenizer: {e}"))?;
                Ok(TextTower { session, tokenizer })
            })();
            match loaded {
                Ok(t) => *guard = Some(t),
                Err(e) => {
                    *failed = true;
                    return Err(e);
                }
            }
        }
        let tower = guard.as_mut().unwrap();
        let ids = tokenize(&tower.tokenizer, text)?;
        let input = Value::from_array((vec![1i64, TEXT_LEN as i64], ids))?;
        let outputs = tower.session.run(inputs!["input_ids" => input])?;
        pooled(&outputs)?
            .pop()
            .ok_or_else(|| anyhow::anyhow!("SigLIP returned no embedding"))
    }

    /// Free the vision tower (~400 MB) after a batch; it reloads on demand.
    pub fn unload_vision(&self) {
        if self.vision.lock().unwrap().take().is_some() {
            tracing::info!("SigLIP 2 vision tower unloaded");
        }
        *self.vision_failed.lock().unwrap() = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probability_is_monotonic_and_centred_on_the_bias() {
        let p_mid = match_probability(-LOGIT_BIAS / LOGIT_SCALE);
        assert!((p_mid - 0.5).abs() < 1e-4);
        assert!(match_probability(0.05) < match_probability(0.1));
        assert!(match_probability(0.3) > 0.99);
    }

    #[test]
    fn preprocess_scales_to_unit_range() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(10, 20, |x, _| {
            if x < 5 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) }
        }));
        let data = preprocess_image(&img);
        assert_eq!(data.len(), 3 * 256 * 256);
        assert_eq!(data[0], -1.0);
        assert_eq!(data[255], 1.0);
    }

    #[test]
    fn normalize_gives_unit_length() {
        let v = l2_normalize(vec![3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
    }
}
