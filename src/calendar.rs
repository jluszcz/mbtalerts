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
            debug!("Updating event for alert {}", alert.id);
            cal.update_event(event_id, alert).await?;
        } else {
            debug!("Creating event for alert {}", alert.id);
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

fn event_time(dt: Option<&str>) -> Value {
    match dt {
        Some(s) => json!({ "dateTime": s }),
        None => json!({ "date": chrono::Utc::now().format("%Y-%m-%d").to_string() }),
    }
}

fn event_body(alert: &Alert) -> Value {
    let period = alert.attributes.active_period.first();
    let start = event_time(period.and_then(|p| p.start.as_deref()));
    let end = event_time(period.and_then(|p| p.end.as_deref()));

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
