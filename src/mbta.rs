use crate::gtfs::RouteType;
use crate::http_get;

const API_URL: &str = "https://api-v3.mbta.com";
const ALERTS: &str = "alerts";

pub async fn query_subway_alerts() -> anyhow::Result<String> {
    http_get(
        &format!("{}/{}", API_URL, ALERTS),
        &[(
            "filter[route_type]",
            format!("{}", i16::from(RouteType::Subway)).as_str(),
        )],
    )
    .await
}
