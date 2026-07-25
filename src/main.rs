use chrono::DateTime;
use clap::Parser;
use jluszcz_rust_utils::cache::CacheMode;
use jluszcz_rust_utils::{Verbosity, set_up_logger};
use log::debug;
use mbtalerts::ai::BedrockSummarizer;
use mbtalerts::calendar::{CalendarClient, sync_alerts};
use mbtalerts::summary::{
    LinePrefixMode, first_sentence, generate_or_fallback, uses_first_sentence_summary,
};
use mbtalerts::types::{Alert, Alerts};
use mbtalerts::{APP_NAME, should_sync_alert};

const SEPARATOR: &str = "----------------------------------------";

#[derive(Debug, Parser)]
#[command(version, author, infer_long_args = true)]
struct RawArgs {
    /// Increase verbosity (-v for debug, -vv for trace; max useful: -vv)
    #[arg(short = 'v', action = clap::ArgAction::Count)]
    verbosity: u8,

    /// Query remote services instead of using cached values.
    #[arg(short = 'n', long)]
    no_cache: bool,

    /// Sync alerts to Google Calendar (requires GOOGLE_SERVICE_ACCOUNT_KEY and either GOOGLE_CALENDAR_ID or GOOGLE_CALENDAR_IDS env vars).
    #[arg(short = 's', long)]
    sync_calendar: bool,
}

#[derive(Debug)]
struct Args {
    verbosity: Verbosity,
    cache_mode: CacheMode,
    sync_calendar: bool,
}

fn parse_args() -> Args {
    let raw = RawArgs::parse();

    Args {
        verbosity: raw.verbosity.into(),
        cache_mode: (!raw.no_cache).into(),
        sync_calendar: raw.sync_calendar,
    }
}

fn format_dt(s: &str) -> String {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.format("%-m/%-d/%Y %-I:%M%p").to_string().to_lowercase())
        .unwrap_or_else(|_| s.to_owned())
}

/// The body to print beneath the title.
///
/// The header's first sentence is dropped only when it *is* the title. An
/// AI-generated title is unrelated to that sentence, so stripping it there would
/// delete content that appears nowhere else in the output.
fn alert_body(alert: &Alert, ai_generated_title: bool) -> &str {
    let header = &alert.attributes.header;

    if ai_generated_title || !uses_first_sentence_summary(alert) {
        return header;
    }

    let first = first_sentence(header);
    let rest = header[first.len()..].trim_start_matches(['.', ' ']);
    if rest.is_empty() { header } else { rest }
}

async fn format_alert(alert: &Alert, summarizer: Option<&BedrockSummarizer>) -> String {
    let effect = &alert.attributes.effect;
    let start = alert.period_start().map(format_dt);
    let end = alert.period_end().map(format_dt);

    let summary = generate_or_fallback(summarizer, alert, LinePrefixMode::Include).await;

    let formatted_summary = if let Some(close) = summary.display.find(']') {
        let (prefix, rest) = summary.display.split_at(close + 1);
        format!("\x1b[1m{prefix}\x1b[22m{rest}")
    } else {
        summary.display.clone()
    };

    let date_part = match (start, end) {
        (Some(s), Some(e)) => format!(" - ({s} - {e})"),
        (Some(s), None) => format!(" - ({s})"),
        _ => String::new(),
    };

    let body = alert_body(alert, summary.raw.is_some());

    format!("{formatted_summary}{date_part}\n{effect} {body}")
}

