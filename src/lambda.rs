use jluszcz_rust_utils::cache::CacheMode;
use jluszcz_rust_utils::lambda;
use lambda_runtime::LambdaEvent;
use mbtalerts::APP_NAME;
use mbtalerts::calendar::{CalendarClient, sync_alerts};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    lambda::run(APP_NAME, module_path!(), false, handler).await
}

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, lambda_runtime::Error> {
    let alerts = mbtalerts::alerts(CacheMode::Disabled).await?;

    let calendar = CalendarClient::from_env().await?;
    sync_alerts(&alerts, &calendar).await?;

    Ok(json!({}))
}
