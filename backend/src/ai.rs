use anyhow::{Context, Result};
use hf_hub::api::sync::Api;
use image::{DynamicImage, GenericImageView};
use ndarray::Array4;
use ort::inputs;
use ort::session::Session;
use ort::value::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Cosine similarity threshold for matching face embeddings to existing persons.
pub const FACE_SIMILARITY_THRESHOLD: f32 = 0.4;

/// Computes the cosine similarity between two embedding vectors.
///
/// Returns a value in [-1, 1] where 1 means identical direction,
/// 0 means orthogonal, and -1 means opposite direction.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(
        a.len(),
        b.len(),
        "Embedding vectors must have the same length"
    );

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a < 1e-10 || norm_b < 1e-10 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

pub struct AiPipeline {
    face_detector: Option<Mutex<Session>>,
    face_recognizer: Option<Mutex<Session>>,
    logged_outputs: AtomicBool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub box_x1: f32,
    pub box_y1: f32,
    pub box_x2: f32,
    pub box_y2: f32,
    pub score: f32,
    pub landmarks: Option<Vec<(f32, f32)>>,
}

fn ensure_model(filename: &str) -> Result<PathBuf> {
    let api = Api::new().context("Failed to initialize Hugging Face API")?;
    let repo = api.model("public-data/insightface".to_string());
    let path = repo
        .get(&format!("models/buffalo_l/{}", filename))
        .with_context(|| format!("Failed to download model '{}'", filename))?;
    Ok(path)
}

/// A raw detection before NMS, including the score and landmarks.
#[derive(Debug, Clone)]
struct RawDetection {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    landmarks: Vec<(f32, f32)>,
}

/// Compute IoU (intersection over union) for two axis-aligned bounding boxes.
fn iou(a: &RawDetection, b: &RawDetection) -> f32 {
    let inter_x1 = a.x1.max(b.x1);
    let inter_y1 = a.y1.max(b.y1);
    let inter_x2 = a.x2.min(b.x2);
    let inter_y2 = a.y2.min(b.y2);

    let inter_w = (inter_x2 - inter_x1).max(0.0);
    let inter_h = (inter_y2 - inter_y1).max(0.0);
    let inter_area = inter_w * inter_h;

    let area_a = (a.x2 - a.x1) * (a.y2 - a.y1);
    let area_b = (b.x2 - b.x1) * (b.y2 - b.y1);
    let union_area = area_a + area_b - inter_area;

    if union_area < 1e-6 {
        return 0.0;
    }
    inter_area / union_area
}

/// Non-maximum suppression: given detections sorted by descending score,
/// greedily keep detections that don't overlap too much with already-kept ones.
fn nms(detections: &mut [RawDetection], iou_threshold: f32) -> Vec<RawDetection> {
    detections.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep: Vec<RawDetection> = Vec::new();
    let mut suppressed = vec![false; detections.len()];

    for i in 0..detections.len() {
        if suppressed[i] {
            continue;
        }
        keep.push(detections[i].clone());
        for j in (i + 1)..detections.len() {
            if suppressed[j] {
                continue;
            }
            if iou(&detections[i], &detections[j]) > iou_threshold {
                suppressed[j] = true;
            }
        }
    }
    keep
}

/// Try to find a tensor in the outputs by checking several candidate names.
/// Returns the (shape_dims, data_slice) if found.
fn find_output_tensor<'a>(
    outputs: &'a ort::session::SessionOutputs<'_>,
    candidates: &[&str],
    output_keys: &[String],
) -> Option<(&'a [i64], &'a [f32])> {
    // First try exact name matches
    for name in candidates {
        if let Some(val) = outputs.get(*name) {
            if let Ok((shape, data)) = val.try_extract_tensor::<f32>() {
                return Some((&**shape, data));
            }
        }
    }
    // Fallback: try matching by substring in actual output keys
    for key in output_keys {
        for name in candidates {
            if key.contains(name) {
                if let Some(val) = outputs.get(key.as_str()) {
                    if let Ok((shape, data)) = val.try_extract_tensor::<f32>() {
                        return Some((&**shape, data));
                    }
                }
            }
        }
    }
    None
}

