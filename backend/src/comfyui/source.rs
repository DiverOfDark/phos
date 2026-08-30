//! The pixels — or the clip — that go into a run.
//!
//! The shot's original, or a specific variant of it when the user picked one.
//! A still always goes up as itself. A video goes up as whichever of these the
//! task asked for:
//!
//! * a **frame** of it, PNG-encoded — the first (what Phos always did), the
//!   last (which is what extending a clip needs), one at an arbitrary
//!   timestamp, or one of the keyframes the scanner already indexed;
//! * or the **file itself**, when the workflow has a video loader to read it.
//!
//! Choosing between them used to not be a choice: `get_source_image` returned
//! frame zero and nothing else, which is what made video→video impossible.

use crate::db;
use crate::scanner::{self, FrameTarget};
use crate::schema::{files, video_keyframes};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Which part of the source a run consumes.
///
/// Serialised into `enhancement_tasks.source_mode` as the strings below, so the
/// column reads as English in a `sqlite3` session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    /// Frame zero. The historical behaviour, and what a still always uses.
    FirstFrame,
    /// The final frame — the one an extension continues from.
    LastFrame,
    /// The frame covering this position, in milliseconds.
    AtTime(i64),
    /// The n-th row in `video_keyframes` for this file, zero-based.
    Keyframe(i64),
    /// The video file itself, uploaded unchanged.
    WholeVideo,
}

impl std::fmt::Display for SourceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceMode::FirstFrame => write!(f, "first_frame"),
            SourceMode::LastFrame => write!(f, "last_frame"),
            SourceMode::AtTime(ms) => write!(f, "at_time:{}", ms),
            SourceMode::Keyframe(n) => write!(f, "keyframe:{}", n),
            SourceMode::WholeVideo => write!(f, "whole_video"),
        }
    }
}

impl FromStr for SourceMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        match s.split_once(':') {
            Some(("at_time", n)) => n
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|ms| *ms >= 0)
                .map(SourceMode::AtTime)
                .ok_or_else(|| format!("at_time needs a millisecond count, got {:?}", n)),
            Some(("keyframe", n)) => n
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|i| *i >= 0)
                .map(SourceMode::Keyframe)
                .ok_or_else(|| format!("keyframe needs a zero-based index, got {:?}", n)),
            _ => match s {
                "first_frame" => Ok(SourceMode::FirstFrame),
                "last_frame" => Ok(SourceMode::LastFrame),
                "whole_video" => Ok(SourceMode::WholeVideo),
                other => Err(format!("unknown source mode {:?}", other)),
            },
        }
    }
}

impl SourceMode {
    /// What the run actually does, given what the task asked for and what it
    /// has to work with.
    ///
    /// The default is the interesting part: a graph with a video loader wants
    /// the clip, so `whole_video`; anything else gets frame zero, which is what
    /// every existing task already assumes.
    pub fn resolve(stored: Option<&str>, graph_takes_video: bool, source_is_video: bool) -> Self {
        if !source_is_video {
            // There is no frame two of a JPEG.
            return SourceMode::FirstFrame;
        }
        match stored.map(str::parse::<SourceMode>) {
            Some(Ok(mode)) => mode,
            _ if graph_takes_video => SourceMode::WholeVideo,
            _ => SourceMode::FirstFrame,
        }
    }

    /// A short tag for the upload filename, so two runs over one file with
    /// different modes never collide in ComfyUI's input directory.
    fn slug(&self) -> String {
        match self {
            SourceMode::FirstFrame => "first".to_string(),
            SourceMode::LastFrame => "last".to_string(),
            SourceMode::AtTime(ms) => format!("t{}", ms),
            SourceMode::Keyframe(n) => format!("k{}", n),
            SourceMode::WholeVideo => "video".to_string(),
        }
    }
}

/// The library file a run reads from.
pub(crate) struct SourceFile {
    pub file_id: String,
    pub shot_id: String,
    pub path: PathBuf,
    pub mime_type: String,
}

impl SourceFile {
    pub fn is_video(&self) -> bool {
        self.mime_type.starts_with("video/")
    }
}

/// What gets pushed to ComfyUI for one run.
pub(crate) struct SourceUpload {
    pub bytes: Vec<u8>,
    /// The name to store it under in ComfyUI's input directory.
    pub filename: String,
    /// The `Content-Type` of the multipart part. `/upload/image` takes video
    /// files happily — the VHS loaders read them out of the same directory —
    /// but only if it is not told they are PNGs.
    pub content_type: String,
}

