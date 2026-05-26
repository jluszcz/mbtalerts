use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow};
use gcp_auth::{CustomServiceAccount, TokenProvider};
use log::{debug, info, warn};
use reqwest::{Client, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::ai::BedrockSummarizer;
use crate::summary::{LinePrefixMode, generate_or_fallback};
use crate::types::{Alert, Alerts};

const CAL_API: &str = "https://www.googleapis.com/calendar/v3/calendars";
const SCOPES: &[&str] = &["https://www.googleapis.com/auth/calendar.events"];

pub enum CalendarConfig {
    Single(String),
    PerLine {
        map: HashMap<String, String>,
        default: String,
    },
}

pub struct CalendarClient {
    token_provider: Arc<dyn TokenProvider>,
    config: CalendarConfig,
    client: Client,
    summarizer: Option<BedrockSummarizer>,
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
    fn get_private_property(&self, key: &str) -> Option<&str> {
        self.extended_properties
            .as_ref()?
            .private
            .as_ref()?
            .get(key)
            .map(String::as_str)
    }

    fn alert_id(&self) -> Option<&str> {
        self.get_private_property("mbta_alert_id")
    }

    fn ai_summary(&self) -> Option<&str> {
        self.get_private_property("mbta_ai_summary")
    }

    fn alert_state_hash(&self) -> Option<&str> {
        self.get_private_property("mbta_alert_state_hash")
    }
}

const CALENDAR_ID_SUFFIX: &str = "@group.calendar.google.com";

async fn check_status(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let url = response.url().clone();
    let body = response.text().await.unwrap_or_default();
    Err(anyhow!("HTTP {status} from {url}: {body}"))
}

fn normalize_calendar_id(id: String) -> String {
    if id.ends_with(CALENDAR_ID_SUFFIX) {
        id
    } else {
        format!("{id}{CALENDAR_ID_SUFFIX}")
    }
}

impl CalendarClient {
    pub async fn from_env() -> Result<Self> {
        let key_json = std::env::var("GOOGLE_SERVICE_ACCOUNT_KEY")
            .context("GOOGLE_SERVICE_ACCOUNT_KEY env var not set")?;
        let token_provider: Arc<dyn TokenProvider> =
            Arc::new(CustomServiceAccount::from_json(&key_json)?);

        let config = if let Ok(json_str) = std::env::var("GOOGLE_CALENDAR_IDS") {
            let raw: HashMap<String, String> =
                serde_json::from_str(&json_str).context("GOOGLE_CALENDAR_IDS is not valid JSON")?;
            let default = raw
                .get("default")
                .cloned()
                .map(normalize_calendar_id)
                .context("GOOGLE_CALENDAR_IDS must include a \"default\" key")?;
            let map = raw
                .into_iter()
                .filter(|(k, _)| k != "default")
                .map(|(k, v)| (k, normalize_calendar_id(v)))
                .collect();
            CalendarConfig::PerLine { map, default }
        } else {
            let id = std::env::var("GOOGLE_CALENDAR_ID")
                .context("Either GOOGLE_CALENDAR_IDS or GOOGLE_CALENDAR_ID env var must be set")?;
            CalendarConfig::Single(normalize_calendar_id(id))
        };

        let summarizer = BedrockSummarizer::from_env().await;

        Ok(Self {
            token_provider,
            config,
            client: Client::new(),
            summarizer,
        })
    }

    async fn access_token(&self) -> Result<String> {
        let token = self.token_provider.token(SCOPES).await?;
        Ok(token.as_str().to_owned())
    }

    async fn send_authenticated(&self, req: reqwest::RequestBuilder) -> Result<Response> {
        let token = self.access_token().await?;
        check_status(req.bearer_auth(&token).send().await?).await
    }

    async fn list_alert_events(&self, calendar_id: &str) -> Result<Vec<CalendarEvent>> {
        let token = self.access_token().await?;
        let mut events = Vec::new();
        let mut page_token: Option<String> = None;
        let time_min = chrono::Utc::now().to_rfc3339();
        let events_url = format!("{}/{}/events", CAL_API, calendar_id);

        debug!("Listing calendar events for {calendar_id}");
        loop {
            let mut req = self.client.get(&events_url).bearer_auth(&token).query(&[
                ("privateExtendedProperty", "mbta_alert_source=true"),
                ("timeMin", &time_min),
            ]);

            if let Some(pt) = &page_token {
                req = req.query(&[("pageToken", pt.as_str())]);
            }

            let response: EventList = check_status(req.send().await?).await?.json().await?;
            events.extend(response.items);

            match response.next_page_token {
                Some(pt) => page_token = Some(pt),
                None => break,
            }
        }
        info!("Listed {} calendar events for {calendar_id}", events.len());

        Ok(events)
    }

    async fn create_event(
        &self,
        calendar_id: &str,
        alert: &Alert,
        summary: &str,
        ai_summary_raw: Option<&str>,
    ) -> Result<()> {
        let events_url = format!("{}/{}/events", CAL_API, calendar_id);
        let body = event_body(alert, summary, ai_summary_raw)?;
        self.send_authenticated(self.client.post(&events_url).json(&body))
            .await?;
        info!("Created calendar event for alert {}", alert.id);
        Ok(())
    }

    async fn update_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        alert: &Alert,
        summary: &str,
        ai_summary_raw: Option<&str>,
    ) -> Result<()> {
        let event_url = format!("{}/{}/events/{}", CAL_API, calendar_id, event_id);
        let body = event_body(alert, summary, ai_summary_raw)?;
        self.send_authenticated(self.client.put(&event_url).json(&body))
            .await?;
        info!("Updated calendar event {event_id} for alert {}", alert.id);
        Ok(())
    }

    async fn delete_event(&self, calendar_id: &str, event_id: &str) -> Result<()> {
        let event_url = format!("{}/{}/events/{}", CAL_API, calendar_id, event_id);
        self.send_authenticated(self.client.delete(&event_url))
            .await?;
        info!("Deleted calendar event {event_id}");
        Ok(())
    }
}

