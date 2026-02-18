use lambda_runtime::{LambdaEvent, service_fn};
use serde_json::{Value, json};

use mbtalerts::calendar::{CalendarClient, sync_alerts};

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    mbtalerts::set_up_logger("bootstrap", false).map_err(|e| e.to_string())?;
    lambda_runtime::run(service_fn(handler)).await
}

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, lambda_runtime::Error> {
    let alerts = mbtalerts::alerts(false).await?;
    let calendar = CalendarClient::from_env().await?;
    sync_alerts(&alerts, &calendar).await?;

    Ok(json!({
        "status": "ok",
        "alerts_processed": alerts.data.len(),
    }))
}
