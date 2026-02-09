use ort::session::Session;
use ort::inputs;
use ort::value::Value;
use image::{DynamicImage, GenericImageView};
use ndarray::{Array4};
use anyhow::{Result};
use std::path::Path;
use std::sync::Mutex;

pub struct AiPipeline {
    face_detector: Option<Mutex<Session>>,
    face_recognizer: Option<Mutex<Session>>,
}

#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub box_x1: f32,
    pub box_y1: f32,
    pub box_x2: f32,
    pub box_y2: f32,
    pub score: f32,
    pub landmarks: Option<Vec<(f32, f32)>>,
}

impl AiPipeline {
    pub fn new(model_dir: &Path) -> Result<Self> {
        if std::env::var("PHOS_DUMMY_AI").is_ok() {
            return Ok(Self {
                face_detector: None,
                face_recognizer: None,
            });
        }

        let face_detector = Session::builder()?
            .commit_from_file(model_dir.join("det_10g.onnx"))?; 
            
        let face_recognizer = Session::builder()?
            .commit_from_file(model_dir.join("w600k_r50.onnx"))?; 

        Ok(Self {
            face_detector: Some(Mutex::new(face_detector)),
            face_recognizer: Some(Mutex::new(face_recognizer)),
        })
    }

    pub fn detect_faces(&self, img: &DynamicImage) -> Result<Vec<FaceDetection>> {
        if let (Some(detector_mutex), false) = (&self.face_detector, std::env::var("PHOS_DUMMY_AI").is_ok()) {
            let (width, height) = img.dimensions();
            let target_size = 640;
            let resized = img.resize_exact(target_size, target_size, image::imageops::FilterType::Triangle);
            let rgb_img = resized.to_rgb8();
            
            let mut input = Array4::<f32>::zeros((1, 3, target_size as usize, target_size as usize));
            for (x, y, rgb) in rgb_img.enumerate_pixels() {
                input[[0, 0, y as usize, x as usize]] = (rgb[0] as f32 - 127.5) / 128.0;
                input[[0, 1, y as usize, x as usize]] = (rgb[1] as f32 - 127.5) / 128.0;
                input[[0, 2, y as usize, x as usize]] = (rgb[2] as f32 - 127.5) / 128.0;
            }

            let input_tensor = Value::from_array((vec![1, 3, target_size as usize, target_size as usize], input.into_raw_vec()))?;
            let mut session = detector_mutex.lock().map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
            let _outputs = session.run(inputs!["input.1" => input_tensor])?;
            
            // Note: Real parsing logic for SCRFD would go here.
            // For now, returning empty to avoid further complexity in this walkthrough.
            Ok(vec![])
        } else {
            // Dummy mode logic
            let (width, height) = img.dimensions();
            Ok(vec![FaceDetection {
                box_x1: 10.0, box_y1: 10.0, box_x2: (width as f32).min(110.0), box_y2: (height as f32).min(110.0),
                score: 0.99,
                landmarks: None
            }])
        }
    }

    pub fn extract_embedding(&self, face_img: &DynamicImage) -> Result<Vec<f32>> {
        if let (Some(recognizer_mutex), false) = (&self.face_recognizer, std::env::var("PHOS_DUMMY_AI").is_ok()) {
            let resized = face_img.resize_exact(112, 112, image::imageops::FilterType::Triangle);
            let rgb_img = resized.to_rgb8();
            
            let mut input = Array4::<f32>::zeros((1, 3, 112, 112));
            for (x, y, rgb) in rgb_img.enumerate_pixels() {
                input[[0, 0, y as usize, x as usize]] = (rgb[0] as f32 - 127.5) / 128.0;
                input[[0, 1, y as usize, x as usize]] = (rgb[1] as f32 - 127.5) / 128.0;
                input[[0, 2, y as usize, x as usize]] = (rgb[2] as f32 - 127.5) / 128.0;
            }

            let input_tensor = Value::from_array((vec![1, 3, 112, 112], input.into_raw_vec()))?;
            let mut session = recognizer_mutex.lock().map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
            let outputs = session.run(inputs!["data" => input_tensor])?;
            let output_tensor = outputs[0].try_extract_tensor::<f32>()?;
            
            let embedding: Vec<f32> = output_tensor.1.iter().cloned().collect();
            let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            Ok(embedding.into_iter().map(|x| x / (norm + 1e-10)).collect())
        } else {
            // Dummy mode: random embedding
            Ok(vec![0.1; 512])
        }
    }
}
