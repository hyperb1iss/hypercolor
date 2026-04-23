#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaRange {
    min: u8,
    max: u8,
}

impl SchemaRange {
    #[must_use]
    pub const fn single(schema: u8) -> Self {
        Self {
            min: schema,
            max: schema,
        }
    }

    #[must_use]
    pub const fn try_new(min: u8, max: u8) -> Option<Self> {
        if min > max {
            return None;
        }

        Some(Self { min, max })
    }

    #[must_use]
    pub const fn min(self) -> u8 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> u8 {
        self.max
    }

    #[must_use]
    pub const fn contains(self, schema: u8) -> bool {
        schema >= self.min && schema <= self.max
    }

    #[must_use]
    pub const fn intersection(self, other: Self) -> Option<Self> {
        let min = if self.min >= other.min {
            self.min
        } else {
            other.min
        };
        let max = if self.max <= other.max {
            self.max
        } else {
            other.max
        };

        Self::try_new(min, max)
    }
}

#[must_use]
pub const fn negotiate_highest_common_schema(
    client: SchemaRange,
    server: SchemaRange,
) -> Option<u8> {
    match client.intersection(server) {
        Some(range) => Some(range.max()),
        None => None,
    }
}
