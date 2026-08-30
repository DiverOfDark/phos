//! What a batch was pointed at, and how to ask the library for the next page.
//!
//! A selection is **either** an explicit list of shot ids **or** a query in the
//! shape `/api/shots` already accepts. There is exactly one filter language in
//! Phos and this is not a second one: the query arm holds an
//! [`crate::api::shots::ShotsQuery`] verbatim and the SQL is built by
//! `api::shots::shot_conditions`.
//!
//! Both arms resolve through the same statement, ordered the same way, so the
//! cursor means the same thing for both — an id list is just one more `WHERE`
//! condition.

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::shots::{bind_text_all, shot_conditions, ShotsQuery};

use super::plan::{cursor_predicate, Cursor};

/// The largest explicit id list a batch will accept.
///
/// An id list is what a checkbox selection produces, and a person cannot tick
/// more than a screenful at a time. Fifty thousand ids in a POST body is the
/// thing this whole feature exists to avoid, so it is refused with a message
/// telling the sender to use a query.
pub const MAX_EXPLICIT_IDS: usize = 5_000;

/// Either a list or a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selection {
    /// These exact shots.
    Ids { ids: Vec<String> },
    /// Whatever matches, now and every tick from here on. A batch built this
    /// way picks up shots imported after it was sent, as long as they sort
    /// after its cursor.
    Query {
        #[serde(default)]
        query: ShotsQuery,
    },
}

impl Selection {
    /// A short human name for what was selected, for the batch's label.
    pub fn shorthand(&self) -> String {
        match self {
            Selection::Ids { ids } => format!("{} selected", ids.len()),
            Selection::Query { query } => {
                let mut parts: Vec<String> = Vec::new();
                if query.person_id.is_some() {
                    parts.push("person".to_string());
                }
                if let (Some(from), Some(to)) = (&query.from, &query.to) {
                    parts.push(format!("{}–{}", short_date(from), short_date(to)));
                } else if let Some(from) = &query.from {
                    parts.push(format!("{}–", short_date(from)));
                } else if let Some(to) = &query.to {
                    parts.push(format!("–{}", short_date(to)));
                }
                if let Some(status) = &query.status {
                    parts.push(status.clone());
                }
                if let Some(q) = &query.q {
                    parts.push(format!("“{}”", q));
                }
                if parts.is_empty() {
                    "whole library".to_string()
                } else {
                    parts.join(" · ")
                }
            }
        }
    }

    /// Why this selection cannot be sent, if it cannot.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Selection::Ids { ids } if ids.is_empty() => {
                Err("Select at least one shot.".to_string())
            }
            Selection::Ids { ids } if ids.len() > MAX_EXPLICIT_IDS => Err(format!(
                "{} shots is more than a list can carry. Send a query instead — \
                 a batch resolves it a page at a time.",
                ids.len()
            )),
            _ => Ok(()),
        }
    }
}

fn short_date(s: &str) -> String {
    s.chars().take(4).collect()
}

/// What else the selection is narrowed by when it is resolved.
#[derive(Debug, Clone, Default)]
pub struct Narrowing<'a> {
    /// Skip shots that already have output from this line: a completed run of
    /// it, or a file made by its last stage's workflow. At batch scale this is
    /// a filter, not the warning dot the Enhance dialog shows for one shot.
    pub skip_line_id: Option<&'a str>,
    /// The last stage's workflow, which is what "output from this line" means
    /// in the generations data.
    pub skip_workflow_id: Option<&'a str>,
    /// Resume strictly after this shot.
    pub after: Option<&'a Cursor>,
    /// At most this many rows. `None` counts instead of listing.
    pub limit: Option<i64>,
}

#[derive(QueryableByName)]
struct ShotKeyRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    cursor_key: String,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    n: i64,
}

