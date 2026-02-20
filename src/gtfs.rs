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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_route_type_values() {
        assert_eq!(i16::from(RouteType::Tram), 0);
        assert_eq!(i16::from(RouteType::Subway), 1);
        assert_eq!(i16::from(RouteType::Rail), 2);
        assert_eq!(i16::from(RouteType::Bus), 3);
        assert_eq!(i16::from(RouteType::Ferry), 4);
        assert_eq!(i16::from(RouteType::CableTram), 5);
        assert_eq!(i16::from(RouteType::AerialLift), 6);
        assert_eq!(i16::from(RouteType::Funicular), 7);
        assert_eq!(i16::from(RouteType::TrolleyBus), 8);
        assert_eq!(i16::from(RouteType::Monorail), 9);
    }

    #[test]
    fn test_route_type_equality() {
        assert_eq!(RouteType::Subway, RouteType::Subway);
        assert_ne!(RouteType::Subway, RouteType::Rail);
    }

    #[test]
    fn test_route_type_copy() {
        let rt = RouteType::Bus;
        let rt2 = rt;
        assert_eq!(rt, rt2);
    }
}
