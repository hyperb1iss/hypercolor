//! Shared native-first detector partition policy.

use std::collections::BTreeSet;

use crate::{DetectorRules, UsbDeviceId, detector_families, detector_prefixes_for_drivers};
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
    /// USB devices the driver's protocol catalog can claim, whether or not
    /// any is connected. Empty when the daemon published none (or the facts
    /// predate the field), which keeps the driver's detector family on the
    /// conservative name-prefix rule.
    #[serde(default)]
    pub usb_ids: BTreeSet<UsbDeviceId>,
}

impl From<&DriverSummary> for DriverFacts {
    fn from(summary: &DriverSummary) -> Self {
        Self {
            id: summary.descriptor.id.clone(),
            module_kind: summary.descriptor.module_kind,
            enabled: summary.enabled,
            usb_ids: summary
                .protocols
                .iter()
                .filter_map(|protocol| {
                    Some(UsbDeviceId::new(protocol.vendor_id?, protocol.product_id?))
                })
                .collect(),
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

/// Which native drivers the detector partition withholds from OpenRGB,
/// which families it hands back, and the USB devices behind them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectorPartitionPlan {
    /// Withheld families: enabled hardware drivers in the detector table
    /// that own at least one enabled device.
    pub disabled_driver_ids: Vec<String>,
    /// Every other driver family the detector table knows: their detectors
    /// are written `true` so OpenRGB may drive the hardware again.
    pub re_enable_driver_ids: Vec<String>,
    /// The members of `disabled_driver_ids` that published a USB catalog.
    /// Their families are withheld per device; the rest stay withheld by
    /// name prefix alone.
    pub id_gated_driver_ids: Vec<String>,
    /// USB devices any withheld native driver can claim, including drivers
    /// the detector table has no family for (QMK, PrismRGB, Push 2).
    pub claimed_usb_ids: BTreeSet<UsbDeviceId>,
    /// USB devices any registered native driver can claim, withheld or not.
    pub native_usb_ids: BTreeSet<UsbDeviceId>,
}

impl DetectorPartitionPlan {
    /// Translate the plan into the per-detector rules the config writer
    /// applies.
    #[must_use]
    pub fn detector_rules(&self) -> DetectorRules {
        let id_gated = |id: &String| {
            self.id_gated_driver_ids
                .iter()
                .any(|gated| gated.eq_ignore_ascii_case(id))
        };
        let (gated, wholesale): (Vec<&String>, Vec<&String>) =
            self.disabled_driver_ids.iter().partition(|id| id_gated(id));
        DetectorRules {
            disabled_prefixes: detector_prefixes_for_drivers(wholesale),
            id_gated_prefixes: detector_prefixes_for_drivers(gated),
            re_enable_prefixes: detector_prefixes_for_drivers(&self.re_enable_driver_ids),
            claimed_usb_ids: self.claimed_usb_ids.clone(),
            native_usb_ids: self.native_usb_ids.clone(),
        }
    }
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
/// Spec 81 §3.1: a native driver is withheld from OpenRGB only when it is
/// enabled and at least one of its devices is not user-disabled (a device in
/// state `known` counts as present). A withheld driver withholds every USB
/// device its catalog can claim, connected or not, so a second supported
/// device plugged in later never races OpenRGB for it; detectors for that
/// brand's other hardware stay with OpenRGB. Every other id in
/// `known_driver_ids` is re-enabled, so a driver the user turned off, a
/// driver whose only devices are disabled, and a family with no device at
/// all are all handed back. The family lists are confined to the ids the
/// detector table knows; the USB id sets cover every native driver. Bridge
/// modules never partition.
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
    let native = || {
        drivers
            .iter()
            .filter(|driver| driver.module_kind != DriverModuleKind::Bridge)
    };
    let withheld: Vec<&DriverFacts> = native()
        .filter(|driver| driver.enabled)
        .filter(|driver| {
            devices
                .iter()
                .any(|device| !device.disabled && device.driver_id.eq_ignore_ascii_case(&driver.id))
        })
        .collect();

    let mut disabled: Vec<String> = withheld
        .iter()
        .filter(|driver| known(&driver.id))
        .map(|driver| driver.id.clone())
        .collect();
    disabled.sort();
    disabled.dedup();
    let mut id_gated: Vec<String> = withheld
        .iter()
        .filter(|driver| known(&driver.id) && !driver.usb_ids.is_empty())
        .map(|driver| driver.id.clone())
        .collect();
    id_gated.sort();
    id_gated.dedup();
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
        id_gated_driver_ids: id_gated,
        claimed_usb_ids: withheld
            .iter()
            .flat_map(|driver| driver.usb_ids.iter().copied())
            .collect(),
        native_usb_ids: native()
            .flat_map(|driver| driver.usb_ids.iter().copied())
            .collect(),
    }
}

/// Whether the daemon has the OpenRGB bridge driver registered and enabled.
#[must_use]
pub fn bridge_enabled(drivers: &[DriverFacts]) -> bool {
    drivers
        .iter()
        .any(|driver| driver.id == "openrgb" && driver.enabled)
}
