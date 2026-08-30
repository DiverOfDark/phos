//! The `_phos` block: how a seeded workflow stays updatable until it is edited.
//!
//! A workflow Phos seeded from a bundled template carries one extra key at the
//! top level of its graph:
//!
//! ```json
//! "_phos": {
//!   "template_key": "photo-to-clip",
//!   "workflow_key": "wan-i2v",
//!   "template_version": 3,
//!   "hash": "sha256:…"
//! }
//! ```
//!
//! `hash` is of the graph *exactly as shipped* — the bundle's JSON, with no
//! marker in it. So on every upgrade the stored graph can be rehashed and
//! compared, and the answer is one of two things:
//!
//! * **the hash still matches** — nobody has touched it, so the new version of
//!   the template is written over it and the marker is refreshed;
//! * **the hash differs** — somebody edited it, so the marker is *dropped* and
//!   the workflow becomes an ordinary imported one that no future upgrade will
//!   ever look at again.
//!
//! The hash is the mechanism, not the marker: nothing depends on an editor
//! remembering to clear a flag, because any edit at all changes the hash.
//! Dropping the block afterwards is bookkeeping — it makes "this one is the
//! user's now" cheap to read, and it means the *second* upgrade after an edit
//! does not have to rehash a graph it has already given up on.
//!
//! # Why the marker lives in the graph
//!
//! Because that is what travels. A workflow exported from one library and
//! imported into another carries its provenance with it, and FR5d's line
//! bundles are the same JSON. A column in `comfyui_workflows` would be lost the
//! moment the graph left the database.
//!
//! It costs one thing: `_phos` is not a node, and ComfyUI validates every
//! top-level key of a prompt as one. [`super::super::workflow::prepare_workflow`]
//! strips it, which is the single funnel every dispatch goes through.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// The top-level key a seeded workflow is marked with.
pub const MARKER_KEY: &str = "_phos";

/// What the marker says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateMarker {
    /// Which bundled template this workflow came from.
    pub template_key: String,
    /// Which workflow *of* that template — a template may seed several.
    #[serde(default)]
    pub workflow_key: String,
    /// The template version this copy was written from.
    pub template_version: u32,
    /// `sha256:…` of the shipped graph, with no marker in it.
    pub hash: String,
}

/// The canonical serialisation a hash is taken over.
///
/// Written out by hand rather than leaning on `serde_json::to_string`, because
/// what that produces depends on whether `serde_json`'s `preserve_order`
/// feature is on somewhere in the tree — and a hash that changes when a
/// dependency gains a feature flag would silently abandon every seeded workflow
/// in every library.
///
/// Object keys sorted, no whitespace, floats through Rust's shortest
/// round-trip representation.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            // serde_json's own string escaping, so control characters and
            // non-ASCII are handled the way every other JSON writer does.
            out.push_str(&Value::String(s.clone()).to_string())
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

/// `sha256:…` over the graph with any marker removed.
///
/// Removing the marker first is what makes the comparison work in both
/// directions: the shipped bundle has no marker and the stored copy has one,
/// and they must hash the same when nothing has been edited.
pub fn content_hash(graph: &Value) -> String {
    let mut text = String::new();
    canonical(&strip_marker(graph), &mut text);
    format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes())))
}

/// A copy of the graph with no `_phos` block.
pub fn strip_marker(graph: &Value) -> Value {
    match graph.as_object() {
        Some(map) if map.contains_key(MARKER_KEY) => {
            let mut map = map.clone();
            map.remove(MARKER_KEY);
            Value::Object(map)
        }
        _ => graph.clone(),
    }
}

/// The marker on a stored graph, if it still has one.
///
/// A marker that does not parse — written by a newer Phos, or mangled by hand —
/// reads as no marker at all, which means the workflow is left alone. Leaving a
/// stranger's workflow untouched is the safe direction to fail in.
pub fn read_marker(graph: &Value) -> Option<TemplateMarker> {
    let raw = graph.get(MARKER_KEY)?;
    serde_json::from_value(raw.clone()).ok()
}

/// The shipped graph, marked as belonging to a template at this version.
pub fn with_marker(
    graph: &Value,
    template_key: &str,
    workflow_key: &str,
    template_version: u32,
) -> Value {
    let hash = content_hash(graph);
    let mut map = match strip_marker(graph) {
        Value::Object(map) => map,
        other => {
            // A graph that is not an object cannot be marked, and cannot be
            // dispatched either. Hand it back unchanged rather than inventing
            // a shape for it.
            return other;
        }
    };
    let marker = TemplateMarker {
        template_key: template_key.to_string(),
        workflow_key: workflow_key.to_string(),
        template_version,
        hash,
    };
    let mut block = Map::new();
    block.insert("template_key".into(), Value::String(marker.template_key));
    block.insert("workflow_key".into(), Value::String(marker.workflow_key));
    block.insert(
        "template_version".into(),
        Value::Number(marker.template_version.into()),
    );
    block.insert("hash".into(), Value::String(marker.hash));
    map.insert(MARKER_KEY.to_string(), Value::Object(block));
    Value::Object(map)
}

/// What an upgrade should do with one stored workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Seeded, unedited, already at the shipped version. Nothing to do.
    UpToDate,
    /// Seeded, unedited, and the shipped version is different: overwrite it.
    Update,
    /// Seeded and then edited. Drop the marker and never look at it again.
    Abandon,
    /// Not ours — imported by hand, or abandoned by an earlier upgrade.
    Unmanaged,
}