/// Find the file a task reads from: the one it named, or the shot's original.
pub(crate) fn resolve_source_file(
    conn: &mut SqliteConnection,
    shot_id: &str,
    source_file_id: Option<&str>,
    library_root: &Path,
) -> anyhow::Result<SourceFile> {
    let mime_sql = diesel::dsl::sql::<diesel::sql_types::Text>("COALESCE(mime_type, '')");
    let (file_id, file_path, mime_type): (String, String, String) =
        if let Some(file_id) = source_file_id {
            let (fp, mt) = files::table
                .filter(files::id.eq(file_id).and(files::shot_id.eq(shot_id)))
                .select((files::path, mime_sql))
                .first::<(String, String)>(conn)
                .map_err(|_| {
                    anyhow::anyhow!("Source file {} not found for shot {}", file_id, shot_id)
                })?;
            (file_id.to_string(), fp, mt)
        } else {
            files::table
                .filter(files::shot_id.eq(shot_id).and(files::is_original.eq(true)))
                .order(files::created_at.asc())
                .select((files::id, files::path, mime_sql))
                .first::<(String, String, String)>(conn)
                .map_err(|_| anyhow::anyhow!("No original file found for shot {}", shot_id))?
        };

    let path = db::resolve_path(library_root, &file_path);
    if !path.exists() {
        anyhow::bail!("Source file does not exist: {}", file_path);
    }
    Ok(SourceFile {
        file_id,
        shot_id: shot_id.to_string(),
        path,
        mime_type,
    })
}

/// Read the source in the shape `mode` asks for.
pub(crate) fn read_source(
    conn: &mut SqliteConnection,
    source: &SourceFile,
    mode: SourceMode,
) -> anyhow::Result<SourceUpload> {
    if mode == SourceMode::WholeVideo && source.is_video() {
        let bytes = std::fs::read(&source.path)?;
        let ext = extension_for(&source.mime_type, &source.path);
        return Ok(SourceUpload {
            filename: upload_name(source, mode, &ext),
            content_type: content_type_for(&source.mime_type, &ext),
            bytes,
        });
    }

    let img = if source.is_video() {
        scanner::extract_video_frame(&source.path, frame_target(conn, source, mode)?)?
    } else {
        scanner::open_image(&source.path)?
    };

    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    img.write_to(&mut cursor, image::ImageFormat::Png)?;
    Ok(SourceUpload {
        filename: upload_name(source, mode, "png"),
        content_type: "image/png".to_string(),
        bytes,
    })
}

/// Turn a mode into a position in the video.
fn frame_target(
    conn: &mut SqliteConnection,
    source: &SourceFile,
    mode: SourceMode,
) -> anyhow::Result<FrameTarget> {
    Ok(match mode {
        SourceMode::LastFrame => FrameTarget::Last,
        SourceMode::AtTime(ms) => FrameTarget::AtMs(ms),
        SourceMode::Keyframe(n) => FrameTarget::AtMs(keyframe_timestamp(conn, source, n)?),
        // `WholeVideo` only reaches here for a still, where frame zero is the
        // whole thing anyway.
        SourceMode::FirstFrame | SourceMode::WholeVideo => FrameTarget::First,
    })
}

/// The timestamp of the n-th keyframe the scanner indexed for this file.
///
/// An index past the end fails the task rather than quietly substituting a
/// different frame: the user asked for a specific moment, and silently handing
/// back a different one is worse than saying so.
fn keyframe_timestamp(
    conn: &mut SqliteConnection,
    source: &SourceFile,
    n: i64,
) -> anyhow::Result<i64> {
    let timestamps: Vec<Option<i32>> = video_keyframes::table
        .filter(video_keyframes::video_file_id.eq(&source.file_id))
        .order(video_keyframes::timestamp_ms.asc())
        .select(video_keyframes::timestamp_ms)
        .load(conn)?;
    let found = usize::try_from(n).ok().and_then(|i| timestamps.get(i));
    match found {
        Some(ts) => Ok(i64::from(ts.unwrap_or(0))),
        None => anyhow::bail!(
            "Keyframe {} was asked for but file {} has {} indexed keyframe(s)",
            n,
            source.file_id,
            timestamps.len()
        ),
    }
}

