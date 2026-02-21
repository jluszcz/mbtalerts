use jluszcz_rust_utils::{Verbosity, set_up_logger, lambda};
use lambda_runtime::{LambdaEvent, service_fn};
use mbtalerts::APP_NAME;
use mbtalerts::calendar::{CalendarClient, sync_alerts};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    lambda::init(APP_NAME, module_path!(), false).await?;
    let func = service_fn(handler);
    lambda_runtime::run(func).await?;
    Ok(())
}

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, lambda_runtime::Error> {
    let alerts = mbtalerts::alerts(false).await?;

    let calendar = CalendarClient::from_env().await?;
    sync_alerts(&alerts, &calendar).await?;

    Ok(json!({}))
}
