use log::warn;

use crate::ai::BedrockSummarizer;
use crate::types::Alert;

#[derive(Clone, Copy)]
pub enum LinePrefixMode {
    Include,
    Omit,
}

pub fn strip_line_prefix(header: &str) -> &str {
    if let Some(colon_idx) = header.find(": ") {
        let prefix = &header[..colon_idx];
        if prefix.contains("Line") && prefix.len() <= 35 {
            return header[colon_idx + 2..].trim_start();
        }
    }
    header.trim_start()
}

pub fn effect_label(effect: &str) -> Option<&str> {
    match effect {
        "SHUTTLE" => Some("Shuttle"),
        "DELAY" => Some("Delay"),
        "SUSPENSION" => Some("Suspension"),
        "SERVICE_CHANGE" => Some("Service change"),
        "SCHEDULE_CHANGE" => Some("Schedule change"),
        "DETOUR" => Some("Detour"),
        _ => None,
    }
}

pub fn first_sentence(s: &str) -> &str {
    if let Some(pos) = s.find(". ") {
        &s[..pos]
    } else {
        s.trim_end_matches('.')
    }
}

/// For DELAY effects, extract "~N minutes" from content like
/// "Delays of about 20 minutes due to signal problem".
fn delay_duration_phrase(content: &str) -> Option<String> {
    let lower = content.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let min_pos = words.iter().position(|w| w.starts_with("minute"))?;
    if min_pos == 0 {
        return None;
    }
    // "N to M minutes" pattern
    if min_pos >= 3
        && words[min_pos - 2] == "to"
        && words[min_pos - 1].chars().all(|c| c.is_ascii_digit())
        && words[min_pos - 3].chars().all(|c| c.is_ascii_digit())
    {
        return Some(format!(
            "~{}-{} minutes",
            words[min_pos - 3],
            words[min_pos - 1]
        ));
    }
    // "N minutes" pattern
    if words[min_pos - 1].chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("~{} minutes", words[min_pos - 1]));
    }
    None
}

const LOCATION_STOP_MARKERS: &[&str] = &[
    ", ",
    " will ",
    " this ",
    " that ",
    " from ",
    " due ",
    " to allow",
    " in order",
    " starting",
    " during",
];

