use anyhow::{Context, Result};
use hf_hub::api::sync::Api;
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use ndarray::Array4;
use ort::inputs;
use ort::session::Session;
use ort::value::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Max cosine distance for a face match (= 1.0 - cosine_similarity).
pub const MAX_FACE_DISTANCE: f32 = 0.6;

/// Minimum neighbors within distance for a face to be considered a "core" point.
pub const MIN_FACES_FOR_CORE: usize = 3;

/// ArcFace canonical reference landmarks for a 112x112 aligned face.
/// Order: left eye, right eye, nose tip, left mouth corner, right mouth corner.
const ARCFACE_REF_LANDMARKS: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

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

/// Estimate a 2D similarity transform (rotation + uniform scale + translation)
/// mapping `src` points to `dst` points via least-squares.
///
/// Returns the 2x3 affine matrix `[[a, -b, tx], [b, a, ty]]`.
fn estimate_similarity_transform(src: &[(f32, f32); 5], dst: &[(f32, f32); 5]) -> [f32; 6] {
    // Build the 10x4 linear system:
    //   For each point i:
    //     dst_x_i = a * src_x_i - b * src_y_i + tx
    //     dst_y_i = b * src_x_i + a * src_y_i + ty
    //
    // Parameters: [a, b, tx, ty]
    // We solve via normal equations: (A^T A) params = A^T rhs

    let mut ata = [0.0f64; 16]; // 4x4
    let mut atb = [0.0f64; 4]; // 4x1

    for i in 0..5 {
        let (sx, sy) = (src[i].0 as f64, src[i].1 as f64);
        let (dx, dy) = (dst[i].0 as f64, dst[i].1 as f64);

        // Row 1: [sx, -sy, 1, 0] -> dx
        let row1 = [sx, -sy, 1.0, 0.0];
        // Row 2: [sy,  sx, 0, 1] -> dy
        let row2 = [sy, sx, 0.0, 1.0];

        for r in 0..4 {
            for c in 0..4 {
                ata[r * 4 + c] += row1[r] * row1[c] + row2[r] * row2[c];
            }
            atb[r] += row1[r] * dx + row2[r] * dy;
        }
    }

    // Solve 4x4 system via Gaussian elimination with partial pivoting
    let mut aug = [[0.0f64; 5]; 4];
    for r in 0..4 {
        for c in 0..4 {
            aug[r][c] = ata[r * 4 + c];
        }
        aug[r][4] = atb[r];
    }

    for col in 0..4 {
        // Partial pivoting
        let mut max_row = col;
        let mut max_val = aug[col][col].abs();
        for row in (col + 1)..4 {
            if aug[row][col].abs() > max_val {
                max_val = aug[row][col].abs();
                max_row = row;
            }
        }
        aug.swap(col, max_row);

        let pivot = aug[col][col];
        if pivot.abs() < 1e-12 {
            // Degenerate — return identity
            return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        }

        for c in col..5 {
            aug[col][c] /= pivot;
        }
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = aug[row][col];
            for c in col..5 {
                aug[row][c] -= factor * aug[col][c];
            }
        }
    }

    let a = aug[0][4] as f32;
    let b = aug[1][4] as f32;
    let tx = aug[2][4] as f32;
    let ty = aug[3][4] as f32;

    // Return the 2x3 matrix: [[a, -b, tx], [b, a, ty]]
    [a, -b, tx, b, a, ty]
}

