use clap::{Arg, ArgAction, Command};
use log::debug;
use mbtalerts::set_up_logger;

#[derive(Debug)]
struct Args {
    verbose: bool,
    use_cache: bool,
}

fn parse_args() -> Args {
    let matches = Command::new("mbtalerts")
        .version("0.1")
        .author("Jacob Luszcz")
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .action(ArgAction::SetTrue)
                .help("Verbose mode. Outputs DEBUG and higher log messages."),
        )
        .arg(
            Arg::new("use-cache")
                .short('c')
                .long("cache")
                .action(ArgAction::SetTrue)
                .help("Use cached values, if present, rather than querying remote services."),
        )
        .get_matches();

    let verbose = matches.get_flag("verbose");

    let use_cache = matches.get_flag("use-cache");

    Args { verbose, use_cache }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args();
    set_up_logger(module_path!(), args.verbose)?;
    debug!("{args:?}");

    let alerts = mbtalerts::alerts(args.use_cache).await?;
    debug!("{alerts:?}");

    Ok(())
}