/// Decide what to do with `stored`, given the template that claims to own it.
///
/// Pure, and the whole of the update policy. `shipped_hash` is
/// [`content_hash`] of the bundle's graph.
///
/// The comparison is against the hash *recorded in the marker*, not against the
/// shipped one: "has this changed since Phos wrote it" is the question, and a
/// workflow sitting at version 2 while version 3 ships is unedited even though
/// its content differs from what ships today.
pub fn decide(stored: &Value, shipped_version: u32, shipped_hash: &str) -> Verdict {
    let Some(marker) = read_marker(stored) else {
        return Verdict::Unmanaged;
    };
    if content_hash(stored) != marker.hash {
        return Verdict::Abandon;
    }
    if marker.template_version == shipped_version && marker.hash == shipped_hash {
        return Verdict::UpToDate;
    }
    Verdict::Update
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shipped() -> Value {
        json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "a.png" } },
            "2": { "class_type": "SaveImage", "inputs": { "images": ["1", 0] } }
        })
    }

    #[test]
    fn the_hash_ignores_key_order_and_whitespace() {
        let a: Value =
            serde_json::from_str(r#"{"1":{"class_type":"LoadImage","inputs":{"image":"a.png"}}}"#)
                .unwrap();
        let b: Value = serde_json::from_str(
            "{\n  \"1\": {\n    \"inputs\": { \"image\": \"a.png\" },\n\
             \"class_type\": \"LoadImage\"\n  }\n}",
        )
        .unwrap();
        assert_eq!(content_hash(&a), content_hash(&b));
    }

    #[test]
    fn the_hash_notices_a_changed_value() {
        let mut edited = shipped();
        edited["1"]["inputs"]["image"] = json!("b.png");
        assert_ne!(content_hash(&shipped()), content_hash(&edited));
    }

    #[test]
    fn marking_a_graph_does_not_change_what_it_hashes_to() {
        let marked = with_marker(&shipped(), "restore-upscale", "upscale", 1);
        assert_eq!(content_hash(&marked), content_hash(&shipped()));
        assert_eq!(strip_marker(&marked), shipped());
    }

    #[test]
    fn a_marked_graph_records_the_hash_of_what_was_shipped() {
        let marked = with_marker(&shipped(), "restore-upscale", "upscale", 3);
        let marker = read_marker(&marked).expect("a marker");
        assert_eq!(marker.template_key, "restore-upscale");
        assert_eq!(marker.workflow_key, "upscale");
        assert_eq!(marker.template_version, 3);
        assert_eq!(marker.hash, content_hash(&shipped()));
    }

    #[test]
    fn an_untouched_copy_at_the_shipped_version_is_left_alone() {
        let marked = with_marker(&shipped(), "k", "w", 1);
        assert_eq!(
            decide(&marked, 1, &content_hash(&shipped())),
            Verdict::UpToDate
        );
    }

    #[test]
    fn an_untouched_copy_at_an_older_version_is_updated() {
        let marked = with_marker(&shipped(), "k", "w", 1);
        let mut v2 = shipped();
        v2["2"]["inputs"]["filename_prefix"] = json!("phos/x");
        assert_eq!(decide(&marked, 2, &content_hash(&v2)), Verdict::Update);
    }

    /// The case the whole design exists for.
    #[test]
    fn a_copy_the_user_edited_is_abandoned_however_the_version_moves() {
        let mut edited = with_marker(&shipped(), "k", "w", 1);
        edited["1"]["inputs"]["image"] = json!("my-own.png");
        assert_eq!(
            decide(&edited, 1, &content_hash(&shipped())),
            Verdict::Abandon
        );
        assert_eq!(decide(&edited, 9, "sha256:whatever"), Verdict::Abandon);
    }

    #[test]
    fn an_edit_that_only_adds_a_node_is_still_an_edit() {
        let mut edited = with_marker(&shipped(), "k", "w", 1);
        edited["3"] = json!({ "class_type": "PreviewImage", "inputs": { "images": ["1", 0] } });
        assert_eq!(
            decide(&edited, 1, &content_hash(&shipped())),
            Verdict::Abandon
        );
    }

    #[test]
    fn a_workflow_with_no_marker_is_nobodys_business() {
        assert_eq!(decide(&shipped(), 1, "sha256:x"), Verdict::Unmanaged);
    }

    #[test]
    fn a_marker_nothing_can_read_means_hands_off() {
        let mut odd = shipped();
        odd[MARKER_KEY] = json!("from a future Phos");
        assert_eq!(decide(&odd, 1, "sha256:x"), Verdict::Unmanaged);
        assert_eq!(read_marker(&odd), None);
    }

    #[test]
    fn a_marker_is_replaced_rather_than_nested_when_a_graph_is_re_marked() {
        let once = with_marker(&shipped(), "k", "w", 1);
        let twice = with_marker(&once, "k", "w", 2);
        assert_eq!(read_marker(&twice).unwrap().template_version, 2);
        assert_eq!(content_hash(&twice), content_hash(&shipped()));
    }

    #[test]
    fn floats_and_nulls_survive_canonicalisation() {
        let a = json!({ "a": 1.5, "b": null, "c": [true, false], "d": "é\n" });
        assert_eq!(content_hash(&a), content_hash(&a.clone()));
        let mut b = a.clone();
        b["a"] = json!(1.6);
        assert_ne!(content_hash(&a), content_hash(&b));
    }
}
