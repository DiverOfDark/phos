//! Getting the pixels that go into a run.
//!
//! The shot's original, or a specific variant of it when the user picked one;
//! a video contributes its first frame. Always handed to ComfyUI as PNG.

use crate::db;
use crate::scanner;
use crate::schema::files;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use image::DynamicImage;
use std::path::Path;

/// Get the source image bytes (PNG-encoded) for a shot.
/// If `source_file_id` is provided, uses that specific file; otherwise falls back to the original.
/// For images: reads the file directly.
/// For videos: extracts the first frame.
pub(crate) fn get_source_image(
    conn: &mut SqliteConnection,
    shot_id: &str,
    source_file_id: Option<&str>,
    library_root: &Path,
) -> anyhow::Result<(Vec<u8>, String)> {
    // If a specific source file is requested, use it; otherwise fall back to the original
    let (file_id_used, file_path, mime_type): (String, String, String) =
        if let Some(file_id) = source_file_id {
            let (fp, mt) = files::table
                .filter(files::id.eq(file_id).and(files::shot_id.eq(shot_id)))
                .select((
                    files::path,
                    diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')"),
                ))
                .first::<(String, String)>(conn)
                .map_err(|_| {
                    anyhow::anyhow!("Source file {} not found for shot {}", file_id, shot_id)
                })?;
            (file_id.to_string(), fp, mt)
        } else {
            let (fid, fp, mt) = files::table
                .filter(files::shot_id.eq(shot_id).and(files::is_original.eq(true)))
                .order(files::created_at.asc())
                .select((
                    files::id,
                    files::path,
                    diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')"),
                ))
                .first::<(String, String, String)>(conn)
                .map_err(|_| anyhow::anyhow!("No original file found for shot {}", shot_id))?;
            (fid, fp, mt)
        };

    let path = db::resolve_path(library_root, &file_path);
    if !path.exists() {
        anyhow::bail!("Source file does not exist: {}", file_path);
    }

    let img: DynamicImage = if mime_type.starts_with("video/") {
        scanner::extract_first_video_frame(&path)?
    } else {
        scanner::open_image(&path)?
    };

    // Encode to PNG bytes
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    img.write_to(&mut cursor, image::ImageFormat::Png)?;

    // Include file ID in the upload name so ComfyUI doesn't reuse a cached image from a different variant
    let upload_name = format!(
        "phos_{}_{}.png",
        &shot_id[..8.min(shot_id.len())],
        &file_id_used[..8.min(file_id_used.len())]
    );
    Ok((buf, upload_name))
}
