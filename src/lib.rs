use anyhow::Result;
use jluszcz_rust_utils::cache::{CacheMode, dated_cache_path, try_cached_query};
use log::{trace, warn};

use crate::mbta::query_subway_alerts;
use crate::types::{Alert, Alerts};

pub mod ai;
pub mod calendar;
pub mod mbta;
pub mod summary;
pub mod types;

pub const APP_NAME: &str = "mbtalerts";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Line {
    Red,
    Orange,
    Blue,
    Green,
}

impl Line {
    pub const ALL: [Line; 4] = [Line::Red, Line::Orange, Line::Blue, Line::Green];

    /// Canonical short name, used as the key in GOOGLE_CALENDAR_IDS.
    pub fn name(self) -> &'static str {
        match self {
            Line::Red => "Red",
            Line::Orange => "Orange",
            Line::Blue => "Blue",
            Line::Green => "Green",
        }
    }

    /// Inverse of [`Line::name`]: parses a GOOGLE_CALENDAR_IDS key.
    pub fn from_name(name: &str) -> Option<Line> {
        Self::ALL.into_iter().find(|line| line.name() == name)
    }

    pub fn full_name(self) -> &'static str {
        match self {
            Line::Red => "Red Line",
            Line::Orange => "Orange Line",
            Line::Blue => "Blue Line",
            Line::Green => "Green Line",
        }
    }
}

pub fn canonical_line(route: &str) -> Option<Line> {
    match route {
        "Red" => Some(Line::Red),
        "Orange" => Some(Line::Orange),
        "Blue" => Some(Line::Blue),
        r if r.starts_with("Green") => Some(Line::Green),
        _ => None,
    }
}

pub fn line_name(alert: &Alert) -> &'static str {
    for entity in &alert.attributes.informed_entity {
        if let Some(route) = &entity.route {
            return match canonical_line(route) {
                Some(line) => line.full_name(),
                None => {
                    warn!("Unknown route '{route}', falling back to MBTA");
                    "MBTA"
                }
            };
        }
    }
    "MBTA"
}

const STATION_EFFECTS_TO_SKIP: &[&str] = &[
    "STATION_ISSUE",
    "STOP_CLOSURE",
    "STATION_CLOSURE",
    "PARKING_ISSUE",
];

/// Station-level issues (closed stairways, parking, etc.) are noise for both
/// the terminal output and calendar sync.
pub fn should_sync_alert(alert: &Alert) -> bool {
    !STATION_EFFECTS_TO_SKIP.contains(&alert.attributes.effect.as_str())
}

pub async fn alerts(cache_mode: CacheMode) -> Result<Alerts> {
    let cache_path = dated_cache_path("alerts");

    let response = try_cached_query(cache_mode, &cache_path, query_subway_alerts).await?;
    trace!("{response}");

    let alerts: Alerts = serde_json::from_str(&response)?;

    Ok(alerts)
}

#[cfg(test)]
mod test {
    use super::*;

    const EXAMPLE_ALERTS_RESPONSE: &str = include_str!("../tests/fixtures/alerts.json");

    fn make_alert(route: &str) -> Alert {
        Alert::builder().route(route).build()
    }

    fn make_alert_no_entities() -> Alert {
        Alert::builder().build()
    }

    fn make_alert_null_route() -> Alert {
        Alert::builder().null_route().build()
    }

    #[test]
    fn test_deserialize() -> Result<()> {
        serde_json::from_str::<Alerts>(EXAMPLE_ALERTS_RESPONSE)?;

        Ok(())
    }

    #[test]
    fn test_line_from_name_round_trips() {
        for line in Line::ALL {
            assert_eq!(Line::from_name(line.name()), Some(line));
        }
    }

    #[test]
    fn test_line_from_name_unknown() {
        assert_eq!(Line::from_name("Silver"), None);
    }

    #[test]
    fn test_canonical_line_red() {
        assert_eq!(canonical_line("Red"), Some(Line::Red));
    }

    #[test]
    fn test_canonical_line_orange() {
        assert_eq!(canonical_line("Orange"), Some(Line::Orange));
    }

    #[test]
    fn test_canonical_line_blue() {
        assert_eq!(canonical_line("Blue"), Some(Line::Blue));
    }

    #[test]
    fn test_canonical_line_green() {
        assert_eq!(canonical_line("Green"), Some(Line::Green));
    }

    #[test]
    fn test_canonical_line_green_b() {
        assert_eq!(canonical_line("Green-B"), Some(Line::Green));
    }

    #[test]
    fn test_canonical_line_green_e() {
        assert_eq!(canonical_line("Green-E"), Some(Line::Green));
    }

    #[test]
    fn test_canonical_line_unknown() {
        assert_eq!(canonical_line("CR-Fitchburg"), None);
    }

    #[test]
    fn test_line_name_red() {
        assert_eq!(line_name(&make_alert("Red")), "Red Line");
    }

    #[test]
    fn test_line_name_orange() {
        assert_eq!(line_name(&make_alert("Orange")), "Orange Line");
    }

    #[test]
    fn test_line_name_green() {
        assert_eq!(line_name(&make_alert("Green")), "Green Line");
    }

    #[test]
    fn test_line_name_green_b() {
        assert_eq!(line_name(&make_alert("Green-B")), "Green Line");
    }

    #[test]
    fn test_line_name_green_c() {
        assert_eq!(line_name(&make_alert("Green-C")), "Green Line");
    }

    #[test]
    fn test_line_name_green_d() {
        assert_eq!(line_name(&make_alert("Green-D")), "Green Line");
    }

    #[test]
    fn test_line_name_green_e() {
        assert_eq!(line_name(&make_alert("Green-E")), "Green Line");
    }

    #[test]
    fn test_line_name_blue() {
        assert_eq!(line_name(&make_alert("Blue")), "Blue Line");
    }

    #[test]
    fn test_line_name_unknown_route() {
        assert_eq!(line_name(&make_alert("CR-Fitchburg")), "MBTA");
    }

    #[test]
    fn test_line_name_no_entities() {
        assert_eq!(line_name(&make_alert_no_entities()), "MBTA");
    }

    #[test]
    fn test_line_name_null_route() {
        assert_eq!(line_name(&make_alert_null_route()), "MBTA");
    }

    fn make_alert_with_effect(effect: &str) -> Alert {
        Alert::builder().route("Red").effect(effect).build()
    }

    #[test]
    fn test_should_sync_station_issue_is_skipped() {
        assert!(!should_sync_alert(&make_alert_with_effect("STATION_ISSUE")));
    }

    #[test]
    fn test_should_sync_stop_closure_is_skipped() {
        assert!(!should_sync_alert(&make_alert_with_effect("STOP_CLOSURE")));
    }

    #[test]
    fn test_should_sync_station_closure_is_skipped() {
        assert!(!should_sync_alert(&make_alert_with_effect(
            "STATION_CLOSURE"
        )));
    }

    #[test]
    fn test_should_sync_parking_issue_is_skipped() {
        assert!(!should_sync_alert(&make_alert_with_effect("PARKING_ISSUE")));
    }

    #[test]
    fn test_should_sync_shuttle_is_synced() {
        assert!(should_sync_alert(&make_alert_with_effect("SHUTTLE")));
    }
}
