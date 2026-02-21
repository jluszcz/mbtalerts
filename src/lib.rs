use anyhow::Result;
use jluszcz_rust_utils::cache::{dated_cache_path, try_cached_query};
use log::trace;

use crate::mbta::query_subway_alerts;
use crate::types::{Alert, Alerts};

pub mod calendar;
pub mod mbta;
pub mod types;

pub const APP_NAME: &str = "mbtalerts";

pub fn line_name(alert: &Alert) -> &str {
    for entity in &alert.attributes.informed_entity {
        if let Some(route) = &entity.route {
            return match route.as_str() {
                "Red" => "Red Line",
                "Orange" => "Orange Line",
                r if r.starts_with("Green") => "Green Line",
                _ => "MBTA",
            };
        }
    }
    "MBTA"
}

pub async fn alerts(use_cache: bool) -> Result<Alerts> {
    let cache_path = dated_cache_path("alerts");

    let response = try_cached_query(use_cache, &cache_path, query_subway_alerts).await?;
    trace!("{response}");

    let alerts: Alerts = serde_json::from_str(&response)?;

    Ok(alerts)
}

#[cfg(test)]
mod test {
    use super::*;

    const EXAMPLE_ALERTS_RESPONSE: &str = include_str!("../tests/fixtures/alerts.json");

    fn make_alert(route: &str) -> Alert {
        Alert {
            id: "test-id".to_owned(),
            attributes: crate::types::AlertAttributes {
                header: "Test header".to_owned(),
                description: None,
                active_period: vec![],
                effect: "DELAY".to_owned(),
                informed_entity: vec![crate::types::InformedEntity {
                    route: Some(route.to_owned()),
                }],
            },
        }
    }

    fn make_alert_no_entities() -> Alert {
        Alert {
            id: "test-id".to_owned(),
            attributes: crate::types::AlertAttributes {
                header: "Test header".to_owned(),
                description: None,
                active_period: vec![],
                effect: "DELAY".to_owned(),
                informed_entity: vec![],
            },
        }
    }

    fn make_alert_null_route() -> Alert {
        Alert {
            id: "test-id".to_owned(),
            attributes: crate::types::AlertAttributes {
                header: "Test header".to_owned(),
                description: None,
                active_period: vec![],
                effect: "DELAY".to_owned(),
                informed_entity: vec![crate::types::InformedEntity { route: None }],
            },
        }
    }

    #[test]
    fn test_deserialize() -> Result<()> {
        let response: Result<Alerts, _> = serde_json::from_str(EXAMPLE_ALERTS_RESPONSE);
        assert!(response.is_ok());

        Ok(())
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
    fn test_line_name_unknown_route() {
        assert_eq!(line_name(&make_alert("Blue")), "MBTA");
    }

    #[test]
    fn test_line_name_no_entities() {
        assert_eq!(line_name(&make_alert_no_entities()), "MBTA");
    }

    #[test]
    fn test_line_name_null_route() {
        assert_eq!(line_name(&make_alert_null_route()), "MBTA");
    }
}
