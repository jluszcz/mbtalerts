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

impl Alert {
    pub fn period_start(&self) -> Option<&str> {
        self.attributes.active_period.first()?.start.as_deref()
    }

    pub fn period_end(&self) -> Option<&str> {
        self.attributes.active_period.first()?.end.as_deref()
    }

    /// Builder with placeholder defaults, intended for tests. Production
    /// alerts are deserialized from the MBTA API.
    pub fn builder() -> AlertBuilder {
        AlertBuilder {
            id: "test-id".to_owned(),
            header: "Test header".to_owned(),
            description: None,
            url: None,
            active_period: Vec::new(),
            effect: "DELAY".to_owned(),
            informed_entity: Vec::new(),
        }
    }
}

pub struct AlertBuilder {
    id: String,
    header: String,
    description: Option<String>,
    url: Option<String>,
    active_period: Vec<ActivePeriod>,
    effect: String,
    informed_entity: Vec<InformedEntity>,
}

impl AlertBuilder {
    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    pub fn header(mut self, header: &str) -> Self {
        self.header = header.to_owned();
        self
    }

    pub fn description(mut self, description: &str) -> Self {
        self.description = Some(description.to_owned());
        self
    }

    pub fn url(mut self, url: &str) -> Self {
        self.url = Some(url.to_owned());
        self
    }

    pub fn effect(mut self, effect: &str) -> Self {
        self.effect = effect.to_owned();
        self
    }

    /// Adds an informed entity for `route`; call repeatedly for multi-route alerts.
    pub fn route(mut self, route: &str) -> Self {
        self.informed_entity.push(InformedEntity {
            route: Some(route.to_owned()),
        });
        self
    }

    /// Adds an informed entity with no route.
    pub fn null_route(mut self) -> Self {
        self.informed_entity.push(InformedEntity { route: None });
        self
    }

    pub fn period(mut self, start: Option<&str>, end: Option<&str>) -> Self {
        self.active_period.push(ActivePeriod {
            start: start.map(str::to_owned),
            end: end.map(str::to_owned),
        });
        self
    }

    pub fn build(self) -> Alert {
        Alert {
            id: self.id,
            attributes: AlertAttributes {
                header: self.header,
                description: self.description,
                url: self.url,
                active_period: self.active_period,
                effect: self.effect,
                informed_entity: self.informed_entity,
            },
        }
    }
}
