//! Shared native-first detector partition policy.

use crate::{
    DetectorRules, UsbClaim, UsbDeviceId, detector_families, detector_prefixes_for_drivers,
};
use hypercolor_types::api::devices::DeviceSummary;
use hypercolor_types::api::drivers::DriverSummary;
use hypercolor_types::device::{DriverModuleKind, DriverProtocolDescriptor};

/// The slice of a daemon driver summary the partition decision needs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DriverFacts {
    /// Stable driver id (`razer`, `openrgb`, ...).
    pub id: String,
    /// Registry category of the module.
    pub module_kind: DriverModuleKind,
    /// Whether the user has the driver enabled.
    pub enabled: bool,
    /// USB hardware the driver's protocol catalog can claim, whether or not
    /// any of it is connected. Empty when the daemon published none (or the
    /// facts predate the field), which keeps the driver's detector family on
    /// the conservative name-prefix rule.
    #[serde(default)]
    pub usb: UsbClaim,
}

impl From<&DriverSummary> for DriverFacts {
    fn from(summary: &DriverSummary) -> Self {
        Self {
            id: summary.descriptor.id.clone(),
            module_kind: summary.descriptor.module_kind,
            enabled: summary.enabled,
            usb: usb_claim(&summary.protocols),
        }
    }
}

/// The USB hardware a protocol catalog claims. A protocol with a vendor id
/// but no product id claims the whole vendor; one with neither (SMBus,
/// network) claims no USB hardware.
fn usb_claim(protocols: &[DriverProtocolDescriptor]) -> UsbClaim {
    let mut claim = UsbClaim::default();
    for protocol in protocols {
        match (protocol.vendor_id, protocol.product_id) {
            (Some(vendor_id), Some(product_id)) => {
                claim
                    .devices
                    .insert(UsbDeviceId::new(vendor_id, product_id));
            }
            (Some(vendor_id), None) => {
                claim.vendors.insert(vendor_id);
            }
            (None, _) => {}
        }
    }
    claim
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
    /// USB hardware any withheld native driver can claim, including drivers
    /// the detector table has no family for (QMK, PrismRGB, Push 2).
    pub claimed_usb: UsbClaim,
    /// USB hardware any registered native driver can claim, withheld or not.
    pub native_usb: UsbClaim,
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
            claimed_usb: self.claimed_usb.clone(),
            native_usb: self.native_usb.clone(),
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
        .filter(|driver| known(&driver.id) && !driver.usb.is_empty())
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
        claimed_usb: withheld.iter().map(|driver| &driver.usb).collect(),
        native_usb: native().map(|driver| &driver.usb).collect(),
    }
}

/// Whether the daemon has the OpenRGB bridge driver registered and enabled.
#[must_use]
pub fn bridge_enabled(drivers: &[DriverFacts]) -> bool {
    drivers
        .iter()
        .any(|driver| driver.id == "openrgb" && driver.enabled)
}
