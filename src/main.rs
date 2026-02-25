use chrono::DateTime;
use clap::{Arg, ArgAction, Command};
use jluszcz_rust_utils::{Verbosity, set_up_logger};
use log::debug;
use mbtalerts::APP_NAME;
use mbtalerts::calendar::{
    CalendarClient, event_summary, first_sentence, sync_alerts, uses_first_sentence_summary,
};
use mbtalerts::types::Alerts;

const SEPARATOR: &str = "----------------------------------------";

#[derive(Debug)]
struct Args {
    verbosity: Verbosity,
    use_cache: bool,
    sync_calendar: bool,
}

fn parse_args() -> Args {
    let matches = Command::new("mbtalerts")
        .version("0.1")
        .author("Jacob Luszcz")
        .arg(
            Arg::new("verbosity")
                .short('v')
                .action(ArgAction::Count)
                .help("Increase verbosity (-v for debug, -vv for trace; max useful: -vv)"),
        )
        .arg(
            Arg::new("no-cache")
                .short('n')
                .long("no-cache")
                .action(ArgAction::SetTrue)
                .help("Query remote services instead of using cached values."),
        )
        .arg(
            Arg::new("sync-calendar")
                .short('s')
                .long("sync-calendar")
                .action(ArgAction::SetTrue)
                .help("Sync alerts to Google Calendar (requires GOOGLE_CALENDAR_ID and GOOGLE_SERVICE_ACCOUNT_KEY env vars)."),
        )
        .get_matches();

    let verbosity = matches.get_count("verbosity").into();

    let use_cache = !matches.get_flag("no-cache");
    let sync_calendar = matches.get_flag("sync-calendar");

    Args {
        verbosity,
        use_cache,
        sync_calendar,
    }
}

fn format_dt(s: &str) -> String {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.format("%-m/%-d/%Y %-I:%M%p").to_string().to_lowercase())
        .unwrap_or_else(|_| s.to_owned())
}

fn format_alert(alert: &mbtalerts::types::Alert) -> String {
    let effect = &alert.attributes.effect;
    let start = alert.period_start().map(format_dt);
    let end = alert.period_end().map(format_dt);

    let summary = event_summary(alert);
    let formatted_summary = if let Some(close) = summary.find(']') {
        let (prefix, rest) = summary.split_at(close + 1);
        format!("\x1b[1m{prefix}\x1b[22m{rest}")
    } else {
        summary
    };

    let date_part = match (start, end) {
        (Some(s), Some(e)) => format!(" - ({s} - {e})"),
        (Some(s), None) => format!(" - ({s})"),
        _ => String::new(),
    };

    let header = &alert.attributes.header;
    let body: &str = if uses_first_sentence_summary(alert) {
        let first = first_sentence(header);
        let rest = header[first.len()..].trim_start_matches(['.', ' ']);
        if rest.is_empty() { header } else { rest }
    } else {
        header
    };

    format!("{formatted_summary}{date_part}\n{effect} {body}")
}

fn print_alerts(alerts: &Alerts) {
    if alerts.data.is_empty() {
        println!("No active alerts.");
        return;
    }

    for alert in &alerts.data {
        println!("{SEPARATOR}");
        println!("{}", format_alert(alert));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let args = parse_args();
    set_up_logger(APP_NAME, module_path!(), args.verbosity)?;
    debug!("{args:?}");

    let alerts = mbtalerts::alerts(args.use_cache).await?;

    if args.sync_calendar {
        let calendar = CalendarClient::from_env().await?;
        sync_alerts(&alerts, &calendar).await?;
    } else {
        print_alerts(&alerts);
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use mbtalerts::types::{ActivePeriod, Alert, AlertAttributes, InformedEntity};

    fn make_alert(route: &str, effect: &str, start: Option<&str>, end: Option<&str>) -> Alert {
        Alert {
            id: "test-id".to_owned(),
            attributes: AlertAttributes {
                header: "Service disruption in effect".to_owned(),
                description: None,
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

    // --- format_alert ---

    #[test]
    fn test_format_alert_with_both_times() {
        let alert = make_alert(
            "Red",
            "DELAY",
            Some("2024-06-01T09:00:00-04:00"),
            Some("2024-06-01T23:00:00-04:00"),
        );
        let output = format_alert(&alert);
        assert!(output.contains("DELAY"));
        assert!(output.contains("Red Line"));
        assert!(output.contains("6/1/2024 9:00am"));
        assert!(output.contains("6/1/2024 11:00pm"));
        assert!(output.contains("Service disruption in effect"));
    }

    #[test]
    fn test_format_alert_no_period_shows_no_dates() {
        let alert = Alert {
            id: "test-id".to_owned(),
            attributes: AlertAttributes {
                header: "Some header".to_owned(),
                description: None,
                url: None,
                active_period: vec![],
                effect: "SUSPENSION".to_owned(),
                informed_entity: vec![InformedEntity {
                    route: Some("Orange".to_owned()),
                }],
            },
        };
        let output = format_alert(&alert);
        assert!(output.contains("SUSPENSION"));
        assert!(output.contains("Orange Line"));
        assert!(!output.contains('('));
    }

    #[test]
    fn test_format_alert_green_line() {
        let alert = make_alert(
            "Green-D",
            "DETOUR",
            Some("2024-06-01T08:00:00-04:00"),
            Some("2024-06-01T20:00:00-04:00"),
        );
        let output = format_alert(&alert);
        assert!(output.contains("Green Line"));
        assert!(output.contains("DETOUR"));
    }
}
