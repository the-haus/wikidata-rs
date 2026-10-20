//! Wikidata time values.
//!
//! A timestamp is `±YYYY…-MM-DDThh:mm:ssZ` with at least four year digits
//! (`+13798000000-00-00T00:00:00Z` is the age of the universe); month and
//! day are `00` when unknown. Precision says which parts are meaningful.

use crate::entity::Time;

/// Names of the precision codes 0–14.
pub const PRECISION_NAMES: [&str; 15] = [
    "billionYears",
    "hundredMillionYears",
    "tenMillionYears",
    "millionYears",
    "hundredThousandYears",
    "tenThousandYears",
    "millennium",
    "century",
    "decade",
    "year",
    "month",
    "day",
    "hour",
    "minute",
    "second",
];

const GREGORIAN: &str = "Q1985727";
const JULIAN: &str = "Q1985786";

/// The item id at the end of a concept URI
/// (`http://www.wikidata.org/entity/Q2` → `Q2`).
pub fn entity_of_uri(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}

/// `gregorian`, `julian`, or the calendar's item id.
pub fn calendar_name(uri: &str) -> String {
    match entity_of_uri(uri) {
        GREGORIAN => "gregorian".into(),
        JULIAN => "julian".into(),
        "" => "gregorian".into(),
        other => other.into(),
    }
}

/// The signed year, month and day of a timestamp.
pub fn parse_timestamp(t: &str) -> Option<(i64, u8, u8)> {
    let b = t.as_bytes();
    let (neg, rest) = match b.first()? {
        b'+' => (false, &t[1..]),
        b'-' => (true, &t[1..]),
        b'0'..=b'9' => (false, t),
        _ => return None,
    };
    let mut parts = rest.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u8 = parts.next()?.parse().ok()?;
    let d: u8 = parts.next()?.get(..2)?.parse().ok()?;
    Some((if neg { -y } else { y }, m, d))
}

fn year_str(y: i64) -> String {
    if y < 0 {
        format!("-{:04}", -y)
    } else {
        format!("{y:04}")
    }
}

/// A [`Time`] from a timestamp, precision and calendar URI.
pub fn make_time(raw: String, precision: Option<f64>, calendar: Option<&str>) -> Option<Time> {
    let (year, m, d) = parse_timestamp(&raw)?;
    let precision = precision.map_or(11, |p| p.clamp(0.0, 14.0) as u8);
    let month = (precision >= 10 && m > 0).then_some(m);
    let day = (precision >= 11 && month.is_some() && d > 0).then_some(d);
    let iso = match (month, day) {
        (Some(m), Some(d)) => format!("{}-{m:02}-{d:02}", year_str(year)),
        (Some(m), None) => format!("{}-{m:02}", year_str(year)),
        _ => year_str(year),
    };
    Some(Time {
        time: raw,
        iso,
        precision,
        precision_name: PRECISION_NAMES[precision as usize],
        year,
        month,
        day,
        calendar: calendar_name(calendar.unwrap_or("")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precisions() {
        let t = make_time(
            "+1830-10-04T00:00:00Z".into(),
            Some(11.0),
            Some("http://www.wikidata.org/entity/Q1985727"),
        )
        .unwrap();
        assert_eq!(
            (t.iso.as_str(), t.year, t.month, t.day),
            ("1830-10-04", 1830, Some(10), Some(4))
        );
        assert_eq!(t.calendar, "gregorian");
        let t = make_time("+1830-10-00T00:00:00Z".into(), Some(10.0), None).unwrap();
        assert_eq!(t.iso, "1830-10");
        let t = make_time("+1830-00-00T00:00:00Z".into(), Some(9.0), None).unwrap();
        assert_eq!((t.iso.as_str(), t.month), ("1830", None));
        let t = make_time("+1990-01-01T00:00:00Z".into(), Some(8.0), None).unwrap();
        assert_eq!((t.iso.as_str(), t.precision_name), ("1990", "decade"));
        let t = make_time(
            "-0500-00-00T00:00:00Z".into(),
            Some(9.0),
            Some("http://www.wikidata.org/entity/Q1985786"),
        )
        .unwrap();
        assert_eq!((t.iso.as_str(), t.year, t.calendar.as_str()), ("-0500", -500, "julian"));
        let t = make_time("+13798000000-00-00T00:00:00Z".into(), Some(3.0), None).unwrap();
        assert_eq!(t.year, 13_798_000_000);
        assert!(make_time("garbage".into(), None, None).is_none());
    }
}
