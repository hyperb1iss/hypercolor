//! Off-platform stand-ins: no WMI, no PawnIO, nothing to report.

use crate::{MotherboardInfo, SystemSnapshot};

/// Motherboard identity is only probed on Windows.
#[must_use]
pub const fn motherboard_info() -> Option<MotherboardInfo> {
    None
}

/// Sensor extras that never contribute readings.
#[derive(Debug, Default)]
pub struct SensorExtras;

impl SensorExtras {
    /// Build the (empty) extras.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Leaves the snapshot untouched.
    pub const fn merge_snapshot(&mut self, _snapshot: &mut SystemSnapshot) {}
}