/// Build the `WHERE` list and its binds for a selection under a narrowing.
///
/// Separated from execution so the shape of the statement can be read — and
/// tested — without a connection.
fn conditions_for(selection: &Selection, narrow: &Narrowing<'_>) -> (Vec<String>, Vec<String>) {
    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    match selection {
        Selection::Ids { ids } => {
            if ids.is_empty() {
                // An empty list matches nothing. Saying so explicitly beats
                // emitting `IN ()`, which SQLite refuses to parse.
                conditions.push("0 = 1".to_string());
            } else {
                let mut slots = Vec::with_capacity(ids.len());
                for id in ids {
                    binds.push(id.clone());
                    slots.push(format!("?{}", binds.len()));
                }
                conditions.push(format!("s.id IN ({})", slots.join(",")));
            }
        }
        Selection::Query { query } => shot_conditions(query, &mut conditions, &mut binds),
    }

    // "Already has output from this line" is asked two ways, because the
    // library holds the answer two ways and either alone would be wrong. A run
    // row is the record of *this line* having been run; a generated file's
    // `source_workflow_id` is what the Enhance dialog reads, and is the only
    // trace left of a one-off enhance made before lines existed.
    if let Some(line_id) = narrow.skip_line_id {
        binds.push(line_id.to_string());
        let mut clauses = vec![format!(
            "NOT EXISTS (SELECT 1 FROM runs rg WHERE rg.shot_id = s.id \
             AND rg.line_id = ?{} AND rg.status = 'completed')",
            binds.len()
        )];
        if let Some(workflow_id) = narrow.skip_workflow_id {
            binds.push(workflow_id.to_string());
            clauses.push(format!(
                "NOT EXISTS (SELECT 1 FROM files fg WHERE fg.shot_id = s.id \
                 AND fg.source_workflow_id = ?{})",
                binds.len()
            ));
        }
        conditions.push(clauses.join(" AND "));
    }

    if let Some(cursor) = narrow.after {
        binds.push(cursor.key.clone());
        let key_slot = binds.len();
        binds.push(cursor.shot_id.clone());
        conditions.push(cursor_predicate(key_slot, binds.len()));
    }

    (conditions, binds)
}

fn where_clause(conditions: &[String]) -> String {
    if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    }
}

/// The next page of shots, in cursor order.
///
/// Ascending, so the oldest matching shot goes first — which is both the
/// natural order for "everything before 1990" and the order that lets a shot
/// imported mid-batch still be picked up, as long as it sorts after where the
/// batch has got to.
pub fn next_page(
    conn: &mut SqliteConnection,
    selection: &Selection,
    narrow: &Narrowing<'_>,
) -> QueryResult<Vec<Cursor>> {
    let (conditions, binds) = conditions_for(selection, narrow);
    let sql = format!(
        "SELECT s.id AS id, COALESCE(s.timestamp,'') AS cursor_key FROM shots s{} \
         ORDER BY COALESCE(s.timestamp,'') ASC, s.id ASC LIMIT {}",
        where_clause(&conditions),
        narrow.limit.unwrap_or(super::plan::DEFAULT_CHUNK).max(0)
    );
    let rows: Vec<ShotKeyRow> = bind_text_all(&sql, binds).load(conn)?;
    Ok(rows
        .into_iter()
        .map(|r| Cursor::new(r.cursor_key, r.id))
        .collect())
}

