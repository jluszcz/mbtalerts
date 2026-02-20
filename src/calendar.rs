use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use gcp_auth::{CustomServiceAccount, TokenProvider};
use log::{debug, info};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::types::{Alert, Alerts};

const CAL_API: &str = "https://www.googleapis.com/calendar/v3/calendars";
const SCOPES: &[&str] = &["https://www.googleapis.com/auth/calendar"];

pub struct CalendarClient {
    token_provider: Arc<dyn TokenProvider>,
    calendar_id: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct EventList {
    #[serde(default)]
    items: Vec<CalendarEvent>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CalendarEvent {
    id: String,
    #[serde(rename = "extendedProperties")]
    extended_properties: Option<ExtendedProperties>,
}

#[derive(Debug, Deserialize)]
struct ExtendedProperties {
    private: Option<HashMap<String, String>>,
}

impl CalendarEvent {
    fn alert_id(&self) -> Option<&str> {
        self.extended_properties
            .as_ref()?
            .private
            .as_ref()?
            .get("mbta_alert_id")
            .map(String::as_str)
    }
}

impl CalendarClient {
    pub async fn from_env() -> Result<Self> {
        let token_provider: Arc<dyn TokenProvider> =
            if let Ok(key_json) = std::env::var("GOOGLE_SERVICE_ACCOUNT_KEY") {
                Arc::new(CustomServiceAccount::from_json(&key_json)?)
            } else {
                gcp_auth::provider().await?
            };

        let calendar_id =
            std::env::var("GOOGLE_CALENDAR_ID").context("GOOGLE_CALENDAR_ID env var not set")?;

        Ok(Self {
            token_provider,
            calendar_id,
            client: Client::new(),
        })
    }

    async fn access_token(&self) -> Result<String> {
        let token = self.token_provider.token(SCOPES).await?;
        Ok(token.as_str().to_owned())
    }

    fn events_url(&self) -> String {
        format!("{}/{}/events", CAL_API, &self.calendar_id)
    }

    fn event_url(&self, event_id: &str) -> String {
        format!("{}/{}/events/{}", CAL_API, &self.calendar_id, event_id)
    }

    async fn list_alert_events(&self) -> Result<Vec<CalendarEvent>> {
        let token = self.access_token().await?;
        let mut events = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut req = self
                .client
                .get(self.events_url())
                .bearer_auth(&token)
                .query(&[("privateExtendedProperty", "mbta_alert_source=true")]);

            if let Some(pt) = &page_token {
                req = req.query(&[("pageToken", pt.as_str())]);
            }

            let response: EventList = req.send().await?.error_for_status()?.json().await?;
            debug!("Fetched {} calendar events", response.items.len());
            events.extend(response.items);

            match response.next_page_token {
                Some(pt) => page_token = Some(pt),
                None => break,
            }
        }

        Ok(events)
    }

    async fn create_event(&self, alert: &Alert) -> Result<()> {
        let token = self.access_token().await?;
        let body = event_body(alert);

        self.client
            .post(self.events_url())
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    async fn update_event(&self, event_id: &str, alert: &Alert) -> Result<()> {
        let token = self.access_token().await?;
        let body = event_body(alert);

        self.client
            .put(self.event_url(event_id))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    async fn delete_event(&self, event_id: &str) -> Result<()> {
        let token = self.access_token().await?;

        self.client
            .delete(self.event_url(event_id))
            .bearer_auth(&token)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }
}

pub async fn sync_alerts(alerts: &Alerts, cal: &CalendarClient) -> Result<()> {
    let existing = cal.list_alert_events().await?;

    let mut existing_by_alert_id: HashMap<String, String> = HashMap::new();
    for event in &existing {
        if let Some(alert_id) = event.alert_id() {
            existing_by_alert_id.insert(alert_id.to_owned(), event.id.clone());
        }
    }

    let mut seen: HashSet<String> = HashSet::new();

    for alert in &alerts.data {
        if let Some(event_id) = existing_by_alert_id.get(&alert.id) {
            debug!(
                "Updating event for alert {}: [{}] {}",
                alert.id,
                crate::line_name(alert),
                alert.attributes.effect
            );
            cal.update_event(event_id, alert).await?;
        } else {
            debug!(
                "Creating event for alert {}: [{}] {}",
                alert.id,
                crate::line_name(alert),
                alert.attributes.effect
            );
            cal.create_event(alert).await?;
        }
        seen.insert(alert.id.clone());
    }

    for (alert_id, event_id) in &existing_by_alert_id {
        if !seen.contains(alert_id) {
            info!("Deleting stale event for alert {}", alert_id);
            cal.delete_event(event_id).await?;
        }
    }

    Ok(())
}

fn next_date(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| {
            (d + chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| date.to_string())
}

fn event_times(start: Option<&str>, end: Option<&str>) -> (Value, Value) {
    match (start, end) {
        (Some(s), Some(e)) => (json!({ "dateTime": s }), json!({ "dateTime": e })),
        (Some(s), None) => {
            // Open-ended alert: all-day event on the start date (end is exclusive in Google Calendar)
            let date = s.get(..10).unwrap_or(s);
            (json!({ "date": date }), json!({ "date": next_date(date) }))
        }
        _ => {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let tomorrow = next_date(&today);
            (json!({ "date": today }), json!({ "date": tomorrow }))
        }
    }
}

fn event_body(alert: &Alert) -> Value {
    let period = alert.attributes.active_period.first();
    let (start, end) = event_times(
        period.and_then(|p| p.start.as_deref()),
        period.and_then(|p| p.end.as_deref()),
    );

    json!({
        "summary": format!("[{}] {}", crate::line_name(alert), alert.attributes.effect),
        "description": alert.attributes.description,
        "start": start,
        "end": end,
        "extendedProperties": {
            "private": {
                "mbta_alert_source": "true",
                "mbta_alert_id": alert.id,
            }
        }
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::types::{ActivePeriod, AlertAttributes, InformedEntity};

    fn make_alert(route: &str, effect: &str, start: Option<&str>, end: Option<&str>) -> Alert {
        Alert {
            id: "alert-42".to_owned(),
            attributes: AlertAttributes {
                header: "Test header".to_owned(),
                description: Some("Test description".to_owned()),
                active_period: vec![ActivePeriod {
                    start: start.map(str::to_owned),
                    end: end.map(str::to_owned),
                }],
                effect: effect.to_owned(),
                informed_entity: vec![InformedEntity {
                    route: Some(route.to_owned()),
                }],
            },
        }
    }

    fn make_alert_no_period(route: &str, effect: &str) -> Alert {
        Alert {
            id: "alert-99".to_owned(),
            attributes: AlertAttributes {
                header: "Test header".to_owned(),
                description: None,
                active_period: vec![],
                effect: effect.to_owned(),
                informed_entity: vec![InformedEntity {
                    route: Some(route.to_owned()),
                }],
            },
        }
    }

    // --- next_date ---

    #[test]
    fn test_next_date_normal() {
        assert_eq!(next_date("2024-03-15"), "2024-03-16");
    }

    #[test]
    fn test_next_date_month_boundary() {
        assert_eq!(next_date("2024-01-31"), "2024-02-01");
    }

    #[test]
    fn test_next_date_year_boundary() {
        assert_eq!(next_date("2024-12-31"), "2025-01-01");
    }

    #[test]
    fn test_next_date_leap_day() {
        assert_eq!(next_date("2024-02-29"), "2024-03-01");
    }

    #[test]
    fn test_next_date_invalid_passthrough() {
        assert_eq!(next_date("not-a-date"), "not-a-date");
    }

    // --- event_times ---

    #[test]
    fn test_event_times_both_present() {
        let (start, end) = event_times(
            Some("2024-01-15T10:00:00-05:00"),
            Some("2024-01-15T22:00:00-05:00"),
        );
        assert_eq!(start, json!({ "dateTime": "2024-01-15T10:00:00-05:00" }));
        assert_eq!(end, json!({ "dateTime": "2024-01-15T22:00:00-05:00" }));
    }

    #[test]
    fn test_event_times_start_only_uses_date_format() {
        let (start, end) = event_times(Some("2024-01-15T10:00:00-05:00"), None);
        assert_eq!(start, json!({ "date": "2024-01-15" }));
        assert_eq!(end, json!({ "date": "2024-01-16" }));
    }

    #[test]
    fn test_event_times_start_only_month_boundary() {
        let (start, end) = event_times(Some("2024-03-31T08:00:00-04:00"), None);
        assert_eq!(start, json!({ "date": "2024-03-31" }));
        assert_eq!(end, json!({ "date": "2024-04-01" }));
    }

    #[test]
    fn test_event_times_neither_returns_today_tomorrow() {
        let (start, end) = event_times(None, None);
        assert!(start.get("date").is_some(), "start should have 'date' key");
        assert!(end.get("date").is_some(), "end should have 'date' key");
        // end date is day after start date
        let start_date = start["date"].as_str().unwrap();
        let end_date = end["date"].as_str().unwrap();
        assert_eq!(next_date(start_date), end_date);
    }

    // --- event_body ---

    #[test]
    fn test_event_body_summary_includes_line_and_effect() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let body = event_body(&alert);
        assert_eq!(body["summary"], "[Red Line] DELAY");
    }

    #[test]
    fn test_event_body_summary_green_line() {
        let alert = make_alert(
            "Green-B",
            "SUSPENSION",
            Some("2024-06-01T09:00:00-04:00"),
            None,
        );
        let body = event_body(&alert);
        assert_eq!(body["summary"], "[Green Line] SUSPENSION");
    }

    #[test]
    fn test_event_body_description() {
        let alert = make_alert(
            "Orange",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let body = event_body(&alert);
        assert_eq!(body["description"], "Test description");
    }

    #[test]
    fn test_event_body_datetimes_when_both_present() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let body = event_body(&alert);
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2024-06-01T09:00:00-04:00" })
        );
        assert_eq!(
            body["end"],
            json!({ "dateTime": "2024-06-01T23:00:00-04:00" })
        );
    }

    #[test]
    fn test_event_body_dates_when_no_end() {
        let alert = make_alert("Red", "DELAY", Some("2024-06-01T09:00:00-04:00"), None);
        let body = event_body(&alert);
        assert_eq!(body["start"], json!({ "date": "2024-06-01" }));
        assert_eq!(body["end"], json!({ "date": "2024-06-02" }));
    }

    #[test]
    fn test_event_body_extended_properties() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let body = event_body(&alert);
        let private = &body["extendedProperties"]["private"];
        assert_eq!(private["mbta_alert_source"], "true");
        assert_eq!(private["mbta_alert_id"], "alert-42");
    }

    #[test]
    fn test_event_body_no_period_falls_back_to_today() {
        let alert = make_alert_no_period("Orange", "SUSPENSION");
        let body = event_body(&alert);
        assert!(body["start"].get("date").is_some());
        assert!(body["end"].get("date").is_some());
    }

    // --- CalendarEvent::alert_id ---

    #[test]
    fn test_calendar_event_alert_id_present() {
        let mut private = HashMap::new();
        private.insert("mbta_alert_id".to_owned(), "alert-123".to_owned());
        let event = CalendarEvent {
            id: "event-1".to_owned(),
            extended_properties: Some(ExtendedProperties {
                private: Some(private),
            }),
        };
        assert_eq!(event.alert_id(), Some("alert-123"));
    }

    #[test]
    fn test_calendar_event_alert_id_missing_key() {
        let private = HashMap::new();
        let event = CalendarEvent {
            id: "event-1".to_owned(),
            extended_properties: Some(ExtendedProperties {
                private: Some(private),
            }),
        };
        assert_eq!(event.alert_id(), None);
    }

    #[test]
    fn test_calendar_event_alert_id_no_extended_properties() {
        let event = CalendarEvent {
            id: "event-1".to_owned(),
            extended_properties: None,
        };
        assert_eq!(event.alert_id(), None);
    }

    #[test]
    fn test_calendar_event_alert_id_no_private() {
        let event = CalendarEvent {
            id: "event-1".to_owned(),
            extended_properties: Some(ExtendedProperties { private: None }),
        };
        assert_eq!(event.alert_id(), None);
    }
}
