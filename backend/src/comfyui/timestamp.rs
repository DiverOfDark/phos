//! How `enhancement_tasks` stores its datetimes.
//!
//! Every timestamp column in the table is written and compared as this one
//! format, so string comparison in SQL and chrono arithmetic in Rust agree.

/// The timestamp format every column in this table uses.
const TS_FMT: &str = "%Y-%m-%d %H:%M:%S";

pub(crate) fn format_ts(dt: chrono::NaiveDateTime) -> String {
    dt.format(TS_FMT).to_string()
}

pub(crate) fn parse_ts(raw: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(raw.trim(), TS_FMT)
        .ok()
        // Sqlite's CURRENT_TIMESTAMP and chrono both round-trip this, but a row
        // written with sub-second precision should not derail the settle clock.
        .or_else(|| chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S%.f").ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_round_trips() {
        let raw = "2026-08-30 12:00:00";
        assert_eq!(format_ts(parse_ts(raw).unwrap()), raw);
    }

    #[test]
    fn sub_second_precision_does_not_derail_the_clock() {
        // Sqlite's CURRENT_TIMESTAMP has none, but a row written by something
        // else might, and dropping the settle deadline over it would be silly.
        let fractional = parse_ts("2026-08-30 12:00:00.123").expect("should parse");
        assert_eq!(format_ts(fractional), "2026-08-30 12:00:00");
    }

    #[test]
    fn a_value_that_is_not_a_timestamp_is_not_guessed_at() {
        assert_eq!(parse_ts("not a timestamp"), None);
        assert_eq!(parse_ts(""), None);
    }
}
