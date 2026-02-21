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

struct SecretString(String);

impl SecretString {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

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
        let key_json = std::env::var("GOOGLE_SERVICE_ACCOUNT_KEY")
            .map(SecretString)
            .context("GOOGLE_SERVICE_ACCOUNT_KEY env var not set")?;
        let token_provider: Arc<dyn TokenProvider> =
            Arc::new(CustomServiceAccount::from_json(key_json.expose())?);

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

    async fn send_authenticated(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let token = self.access_token().await?;
        Ok(req.bearer_auth(&token).send().await?.error_for_status()?)
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
        self.send_authenticated(self.client.post(self.events_url()).json(&event_body(alert)))
            .await?;
        Ok(())
    }

    async fn update_event(&self, event_id: &str, alert: &Alert) -> Result<()> {
        self.send_authenticated(
            self.client
                .put(self.event_url(event_id))
                .json(&event_body(alert)),
        )
        .await?;
        Ok(())
    }

    async fn delete_event(&self, event_id: &str) -> Result<()> {
        self.send_authenticated(self.client.delete(self.event_url(event_id)))
            .await?;
        Ok(())
    }
}

const STATION_EFFECTS_TO_SKIP: &[&str] = &["STATION_ISSUE", "STOP_CLOSURE", "STATION_CLOSURE"];

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
        if STATION_EFFECTS_TO_SKIP.contains(&alert.attributes.effect.as_str()) {
            debug!(
                "Skipping station issue alert {}: {}",
                alert.id, alert.attributes.effect
            );
            continue;
        }
        let summary = event_summary(alert);
        if let Some(event_id) = existing_by_alert_id.get(&alert.id) {
            debug!("Updating event for alert {}: {}", alert.id, summary);
            cal.update_event(event_id, alert).await?;
        } else {
            debug!("Creating event for alert {}: {}", alert.id, summary);
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

fn strip_line_prefix(header: &str) -> &str {
    if let Some(colon_idx) = header.find(": ") {
        let prefix = &header[..colon_idx];
        if prefix.contains("Line") && prefix.len() <= 35 {
            return header[colon_idx + 2..].trim_start();
        }
    }
    header.trim_start()
}

/// Truncate content at boilerplate rationale clauses and the first ", " after 30 chars.
fn brief_content(content: &str) -> String {
    let boilerplate_phrases = [" to allow", " in order to"];
    let mut end = content.len();

    for phrase in &boilerplate_phrases {
        if let Some(idx) = content.find(phrase) {
            end = end.min(idx);
        }
    }

    // Truncate at first ", " after 30 chars
    let scan_from = 30.min(end);
    if let Some(rel_idx) = content[scan_from..end].find(", ") {
        end = scan_from + rel_idx;
    }

    content[..end].trim_end_matches(['.', ',', ' ']).to_string()
}

const LOCATION_STOP_MARKERS: &[&str] = &[
    ", ",
    " will ",
    " this ",
    " that ",
    " from ",
    " to allow",
    " in order",
    " starting",
    " during",
];

/// Extract a location phrase ("between A and B" or "from A through B") from stripped content.
fn location_phrase(content: &str) -> Option<String> {
    let fragment = if let Some(idx) = content.find(" between ") {
        &content[idx + 1..] // "between A and B..."
    } else if let Some(idx) = content.find(" from ") {
        let candidate = &content[idx + 1..]; // "from A through B..."
        if !candidate.contains(" through ") {
            return None;
        }
        candidate
    } else {
        return None;
    };

    let end = LOCATION_STOP_MARKERS
        .iter()
        .filter_map(|m| fragment.find(m))
        .min()
        .unwrap_or(fragment.len());

    let phrase = fragment[..end].trim_end_matches(['.', ',', ' ']);
    if phrase.is_empty() {
        None
    } else {
        Some(phrase.to_string())
    }
}

fn effect_label(effect: &str) -> &str {
    match effect {
        "SHUTTLE" => "Shuttle",
        "DELAY" => "Delay",
        "SUSPENSION" => "Suspension",
        "SERVICE_CHANGE" => "Service change",
        "SCHEDULE_CHANGE" => "Schedule change",
        "DETOUR" => "Detour",
        other => other,
    }
}

fn event_summary(alert: &Alert) -> String {
    let line = crate::line_name(alert);
    let content = strip_line_prefix(&alert.attributes.header);
    if let Some(loc) = location_phrase(content) {
        format!(
            "[{}] {} {}",
            line,
            effect_label(&alert.attributes.effect),
            loc
        )
    } else {
        format!("[{}] {}", line, brief_content(content))
    }
}

fn event_body(alert: &Alert) -> Value {
    let period = alert.attributes.active_period.first();
    let (start, end) = event_times(
        period.and_then(|p| p.start.as_deref()),
        period.and_then(|p| p.end.as_deref()),
    );

    json!({
        "summary": event_summary(alert),
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

    fn brief_header(header: &str) -> String {
        brief_content(strip_line_prefix(header))
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

    // --- brief_header ---

    #[test]
    fn test_brief_header_strips_red_line_prefix() {
        assert_eq!(
            brief_header(
                "Red Line: Shuttle buses replace service from JFK/UMass through Ashmont (and Mattapan), April 1 - 9, to allow for critical track work."
            ),
            "Shuttle buses replace service from JFK/UMass through Ashmont (and Mattapan)"
        );
    }

    #[test]
    fn test_brief_header_strips_branch_prefix() {
        assert_eq!(
            brief_header(
                "Green Line B Branch: Service will originate / terminate at the Lake Street platform, just outside of Boston College Station, from 8:45 PM on Friday."
            ),
            "Service will originate / terminate at the Lake Street platform"
        );
    }

    #[test]
    fn test_brief_header_truncates_at_to_allow() {
        assert_eq!(
            brief_header(
                "Red Line Ashmont Branch: Service between JFK/UMass and Ashmont will operate with two shuttle trains from April 10 - 30 to allow for critical track work."
            ),
            "Service between JFK/UMass and Ashmont will operate with two shuttle trains from April 10 - 30"
        );
    }

    #[test]
    fn test_brief_header_truncates_at_comma_after_location() {
        assert_eq!(
            brief_header(
                "Red Line: Shuttle buses will replace service between JFK/UMass and Braintree, the weekend of Mar 29 - 30, for signal upgrades."
            ),
            "Shuttle buses will replace service between JFK/UMass and Braintree"
        );
    }

    #[test]
    fn test_brief_header_no_prefix_no_truncation() {
        assert_eq!(
            brief_header("Delays expected on the Orange Line due to an earlier incident."),
            "Delays expected on the Orange Line due to an earlier incident"
        );
    }

    #[test]
    fn test_brief_header_attention_passengers_prefix_not_stripped() {
        // "Attention Passengers" doesn't contain "Line", so it should not be stripped
        assert_eq!(
            brief_header("Attention Passengers: Some service change is in effect."),
            "Attention Passengers: Some service change is in effect"
        );
    }

    // --- event_body ---

    #[test]
    fn test_event_body_summary_uses_header() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let body = event_body(&alert);
        assert_eq!(body["summary"], "[Red Line] Test header");
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
        assert_eq!(body["summary"], "[Green Line] Test header");
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
        let body = event_body(&alert);
        assert_eq!(
            body["summary"],
            "[Red Line] Shuttle between Broadway and Ashmont"
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
        let body = event_body(&alert);
        assert_eq!(
            body["summary"],
            "[Red Line] Service change between JFK/UMass and Ashmont"
        );
    }

    // --- location_phrase ---

    #[test]
    fn test_location_phrase_between_truncates_at_this() {
        assert_eq!(
            location_phrase(
                "Shuttle buses will replace service between Broadway and Ashmont this weekend."
            ),
            Some("between Broadway and Ashmont".to_string())
        );
    }

    #[test]
    fn test_location_phrase_between_truncates_at_comma() {
        assert_eq!(
            location_phrase(
                "Shuttle buses will replace service between JFK/UMass and Braintree, the weekend of Mar 29 - 30."
            ),
            Some("between JFK/UMass and Braintree".to_string())
        );
    }

    #[test]
    fn test_location_phrase_between_truncates_at_will() {
        assert_eq!(
            location_phrase(
                "Service between JFK/UMass and Ashmont will operate with two shuttle trains."
            ),
            Some("between JFK/UMass and Ashmont".to_string())
        );
    }

    #[test]
    fn test_location_phrase_from_through() {
        assert_eq!(
            location_phrase(
                "Shuttle buses replace service from JFK/UMass through Ashmont (and Mattapan), April 1 - 9."
            ),
            Some("from JFK/UMass through Ashmont (and Mattapan)".to_string())
        );
    }

    #[test]
    fn test_location_phrase_between_truncates_at_from_date() {
        assert_eq!(
            location_phrase(
                "Shuttle buses replace service between Back Bay and Forest Hills from February 28 - March 8 to allow for track work."
            ),
            Some("between Back Bay and Forest Hills".to_string())
        );
    }

    #[test]
    fn test_location_phrase_from_to_without_through_returns_none() {
        assert_eq!(
            location_phrase("Shuttle buses replace service from North Station to Anderson/Woburn."),
            None
        );
    }

    #[test]
    fn test_location_phrase_none_when_no_pattern() {
        assert_eq!(
            location_phrase("Service will originate / terminate at the Lake Street platform."),
            None
        );
    }

    // --- effect_label ---

    #[test]
    fn test_effect_label_shuttle() {
        assert_eq!(effect_label("SHUTTLE"), "Shuttle");
    }

    #[test]
    fn test_effect_label_service_change() {
        assert_eq!(effect_label("SERVICE_CHANGE"), "Service change");
    }

    #[test]
    fn test_effect_label_delay() {
        assert_eq!(effect_label("DELAY"), "Delay");
    }

    #[test]
    fn test_effect_label_unknown_passthrough() {
        assert_eq!(effect_label("SOME_NEW_EFFECT"), "SOME_NEW_EFFECT");
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
