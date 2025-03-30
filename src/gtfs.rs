/// See https://gtfs.org/documentation/schedule/reference/#routestxt
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RouteType {
    Tram,
    Subway,
    Rail,
    Bus,
    Ferry,
    CableTram,
    AerialLift,
    Funicular,
    TrolleyBus,
    Monorail,
}

impl From<RouteType> for i16 {
    fn from(value: RouteType) -> Self {
        match value {
            RouteType::Tram => 0,
            RouteType::Subway => 1,
            RouteType::Rail => 2,
            RouteType::Bus => 3,
            RouteType::Ferry => 4,
            RouteType::CableTram => 5,
            RouteType::AerialLift => 6,
            RouteType::Funicular => 7,
            RouteType::TrolleyBus => 8,
            RouteType::Monorail => 9,
        }
    }
}
