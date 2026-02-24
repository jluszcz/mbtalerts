use chrono::DateTime;
use clap::{Arg, ArgAction, Command};
use jluszcz_rust_utils::{Verbosity, set_up_logger};
use log::debug;
use mbtalerts::calendar::{CalendarClient, sync_alerts};
use mbtalerts::types::Alerts;
use mbtalerts::{APP_NAME, line_name};

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
        .map(|dt| dt.format("%-I:%M %p %m/%d/%Y").to_string())
        .unwrap_or_else(|_| s.to_owned())
}

fn format_alert(alert: &mbtalerts::types::Alert) -> String {
    let line = line_name(alert);
    let effect = &alert.attributes.effect;
    let period = alert.attributes.active_period.first();
    let start = period
        .and_then(|p| p.start.as_deref())
        .map(format_dt)
        .unwrap_or_else(|| "—".to_owned());
    let end = period
        .and_then(|p| p.end.as_deref())
        .map(format_dt)
        .unwrap_or_else(|| "-".to_owned());
    format!(
        "{effect}  {line}  {start}  {end}\n{}",
        alert.attributes.header
    )
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
        assert_eq!(
            format_dt("2024-01-15T10:30:00-05:00"),
            "10:30 AM 01/15/2024"
        );
    }

    #[test]
    fn test_format_dt_pm() {
        assert_eq!(format_dt("2024-01-15T14:45:00-05:00"), "2:45 PM 01/15/2024");
    }

    #[test]
    fn test_format_dt_midnight() {
        assert_eq!(
            format_dt("2024-01-15T00:00:00-05:00"),
            "12:00 AM 01/15/2024"
        );
    }

    #[test]
    fn test_format_dt_noon() {
        assert_eq!(
            format_dt("2024-01-15T12:00:00-05:00"),
            "12:00 PM 01/15/2024"
        );
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
        assert!(output.contains("9:00 AM 06/01/2024"));
        assert!(output.contains("11:00 PM 06/01/2024"));
        assert!(output.contains("Service disruption in effect"));
    }

    #[test]
    fn test_format_alert_no_period_uses_dashes() {
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
        assert!(output.contains('—'));
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
