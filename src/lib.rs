use std::env;
use std::path::Path;
use std::time::Duration;

use again::RetryPolicy;
use anyhow::Result;
use chrono::Utc;
use log::{debug, trace};
use reqwest::{Client, Method};
use serde::Serialize;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use crate::mbta::query_subway_alerts;
use crate::types::{Alert, Alerts};

pub mod calendar;
pub mod gtfs;
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
    let mut cache_path = env::temp_dir();
    cache_path.push(format!(
        "alerts.{}.json",
        Utc::now().date_naive().format("%Y%m%d")
    ));

    let response = try_cached_query(use_cache, &cache_path, query_subway_alerts).await?;
    trace!("{response}");

    let alerts: Alerts = serde_json::from_str(&response)?;

    Ok(alerts)
}

async fn http_get<T>(url: &str, params: &T) -> Result<String>
where
    T: Serialize + ?Sized,
{
    let retry_policy = RetryPolicy::exponential(Duration::from_millis(100))
        .with_jitter(true)
        .with_max_delay(Duration::from_secs(1))
        .with_max_retries(3);

    let response = retry_policy
        .retry(|| {
            Client::new()
                .request(Method::GET, url)
                .header("Accept", "application/json")
                .header("Accept-Encoding", "gzip")
                .query(params)
                .send()
        })
        .await?
        .text()
        .await?;

    trace!("{}", response);

    Ok(response)
}

async fn try_cached_query<F>(
    use_cache: bool,
    cache_path: &Path,
    query: impl Fn() -> F,
) -> Result<String>
where
    F: Future<Output = Result<String>>,
{
    match try_cached(use_cache, cache_path).await? {
        Some(cached) => Ok(cached),
        _ => {
            let response = query().await?;
            try_write_cache(use_cache, cache_path, &response).await?;
            Ok(response)
        }
    }
}

async fn try_cached(use_cache: bool, cache_path: &Path) -> Result<Option<String>> {
    if use_cache && cache_path.exists() {
        debug!("Reading cache file: {:?}", cache_path);
        Ok(Some(fs::read_to_string(cache_path).await?))
    } else {
        Ok(None)
    }
}

async fn try_write_cache(use_cache: bool, cache_path: &Path, response: &str) -> Result<()> {
    if use_cache {
        debug!("Writing response to cache file: {:?}", cache_path);

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(cache_path)
            .await?;

        file.write_all(response.as_bytes()).await?;
    }

    Ok(())
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