/// Apply an affine warp to produce a 112x112 aligned face image.
///
/// For each pixel (ox, oy) in the output, we compute the corresponding source
/// position using the *inverse* of the forward transform, then bilinear-interpolate.
fn affine_warp_face(img: &DynamicImage, forward: &[f32; 6]) -> RgbImage {
    let size = 112u32;
    let rgb = img.to_rgb8();
    let (src_w, src_h) = (rgb.width(), rgb.height());

    // Invert the 2x3 similarity matrix [[a, -b, tx], [b, a, ty]]
    let (a, mb, tx, b, a2, ty) = (
        forward[0], forward[1], forward[2], forward[3], forward[4], forward[5],
    );
    let det = a * a2 - mb * b;
    if det.abs() < 1e-10 {
        // Degenerate — just resize the image
        return img
            .resize_exact(size, size, image::imageops::FilterType::Triangle)
            .to_rgb8();
    }
    let inv_det = 1.0 / det;
    // Inverse: [[a2, -mb, mb*ty - a2*tx], [-b, a, b*tx - a*ty]] / det
    let ia = a2 * inv_det;
    let imb = -mb * inv_det;
    let itx = (mb * ty - a2 * tx) * inv_det;
    let ib = -b * inv_det;
    let ia2 = a * inv_det;
    let ity = (b * tx - a * ty) * inv_det;

    let mut out = RgbImage::new(size, size);

    for oy in 0..size {
        for ox in 0..size {
            let sx = ia * ox as f32 + imb * oy as f32 + itx;
            let sy = ib * ox as f32 + ia2 * oy as f32 + ity;

            // Bilinear interpolation
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let x1 = x0 + 1;
            let y1 = y0 + 1;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

            let sample = |x: i32, y: i32| -> [f32; 3] {
                if x < 0 || y < 0 || x >= src_w as i32 || y >= src_h as i32 {
                    [0.0, 0.0, 0.0]
                } else {
                    let p = rgb.get_pixel(x as u32, y as u32);
                    [p[0] as f32, p[1] as f32, p[2] as f32]
                }
            };

            let p00 = sample(x0, y0);
            let p10 = sample(x1, y0);
            let p01 = sample(x0, y1);
            let p11 = sample(x1, y1);

            let mut pixel = [0u8; 3];
            for c in 0..3 {
                let v = p00[c] * (1.0 - fx) * (1.0 - fy)
                    + p10[c] * fx * (1.0 - fy)
                    + p01[c] * (1.0 - fx) * fy
                    + p11[c] * fx * fy;
                pixel[c] = v.round().clamp(0.0, 255.0) as u8;
            }
            out.put_pixel(ox, oy, Rgb(pixel));
        }
    }

    out
}

/// Produce an aligned 112x112 face crop using the detected landmarks.
/// Falls back to a simple bbox crop + resize when landmarks are unavailable.
pub fn align_face(
    img: &DynamicImage,
    landmarks: Option<&[(f32, f32)]>,
    bbox: (f32, f32, f32, f32),
) -> RgbImage {
    if let Some(lms) = landmarks {
        if lms.len() == 5 {
            let src: [(f32, f32); 5] = [lms[0], lms[1], lms[2], lms[3], lms[4]];
            let transform = estimate_similarity_transform(&src, &ARCFACE_REF_LANDMARKS);
            return affine_warp_face(img, &transform);
        }
    }

    // Fallback: crop by bbox and resize
    let (img_w, img_h) = img.dimensions();
    let x1 = (bbox.0 as u32).min(img_w.saturating_sub(1));
    let y1 = (bbox.1 as u32).min(img_h.saturating_sub(1));
    let x2 = (bbox.2 as u32).min(img_w);
    let y2 = (bbox.3 as u32).min(img_h);
    let w = x2.saturating_sub(x1).max(1);
    let h = y2.saturating_sub(y1).max(1);
    let sub = img.crop_imm(x1, y1, w, h);
    sub.resize_exact(112, 112, image::imageops::FilterType::Triangle)
        .to_rgb8()
}