const STATION_EFFECTS_TO_SKIP: &[&str] = &[
    "STATION_ISSUE",
    "STOP_CLOSURE",
    "STATION_CLOSURE",
    "PARKING_ISSUE",
];

pub fn should_sync_alert(alert: &Alert) -> bool {
    !STATION_EFFECTS_TO_SKIP.contains(&alert.attributes.effect.as_str())
}

fn calendar_ids_for_alert<'a>(alert: &Alert, config: &'a CalendarConfig) -> Vec<&'a str> {
    match config {
        CalendarConfig::Single(id) => vec![id.as_str()],
        CalendarConfig::PerLine { map, default } => {
            let mut ids: HashSet<&str> = HashSet::new();
            let mut found_any_route = false;

            for entity in &alert.attributes.informed_entity {
                let Some(route) = &entity.route else {
                    continue;
                };
                found_any_route = true;
                match crate::canonical_line(route) {
                    Some(line) => {
                        if let Some(id) = map.get(line) {
                            ids.insert(id.as_str());
                        } else {
                            warn!(
                                "Alert {}: line '{}' not in GOOGLE_CALENDAR_IDS, using default",
                                alert.id, line
                            );
                            ids.insert(default.as_str());
                        }
                    }
                    None => {
                        warn!(
                            "Alert {}: unknown route '{}', using default",
                            alert.id, route
                        );
                        ids.insert(default.as_str());
                    }
                }
            }

            if !found_any_route {
                warn!(
                    "Alert {}: no route information found, using default calendar",
                    alert.id
                );
                ids.insert(default.as_str());
            }

            ids.into_iter().collect()
        }
    }
}

pub async fn sync_alerts(alerts: &Alerts, cal: &CalendarClient) -> Result<()> {
    let calendar_ids: HashSet<&str> = match &cal.config {
        CalendarConfig::Single(id) => std::iter::once(id.as_str()).collect(),
        CalendarConfig::PerLine { map, default } => map
            .values()
            .map(String::as_str)
            .chain(std::iter::once(default.as_str()))
            .collect(),
    };

    let sync_alerts: Vec<&Alert> = alerts
        .data
        .iter()
        .filter(|a| {
            if !should_sync_alert(a) {
                debug!(
                    "Skipping station issue alert {}: {}",
                    a.id, a.attributes.effect
                );
                false
            } else {
                true
            }
        })
        .collect();

    let tasks = calendar_ids.into_iter().map(|calendar_id| {
        let cal_alerts: Vec<&Alert> = sync_alerts
            .iter()
            .copied()
            .filter(|a| calendar_ids_for_alert(a, &cal.config).contains(&calendar_id))
            .collect();
        async move { sync_calendar(cal, calendar_id, &cal_alerts).await }
    });

    futures::future::try_join_all(tasks).await?;

    Ok(())
}

fn line_prefix_for_alert(
    alert: &Alert,
    calendar_id: &str,
    config: &CalendarConfig,
) -> LinePrefixMode {
    let CalendarConfig::PerLine { map, .. } = config else {
        return LinePrefixMode::Include;
    };
    for entity in &alert.attributes.informed_entity {
        let Some(route) = &entity.route else { continue };
        if let Some(line) = crate::canonical_line(route)
            && map.get(line).map(String::as_str) == Some(calendar_id)
        {
            return LinePrefixMode::Omit;
        }
    }
    LinePrefixMode::Include
}

struct ExistingEvent {
    event_id: String,
    ai_summary: Option<String>,
    state_hash: Option<String>,
}

