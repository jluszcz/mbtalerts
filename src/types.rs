use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Alerts {
    pub data: Vec<Alert>,
}

#[derive(Debug, Deserialize)]
pub struct Alert {
    pub id: String,
    pub attributes: AlertAttributes,
}

#[derive(Debug, Deserialize)]
pub struct AlertAttributes {
    pub header: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub active_period: Vec<ActivePeriod>,
    pub effect: String,
    pub informed_entity: Vec<InformedEntity>,
}

#[derive(Debug, Deserialize)]
pub struct ActivePeriod {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InformedEntity {
    pub route: Option<String>,
}