/// `phos_<shot>_<file>_<mode>.<ext>`.
///
/// The file id keeps two variants of one shot apart; the mode keeps two runs
/// over one variant apart, so asking for the last frame never gets served the
/// first one out of ComfyUI's input directory.
fn upload_name(source: &SourceFile, mode: SourceMode, ext: &str) -> String {
    format!(
        "phos_{}_{}_{}.{}",
        short(&source.shot_id),
        short(&source.file_id),
        mode.slug(),
        ext
    )
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

/// The extension ComfyUI should see. VHS lists the input directory by
/// extension, so a clip stored as `.png` is a clip it will never offer.
fn extension_for(mime_type: &str, path: &Path) -> String {
    let from_mime = match mime_type {
        "video/mp4" => Some("mp4"),
        "video/quicktime" => Some("mov"),
        "video/x-matroska" => Some("mkv"),
        "video/x-msvideo" => Some("avi"),
        "video/webm" => Some("webm"),
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        _ => None,
    };
    from_mime.map(str::to_string).unwrap_or_else(|| {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or_else(|| "bin".to_string())
    })
}

/// Prefer the mime type the scanner recorded; fall back to guessing from the
/// extension, because a file imported before that column existed has none.
fn content_type_for(mime_type: &str, ext: &str) -> String {
    if !mime_type.is_empty() && mime_type != "application/octet-stream" {
        return mime_type.to_string();
    }
    mime_guess::from_ext(ext)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(mime: &str, path: &str) -> SourceFile {
        SourceFile {
            file_id: "ffffffffffff".to_string(),
            shot_id: "ssssssssssss".to_string(),
            path: PathBuf::from(path),
            mime_type: mime.to_string(),
        }
    }

    #[test]
    fn every_mode_round_trips_through_the_column() {
        for mode in [
            SourceMode::FirstFrame,
            SourceMode::LastFrame,
            SourceMode::AtTime(0),
            SourceMode::AtTime(12_345),
            SourceMode::Keyframe(0),
            SourceMode::Keyframe(7),
            SourceMode::WholeVideo,
        ] {
            let stored = mode.to_string();
            assert_eq!(stored.parse::<SourceMode>().unwrap(), mode, "{}", stored);
        }
    }

    #[test]
    fn nonsense_in_the_column_is_rejected_rather_than_guessed_at() {
        for bad in [
            "",
            "middle",
            "at_time",
            "at_time:",
            "at_time:soon",
            "at_time:-5",
            "keyframe:-1",
            "keyframe:two",
        ] {
            assert!(bad.parse::<SourceMode>().is_err(), "{:?} was accepted", bad);
        }
    }

    #[test]
    fn a_still_is_always_its_own_first_frame() {
        // Whatever the task says, there is no last frame of a JPEG.
        for stored in [None, Some("last_frame"), Some("whole_video")] {
            assert_eq!(
                SourceMode::resolve(stored, true, false),
                SourceMode::FirstFrame
            );
        }
    }

    #[test]
    fn a_video_workflow_defaults_to_taking_the_whole_clip() {
        assert_eq!(
            SourceMode::resolve(None, true, true),
            SourceMode::WholeVideo
        );
    }

    #[test]
    fn an_image_workflow_still_defaults_to_frame_zero() {
        // The existing behaviour, and every task queued before this column
        // existed has a NULL here.
        assert_eq!(
            SourceMode::resolve(None, false, true),
            SourceMode::FirstFrame
        );
    }

    #[test]
    fn an_explicit_mode_beats_the_default_in_both_directions() {
        assert_eq!(
            SourceMode::resolve(Some("last_frame"), true, true),
            SourceMode::LastFrame
        );
        assert_eq!(
            SourceMode::resolve(Some("whole_video"), false, true),
            SourceMode::WholeVideo
        );
    }

    #[test]
    fn an_unreadable_mode_falls_back_rather_than_failing_the_task() {
        assert_eq!(
            SourceMode::resolve(Some("nonsense"), true, true),
            SourceMode::WholeVideo
        );
    }

    #[test]
    fn a_clip_keeps_its_own_extension_and_content_type() {
        let mp4 = video("video/mp4", "/lib/a.mp4");
        assert_eq!(extension_for(&mp4.mime_type, &mp4.path), "mp4");
        assert_eq!(content_type_for(&mp4.mime_type, "mp4"), "video/mp4");

        let mov = video("video/quicktime", "/lib/a.MOV");
        assert_eq!(extension_for(&mov.mime_type, &mov.path), "mov");
    }

    #[test]
    fn a_file_the_scanner_could_not_type_falls_back_to_its_extension() {
        let unknown = video("application/octet-stream", "/lib/clip.MKV");
        assert_eq!(extension_for(&unknown.mime_type, &unknown.path), "mkv");
        assert_eq!(
            content_type_for(&unknown.mime_type, "mkv"),
            "video/x-matroska"
        );

        let nameless = video("", "/lib/clip");
        assert_eq!(extension_for(&nameless.mime_type, &nameless.path), "bin");
        assert_eq!(
            content_type_for(&nameless.mime_type, "bin"),
            "application/octet-stream"
        );
    }

    #[test]
    fn two_modes_over_one_file_do_not_collide_in_comfyuis_input_directory() {
        let src = video("video/mp4", "/lib/a.mp4");
        let first = upload_name(&src, SourceMode::FirstFrame, "png");
        let last = upload_name(&src, SourceMode::LastFrame, "png");
        let whole = upload_name(&src, SourceMode::WholeVideo, "mp4");
        assert_ne!(first, last);
        assert_ne!(last, whole);
        assert!(whole.ends_with(".mp4"), "{}", whole);
        assert!(first.starts_with("phos_ssssssss_ffffffff_"), "{}", first);
        assert_eq!(
            upload_name(&src, SourceMode::AtTime(2500), "png"),
            "phos_ssssssss_ffffffff_t2500.png"
        );
    }

    #[test]
    fn a_short_id_does_not_slice_out_of_bounds() {
        let src = SourceFile {
            file_id: "ab".to_string(),
            shot_id: "cd".to_string(),
            path: PathBuf::from("/lib/a.png"),
            mime_type: "image/png".to_string(),
        };
        assert_eq!(
            upload_name(&src, SourceMode::FirstFrame, "png"),
            "phos_cd_ab_first.png"
        );
    }
}
