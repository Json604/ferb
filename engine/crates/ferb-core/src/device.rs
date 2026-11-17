//! A device's own facts as every model call is told them (T-1130, SENSE-420): one short line per device, the same
//! record whichever device supplies it (the Mac now; a phone in stage 3), and the coarse place a device reports.
//! Portable: the Mac's readings come in through the daemon's composition, a phone's through its link.
//!
//! The line never holds coordinates (SENSE-420: city level always; the exact position only through a lookup the
//! owner says Yes to, at that moment). Time is the caller's: the line is written for the instant it is given.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::when::DateTime;

/// The longest place name kept from a report (a city, region or country): a technical bound, not a cap.
pub const PLACE_PART_CHARS: usize = 80;

/// A device's coarse place: city, region and country (reverse-geocoded on the device). Never coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub city: String,
    pub region: String,
    pub country: String,
    /// ISO 3166-1 alpha-2, as the geocoder gives it ("" when none).
    pub country_code: String,
}

/// A reported name as kept: control characters dropped, whitespace collapsed, at most [`PLACE_PART_CHARS`].
pub fn clean_part(text: &str) -> String {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect();
    words.join(" ").chars().take(PLACE_PART_CHARS).collect()
}

impl Place {
    /// A place as a device's report holds it (`{"city", "region", "country", "country_code"}`), cleaned; None when it
    /// names nothing.
    pub fn from_report(v: &Value) -> Option<Place> {
        let part = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(clean_part)
                .unwrap_or_default()
        };
        let place = Place {
            city: part("city"),
            region: part("region"),
            country: part("country"),
            country_code: part("country_code")
                .chars()
                .filter(char::is_ascii_alphabetic)
                .take(2)
                .collect(),
        };
        (!place.text().is_empty()).then_some(place)
    }

    /// "Bengaluru, Karnataka, India": the named parts in order, each once.
    pub fn text(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for p in [&self.city, &self.region, &self.country] {
            if !p.is_empty() && !parts.contains(&p.as_str()) {
                parts.push(p);
            }
        }
        parts.join(", ")
    }

    pub fn to_json(&self) -> Value {
        json!({"city": self.city, "region": self.region, "country": self.country, "country_code": self.country_code})
    }
}

/// "UTC+05:30", "UTC-04:00", "UTC+00:00".
pub fn utc_offset(t: &DateTime) -> String {
    let secs = t.offset().local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let m = secs.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// What one device's line is made of.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceLine<'a> {
    /// How the line names the device: "This Mac" for the device FERB runs on.
    pub label: &'a str,
    /// The device's own name (the Mac's computer name); "" when unknown.
    pub name: &'a str,
    /// Now, in the device's own time zone (its wall time and offset).
    pub now: &'a DateTime,
    /// The device's IANA time zone ("Asia/Kolkata"); "" when unknown.
    pub zone: &'a str,
    /// Its coarse place when the device may read it and has a fix.
    pub place: Option<&'a Place>,
    /// A last clause, when there is one ("connected, screen on and unlocked"; "not connected, last seen 2 h ago"): "" for none.
    pub note: &'a str,
}

/// `- This Mac “Studio”: Monday 2026-11-16 10:05, time zone Asia/Kolkata (UTC+05:30); place: Bengaluru, Karnataka,
/// India (city level)`; without a place, `place unknown (time zone only)`; a note after a last `; `.
pub fn line(d: &DeviceLine<'_>) -> String {
    let name = if d.name.is_empty() {
        String::new()
    } else {
        format!(" “{}”", d.name)
    };
    let zone = if d.zone.is_empty() {
        utc_offset(d.now)
    } else {
        format!("{} ({})", d.zone, utc_offset(d.now))
    };
    let place = match d.place.map(Place::text).filter(|t| !t.is_empty()) {
        Some(t) => format!("place: {t} (city level)"),
        None => "place unknown (time zone only)".to_string(),
    };
    let note = if d.note.is_empty() {
        String::new()
    } else {
        format!("; {}", d.note)
    };
    format!(
        "- {}{name}: {}, time zone {zone}; {place}{note}",
        d.label,
        d.now.format("%A %Y-%m-%d %H:%M")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime {
        chrono::DateTime::parse_from_rfc3339(text).unwrap()
    }

    // decision: SENSE-420
    #[test]
    fn the_line_names_the_device_its_time_and_city_never_coordinates() {
        let now = at("2026-11-16T10:05:31+05:30");
        let place = Place::from_report(
            &json!({"city": "Pune", "region": "Maharashtra", "country": "India",
                                               "country_code": "IN", "latitude": 18.52}),
        )
        .unwrap();
        let got = line(&DeviceLine {
            label: "This Mac",
            name: "Studio",
            now: &now,
            zone: "Asia/Kolkata",
            place: Some(&place),
            note: "",
        });
        assert_eq!(
            got,
            "- This Mac “Studio”: Monday 2026-11-16 10:05, time zone Asia/Kolkata (UTC+05:30); place: Pune, \
             Maharashtra, India (city level)"
        );
        assert!(!place.to_json().to_string().contains("18.52"));
        let none = line(&DeviceLine {
            label: "This Mac",
            name: "",
            now: &at("2026-03-01T23:59:00-05:00"),
            zone: "America/New_York",
            place: None,
            note: "",
        });
        assert_eq!(
            none,
            "- This Mac: Sunday 2026-03-01 23:59, time zone America/New_York (UTC-05:00); place unknown (time zone only)"
        );
        // a phone's line (T-1214): the same record with its last clause
        let phone = line(&DeviceLine {
            label: "Phone",
            name: "Moto",
            now: &at("2026-11-16T15:35:00+05:30"),
            zone: "Asia/Kolkata",
            place: Some(&place),
            note: "connected, screen on and unlocked",
        });
        assert_eq!(
            phone,
            "- Phone “Moto”: Monday 2026-11-16 15:35, time zone Asia/Kolkata (UTC+05:30); place: Pune, Maharashtra, \
             India (city level); connected, screen on and unlocked"
        );
    }

    #[test]
    fn a_reported_place_is_cleaned_and_an_empty_one_is_none() {
        let p = Place::from_report(
            &json!({"city": " New\u{7}  York\n", "region": "New York", "country": "United States",
                                           "country_code": "US1"}),
        )
        .unwrap();
        assert_eq!(p.city, "New York");
        assert_eq!(p.country_code, "US");
        assert_eq!(p.text(), "New York, United States");
        assert_eq!(Place::from_report(&json!({"city": "  "})), None);
        assert_eq!(clean_part(&"x".repeat(200)).len(), PLACE_PART_CHARS);
    }
}
