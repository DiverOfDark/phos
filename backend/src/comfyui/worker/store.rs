//! Bringing a finished file into the library.
//!
//! Downloads what ComfyUI named and saves it beside the shot's original as a
//! non-original variant. Re-running the same task is safe: an identical file
//! already on disk is recognised by hash and reused, and a *different* file at
//! the expected path gets its own suffixed name rather than overwriting
//! something the user may already have looked at.

use super::complete::ActiveTask;
use super::status::live_task;
use crate::comfyui::client::ComfyUiClient;
use crate::comfyui::outputs::OutputRef;
use crate::db;
use crate::models::NewFile;
use crate::schema::{enhancement_tasks, files};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing::info;
use uuid::Uuid;

/// Download an output file from ComfyUI and save it alongside the original.
pub(super) fn download_and_save_output(
    conn: &mut SqliteConnection,
    client: &ComfyUiClient,
    task: &ActiveTask,
    out: &OutputRef,
    library_root: &Path,
) -> anyhow::Result<()> {
    let data = client.download_output(out)?;
    let file_id = store_output_file(conn, task, out, &data, library_root)?;

    // Store the output file ID on the task
    diesel::update(live_task(&task.id))
        .set(enhancement_tasks::output_file_id.eq(&file_id))
        .execute(conn)?;

    Ok(())
}

/// Write the bytes next to the shot's original and register them as a file row.
/// Returns the id of the file the task should point at.
fn store_output_file(
    conn: &mut SqliteConnection,
    task: &ActiveTask,
    out: &OutputRef,
    data: &[u8],
    library_root: &Path,
) -> anyhow::Result<String> {
    let shot_id = task.shot_id.as_str();

    // Get the original file path to determine where to save
    let original_path_str: String = files::table
        .filter(files::shot_id.eq(shot_id).and(files::is_original.eq(true)))
        .select(files::path)
        .first::<String>(conn)
        .map_err(|_| anyhow::anyhow!("No original file found for shot {}", shot_id))?;

    let original = db::resolve_path(library_root, &original_path_str);
    let parent = original
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No parent directory"))?;
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    // Determine extension from the downloaded filename
    let ext = Path::new(&out.filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png");

    // Compute hash before writing to disk so we can check for duplicates
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hex::encode(hasher.finalize());

    let task_short = &task.id[..8.min(task.id.len())];
    let base_output_path = parent.join(format!("{}_enhanced_{}.{}", stem, task_short, ext));
    let base_path_str = db::make_relative(library_root, &base_output_path);

    // Check if a file with the expected path already exists in the DB (from a previous attempt)
    let existing: Option<(String, String)> = files::table
        .filter(files::path.eq(&base_path_str))
        .select((files::id, files::hash))
        .first::<(String, String)>(conn)
        .ok();

    if let Some((existing_id, existing_hash)) = &existing {
        if *existing_hash == hash {
            // Same content already saved — nothing to do
            info!(
                "Task {} output already exists with same hash, skipping write",
                task.id
            );
            return Ok(existing_id.clone());
        }
    }

    let path_taken = existing.is_some();
    let target = if path_taken {
        // Path is taken but content differs — save as a new variant with a unique suffix
        let unique = &Uuid::new_v4().to_string()[..8];
        parent.join(format!(
            "{}_enhanced_{}_{}.{}",
            stem, task_short, unique, ext
        ))
    } else {
        base_output_path
    };

    std::fs::write(&target, data)?;
    if path_taken {
        info!("Saved enhanced output (new variant) to {:?}", target);
    } else {
        info!("Saved enhanced output to {:?}", target);
    }

    let path_str = db::make_relative(library_root, &target);
    let file_id = Uuid::new_v4().to_string();
    diesel::insert_into(files::table)
        .values(NewFile {
            id: &file_id,
            shot_id,
            path: &path_str,
            hash: &hash,
            mime_type: Some(mime_for(ext)),
            file_size: Some(data.len() as i32),
            is_original: Some(false),
            visual_embedding: None,
            source_workflow_id: Some(&task.workflow_id),
            source_text_overrides: Some(&task.text_overrides),
        })
        .execute(conn)?;
    Ok(file_id)
}

/// Guess mime type from extension.
fn mime_for(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_the_mime_types_the_library_expects() {
        assert_eq!(mime_for("png"), "image/png");
        assert_eq!(mime_for("jpeg"), "image/jpeg");
        assert_eq!(mime_for("mp4"), "video/mp4");
        assert_eq!(mime_for("exr"), "application/octet-stream");
    }
}
