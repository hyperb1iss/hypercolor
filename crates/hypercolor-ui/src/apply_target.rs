pub const ALL_ZONES_VALUE: &str = "__all_zones__";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ApplyTarget {
    #[default]
    Primary,
    Zone(String),
    AllZones,
}

impl ApplyTarget {
    #[must_use]
    pub fn from_select_value(value: String) -> Self {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed == "default" {
            Self::Primary
        } else if trimmed == ALL_ZONES_VALUE {
            Self::AllZones
        } else {
            Self::Zone(trimmed.to_owned())
        }
    }

    #[must_use]
    pub fn select_value(&self) -> String {
        match self {
            Self::Primary => String::new(),
            Self::Zone(zone_id) => zone_id.clone(),
            Self::AllZones => ALL_ZONES_VALUE.to_owned(),
        }
    }

    #[must_use]
    pub fn zone_id(&self) -> Option<&str> {
        match self {
            Self::Zone(zone_id) => Some(zone_id),
            Self::Primary | Self::AllZones => None,
        }
    }
}
