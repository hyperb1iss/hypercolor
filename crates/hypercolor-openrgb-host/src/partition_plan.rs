//! Shared native-first detector partition policy.

use crate::detector_families;
use hypercolor_types::api::devices::DeviceSummary;
use hypercolor_types::api::drivers::DriverSummary;
use hypercolor_types::device::DriverModuleKind;

/// The slice of a daemon driver summary the partition decision needs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DriverFacts {
    /// Stable driver id (`razer`, `openrgb`, ...).
    pub id: String,
    /// Registry category of the module.
    pub module_kind: DriverModuleKind,
    /// Whether the user has the driver enabled.
    pub enabled: bool,
}

impl From<&DriverSummary> for DriverFacts {
    fn from(summary: &DriverSummary) -> Self {
        Self {
            id: summary.descriptor.id.clone(),
            module_kind: summary.descriptor.module_kind,
            enabled: summary.enabled,
        }
    }
}

/// The slice of a daemon device summary the partition decision needs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeviceFacts {
    /// Driver module that owns the device.
    pub driver_id: String,
    /// Whether the user has disabled the device (`status == "disabled"`).
    pub disabled: bool,
}

impl From<&DeviceSummary> for DeviceFacts {
    fn from(summary: &DeviceSummary) -> Self {
        Self {
            driver_id: summary.origin.driver_id.clone(),
            disabled: summary.status.eq_ignore_ascii_case("disabled"),
        }
    }
}

/// Which native driver families the detector partition disables and which
/// it hands back to OpenRGB, as driver ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectorPartitionPlan {
    /// Enabled hardware drivers that own at least one enabled device: their
    /// OpenRGB detectors are written `false`.
    pub disabled_driver_ids: Vec<String>,
    /// Every other driver family the detector table knows: their detectors
    /// are written `true` so OpenRGB may drive the hardware again.
    pub re_enable_driver_ids: Vec<String>,
}

/// Driver ids the embedded detector table has families for.
#[must_use]
pub fn known_detector_driver_ids() -> Vec<String> {
    detector_families()
        .iter()
        .map(|family| family.driver_id.clone())
        .collect()
}

/// Decide the detector partition from the daemon's drivers and devices.
///
/// Spec 81 §3.1: a family is disabled for OpenRGB only when its native
/// driver is enabled and at least one of its devices is not user-disabled
/// (a device in state `known` counts as present). Every other id in
/// `known_driver_ids` is re-enabled, so a driver the user turned off, a
/// driver whose only devices are disabled, and a family with no device at
/// all are all handed back to OpenRGB. Both lists are confined to the ids
/// the detector table knows; bridge modules never partition.
#[must_use]
pub fn partition_driver_ids<S: AsRef<str>>(
    drivers: &[DriverFacts],
    devices: &[DeviceFacts],
    known_driver_ids: &[S],
) -> DetectorPartitionPlan {
    let known = |id: &str| {
        known_driver_ids
            .iter()
            .any(|candidate| candidate.as_ref().eq_ignore_ascii_case(id))
    };
    let mut disabled: Vec<String> = drivers
        .iter()
        .filter(|driver| driver.enabled && driver.module_kind != DriverModuleKind::Bridge)
        .filter(|driver| known(&driver.id))
        .filter(|driver| {
            devices
                .iter()
                .any(|device| !device.disabled && device.driver_id.eq_ignore_ascii_case(&driver.id))
        })
        .map(|driver| driver.id.clone())
        .collect();
    disabled.sort();
    disabled.dedup();
    let mut re_enable: Vec<String> = known_driver_ids
        .iter()
        .map(|id| id.as_ref().to_owned())
        .filter(|id| !disabled.iter().any(|kept| kept.eq_ignore_ascii_case(id)))
        .collect();
    re_enable.sort();
    re_enable.dedup();
    DetectorPartitionPlan {
        disabled_driver_ids: disabled,
        re_enable_driver_ids: re_enable,
    }
}

/// Whether the daemon has the OpenRGB bridge driver registered and enabled.
#[must_use]
pub fn bridge_enabled(drivers: &[DriverFacts]) -> bool {
    drivers
        .iter()
        .any(|driver| driver.id == "openrgb" && driver.enabled)
}
