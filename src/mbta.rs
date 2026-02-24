use jluszcz_rust_utils::query::http_get;
use log::{debug, info};

const API_URL: &str = "https://api-v3.mbta.com";
const ALERTS: &str = "alerts";
const ROUTES: &str = "Red,Orange,Blue,Green-B,Green-C,Green-D,Green-E";

pub async fn query_subway_alerts() -> anyhow::Result<String> {
    debug!("Fetching MBTA subway alerts");
    let response = http_get(
        &format!("{}/{}", API_URL, ALERTS),
        &[("filter[route]", ROUTES)],
    )
    .await?;
    info!("Fetched MBTA subway alerts");
    Ok(response)
}
