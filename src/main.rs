use chrono::DateTime;
use clap::{Arg, ArgAction, Command};
use jluszcz_rust_utils::{Verbosity, set_up_logger};
use log::debug;
use mbtalerts::types::Alerts;
use mbtalerts::{APP_NAME, line_name};

const SEPARATOR: &str = "----------------------------------------";

#[derive(Debug)]
struct Args {
    verbosity: Verbosity,
    use_cache: bool,
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
        .get_matches();

    let verbosity = matches.get_count("verbosity").into();

    let use_cache = !matches.get_flag("no-cache");

    Args {
        verbosity,
        use_cache,
    }
}

fn format_dt(s: &str) -> String {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.format("%-I:%M %p %m/%d/%Y").to_string())
        .unwrap_or_else(|_| s.to_owned())
}

fn print_alerts(alerts: &Alerts) {
    if alerts.data.is_empty() {
        println!("No active alerts.");
        return;
    }

    for alert in &alerts.data {
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

        println!("{SEPARATOR}");
        println!("{effect}  {line}  {start}  {end}");
        println!("{}", alert.attributes.header);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args();
    set_up_logger(APP_NAME, module_path!(), args.verbosity)?;
    debug!("{args:?}");

    let alerts = mbtalerts::alerts(args.use_cache).await?;
    print_alerts(&alerts);

    Ok(())
}