impl AiPipeline {
    pub fn new() -> Result<Self> {
        if std::env::var("PHOS_DUMMY_AI")
            .ok()
            .is_some_and(|v| v == "1")
        {
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
        if let (Some(detector_mutex), false) = (
            &self.face_detector,
            std::env::var("PHOS_DUMMY_AI")
                .ok()
                .is_some_and(|v| v == "1"),
        ) {
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

            let det_input_name = session.inputs()[0].name().to_string();
            let outputs = session.run(inputs![det_input_name.as_str() => input_tensor])?;

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
                // SCRFD outputs can be in three formats:
                // Format A (insightface 3D): [1, num_anchors*H*W, 1] for scores — n = shape[1]
                // Format A (insightface 2D): [num_anchors*H*W, 1] for scores — n = shape[0]
                // Format B (standard 4D):    [1, num_anchors, H, W] for scores
                let is_flat = score_shape.len() <= 3;

                if is_flat {
                    // Format A: [N, 1] or [1, N, 1] scores, same pattern for bbox/kps
                    let n = if score_shape.len() == 3 {
                        score_shape[1] as usize
                    } else if score_shape.len() == 2 {
                        score_shape[0] as usize
                    } else {
                        score_data.len()
                    };
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
                // Scores have last dim 1, bboxes have last dim 4, kps have last dim 10
                if tensor_data.len() == 9 {
                    // Determine if grouped by type or by stride using last dimension:
                    // Type-grouped: [score_s8, score_s16, score_s32, bbox_s8, ..., kps_s8, ...]
                    //   → first 3 outputs all have last dim = 1
                    // Stride-grouped: [score_s8, bbox_s8, kps_s8, score_s16, ...]
                    //   → output[1] has last dim = 4 (bbox)
                    let first_three_are_scores = tensor_data[0..3]
                        .iter()
                        .all(|(dims, _)| dims.last() == Some(&1));

                    let (score_indices, bbox_indices, kps_indices) = if first_three_are_scores {
                        // Type-grouped: [scores..., bboxes..., kps...]
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

                        let is_flat = score_shape.len() <= 3;

                        if is_flat {
                            let n = if score_shape.len() == 3 {
                                score_shape[1] as usize
                            } else if score_shape.len() == 2 {
                                score_shape[0] as usize
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

    /// Extract a face embedding from a full image using landmark-based alignment.
    ///
    /// When landmarks are available, the face is aligned to the ArcFace canonical
    /// position via a similarity transform before embedding extraction.
    /// Falls back to bbox crop + resize when landmarks are unavailable.
    pub fn extract_embedding(
        &self,
        img: &DynamicImage,
        landmarks: Option<&[(f32, f32)]>,
        bbox: (f32, f32, f32, f32),
    ) -> Result<Vec<f32>> {
        if let (Some(recognizer_mutex), false) = (
            &self.face_recognizer,
            std::env::var("PHOS_DUMMY_AI")
                .ok()
                .is_some_and(|v| v == "1"),
        ) {
            let rgb_img = align_face(img, landmarks, bbox);

            let mut input = Array4::<f32>::zeros((1, 3, 112, 112));
            for (x, y, rgb) in rgb_img.enumerate_pixels() {
                input[[0, 0, y as usize, x as usize]] = (rgb[0] as f32 - 127.5) / 127.5;
                input[[0, 1, y as usize, x as usize]] = (rgb[1] as f32 - 127.5) / 127.5;
                input[[0, 2, y as usize, x as usize]] = (rgb[2] as f32 - 127.5) / 127.5;
            }

            let input_tensor = Value::from_array((vec![1, 3, 112, 112], input.into_raw_vec()))?;
            let mut session = recognizer_mutex
                .lock()
                .map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
            let input_name = session.inputs()[0].name().to_string();
            let outputs = session.run(inputs![input_name.as_str() => input_tensor])?;
            let output_tensor = outputs[0].try_extract_tensor::<f32>()?;

            let embedding: Vec<f32> = output_tensor.1.to_vec();
            let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            Ok(embedding.into_iter().map(|x| x / (norm + 1e-10)).collect())
        } else {
            // Dummy mode: deterministic embedding based on bbox position
            let mut emb = vec![0.1; 512];
            emb[0] = bbox.0 / 1000.0;
            emb[1] = bbox.1 / 1000.0;
            Ok(emb)
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
    fn test_similarity_transform_identity() {
        // When src == dst, the transform should be close to identity
        let pts: [(f32, f32); 5] = ARCFACE_REF_LANDMARKS;
        let m = estimate_similarity_transform(&pts, &pts);
        // m should be [1, 0, 0, 0, 1, 0] (identity)
        assert!((m[0] - 1.0).abs() < 1e-4, "a should be ~1, got {}", m[0]);
        assert!(m[1].abs() < 1e-4, "-b should be ~0, got {}", m[1]);
        assert!(m[2].abs() < 1e-4, "tx should be ~0, got {}", m[2]);
        assert!(m[3].abs() < 1e-4, "b should be ~0, got {}", m[3]);
        assert!((m[4] - 1.0).abs() < 1e-4, "a should be ~1, got {}", m[4]);
        assert!(m[5].abs() < 1e-4, "ty should be ~0, got {}", m[5]);
    }

    #[test]
    fn test_similarity_transform_translation() {
        // Translate all points by (10, 20)
        let dst = ARCFACE_REF_LANDMARKS;
        let src: [(f32, f32); 5] = dst.map(|(x, y)| (x - 10.0, y - 20.0));
        let m = estimate_similarity_transform(&src, &dst);
        // Should be [1, 0, 10, 0, 1, 20]
        assert!((m[0] - 1.0).abs() < 1e-3);
        assert!(m[1].abs() < 1e-3);
        assert!((m[2] - 10.0).abs() < 1e-3);
        assert!(m[3].abs() < 1e-3);
        assert!((m[4] - 1.0).abs() < 1e-3);
        assert!((m[5] - 20.0).abs() < 1e-3);
    }

    #[test]
    fn test_align_face_no_landmarks_fallback() {
        // Without landmarks, should produce a 112x112 image from bbox crop
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(200, 200, |_, _| {
            image::Rgb([128, 128, 128])
        }));
        let result = align_face(&img, None, (10.0, 10.0, 100.0, 100.0));
        assert_eq!(result.width(), 112);
        assert_eq!(result.height(), 112);
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
