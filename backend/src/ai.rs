use ort::{Session, Value};
use image::{DynamicImage, GenericImageView};
use ndarray::{Array4, Axis};
use anyhow::Result;
use std::path::Path;

pub struct AiPipeline {
    face_detector: Session,
    face_recognizer: Session,
}

impl AiPipeline {
    pub fn new(model_dir: &Path) -> Result<Self> {
        // We assume models are downloaded/placed in model_dir
        // For detection: yolov8n-face.onnx or similar
        // For recognition: buffalo_l (insightface) or similar
        
        let face_detector = Session::builder()?
            .commit_from_file(model_dir.join("det_10g.onnx"))?; // SCRFD model
            
        let face_recognizer = Session::builder()?
            .commit_from_file(model_dir.join("w600k_r50.onnx"))?; // ArcFace model

        Ok(Self {
            face_detector,
            face_recognizer,
        })
    }

    pub fn detect_faces(&self, img: &DynamicImage) -> Result<Vec<FaceDetection>> {
        // Preprocess image to 640x640 for SCRFD
        let (width, height) = img.dimensions();
        let resized = img.resize_exact(640, 640, image::imageops::FilterType::Triangle);
        let rgb_img = resized.to_rgb8();
        
        let mut input = Array4::<f32>::zeros((1, 3, 640, 640));
        for (x, y, rgb) in rgb_img.enumerate_pixels() {
            input[[0, 0, y as usize, x as usize]] = (rgb[0] as f32 - 127.5) / 128.0;
            input[[0, 1, y as usize, x as usize]] = (rgb[1] as f32 - 127.5) / 128.0;
            input[[0, 2, y as usize, x as usize]] = (rgb[2] as f32 - 127.5) / 128.0;
        }

        let outputs = self.face_detector.run(ort::inputs!["input.1" => input.view()]?)?;
        
        // This is a placeholder for actual SCRFD output decoding
        // In reality, SCRFD returns multiple tensors (scores, boxes, kps) across scales
        
        Ok(vec![])
    }

    pub fn extract_embedding(&self, face_img: &DynamicImage) -> Result<Vec<f32>> {
        let resized = face_img.resize_exact(112, 112, image::imageops::FilterType::Triangle);
        let rgb_img = resized.to_rgb8();
        
        let mut input = Array4::<f32>::zeros((1, 3, 112, 112));
        for (x, y, rgb) in rgb_img.enumerate_pixels() {
            input[[0, 0, y as usize, x as usize]] = (rgb[0] as f32 - 127.5) / 128.0;
            input[[0, 1, y as usize, x as usize]] = (rgb[1] as f32 - 127.5) / 128.0;
            input[[0, 2, y as usize, x as usize]] = (rgb[2] as f32 - 127.5) / 128.0;
        }

        let outputs = self.face_recognizer.run(ort::inputs!["data" => input.view()]?)?;
        let output_tensor = outputs[0].try_extract_tensor::<f32>()?;
        
        Ok(output_tensor.to_owned().into_raw_vec())
    }
}

pub struct FaceDetection {
    pub box_x1: f32,
    pub box_y1: f32,
    pub box_x2: f32,
    pub box_y2: f32,
    pub score: f32,
}
