//! Competing RGB software contracts: `/api/v1/system/conflicts`.

use serde::{Deserialize, Serialize};

/// RGB software running on the host that competes with Hypercolor for
/// devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SoftwareConflict {
    /// Stable catalog id (`signalrgb`, `lian_li_l_connect`).
    pub id: String,
    /// Product name for display.
    pub name: String,
    /// Process and service names that matched, as the host reported them.
    pub matched: Vec<String>,
    /// Hypercolor driver ids whose devices this software holds or fights
    /// over. Empty when [`Self::all_drivers`] is set.
    pub driver_ids: Vec<String>,
    /// Whether it competes with every driver, as whole-system RGB suites do.
    pub all_drivers: bool,
    /// Whether it drives SMBus lighting (motherboard, RAM, GPU).
    pub smbus: bool,
    /// What the user should do about it.
    pub remedy: String,
}

impl SoftwareConflict {
    /// Whether this software can take a device from `driver_id` away from
    /// Hypercolor. `smbus_device` marks devices reached over SMBus, which
    /// every SMBus tool competes for whatever driver owns them.
    #[must_use]
    pub fn affects(&self, driver_id: &str, smbus_device: bool) -> bool {
        self.all_drivers
            || (self.smbus && smbus_device)
            || self.driver_ids.iter().any(|id| id == driver_id)
    }
}

/// What the latest conflict scan found.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SoftwareConflictsStatus {
    /// Whether this host can list running software. When false,
    /// `conflicts` is always empty and proves nothing.
    pub supported: bool,
    /// Whether a scan has finished since the daemon started.
    pub scanned: bool,
    /// Whether the latest scan failed. `conflicts` then still holds the
    /// last successful scan's result rather than claiming nothing runs.
    #[serde(default)]
    pub scan_failed: bool,
    /// Competing software that is running now, in catalog order.
    pub conflicts: Vec<SoftwareConflict>,
}