impl AiPipeline {
    pub fn new() -> Result<Self> {
        if std::env::var("PHOS_DUMMY_AI").is_ok() {
            return Ok(Self {
                face_detector: None,
                face_recognizer: None,
                logged_outputs: AtomicBool::new(false),
            });
        }

        let det_path = ensure_model("det_10g.onnx")?;
        let rec_path = ensure_model("w600k_r50.onnx")?;

        let face_detector = Session::builder()?.commit_from_file(&det_path)?;

        let face_recognizer = Session::builder()?.commit_from_file(&rec_path)?;

        Ok(Self {
            face_detector: Some(Mutex::new(face_detector)),
            face_recognizer: Some(Mutex::new(face_recognizer)),
            logged_outputs: AtomicBool::new(false),
        })
    }

    pub fn detect_faces(&self, img: &DynamicImage) -> Result<Vec<FaceDetection>> {
        if let (Some(detector_mutex), false) =
            (&self.face_detector, std::env::var("PHOS_DUMMY_AI").is_ok())
        {
            let (orig_w, orig_h) = img.dimensions();
            let target_size: u32 = 640;
            let resized = img.resize_exact(
                target_size,
                target_size,
                image::imageops::FilterType::Triangle,
            );
            let rgb_img = resized.to_rgb8();

            let mut input =
                Array4::<f32>::zeros((1, 3, target_size as usize, target_size as usize));
            for (x, y, rgb) in rgb_img.enumerate_pixels() {
                input[[0, 0, y as usize, x as usize]] = (rgb[0] as f32 - 127.5) / 128.0;
                input[[0, 1, y as usize, x as usize]] = (rgb[1] as f32 - 127.5) / 128.0;
                input[[0, 2, y as usize, x as usize]] = (rgb[2] as f32 - 127.5) / 128.0;
            }

            let input_tensor = Value::from_array((
                vec![1, 3, target_size as usize, target_size as usize],
                input.into_raw_vec(),
            ))?;
            let mut session = detector_mutex
                .lock()
                .map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;

            // Log output names and shapes on first run for debugging
            if !self.logged_outputs.load(Ordering::Relaxed) {
                self.logged_outputs.store(true, Ordering::Relaxed);
                tracing::debug!("SCRFD model outputs ({} total):", session.outputs().len());
                for (i, output) in session.outputs().iter().enumerate() {
                    tracing::debug!(
                        "  output[{}]: name='{}', type={:?}",
                        i,
                        output.name(),
                        output.dtype()
                    );
                }
                tracing::debug!("SCRFD model inputs ({} total):", session.inputs().len());
                for (i, input_info) in session.inputs().iter().enumerate() {
                    tracing::debug!(
                        "  input[{}]: name='{}', type={:?}",
                        i,
                        input_info.name(),
                        input_info.dtype()
                    );
                }
            }

            let outputs = session.run(inputs!["input.1" => input_tensor])?;

            // Collect actual output names for flexible matching
            let output_keys: Vec<String> = outputs.keys().map(|k| k.to_string()).collect();
            tracing::debug!("SCRFD inference output keys: {:?}", output_keys);

            // Parse SCRFD outputs for each stride
            let score_threshold: f32 = 0.5;
            let nms_threshold: f32 = 0.4;
            let strides: [u32; 3] = [8, 16, 32];
            let num_anchors: usize = 2;

            let mut all_detections: Vec<RawDetection> = Vec::new();

            for &stride in &strides {
                let fh = target_size / stride;
                let fw = target_size / stride;

                // Find score, bbox, and kps tensors for this stride
                let stride_str = stride.to_string();

                let score_candidates: Vec<String> = vec![
                    format!("score_{}", stride_str),
                    format!("scores_{}", stride_str),
                    format!("score{}", stride_str),
                ];
                let score_candidate_refs: Vec<&str> =
                    score_candidates.iter().map(|s| s.as_str()).collect();

                let bbox_candidates: Vec<String> = vec![
                    format!("bbox_{}", stride_str),
                    format!("bboxes_{}", stride_str),
                    format!("bbox{}", stride_str),
                ];
                let bbox_candidate_refs: Vec<&str> =
                    bbox_candidates.iter().map(|s| s.as_str()).collect();

                let kps_candidates: Vec<String> = vec![
                    format!("kps_{}", stride_str),
                    format!("keypoints_{}", stride_str),
                    format!("kps{}", stride_str),
                ];
                let kps_candidate_refs: Vec<&str> =
                    kps_candidates.iter().map(|s| s.as_str()).collect();

                let score_result =
                    find_output_tensor(&outputs, &score_candidate_refs, &output_keys);
                let bbox_result = find_output_tensor(&outputs, &bbox_candidate_refs, &output_keys);
                let kps_result = find_output_tensor(&outputs, &kps_candidate_refs, &output_keys);

                if score_result.is_none() || bbox_result.is_none() {
                    // If we can't find outputs by name, try positional fallback
                    tracing::debug!(
                        "Could not find named outputs for stride {}. Trying positional fallback.",
                        stride
                    );
                    continue;
                }

                let (score_shape, score_data) = score_result.unwrap();
                let (bbox_shape, bbox_data) = bbox_result.unwrap();
                let kps_data_opt = kps_result.map(|(_, data)| data);

                tracing::debug!(
                    "Stride {}: score_shape={:?}, bbox_shape={:?}, fh={}, fw={}",
                    stride,
                    score_shape,
                    bbox_shape,
                    fh,
                    fw
                );

                // Determine layout from shapes
                // SCRFD outputs can be in two formats:
                // Format A (insightface): [1, num_anchors*H*W, 1] for scores, [1, num_anchors*H*W, 4] for bbox
                // Format B (standard):    [1, num_anchors, H, W] for scores, [1, num_anchors*4, H, W] for bbox
                let is_flat = score_shape.len() == 3;

                if is_flat {
                    // Format A: [1, N, 1] scores, [1, N, 4] bbox, [1, N, 10] kps
                    let n = score_shape[1] as usize;
                    for (idx, &score) in score_data.iter().enumerate().take(n) {
                        if score < score_threshold {
                            continue;
                        }

                        // Determine anchor position
                        let anchor_idx = idx / num_anchors;
                        let anchor_row = anchor_idx / fw as usize;
                        let anchor_col = anchor_idx % fw as usize;
                        let cx = (anchor_col as f32 + 0.5) * stride as f32;
                        let cy = (anchor_row as f32 + 0.5) * stride as f32;

                        // Decode bbox: deltas are distances from anchor center
                        let base = idx * 4;
                        if base + 3 >= bbox_data.len() {
                            continue;
                        }
                        let x1 = cx - bbox_data[base] * stride as f32;
                        let y1 = cy - bbox_data[base + 1] * stride as f32;
                        let x2 = cx + bbox_data[base + 2] * stride as f32;
                        let y2 = cy + bbox_data[base + 3] * stride as f32;

                        // Decode landmarks
                        let mut landmarks = Vec::new();
                        if let Some(kps) = kps_data_opt {
                            let kps_base = idx * 10;
                            if kps_base + 9 < kps.len() {
                                for k in 0..5 {
                                    let lx = cx + kps[kps_base + k * 2] * stride as f32;
                                    let ly = cy + kps[kps_base + k * 2 + 1] * stride as f32;
                                    landmarks.push((lx, ly));
                                }
                            }
                        }

                        all_detections.push(RawDetection {
                            x1: x1.max(0.0),
                            y1: y1.max(0.0),
                            x2: x2.min(target_size as f32),
                            y2: y2.min(target_size as f32),
                            score,
                            landmarks,
                        });
                    }
                } else {
                    // Format B: [1, num_anchors, H, W] for scores
                    // bbox: [1, num_anchors*4, H, W], kps: [1, num_anchors*10, H, W]
                    for row in 0..fh as usize {
                        for col in 0..fw as usize {
                            for a in 0..num_anchors {
                                let cx = (col as f32 + 0.5) * stride as f32;
                                let cy = (row as f32 + 0.5) * stride as f32;

                                // Score: [1, num_anchors, H, W]
                                let score_idx =
                                    a * (fh as usize * fw as usize) + row * fw as usize + col;
                                if score_idx >= score_data.len() {
                                    continue;
                                }
                                let score = score_data[score_idx];
                                if score < score_threshold {
                                    continue;
                                }

                                // Bbox: [1, num_anchors*4, H, W]
                                let spatial = fh as usize * fw as usize;
                                let pixel = row * fw as usize + col;
                                let dl = bbox_data[(a * 4) * spatial + pixel];
                                let dt = bbox_data[(a * 4 + 1) * spatial + pixel];
                                let dr = bbox_data[(a * 4 + 2) * spatial + pixel];
                                let db = bbox_data[(a * 4 + 3) * spatial + pixel];

                                let x1 = cx - dl * stride as f32;
                                let y1 = cy - dt * stride as f32;
                                let x2 = cx + dr * stride as f32;
                                let y2 = cy + db * stride as f32;

                                // Landmarks: [1, num_anchors*10, H, W]
                                let mut landmarks = Vec::new();
                                if let Some(kps) = kps_data_opt {
                                    for k in 0..5 {
                                        let lx_idx = (a * 10 + k * 2) * spatial + pixel;
                                        let ly_idx = (a * 10 + k * 2 + 1) * spatial + pixel;
                                        if ly_idx < kps.len() {
                                            let lx = cx + kps[lx_idx] * stride as f32;
                                            let ly = cy + kps[ly_idx] * stride as f32;
                                            landmarks.push((lx, ly));
                                        }
                                    }
                                }

                                all_detections.push(RawDetection {
                                    x1: x1.max(0.0),
                                    y1: y1.max(0.0),
                                    x2: x2.min(target_size as f32),
                                    y2: y2.min(target_size as f32),
                                    score,
                                    landmarks,
                                });
                            }
                        }
                    }
                }
            }

            // If named outputs didn't work, try positional fallback
            if all_detections.is_empty() && outputs.len() >= 9 {
                tracing::debug!(
                    "Named output matching failed, trying positional fallback for 9 outputs"
                );
                // Standard SCRFD det_10g ordering: 3 groups of (score, bbox, kps), one per stride
                // Outputs are typically ordered as:
                // [score_8, score_16, score_32, bbox_8, bbox_16, bbox_32, kps_8, kps_16, kps_32]
                // or [score_8, bbox_8, kps_8, score_16, bbox_16, kps_16, score_32, bbox_32, kps_32]
                // We'll try to auto-detect the layout by looking at shapes.
                let mut tensor_data: Vec<(Vec<i64>, Vec<f32>)> = Vec::new();
                for i in 0..outputs.len() {
                    if let Ok((shape, data)) = outputs[i].try_extract_tensor::<f32>() {
                        let dims: Vec<i64> = shape.iter().copied().collect();
                        tracing::debug!("  positional output[{}]: shape={:?}", i, dims);
                        tensor_data.push((dims, data.to_vec()));
                    }
                }

                // Try to identify score/bbox/kps by shape patterns
                // Scores have last dim 1 or channel count = num_anchors
                // Bboxes have last dim 4 or channel count = num_anchors*4
                // Kps have last dim 10 or channel count = num_anchors*10
                if tensor_data.len() == 9 {
                    // Determine if grouped by stride or by type
                    // Check if outputs are grouped by stride (score,bbox,kps per stride)
                    // or grouped by type (all scores, all bboxes, all kps)

                    // Heuristic: if output[0] has fewer elements than output[1], likely type-grouped
                    // (scores are smaller than bboxes). If similar, likely stride-grouped.

                    let elem_0 = tensor_data[0].1.len();
                    let elem_1 = tensor_data[1].1.len();

                    let (score_indices, bbox_indices, kps_indices) = if elem_1 > elem_0 * 2 {
                        // Type-grouped: [scores..., bboxes..., kps...]
                        // scores for stride 8,16,32 then bboxes then kps
                        ([0, 1, 2], [3, 4, 5], [6, 7, 8])
                    } else {
                        // Stride-grouped: [score8, bbox8, kps8, score16, bbox16, kps16, ...]
                        ([0, 3, 6], [1, 4, 7], [2, 5, 8])
                    };

                    for (si, &stride) in strides.iter().enumerate() {
                        let fh = (target_size / stride) as usize;
                        let fw = (target_size / stride) as usize;

                        let score_data = &tensor_data[score_indices[si]].1;
                        let bbox_data = &tensor_data[bbox_indices[si]].1;
                        let kps_data = &tensor_data[kps_indices[si]].1;
                        let score_shape = &tensor_data[score_indices[si]].0;

                        let is_flat = score_shape.len() == 3;

                        if is_flat {
                            let n = if score_shape.len() >= 2 {
                                score_shape[1] as usize
                            } else {
                                score_data.len()
                            };
                            for idx in 0..n {
                                if idx >= score_data.len() {
                                    break;
                                }
                                let score = score_data[idx];
                                if score < score_threshold {
                                    continue;
                                }

                                let anchor_idx = idx / num_anchors;
                                let anchor_row = anchor_idx / fw;
                                let anchor_col = anchor_idx % fw;
                                let cx = (anchor_col as f32 + 0.5) * stride as f32;
                                let cy = (anchor_row as f32 + 0.5) * stride as f32;

                                let base = idx * 4;
                                if base + 3 >= bbox_data.len() {
                                    continue;
                                }
                                let x1 = cx - bbox_data[base] * stride as f32;
                                let y1 = cy - bbox_data[base + 1] * stride as f32;
                                let x2 = cx + bbox_data[base + 2] * stride as f32;
                                let y2 = cy + bbox_data[base + 3] * stride as f32;

                                let mut landmarks = Vec::new();
                                let kps_base = idx * 10;
                                if kps_base + 9 < kps_data.len() {
                                    for k in 0..5 {
                                        let lx = cx + kps_data[kps_base + k * 2] * stride as f32;
                                        let ly =
                                            cy + kps_data[kps_base + k * 2 + 1] * stride as f32;
                                        landmarks.push((lx, ly));
                                    }
                                }

                                all_detections.push(RawDetection {
                                    x1: x1.max(0.0),
                                    y1: y1.max(0.0),
                                    x2: x2.min(target_size as f32),
                                    y2: y2.min(target_size as f32),
                                    score,
                                    landmarks,
                                });
                            }
                        } else {
                            for row in 0..fh {
                                for col in 0..fw {
                                    for a in 0..num_anchors {
                                        let cx = (col as f32 + 0.5) * stride as f32;
                                        let cy = (row as f32 + 0.5) * stride as f32;

                                        let score_idx = a * (fh * fw) + row * fw + col;
                                        if score_idx >= score_data.len() {
                                            continue;
                                        }
                                        let score = score_data[score_idx];
                                        if score < score_threshold {
                                            continue;
                                        }

                                        let spatial = fh * fw;
                                        let pixel = row * fw + col;
                                        if (a * 4 + 3) * spatial + pixel >= bbox_data.len() {
                                            continue;
                                        }
                                        let dl = bbox_data[(a * 4) * spatial + pixel];
                                        let dt = bbox_data[(a * 4 + 1) * spatial + pixel];
                                        let dr = bbox_data[(a * 4 + 2) * spatial + pixel];
                                        let db = bbox_data[(a * 4 + 3) * spatial + pixel];

                                        let x1 = cx - dl * stride as f32;
                                        let y1 = cy - dt * stride as f32;
                                        let x2 = cx + dr * stride as f32;
                                        let y2 = cy + db * stride as f32;

                                        let mut landmarks = Vec::new();
                                        for k in 0..5 {
                                            let lx_idx = (a * 10 + k * 2) * spatial + pixel;
                                            let ly_idx = (a * 10 + k * 2 + 1) * spatial + pixel;
                                            if ly_idx < kps_data.len() {
                                                let lx = cx + kps_data[lx_idx] * stride as f32;
                                                let ly = cy + kps_data[ly_idx] * stride as f32;
                                                landmarks.push((lx, ly));
                                            }
                                        }

                                        all_detections.push(RawDetection {
                                            x1: x1.max(0.0),
                                            y1: y1.max(0.0),
                                            x2: x2.min(target_size as f32),
                                            y2: y2.min(target_size as f32),
                                            score,
                                            landmarks,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Apply NMS
            let kept = nms(&mut all_detections, nms_threshold);

            tracing::debug!("SCRFD: {} detections after NMS", kept.len());

            // Scale from 640x640 back to original image dimensions
            let scale_x = orig_w as f32 / target_size as f32;
            let scale_y = orig_h as f32 / target_size as f32;

            let results: Vec<FaceDetection> = kept
                .into_iter()
                .map(|det| {
                    let landmarks = if det.landmarks.is_empty() {
                        None
                    } else {
                        Some(
                            det.landmarks
                                .iter()
                                .map(|(lx, ly)| (lx * scale_x, ly * scale_y))
                                .collect(),
                        )
                    };
                    FaceDetection {
                        box_x1: det.x1 * scale_x,
                        box_y1: det.y1 * scale_y,
                        box_x2: det.x2 * scale_x,
                        box_y2: det.y2 * scale_y,
                        score: det.score,
                        landmarks,
                    }
                })
                .collect();

            Ok(results)
        } else {
            // Dummy mode logic
            let (width, height) = img.dimensions();
            Ok(vec![FaceDetection {
                box_x1: 10.0,
                box_y1: 10.0,
                box_x2: (width as f32).min(110.0),
                box_y2: (height as f32).min(110.0),
                score: 0.99,
                landmarks: None,
            }])
        }
    }

    pub fn extract_embedding(&self, face_img: &DynamicImage) -> Result<Vec<f32>> {
        if let (Some(recognizer_mutex), false) = (
            &self.face_recognizer,
            std::env::var("PHOS_DUMMY_AI").is_ok(),
        ) {
            let resized = face_img.resize_exact(112, 112, image::imageops::FilterType::Triangle);
            let rgb_img = resized.to_rgb8();

            let mut input = Array4::<f32>::zeros((1, 3, 112, 112));
            for (x, y, rgb) in rgb_img.enumerate_pixels() {
                input[[0, 0, y as usize, x as usize]] = (rgb[0] as f32 - 127.5) / 128.0;
                input[[0, 1, y as usize, x as usize]] = (rgb[1] as f32 - 127.5) / 128.0;
                input[[0, 2, y as usize, x as usize]] = (rgb[2] as f32 - 127.5) / 128.0;
            }

            let input_tensor = Value::from_array((vec![1, 3, 112, 112], input.into_raw_vec()))?;
            let mut session = recognizer_mutex
                .lock()
                .map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
            let outputs = session.run(inputs!["data" => input_tensor])?;
            let output_tensor = outputs[0].try_extract_tensor::<f32>()?;

            let embedding: Vec<f32> = output_tensor.1.to_vec();
            let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            Ok(embedding.into_iter().map(|x| x / (norm + 1e-10)).collect())
        } else {
            // Dummy mode: random embedding
            Ok(vec![0.1; 512])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_normalized() {
        // Normalized vectors: dot product IS the cosine similarity
        let a = vec![0.6, 0.8, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_nms_basic() {
        let mut dets = vec![
            RawDetection {
                x1: 0.0,
                y1: 0.0,
                x2: 10.0,
                y2: 10.0,
                score: 0.9,
                landmarks: vec![],
            },
            RawDetection {
                x1: 1.0,
                y1: 1.0,
                x2: 11.0,
                y2: 11.0,
                score: 0.8,
                landmarks: vec![],
            },
            RawDetection {
                x1: 100.0,
                y1: 100.0,
                x2: 110.0,
                y2: 110.0,
                score: 0.7,
                landmarks: vec![],
            },
        ];
        let kept = nms(&mut dets, 0.4);
        // First two overlap heavily, so only the higher-scoring one survives.
        // The third is far away and should survive.
        assert_eq!(kept.len(), 2);
        assert!((kept[0].score - 0.9).abs() < 1e-6);
        assert!((kept[1].score - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_iou_no_overlap() {
        let a = RawDetection {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 10.0,
            score: 1.0,
            landmarks: vec![],
        };
        let b = RawDetection {
            x1: 20.0,
            y1: 20.0,
            x2: 30.0,
            y2: 30.0,
            score: 1.0,
            landmarks: vec![],
        };
        assert!(iou(&a, &b) < 1e-6);
    }

    #[test]
    fn test_iou_full_overlap() {
        let a = RawDetection {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 10.0,
            score: 1.0,
            landmarks: vec![],
        };
        let iou_val = iou(&a, &a);
        assert!((iou_val - 1.0).abs() < 1e-6);
    }
}
