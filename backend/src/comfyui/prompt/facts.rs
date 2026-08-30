//! Reading what Phos knows about a shot, and remembering what a describe stage
//! said about it.
//!
//! The database half of the compiler, kept apart from [`super`] for the reason
//! the rest of this module directory is: the wording, the parsing and the
//! binding are decisions, and decisions that need a `SqliteConnection` to
//! exercise are decisions nobody exercises.

use super::ShotFacts;
use crate::schema::{faces, files, people, shots};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use serde::{Deserialize, Serialize};

/// Everything the library holds about one photograph that a vision model
/// cannot see for itself.
///
/// Missing rows are not an error — a shot with no faces, no EXIF and no caption
/// is a perfectly ordinary shot, and the instruction simply says less about it.
pub(crate) fn shot_facts(conn: &mut SqliteConnection, shot_id: &str) -> ShotFacts {
    /// Capture time, latitude, longitude, caption, and who the shot is filed
    /// under — every one of them optional, because every one of them is.
    type ShotRow = (
        Option<String>,
        Option<f32>,
        Option<f32>,
        Option<String>,
        Option<String>,
    );
    let row: Option<ShotRow> = shots::table
        .filter(shots::id.eq(shot_id))
        .select((
            shots::timestamp,
            shots::latitude,
            shots::longitude,
            shots::description,
            shots::primary_person_id,
        ))
        .first(conn)
        .optional()
        .unwrap_or(None);

    let Some((taken_at, lat, lon, caption, primary_person_id)) = row else {
        return ShotFacts::default();
    };

    ShotFacts {
        people: people_in_shot(conn, shot_id, primary_person_id.as_deref()),
        taken_at: non_empty(taken_at),
        place: lat.zip(lon),
        caption: non_empty(caption),
    }
}

/// The named people whose faces are on this shot, the primary person first.
///
/// Unnamed clusters are left out on purpose. "person 4f2a91" tells a model
/// nothing it could use, and telling it there is *an* unnamed person invites it
/// to invent one.
fn people_in_shot(
    conn: &mut SqliteConnection,
    shot_id: &str,
    primary_person_id: Option<&str>,
) -> Vec<String> {
    let rows: Vec<(String, Option<String>)> = faces::table
        .inner_join(files::table.on(files::id.eq(faces::file_id)))
        .inner_join(people::table.on(people::id.nullable().eq(faces::person_id)))
        .filter(files::shot_id.eq(shot_id))
        .select((people::id, people::name))
        .load(conn)
        .unwrap_or_default();

    let mut named: Vec<(String, String)> = Vec::new();
    for (person_id, name) in rows {
        let Some(name) = non_empty(name) else {
            continue;
        };
        if named.iter().any(|(id, _)| id == &person_id) {
            continue;
        }
        named.push((person_id, name));
    }
    // The primary person is the one the shot is filed under, so they lead.
    named.sort_by_key(|(id, _)| Some(id.as_str()) != primary_person_id);
    named.into_iter().map(|(_, name)| name).collect()
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ===== The per-shot cache ==================================================

/// What a describe stage said about a shot, and where it came from.
///
/// Stored as an envelope rather than as bare text so a second reader can tell a
/// description from last week apart from one from this run, and so a better
/// parser can be pointed at the original answer later without paying for
/// another GPU round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AnalysisCache {
    #[serde(default = "cache_version")]
    pub version: u32,
    /// Exactly what the describe stage published, unedited.
    pub text: String,
    /// Which workflow said it — a description from a different describe graph
    /// is still a description, but a reader deserves to know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

fn cache_version() -> u32 {
    1
}

/// The description already on this shot, if there is one.
pub(crate) fn cached_analysis(conn: &mut SqliteConnection, shot_id: &str) -> Option<AnalysisCache> {
    let stored: Option<String> = shots::table
        .filter(shots::id.eq(shot_id))
        .select(shots::analysis_json)
        .first::<Option<String>>(conn)
        .ok()
        .flatten();
    serde_json::from_str(stored.as_deref()?).ok()
}

/// Remember what a describe stage said, so the next line over this shot does
/// not pay for it again.
///
/// A failure to write is logged and swallowed: the description is already on
/// the task that produced it, and losing a cache entry costs a GPU round-trip
/// rather than a result.
pub(crate) fn cache_analysis(
    conn: &mut SqliteConnection,
    shot_id: &str,
    text: &str,
    workflow_id: &str,
) {
    let entry = AnalysisCache {
        version: cache_version(),
        text: text.to_string(),
        workflow_id: Some(workflow_id.to_string()),
        generated_at: Some(crate::comfyui::timestamp::format_ts(
            chrono::Utc::now().naive_utc(),
        )),
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return;
    };
    if let Err(e) = diesel::update(shots::table.filter(shots::id.eq(shot_id)))
        .set(shots::analysis_json.eq(&json))
        .execute(conn)
    {
        tracing::warn!(
            "Could not cache the description for shot {}: {}",
            shot_id,
            e
        );
    }
}
