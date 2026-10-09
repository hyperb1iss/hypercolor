//! OpenRGB detector names mapped to the USB devices each detector claims.
//!
//! The map in `data/detector_usb_ids.toml` is generated from the udev rules
//! an unmodified OpenRGB binary prints (`openrgb --print-udev-rules`), whose
//! sections are headed by detector names spelled exactly as the
//! `Detectors.detectors` map in `OpenRGB.json` spells them. It lets the
//! detector partition disable only the detectors whose hardware a native
//! driver can claim. Detectors with no USB id (SMBus RAM and mainboard
//! controllers) are absent and stay on the name-prefix rule.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::error::{HostError, Result};

const EMBEDDED_DETECTOR_USB_IDS_TOML: &str = include_str!("../data/detector_usb_ids.toml");

/// One USB device, by vendor and product id.
///
/// Serializes as `"vvvv:pppp"` in lowercase hex, the spelling the id map and
/// `lsusb` use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UsbDeviceId {
    /// USB vendor id.
    pub vendor_id: u16,
    /// USB product id.
    pub product_id: u16,
}

impl UsbDeviceId {
    /// A device id from its vendor and product halves.
    #[must_use]
    pub const fn new(vendor_id: u16, product_id: u16) -> Self {
        Self {
            vendor_id,
            product_id,
        }
    }
}

impl fmt::Display for UsbDeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor_id, self.product_id)
    }
}

impl FromStr for UsbDeviceId {
    type Err = HostError;

    fn from_str(text: &str) -> Result<Self> {
        let (vendor, product) = text
            .split_once(':')
            .ok_or_else(|| HostError::DetectorUsbIds(format!("{text:?} is not vvvv:pppp")))?;
        Ok(Self::new(
            parse_hex_id(vendor, text)?,
            parse_hex_id(product, text)?,
        ))
    }
}

impl TryFrom<String> for UsbDeviceId {
    type Error = HostError;

    fn try_from(text: String) -> Result<Self> {
        text.parse()
    }
}

impl From<UsbDeviceId> for String {
    fn from(id: UsbDeviceId) -> Self {
        id.to_string()
    }
}

/// USB hardware an OpenRGB detector or a native driver can claim: exact
/// devices plus vendors claimed whole.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbClaim {
    /// Exact devices.
    #[serde(default)]
    pub devices: BTreeSet<UsbDeviceId>,
    /// Vendors whose every product is claimed. OpenRGB probes these by HID
    /// usage rather than product id; a native protocol with a vendor id but
    /// no product id lands here too.
    #[serde(default)]
    pub vendors: BTreeSet<u16>,
}

impl UsbClaim {
    /// Whether nothing is claimed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty() && self.vendors.is_empty()
    }

    /// Whether the two claims could reach the same physical device.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        !self.devices.is_disjoint(&other.devices)
            || !self.vendors.is_disjoint(&other.vendors)
            || claims_vendor_of(&self.vendors, &other.devices)
            || claims_vendor_of(&other.vendors, &self.devices)
    }

    /// Add everything `other` claims.
    pub fn merge(&mut self, other: &Self) {
        self.devices.extend(other.devices.iter().copied());
        self.vendors.extend(other.vendors.iter().copied());
    }
}

impl<'a> FromIterator<&'a UsbClaim> for UsbClaim {
    fn from_iter<I: IntoIterator<Item = &'a UsbClaim>>(claims: I) -> Self {
        let mut merged = Self::default();
        for claim in claims {
            merged.merge(claim);
        }
        merged
    }
}

fn claims_vendor_of(vendors: &BTreeSet<u16>, devices: &BTreeSet<UsbDeviceId>) -> bool {
    !vendors.is_empty() && devices.iter().any(|id| vendors.contains(&id.vendor_id))
}

#[derive(Debug, Deserialize)]
struct DetectorUsbIdTable {
    #[serde(default)]
    detectors: BTreeMap<String, Vec<String>>,
}

static DETECTOR_USB_IDS: LazyLock<BTreeMap<String, UsbClaim>> = LazyLock::new(|| {
    parse_detector_usb_ids(EMBEDDED_DETECTOR_USB_IDS_TOML)
        .expect("embedded data/detector_usb_ids.toml must parse; run the crate tests")
});

/// Parse a detector USB id map in the `data/detector_usb_ids.toml` schema.
///
/// Each value lists `vvvv:pppp` device ids or `vvvv:*` vendor wildcards.
///
/// # Errors
///
/// Returns [`HostError::DetectorUsbIds`] when the TOML is malformed, an id is
/// not four-digit hex, or a detector lists no id at all.
pub fn parse_detector_usb_ids(text: &str) -> Result<BTreeMap<String, UsbClaim>> {
    let table: DetectorUsbIdTable =
        toml::from_str(text).map_err(|error| HostError::DetectorUsbIds(error.to_string()))?;
    table
        .detectors
        .into_iter()
        .map(|(name, patterns)| {
            if patterns.is_empty() {
                return Err(HostError::DetectorUsbIds(format!(
                    "detector {name:?} lists no USB id"
                )));
            }
            let mut claim = UsbClaim::default();
            for pattern in &patterns {
                match pattern.split_once(':') {
                    Some((vendor, "*")) => {
                        claim.vendors.insert(parse_hex_id(vendor, pattern)?);
                    }
                    _ => {
                        claim.devices.insert(pattern.parse()?);
                    }
                }
            }
            Ok((name, claim))
        })
        .collect()
}

/// The embedded detector USB id map, keyed by exact OpenRGB detector name.
#[must_use]
pub fn detector_usb_ids() -> &'static BTreeMap<String, UsbClaim> {
    &DETECTOR_USB_IDS
}

/// The USB hardware the named detector claims, when the embedded map knows
/// the name. Lookup is exact: OpenRGB keys its detector map by exact name.
#[must_use]
pub fn detector_usb_claim(name: &str) -> Option<&'static UsbClaim> {
    DETECTOR_USB_IDS.get(name)
}

fn parse_hex_id(half: &str, whole: &str) -> Result<u16> {
    if half.len() != 4 || !half.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HostError::DetectorUsbIds(format!(
            "{whole:?} needs four hex digits per half"
        )));
    }
    u16::from_str_radix(half, 16)
        .map_err(|error| HostError::DetectorUsbIds(format!("{whole:?}: {error}")))
}