struct SyncPlan<'a> {
    to_create: Vec<&'a Alert>,
    to_update: Vec<(String, &'a Alert)>, // (event_id, alert)
    to_delete: Vec<String>,              // event_id
}

fn plan_calendar_sync<'a>(
    existing_by_alert_id: &HashMap<String, ExistingEvent>,
    alerts: &[&'a Alert],
) -> SyncPlan<'a> {
    let mut to_create = Vec::new();
    let mut to_update = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for alert in alerts {
        let current_hash = event_state_hash(alert);
        match existing_by_alert_id.get(&alert.id) {
            Some(ExistingEvent {
                ai_summary: Some(_),
                state_hash: Some(cached_hash),
                ..
            }) if *cached_hash == current_hash => {
                // Event exists and summary is already up-to-date; no write needed.
            }
            Some(ExistingEvent { event_id, .. }) => {
                to_update.push((event_id.clone(), *alert));
            }
            None => {
                to_create.push(*alert);
            }
        }
        seen.insert(alert.id.clone());
    }

    let to_delete = existing_by_alert_id
        .iter()
        .filter(|(alert_id, _)| !seen.contains(*alert_id))
        .map(|(_, existing)| existing.event_id.clone())
        .collect();

    SyncPlan {
        to_create,
        to_update,
        to_delete,
    }
}

async fn sync_calendar(cal: &CalendarClient, calendar_id: &str, alerts: &[&Alert]) -> Result<()> {
    let existing = cal.list_alert_events(calendar_id).await?;

    let mut existing_by_alert_id: HashMap<String, ExistingEvent> = HashMap::new();
    for event in &existing {
        if let Some(alert_id) = event.alert_id() {
            existing_by_alert_id.insert(
                alert_id.to_owned(),
                ExistingEvent {
                    event_id: event.id.clone(),
                    ai_summary: event.ai_summary().map(str::to_owned),
                    state_hash: event.alert_state_hash().map(str::to_owned),
                },
            );
        }
    }

    let plan = plan_calendar_sync(&existing_by_alert_id, alerts);

    for alert in plan.to_create {
        let line_prefix = line_prefix_for_alert(alert, calendar_id, &cal.config);
        let (ai_summary_raw, display_summary) =
            generate_or_fallback(cal.summarizer.as_ref(), alert, line_prefix).await;
        cal.create_event(
            calendar_id,
            alert,
            &display_summary,
            ai_summary_raw.as_deref(),
        )
        .await?;
    }

    for (event_id, alert) in &plan.to_update {
        let line_prefix = line_prefix_for_alert(alert, calendar_id, &cal.config);
        let (ai_summary_raw, display_summary) =
            generate_or_fallback(cal.summarizer.as_ref(), alert, line_prefix).await;
        cal.update_event(
            calendar_id,
            event_id,
            alert,
            &display_summary,
            ai_summary_raw.as_deref(),
        )
        .await?;
    }

    for event_id in &plan.to_delete {
        cal.delete_event(calendar_id, event_id).await?;
    }

    Ok(())
}

fn next_date(date: &str) -> Result<String> {
    let parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("Failed to parse date {date:?} as %Y-%m-%d"))?;
    Ok((parsed + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string())
}

fn event_times(start: Option<&str>, end: Option<&str>) -> Result<(Value, Value)> {
    match (start, end) {
        (Some(s), Some(e)) => Ok((
            json!({ "dateTime": s, "timeZone": "America/New_York" }),
            json!({ "dateTime": e, "timeZone": "America/New_York" }),
        )),
        (Some(s), None) => {
            // Open-ended alert: all-day event on the start date (end is exclusive in Google Calendar)
            let date = s.get(..10).unwrap_or(s);
            Ok((json!({ "date": date }), json!({ "date": next_date(date)? })))
        }
        _ => {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let tomorrow = next_date(&today)?;
            Ok((json!({ "date": today }), json!({ "date": tomorrow })))
        }
    }
}

/// Builds the calendar event description from available alert fields.
///
/// Always includes the alert header. Appends the full description and URL
/// on separate sections when present.
fn event_description(alert: &Alert) -> String {
    let mut parts = vec![alert.attributes.header.trim().to_owned()];
    if let Some(desc) = &alert.attributes.description {
        parts.push(desc.trim().to_owned());
    }
    if let Some(url) = &alert.attributes.url {
        parts.push(url.clone());
    }
    parts.join("\n\n")
}