/// How many shots the selection names under this narrowing.
///
/// Used twice by the confirm sheet — once with `skip_line_id` set and once
/// without — which is how it can say "12,431 matched, 9,102 already done".
pub fn count(
    conn: &mut SqliteConnection,
    selection: &Selection,
    narrow: &Narrowing<'_>,
) -> QueryResult<i64> {
    let (conditions, binds) = conditions_for(selection, narrow);
    let sql = format!(
        "SELECT COUNT(*) AS n FROM shots s{}",
        where_clause(&conditions)
    );
    let row: CountRow = bind_text_all(&sql, binds).get_result(conn)?;
    Ok(row.n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_id_list_matches_nothing_rather_than_failing_to_parse() {
        let (conditions, binds) = conditions_for(
            &Selection::Ids { ids: vec![] },
            &Narrowing::default(),
        );
        assert_eq!(conditions, vec!["0 = 1".to_string()]);
        assert!(binds.is_empty());
    }

    #[test]
    fn an_id_list_becomes_one_in_clause_with_a_slot_each() {
        let (conditions, binds) = conditions_for(
            &Selection::Ids {
                ids: vec!["a".into(), "b".into(), "c".into()],
            },
            &Narrowing::default(),
        );
        assert_eq!(conditions, vec!["s.id IN (?1,?2,?3)".to_string()]);
        assert_eq!(binds, vec!["a", "b", "c"]);
    }

    #[test]
    fn the_query_arm_delegates_to_the_shots_filter() {
        let query = ShotsQuery {
            person_id: Some("p-1".into()),
            from: Some("1900".into()),
            to: Some("1990".into()),
            ..Default::default()
        };
        let (conditions, binds) =
            conditions_for(&Selection::Query { query }, &Narrowing::default());
        assert_eq!(binds, vec!["p-1", "1900", "1990"]);
        assert!(conditions[0].contains("s.primary_person_id = ?1"));
        assert!(conditions[1].contains("CAST(s.timestamp AS TEXT) >= ?2"));
        assert!(conditions[2].contains("CAST(s.timestamp AS TEXT) <= ?3"));
    }

    #[test]
    fn bind_slots_stay_in_step_when_narrowings_are_added() {
        // The whole raw-SQL approach hinges on `?N` matching `binds[N-1]`.
        // Person + skip + cursor is three sources of binds in one statement.
        let query = ShotsQuery {
            person_id: Some("p-1".into()),
            ..Default::default()
        };
        let cursor = Cursor::new("1975", "s-7");
        let (conditions, binds) = conditions_for(
            &Selection::Query { query },
            &Narrowing {
                skip_line_id: Some("line-1"),
                skip_workflow_id: Some("wf-9"),
                after: Some(&cursor),
                limit: None,
            },
        );
        assert_eq!(binds, vec!["p-1", "line-1", "wf-9", "1975", "s-7"]);
        assert!(conditions[1].contains("rg.line_id = ?2"));
        assert!(conditions[1].contains("fg.source_workflow_id = ?3"));
        assert!(conditions[2].contains("> ?4"));
        assert!(conditions[2].contains("> ?5"));
    }

    #[test]
    fn skipping_without_a_final_workflow_still_asks_about_runs() {
        let (conditions, binds) = conditions_for(
            &Selection::Query {
                query: ShotsQuery::default(),
            },
            &Narrowing {
                skip_line_id: Some("line-1"),
                skip_workflow_id: None,
                ..Default::default()
            },
        );
        assert_eq!(binds, vec!["line-1"]);
        assert!(conditions[0].contains("rg.line_id = ?1"));
        assert!(!conditions[0].contains("source_workflow_id"));
    }

    #[test]
    fn no_conditions_means_no_where_clause() {
        assert_eq!(where_clause(&[]), "");
        assert_eq!(where_clause(&["a = 1".into()]), " WHERE a = 1");
        assert_eq!(
            where_clause(&["a = 1".into(), "b = 2".into()]),
            " WHERE a = 1 AND b = 2"
        );
    }

    #[test]
    fn an_oversized_id_list_is_refused_with_the_alternative() {
        let ids: Vec<String> = (0..MAX_EXPLICIT_IDS + 1).map(|i| i.to_string()).collect();
        let err = Selection::Ids { ids }.validate().unwrap_err();
        assert!(err.contains("query"));
        assert!(Selection::Ids {
            ids: vec!["a".into()]
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn an_empty_id_list_is_refused_but_an_empty_query_is_not() {
        assert!(Selection::Ids { ids: vec![] }.validate().is_err());
        // An empty query is the whole library, which is legal and is exactly
        // why the confirm sheet exists.
        assert!(Selection::Query {
            query: ShotsQuery::default()
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn the_shorthand_reads_like_the_sentence_that_produced_it() {
        let query = ShotsQuery {
            person_id: Some("p-1".into()),
            to: Some("1990-01-01".into()),
            ..Default::default()
        };
        assert_eq!(
            Selection::Query { query }.shorthand(),
            "person · –1990"
        );
        assert_eq!(
            Selection::Query {
                query: ShotsQuery::default()
            }
            .shorthand(),
            "whole library"
        );
    }

    #[test]
    fn a_selection_round_trips_through_json_with_its_tag() {
        let selection = Selection::Query {
            query: ShotsQuery {
                q: Some("grandma".into()),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&selection).unwrap();
        assert!(json.contains("\"kind\":\"query\""));
        assert_eq!(
            serde_json::from_str::<Selection>(&json).unwrap(),
            selection
        );

        let ids = Selection::Ids {
            ids: vec!["a".into()],
        };
        let json = serde_json::to_string(&ids).unwrap();
        assert!(json.contains("\"kind\":\"ids\""));
        assert_eq!(serde_json::from_str::<Selection>(&json).unwrap(), ids);
    }
}