async fn print_alerts(alerts: &Alerts, summarizer: Option<&BedrockSummarizer>) {
    let mut printed = false;
    for alert in alerts.data.iter().filter(|a| should_sync_alert(a)) {
        println!("{SEPARATOR}");
        println!("{}", format_alert(alert, summarizer).await);
        printed = true;
    }
    if !printed {
        println!("No active alerts.");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let args = parse_args();
    set_up_logger(APP_NAME, module_path!(), args.verbosity)?;
    debug!("{args:?}");

    let alerts = mbtalerts::alerts(args.cache_mode).await?;

    if args.sync_calendar {
        let calendar = CalendarClient::from_env().await?;
        sync_alerts(&alerts, &calendar).await?;
    } else {
        let summarizer = BedrockSummarizer::from_env().await;
        print_alerts(&alerts, summarizer.as_ref()).await;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    fn make_alert(route: &str, effect: &str, start: Option<&str>, end: Option<&str>) -> Alert {
        Alert::builder()
            .header("Service disruption in effect")
            .route(route)
            .effect(effect)
            .period(start, end)
            .build()
    }

    // --- format_dt ---

    #[test]
    fn test_format_dt_am() {
        assert_eq!(format_dt("2024-01-15T10:30:00-05:00"), "1/15/2024 10:30am");
    }

    #[test]
    fn test_format_dt_pm() {
        assert_eq!(format_dt("2024-01-15T14:45:00-05:00"), "1/15/2024 2:45pm");
    }

    #[test]
    fn test_format_dt_midnight() {
        assert_eq!(format_dt("2024-01-15T00:00:00-05:00"), "1/15/2024 12:00am");
    }

    #[test]
    fn test_format_dt_noon() {
        assert_eq!(format_dt("2024-01-15T12:00:00-05:00"), "1/15/2024 12:00pm");
    }

    #[test]
    fn test_format_dt_invalid_passthrough() {
        assert_eq!(format_dt("not-a-date"), "not-a-date");
    }

    // --- alert_body ---

    #[test]
    fn test_alert_body_keeps_first_sentence_when_title_is_ai_generated() {
        // The AI title is unrelated to the first sentence, so stripping it would
        // delete content that then appears nowhere in the output.
        let mut alert = make_alert("Red", "SHUTTLE", None, None);
        alert.attributes.header =
            "Shuttle buses replace service. Expect delays of 20 minutes.".to_owned();

        assert_eq!(
            alert_body(&alert, true),
            "Shuttle buses replace service. Expect delays of 20 minutes."
        );
    }

    #[test]
    fn test_alert_body_strips_first_sentence_when_it_is_the_title() {
        let mut alert = make_alert("Red", "SHUTTLE", None, None);
        alert.attributes.header =
            "Shuttle buses replace service. Expect delays of 20 minutes.".to_owned();

        assert_eq!(alert_body(&alert, false), "Expect delays of 20 minutes.");
    }

    #[test]
    fn test_alert_body_keeps_header_when_summary_is_not_the_first_sentence() {
        let mut alert = make_alert("Red", "DELAY", None, None);
        alert.attributes.header =
            "Red Line: Delays of about 20 minutes due to a signal problem.".to_owned();

        assert_eq!(
            alert_body(&alert, false),
            "Red Line: Delays of about 20 minutes due to a signal problem."
        );
    }

    #[test]
    fn test_alert_body_keeps_header_when_stripping_would_empty_it() {
        let mut alert = make_alert("Red", "SHUTTLE", None, None);
        alert.attributes.header = "Shuttle buses replace service.".to_owned();

        assert_eq!(alert_body(&alert, false), "Shuttle buses replace service.");
    }

    // --- format_alert ---

    #[tokio::test]
    async fn test_format_alert_with_both_times() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let output = format_alert(&alert, None).await;
        assert!(output.contains("DELAY"));
        assert!(output.contains("Red Line"));
        assert!(output.contains("6/1/2024 9:00am"));
        assert!(output.contains("6/1/2024 11:00pm"));
        assert!(output.contains("Service disruption in effect"));
    }

    #[tokio::test]
    async fn test_format_alert_no_period_shows_no_dates() {
        let alert = Alert::builder()
            .header("Some header")
            .route("Orange")
            .effect("SUSPENSION")
            .build();
        let output = format_alert(&alert, None).await;
        assert!(output.contains("SUSPENSION"));
        assert!(output.contains("Orange Line"));
        assert!(!output.contains('('));
    }

    #[tokio::test]
    async fn test_format_alert_green_line() {
        let alert = make_alert(
            "Green-D",
            "DETOUR",
            Some("2024-06-01T08:00:00-04:00"),
            Some("2024-06-01T20:00:00-04:00"),
        );
        let output = format_alert(&alert, None).await;
        assert!(output.contains("Green Line"));
        assert!(output.contains("DETOUR"));
    }
}