/// Extract a location phrase ("between A and B" or "from A through B") from stripped content.
fn location_phrase(content: &str) -> Option<String> {
    let fragment = if let Some(idx) = content.find(" between ") {
        &content[idx + 1..]
    } else {
        let idx = content.find(" from ")?;
        let candidate = &content[idx + 1..];
        if !candidate.contains(" through ") {
            return None;
        }
        candidate
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

fn apply_line_prefix(raw: &str, alert: &Alert, mode: LinePrefixMode) -> String {
    match mode {
        LinePrefixMode::Include => format!("[{}] {}", crate::line_name(alert), raw),
        LinePrefixMode::Omit => raw.to_owned(),
    }
}

pub fn event_summary(alert: &Alert, line_prefix: LinePrefixMode) -> String {
    let content = strip_line_prefix(&alert.attributes.header);
    if alert.attributes.effect == "DELAY"
        && let Some(duration) = delay_duration_phrase(content)
    {
        return match line_prefix {
            LinePrefixMode::Include => format!("[{}] Delay {}", crate::line_name(alert), duration),
            LinePrefixMode::Omit => format!("Delay {duration}"),
        };
    }
    if let Some(label) = effect_label(&alert.attributes.effect)
        && let Some(loc) = location_phrase(content)
    {
        return match line_prefix {
            LinePrefixMode::Include => {
                format!("[{}] {} {}", crate::line_name(alert), label, loc)
            }
            LinePrefixMode::Omit => format!("{label} {loc}"),
        };
    }
    match line_prefix {
        LinePrefixMode::Include => {
            format!("[{}] {}", crate::line_name(alert), first_sentence(content))
        }
        LinePrefixMode::Omit => first_sentence(content).to_owned(),
    }
}

/// Returns true when event_summary uses first_sentence as the title text,
/// i.e. when no structured format (delay duration or location phrase) applies.
pub fn uses_first_sentence_summary(alert: &Alert) -> bool {
    let content = strip_line_prefix(&alert.attributes.header);
    if alert.attributes.effect == "DELAY" && delay_duration_phrase(content).is_some() {
        return false;
    }
    if effect_label(&alert.attributes.effect).is_some() && location_phrase(content).is_some() {
        return false;
    }
    true
}

pub struct AlertSummary {
    /// AI-generated summary without the line prefix, when Bedrock produced one.
    pub raw: Option<String>,
    /// Summary to display, with the line prefix applied per `LinePrefixMode`.
    pub display: String,
}

pub async fn generate_or_fallback(
    summarizer: Option<&BedrockSummarizer>,
    alert: &Alert,
    line_prefix: LinePrefixMode,
) -> AlertSummary {
    if let Some(s) = summarizer {
        match s.generate_summary(&alert.attributes.header).await {
            Ok(raw) => {
                let display = apply_line_prefix(&raw, alert, line_prefix);
                return AlertSummary {
                    raw: Some(raw),
                    display,
                };
            }
            Err(e) => {
                warn!("Bedrock inference failed for alert {}: {e:#}", alert.id);
            }
        }
    }
    AlertSummary {
        raw: None,
        display: event_summary(alert, line_prefix),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn make_alert(route: &str, effect: &str) -> Alert {
        Alert::builder()
            .id("alert-42")
            .description("Test description")
            .route(route)
            .effect(effect)
            .build()
    }

    // --- effect_label ---

    #[test]
    fn test_effect_label_shuttle() {
        assert_eq!(effect_label("SHUTTLE"), Some("Shuttle"));
    }

    #[test]
    fn test_effect_label_service_change() {
        assert_eq!(effect_label("SERVICE_CHANGE"), Some("Service change"));
    }

    #[test]
    fn test_effect_label_delay() {
        assert_eq!(effect_label("DELAY"), Some("Delay"));
    }

    #[test]
    fn test_effect_label_unknown_returns_none() {
        assert_eq!(effect_label("STATION_ISSUE"), None);
        assert_eq!(effect_label("SOME_NEW_EFFECT"), None);
    }

    // --- first_sentence ---

    #[test]
    fn test_first_sentence_multi_sentence() {
        assert_eq!(
            first_sentence("The elevator is closed. Use the stairs. Thank you."),
            "The elevator is closed"
        );
    }

    #[test]
    fn test_first_sentence_single_with_period() {
        assert_eq!(
            first_sentence("The elevator is closed."),
            "The elevator is closed"
        );
    }

    #[test]
    fn test_first_sentence_no_period() {
        assert_eq!(
            first_sentence("The elevator is closed"),
            "The elevator is closed"
        );
    }

    #[test]
    fn test_first_sentence_station_prefix() {
        assert_eq!(
            first_sentence(
                "Jackson Square: The stairway is closed until winter 2026. Use the other stairway."
            ),
            "Jackson Square: The stairway is closed until winter 2026"
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
    fn test_location_phrase_between_truncates_at_due() {
        assert_eq!(
            location_phrase(
                "Shuttle buses replacing service between Suffolk Downs and Maverick due to a power problem at Airport."
            ),
            Some("between Suffolk Downs and Maverick".to_string())
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

    // --- event_summary ---

    #[test]
    fn test_event_summary_no_prefix_delay_with_duration() {
        let mut alert = make_alert("Red", "DELAY");
        alert.attributes.header = "Red Line Braintree Branch: Delays of about 20 minutes due to a signal problem at Braintree.".to_owned();
        assert_eq!(
            event_summary(&alert, LinePrefixMode::Omit),
            "Delay ~20 minutes"
        );
    }

    #[test]
    fn test_event_summary_no_prefix_shuttle_with_location() {
        let mut alert = make_alert("Red", "SHUTTLE");
        alert.attributes.header = "Red Line: Shuttle buses will replace service between Broadway and Ashmont this weekend.".to_owned();
        assert_eq!(
            event_summary(&alert, LinePrefixMode::Omit),
            "Shuttle between Broadway and Ashmont"
        );
    }

    #[test]
    fn test_event_summary_no_prefix_first_sentence() {
        let alert = make_alert("Red", "DELAY");
        assert_eq!(event_summary(&alert, LinePrefixMode::Omit), "Test header");
    }

    #[test]
    fn test_event_summary_with_prefix_first_sentence() {
        let alert = make_alert("Red", "DELAY");
        assert_eq!(
            event_summary(&alert, LinePrefixMode::Include),
            "[Red Line] Test header"
        );
    }

    // --- uses_first_sentence_summary ---

    #[test]
    fn test_uses_first_sentence_delay_with_duration_is_false() {
        let mut alert = make_alert("Red", "DELAY");
        alert.attributes.header =
            "Red Line: Delays of about 20 minutes due to a signal problem.".to_owned();
        assert!(!uses_first_sentence_summary(&alert));
    }

    #[test]
    fn test_uses_first_sentence_delay_without_duration_is_true() {
        let alert = make_alert("Red", "DELAY");
        assert!(uses_first_sentence_summary(&alert));
    }

    #[test]
    fn test_uses_first_sentence_shuttle_with_location_is_false() {
        let mut alert = make_alert("Red", "SHUTTLE");
        alert.attributes.header =
            "Red Line: Shuttle buses will replace service between Broadway and Ashmont this weekend."
                .to_owned();
        assert!(!uses_first_sentence_summary(&alert));
    }

    #[test]
    fn test_uses_first_sentence_shuttle_without_location_is_true() {
        let alert = make_alert("Red", "SHUTTLE");
        assert!(uses_first_sentence_summary(&alert));
    }

    #[test]
    fn test_uses_first_sentence_service_change_no_location_is_true() {
        let mut alert = make_alert("Blue", "SERVICE_CHANGE");
        alert.attributes.header =
            "Subway, Bus, and Ferry have returned to regular schedules. Storm cleanup continues."
                .to_owned();
        assert!(uses_first_sentence_summary(&alert));
    }

    #[test]
    fn test_uses_first_sentence_unmapped_effect_is_true() {
        let alert = make_alert("Orange", "STATION_ISSUE");
        assert!(uses_first_sentence_summary(&alert));
    }
}