/// FNV-1a 64-bit hash over header, description, url, and active period bounds —
/// deterministic across platforms and Rust versions.
fn event_state_hash(alert: &Alert) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let feed = |hash: &mut u64, s: &str| {
        for byte in s.bytes() {
            *hash ^= byte as u64;
            *hash = hash.wrapping_mul(0x100000001b3);
        }
        // separator to prevent "ab"+"c" == "a"+"bc"
        *hash ^= 0xff;
        *hash = hash.wrapping_mul(0x100000001b3);
    };
    feed(&mut hash, &alert.attributes.header);
    feed(
        &mut hash,
        alert.attributes.description.as_deref().unwrap_or(""),
    );
    feed(&mut hash, alert.attributes.url.as_deref().unwrap_or(""));
    feed(&mut hash, alert.period_start().unwrap_or(""));
    feed(&mut hash, alert.period_end().unwrap_or(""));
    hash.to_string()
}

fn event_body(alert: &Alert, summary: &str, ai_summary_raw: Option<&str>) -> Result<Value> {
    let (start, end) = event_times(alert.period_start(), alert.period_end())?;

    let mut private = serde_json::Map::new();
    private.insert("mbta_alert_source".to_owned(), json!("true"));
    private.insert("mbta_alert_id".to_owned(), json!(alert.id));
    if let Some(raw) = ai_summary_raw {
        private.insert("mbta_ai_summary".to_owned(), json!(raw));
        private.insert(
            "mbta_alert_state_hash".to_owned(),
            json!(event_state_hash(alert)),
        );
    }

    Ok(json!({
        "summary": summary,
        "description": event_description(alert),
        "start": start,
        "end": end,
        "transparency": "transparent",
        "extendedProperties": {
            "private": Value::Object(private)
        }
    }))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::summary::event_summary;
    use crate::types::{ActivePeriod, AlertAttributes, InformedEntity};

    fn make_alert(route: &str, effect: &str, start: Option<&str>, end: Option<&str>) -> Alert {
        Alert {
            id: "alert-42".to_owned(),
            attributes: AlertAttributes {
                header: "Test header".to_owned(),
                description: Some("Test description".to_owned()),
                url: None,
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
                url: None,
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
        assert_eq!(next_date("2024-03-15").unwrap(), "2024-03-16");
    }

    #[test]
    fn test_next_date_month_boundary() {
        assert_eq!(next_date("2024-01-31").unwrap(), "2024-02-01");
    }

    #[test]
    fn test_next_date_year_boundary() {
        assert_eq!(next_date("2024-12-31").unwrap(), "2025-01-01");
    }

    #[test]
    fn test_next_date_leap_day() {
        assert_eq!(next_date("2024-02-29").unwrap(), "2024-03-01");
    }

    #[test]
    fn test_next_date_invalid_errors() {
        assert!(next_date("not-a-date").is_err());
    }

    // --- event_times ---

    #[test]
    fn test_event_times_both_present() {
        let (start, end) = event_times(
            Some("2024-01-15T10:00:00-05:00"),
            Some("2024-01-15T22:00:00-05:00"),
        )
        .unwrap();
        assert_eq!(
            start,
            json!({ "dateTime": "2024-01-15T10:00:00-05:00", "timeZone": "America/New_York" })
        );
        assert_eq!(
            end,
            json!({ "dateTime": "2024-01-15T22:00:00-05:00", "timeZone": "America/New_York" })
        );
    }

    #[test]
    fn test_event_times_start_only_uses_date_format() {
        let (start, end) = event_times(Some("2024-01-15T10:00:00-05:00"), None).unwrap();
        assert_eq!(start, json!({ "date": "2024-01-15" }));
        assert_eq!(end, json!({ "date": "2024-01-16" }));
    }

    #[test]
    fn test_event_times_start_only_month_boundary() {
        let (start, end) = event_times(Some("2024-03-31T08:00:00-04:00"), None).unwrap();
        assert_eq!(start, json!({ "date": "2024-03-31" }));
        assert_eq!(end, json!({ "date": "2024-04-01" }));
    }

    #[test]
    fn test_event_times_neither_returns_today_tomorrow() {
        let (start, end) = event_times(None, None).unwrap();
        assert!(start.get("date").is_some(), "start should have 'date' key");
        assert!(end.get("date").is_some(), "end should have 'date' key");
        // end date is day after start date
        let start_date = start["date"].as_str().unwrap();
        let end_date = end["date"].as_str().unwrap();
        assert_eq!(next_date(start_date).unwrap(), end_date);
    }

    // --- event_body ---

    #[test]
    fn test_event_body_summary_delay_no_duration_uses_first_sentence() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(body["summary"], "[Red Line] Test header");
    }

    #[test]
    fn test_event_body_summary_delay_about_minutes() {
        let mut alert = make_alert(
            "Red",
            "DELAY",
            Some("2026-02-23T05:49:00-05:00"),
            Some("2026-02-23T13:47:00-05:00"),
        );
        alert.attributes.header = "Red Line Braintree Branch: Delays of about 20 minutes due to a signal problem at Braintree.".to_owned();
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(body["summary"], "[Red Line] Delay ~20 minutes");
    }

    #[test]
    fn test_event_body_summary_delay_up_to_minutes() {
        let mut alert = make_alert(
            "Blue",
            "DELAY",
            Some("2026-02-23T11:16:00-05:00"),
            Some("2026-02-23T13:47:00-05:00"),
        );
        alert.attributes.header = "Blue Line: Delays of up to 20 minutes due to signal problem near Wonderland. Trains may stand by at stations.".to_owned();
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(body["summary"], "[Blue Line] Delay ~20 minutes");
    }

    #[test]
    fn test_event_body_summary_suspension_uses_first_sentence() {
        let alert = make_alert(
            "Green-B",
            "SUSPENSION",
            Some("2024-06-01T09:00:00-04:00"),
            None,
        );
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(body["summary"], "[Green Line] Test header");
    }

    #[test]
    fn test_event_body_summary_service_change_no_location_uses_first_sentence() {
        let mut alert = make_alert(
            "MBTA",
            "SERVICE_CHANGE",
            Some("2026-02-23T03:00:00-05:00"),
            Some("2026-02-24T02:59:00-05:00"),
        );
        alert.attributes.header = "Due to severe weather, Subway, Bus, and Commuter Rail are operating on a reduced schedule. Ferry service is canceled.".to_owned();
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(
            body["summary"],
            "[MBTA] Due to severe weather, Subway, Bus, and Commuter Rail are operating on a reduced schedule"
        );
    }

    #[test]
    fn test_event_body_summary_shuttle_with_location() {
        let mut alert = make_alert(
            "Red",
            "SHUTTLE",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        alert.attributes.header = "Red Line: Shuttle buses will replace service between Broadway and Ashmont this weekend.".to_owned();
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(
            body["summary"],
            "[Red Line] Shuttle between Broadway and Ashmont"
        );
    }

    #[test]
    fn test_event_body_summary_station_issue_uses_first_sentence() {
        let mut alert = make_alert(
            "Orange",
            "STATION_ISSUE",
            Some("2025-06-30T03:00:00-04:00"),
            None,
        );
        alert.attributes.header = "Jackson Square: The stairway connecting the Jackson Sq lobby and the south end of the platform is closed until winter 2026. Use the stairway at the north end of the platform.".to_owned();
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(
            body["summary"],
            "[Orange Line] Jackson Square: The stairway connecting the Jackson Sq lobby and the south end of the platform is closed until winter 2026"
        );
    }

    #[test]
    fn test_event_body_summary_shuttle_with_due_cause() {
        let mut alert = make_alert(
            "Blue",
            "SHUTTLE",
            Some("2026-02-25T05:13:00-05:00"),
            Some("2026-02-25T08:27:00-05:00"),
        );
        alert.attributes.header = "Blue Line: Shuttle buses replacing service between Suffolk Downs and Maverick due to a power problem at Airport.".to_owned();
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(
            body["summary"],
            "[Blue Line] Shuttle between Suffolk Downs and Maverick"
        );
    }

    #[test]
    fn test_event_body_summary_service_change_between() {
        let mut alert = make_alert(
            "Red",
            "SERVICE_CHANGE",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        alert.attributes.header = "Red Line Ashmont Branch: Service between JFK/UMass and Ashmont will operate with two shuttle trains from April 10 - 30 to allow for critical track work.".to_owned();
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(
            body["summary"],
            "[Red Line] Service change between JFK/UMass and Ashmont"
        );
    }

    // --- line_prefix_for_alert ---

    #[test]
    fn test_line_prefix_single_config_always_include() {
        let alert = make_alert("Red", "DELAY", None, None);
        let config = CalendarConfig::Single("cal".to_string());
        assert!(matches!(
            line_prefix_for_alert(&alert, "cal", &config),
            LinePrefixMode::Include
        ));
    }

    #[test]
    fn test_line_prefix_definitive_match_omits() {
        let alert = make_alert("Red", "DELAY", None, None);
        assert!(matches!(
            line_prefix_for_alert(&alert, "cal-red", &per_line_config()),
            LinePrefixMode::Omit
        ));
    }

    #[test]
    fn test_line_prefix_fallback_no_route_includes() {
        let alert = Alert {
            id: "x".to_owned(),
            attributes: AlertAttributes {
                header: "Test".to_owned(),
                description: None,
                url: None,
                active_period: vec![],
                effect: "DELAY".to_owned(),
                informed_entity: vec![InformedEntity { route: None }],
            },
        };
        assert!(matches!(
            line_prefix_for_alert(&alert, "cal-default", &per_line_config()),
            LinePrefixMode::Include
        ));
    }

    #[test]
    fn test_line_prefix_known_line_not_in_map_includes() {
        // Line is identified as Red but Red has no calendar mapping
        let alert = make_alert("Red", "DELAY", None, None);
        let config = CalendarConfig::PerLine {
            map: [("Orange".to_owned(), "cal-orange".to_owned())].into(),
            default: "cal-default".to_owned(),
        };
        assert!(matches!(
            line_prefix_for_alert(&alert, "cal-default", &config),
            LinePrefixMode::Include
        ));
    }

    #[test]
    fn test_line_prefix_red_alert_on_default_calendar_includes() {
        // Red is mapped to cal-red; a Red alert on the default calendar → Include
        let alert = make_alert("Red", "DELAY", None, None);
        assert!(matches!(
            line_prefix_for_alert(&alert, "cal-default", &per_line_config()),
            LinePrefixMode::Include
        ));
    }

    // --- event_description ---

    #[test]
    fn test_event_description_header_only() {
        let mut alert = make_alert("Red", "DELAY", None, None);
        alert.attributes.description = None;
        assert_eq!(event_description(&alert), "Test header");
    }

    #[test]
    fn test_event_description_header_and_description() {
        let alert = make_alert("Red", "DELAY", None, None);
        // make_alert sets description = Some("Test description")
        assert_eq!(event_description(&alert), "Test header\n\nTest description");
    }

    #[test]
    fn test_event_description_header_and_url() {
        let mut alert = make_alert("Red", "DELAY", None, None);
        alert.attributes.description = None;
        alert.attributes.url = Some("https://mbta.com/RedLine".to_owned());
        assert_eq!(
            event_description(&alert),
            "Test header\n\nhttps://mbta.com/RedLine"
        );
    }

    #[test]
    fn test_event_description_all_fields() {
        let mut alert = make_alert("Red", "SHUTTLE", None, None);
        alert.attributes.url = Some("https://mbta.com/RedLine".to_owned());
        assert_eq!(
            event_description(&alert),
            "Test header\n\nTest description\n\nhttps://mbta.com/RedLine"
        );
    }

    #[test]
    fn test_event_description_trims_whitespace() {
        let mut alert = make_alert("Red", "DELAY", None, None);
        alert.attributes.header = "  Header with spaces  ".to_owned();
        alert.attributes.description = Some("  Description with spaces  ".to_owned());
        assert_eq!(
            event_description(&alert),
            "Header with spaces\n\nDescription with spaces"
        );
    }

    // --- STATION_EFFECTS_TO_SKIP ---

    #[test]
    fn test_station_issue_is_skipped() {
        assert!(STATION_EFFECTS_TO_SKIP.contains(&"STATION_ISSUE"));
    }

    #[test]
    fn test_stop_closure_is_skipped() {
        assert!(STATION_EFFECTS_TO_SKIP.contains(&"STOP_CLOSURE"));
    }

    #[test]
    fn test_station_closure_is_skipped() {
        assert!(STATION_EFFECTS_TO_SKIP.contains(&"STATION_CLOSURE"));
    }

    #[test]
    fn test_parking_issue_is_skipped() {
        assert!(STATION_EFFECTS_TO_SKIP.contains(&"PARKING_ISSUE"));
    }

    #[test]
    fn test_shuttle_is_not_skipped() {
        assert!(!STATION_EFFECTS_TO_SKIP.contains(&"SHUTTLE"));
    }

    #[test]
    fn test_event_body_description() {
        let alert = make_alert(
            "Orange",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(body["description"], "Test header\n\nTest description");
    }

    #[test]
    fn test_event_body_datetimes_when_both_present() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2024-06-01T09:00:00-04:00", "timeZone": "America/New_York" })
        );
        assert_eq!(
            body["end"],
            json!({ "dateTime": "2024-06-01T23:00:00-04:00", "timeZone": "America/New_York" })
        );
    }

    #[test]
    fn test_event_body_dates_when_no_end() {
        let alert = make_alert("Red", "DELAY", Some("2024-06-01T09:00:00-04:00"), None);
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert_eq!(body["start"], json!({ "date": "2024-06-01" }));
        assert_eq!(body["end"], json!({ "date": "2024-06-02" }));
    }

    #[test]
    fn test_event_body_extended_properties_without_ai_summary() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        let private = &body["extendedProperties"]["private"];
        assert_eq!(private["mbta_alert_source"], "true");
        assert_eq!(private["mbta_alert_id"], "alert-42");
        assert!(private.get("mbta_ai_summary").is_none());
    }

    #[test]
    fn test_event_body_extended_properties_with_ai_summary() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let body = event_body(&alert, "AI-generated title", Some("AI-generated title")).unwrap();
        let private = &body["extendedProperties"]["private"];
        assert_eq!(private["mbta_alert_source"], "true");
        assert_eq!(private["mbta_alert_id"], "alert-42");
        assert_eq!(private["mbta_ai_summary"], "AI-generated title");
    }

    #[test]
    fn test_event_body_no_period_falls_back_to_today() {
        let alert = make_alert_no_period("Orange", "SUSPENSION");
        let summary = event_summary(&alert, LinePrefixMode::Include);
        let body = event_body(&alert, &summary, None).unwrap();
        assert!(body["start"].get("date").is_some());
        assert!(body["end"].get("date").is_some());
    }

    // --- normalize_calendar_id ---

    #[test]
    fn test_normalize_calendar_id_already_suffixed() {
        let id = "abc123@group.calendar.google.com".to_owned();
        assert_eq!(normalize_calendar_id(id.clone()), id);
    }

    #[test]
    fn test_normalize_calendar_id_bare_adds_suffix() {
        assert_eq!(
            normalize_calendar_id("abc123".to_owned()),
            "abc123@group.calendar.google.com"
        );
    }

    // --- calendar_ids_for_alert ---

    fn per_line_config() -> CalendarConfig {
        CalendarConfig::PerLine {
            map: [
                ("Red".to_owned(), "cal-red".to_owned()),
                ("Orange".to_owned(), "cal-orange".to_owned()),
                ("Blue".to_owned(), "cal-blue".to_owned()),
                ("Green".to_owned(), "cal-green".to_owned()),
            ]
            .into(),
            default: "cal-default".to_owned(),
        }
    }

    fn make_alert_multi_route(routes: &[&str], effect: &str) -> Alert {
        Alert {
            id: "alert-multi".to_owned(),
            attributes: AlertAttributes {
                header: "Test header".to_owned(),
                description: None,
                url: None,
                active_period: vec![],
                effect: effect.to_owned(),
                informed_entity: routes
                    .iter()
                    .map(|r| InformedEntity {
                        route: Some(r.to_string()),
                    })
                    .collect(),
            },
        }
    }

    fn make_alert_no_route(effect: &str) -> Alert {
        Alert {
            id: "alert-no-route".to_owned(),
            attributes: AlertAttributes {
                header: "Test header".to_owned(),
                description: None,
                url: None,
                active_period: vec![],
                effect: effect.to_owned(),
                informed_entity: vec![InformedEntity { route: None }],
            },
        }
    }

    #[test]
    fn test_calendar_ids_single_config_always_returns_single() {
        let config = CalendarConfig::Single("cal-all".to_owned());
        let alert = make_alert("Red", "DELAY", None, None);
        assert_eq!(calendar_ids_for_alert(&alert, &config), vec!["cal-all"]);
    }

    #[test]
    fn test_calendar_ids_per_line_single_route() {
        let config = per_line_config();
        let alert = make_alert("Blue", "DELAY", None, None);
        assert_eq!(calendar_ids_for_alert(&alert, &config), vec!["cal-blue"]);
    }

    #[test]
    fn test_calendar_ids_per_line_green_branch_maps_to_green() {
        let config = per_line_config();
        let alert = make_alert("Green-D", "SHUTTLE", None, None);
        assert_eq!(calendar_ids_for_alert(&alert, &config), vec!["cal-green"]);
    }

    #[test]
    fn test_calendar_ids_per_line_multi_route_returns_both() {
        let config = per_line_config();
        let alert = make_alert_multi_route(&["Red", "Orange"], "DELAY");
        let mut ids = calendar_ids_for_alert(&alert, &config);
        ids.sort();
        assert_eq!(ids, vec!["cal-orange", "cal-red"]);
    }

    #[test]
    fn test_calendar_ids_per_line_no_route_returns_default() {
        let config = per_line_config();
        let alert = make_alert_no_route("DELAY");
        assert_eq!(calendar_ids_for_alert(&alert, &config), vec!["cal-default"]);
    }

    #[test]
    fn test_calendar_ids_per_line_unknown_route_returns_default() {
        let config = per_line_config();
        let alert = make_alert("CR-Fitchburg", "DELAY", None, None);
        assert_eq!(calendar_ids_for_alert(&alert, &config), vec!["cal-default"]);
    }

    #[test]
    fn test_calendar_ids_per_line_unmapped_line_returns_default() {
        let config = CalendarConfig::PerLine {
            map: [("Red".to_owned(), "cal-red".to_owned())].into(),
            default: "cal-default".to_owned(),
        };
        let alert = make_alert("Blue", "DELAY", None, None);
        assert_eq!(calendar_ids_for_alert(&alert, &config), vec!["cal-default"]);
    }

    #[test]
    fn test_calendar_ids_per_line_deduplicates_same_calendar() {
        let config = CalendarConfig::PerLine {
            map: [
                ("Red".to_owned(), "cal-shared".to_owned()),
                ("Orange".to_owned(), "cal-shared".to_owned()),
            ]
            .into(),
            default: "cal-default".to_owned(),
        };
        let alert = make_alert_multi_route(&["Red", "Orange"], "DELAY");
        assert_eq!(calendar_ids_for_alert(&alert, &config), vec!["cal-shared"]);
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

    // --- plan_calendar_sync ---

    fn make_existing(
        alert_id: &str,
        event_id: &str,
        ai_summary: Option<&str>,
        hash: Option<&str>,
    ) -> HashMap<String, ExistingEvent> {
        [(
            alert_id.to_owned(),
            ExistingEvent {
                event_id: event_id.to_owned(),
                ai_summary: ai_summary.map(str::to_owned),
                state_hash: hash.map(str::to_owned),
            },
        )]
        .into()
    }

    #[test]
    fn test_plan_skip_when_hash_and_summary_match() {
        let alert = make_alert("Red", "DELAY", None, None);
        let current_hash = event_state_hash(&alert);
        let existing = make_existing(
            &alert.id,
            "event-1",
            Some("AI summary"),
            Some(&current_hash),
        );

        let plan = plan_calendar_sync(&existing, &[&alert]);

        assert!(plan.to_create.is_empty(), "no creates expected");
        assert!(plan.to_update.is_empty(), "no updates expected");
        assert!(plan.to_delete.is_empty(), "no deletes expected");
    }

    #[test]
    fn test_plan_update_when_hash_changed() {
        let alert = make_alert("Red", "DELAY", None, None);
        let existing = make_existing(
            &alert.id,
            "event-1",
            Some("Old summary"),
            Some("stale-hash"),
        );

        let plan = plan_calendar_sync(&existing, &[&alert]);

        assert!(plan.to_create.is_empty());
        assert_eq!(plan.to_update.len(), 1);
        assert_eq!(plan.to_update[0].0, "event-1");
        assert!(plan.to_delete.is_empty());
    }

    #[test]
    fn test_plan_update_when_ai_summary_missing() {
        // Event exists with matching hash but no AI summary — needs update to populate it.
        let alert = make_alert("Red", "DELAY", None, None);
        let current_hash = event_state_hash(&alert);
        let existing = make_existing(&alert.id, "event-1", None, Some(&current_hash));

        let plan = plan_calendar_sync(&existing, &[&alert]);

        assert!(plan.to_create.is_empty());
        assert_eq!(plan.to_update.len(), 1);
    }

    #[test]
    fn test_plan_create_for_new_alert() {
        let alert = make_alert("Red", "DELAY", None, None);
        let existing = HashMap::new();

        let plan = plan_calendar_sync(&existing, &[&alert]);

        assert_eq!(plan.to_create.len(), 1);
        assert!(plan.to_update.is_empty());
        assert!(plan.to_delete.is_empty());
    }

    #[test]
    fn test_plan_delete_stale_event() {
        let existing = make_existing("stale-alert", "event-99", Some("summary"), Some("hash"));

        let plan = plan_calendar_sync(&existing, &[]);

        assert!(plan.to_create.is_empty());
        assert!(plan.to_update.is_empty());
        assert_eq!(plan.to_delete.len(), 1);
        assert_eq!(plan.to_delete[0], "event-99");
    }

    #[test]
    fn test_plan_mixed_create_update_skip_delete() {
        let alert_skip = make_alert("Red", "DELAY", None, None);
        let mut alert_update = make_alert("Blue", "SUSPENSION", None, None);
        alert_update.id = "alert-update".to_owned();
        let mut alert_create = make_alert("Orange", "SHUTTLE", None, None);
        alert_create.id = "alert-create".to_owned();

        let skip_hash = event_state_hash(&alert_skip);
        let existing: HashMap<String, ExistingEvent> = [
            (
                alert_skip.id.clone(),
                ExistingEvent {
                    event_id: "event-skip".to_owned(),
                    ai_summary: Some("summary".to_owned()),
                    state_hash: Some(skip_hash),
                },
            ),
            (
                alert_update.id.clone(),
                ExistingEvent {
                    event_id: "event-update".to_owned(),
                    ai_summary: Some("old".to_owned()),
                    state_hash: Some("stale".to_owned()),
                },
            ),
            (
                "stale-alert".to_owned(),
                ExistingEvent {
                    event_id: "event-stale".to_owned(),
                    ai_summary: Some("x".to_owned()),
                    state_hash: Some("h".to_owned()),
                },
            ),
        ]
        .into();

        let plan = plan_calendar_sync(&existing, &[&alert_skip, &alert_update, &alert_create]);

        assert_eq!(plan.to_create.len(), 1);
        assert_eq!(plan.to_create[0].id, "alert-create");
        assert_eq!(plan.to_update.len(), 1);
        assert_eq!(plan.to_update[0].0, "event-update");
        assert_eq!(plan.to_delete.len(), 1);
        assert_eq!(plan.to_delete[0], "event-stale");
    }
}
