//! Portable device identity, carried as typed provenance.
//!
//! A [`PortableIdentityClaim`] is a scanner's proof that a device carries a
//! stable, OS-independent identity. Claims are constructed once, where the
//! deciding fact is still known (the USB scanner knows whether it read a
//! string descriptor; a fingerprint string no longer does), and are carried
//! through discovery rather than re-derived downstream.
//!
//! Refusal is the default. A scanner that cannot prove a stable identity
//! emits no claim, and the device re-binds per machine, which is the honest
//! outcome for hardware that cannot identify itself. `SMBus` devices have no
//! constructor here on purpose: a bus path plus address is a location on one
//! host, never an identity.
//!
//! Every constructor canonicalizes and validates before a
//! [`PortableDeviceKey`] exists, so an un-normalized or placeholder value
//! cannot become a key. Variants pair each source with exactly the evidence
//! that source can produce, so an impossible combination (a USB serial with
//! a network peer, say) is unrepresentable.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Canonical, OS-independent identity key for one physical device.
///
/// Only claim constructors mint these, so holding one implies the value has
/// been canonicalized and passed the refusal rules for its source.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PortableDeviceKey(String);

impl PortableDeviceKey {
    /// The canonical key text, for storage and comparison.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PortableDeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a portable identity came from.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortableIdentitySource {
    /// USB string descriptor read from the device.
    UsbSerial,
    /// Burned-in hardware MAC.
    MacAddress,
    /// Hue bridge id (MAC-derived, fixed for the life of the bridge).
    HueBridgeId,
    /// Nanoleaf controller serial. Never a device name and never an address.
    NanoleafSerial,
}

/// How network-sourced hardware was reachable when it was observed.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAttachment {
    /// A link-local peer observed by this host. Weak evidence: an address
    /// can be reassigned, so it distinguishes concurrently reachable peers
    /// and nothing more.
    Peer(IpAddr),
    /// Known only through a vendor's cloud inventory, with no local
    /// attachment at all. A claim whose observations are all cloud-sourced
    /// can never be proven collided automatically.
    CloudInventory,
}

/// A scanner's proof that a device carries a stable, OS-independent
/// identity.
///
/// Variants are `non_exhaustive` so construction outside this crate goes
/// through the checked constructors; matching stays open for readers. The
/// `raw` field keeps the value as the device reported it, for diagnostics
/// and for showing a human what actually differed when canonical keys
/// collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "source")]
pub enum PortableIdentityClaim {
    /// A USB serial that a registered per-vendor normalizer accepted.
    #[non_exhaustive]
    UsbSerial {
        /// Canonical key, `usb:{vid:04x}:{pid:04x}:{canonical_serial}`.
        key: PortableDeviceKey,
        /// The serial exactly as the device reported it.
        raw: String,
        /// Bus location at observation time. Machine-scoped; two
        /// simultaneously present units cannot share one, which is what
        /// makes collision detection possible.
        topology: String,
    },
    /// A globally administered unicast hardware MAC.
    #[non_exhaustive]
    MacAddress {
        /// Canonical key, `net:` plus twelve lowercase hex characters.
        key: PortableDeviceKey,
        /// The encoding the scanner actually saw.
        raw: String,
        /// How the device was reachable when observed.
        attachment: NetworkAttachment,
    },
    /// A Hue bridge id.
    #[non_exhaustive]
    HueBridgeId {
        /// Canonical key, `hue:{bridge id, lowercased}`.
        key: PortableDeviceKey,
        /// The id as the bridge reported it.
        raw: String,
        /// The bridge's address when observed.
        peer: IpAddr,
    },
    /// A Nanoleaf controller serial (`serial_no` or the API's `device_id`).
    #[non_exhaustive]
    NanoleafSerial {
        /// Canonical key, `nanoleaf:{serial, lowercased}`.
        key: PortableDeviceKey,
        /// The serial as the controller reported it.
        raw: String,
        /// The controller's address when observed.
        peer: IpAddr,
    },
}

impl PortableIdentityClaim {
    /// Claims a USB serial through the host's per-vendor normalizer
    /// registry.
    ///
    /// Returns `None` when the `(vendor, product)` pair has no registered
    /// normalizer, or when the serial is a refused placeholder, because an
    /// unreviewed or constant serial admitted as identity would merge two
    /// physical devices into one account-wide record.
    #[must_use]
    pub fn usb_serial(
        vendor_id: u16,
        product_id: u16,
        raw_serial: &str,
        topology: impl Into<String>,
        registry: &SerialNormalizerRegistry,
    ) -> Option<Self> {
        let canonical = registry.normalize(vendor_id, product_id, raw_serial)?;

        Some(Self::UsbSerial {
            key: PortableDeviceKey(format!("usb:{vendor_id:04x}:{product_id:04x}:{canonical}")),
            raw: raw_serial.to_owned(),
            topology: topology.into(),
        })
    }

    /// Claims a hardware MAC in any common encoding.
    ///
    /// Accepts colon, hyphen, and dot separators as well as bare hex, so
    /// WLED's unseparated string and a service-prefixed value stripped by
    /// the scanner land on the same key. Returns `None` for anything that
    /// is not exactly twelve hex characters after separator removal, for
    /// the all-zero address, for multicast addresses, and for locally
    /// administered addresses, which are commonly randomized per network
    /// or per boot and would bind an account-wide identity to a value the
    /// device is free to change.
    #[must_use]
    pub fn mac_address(raw_mac: &str, attachment: NetworkAttachment) -> Option<Self> {
        let canonical = canonical_mac(raw_mac)?;

        Some(Self::MacAddress {
            key: PortableDeviceKey(format!("net:{canonical}")),
            raw: raw_mac.to_owned(),
            attachment,
        })
    }

    /// Claims a Hue bridge id.
    #[must_use]
    pub fn hue_bridge_id(raw_bridge_id: &str, peer: IpAddr) -> Option<Self> {
        let canonical = plausible_serial(raw_bridge_id)?.to_lowercase();

        Some(Self::HueBridgeId {
            key: PortableDeviceKey(format!("hue:{canonical}")),
            raw: raw_bridge_id.to_owned(),
            peer,
        })
    }

    /// Claims a Nanoleaf controller serial.
    ///
    /// The caller must pass `serial_no` or the API's `device_id`, never a
    /// device name and never an address; this constructor can validate
    /// shape but cannot know which field the scanner read.
    #[must_use]
    pub fn nanoleaf_serial(raw_serial: &str, peer: IpAddr) -> Option<Self> {
        let canonical = plausible_serial(raw_serial)?.to_lowercase();

        Some(Self::NanoleafSerial {
            key: PortableDeviceKey(format!("nanoleaf:{canonical}")),
            raw: raw_serial.to_owned(),
            peer,
        })
    }

    /// The canonical key this claim proves.
    #[must_use]
    pub fn key(&self) -> &PortableDeviceKey {
        match self {
            Self::UsbSerial { key, .. }
            | Self::MacAddress { key, .. }
            | Self::HueBridgeId { key, .. }
            | Self::NanoleafSerial { key, .. } => key,
        }
    }

    /// Where this identity came from.
    #[must_use]
    pub fn source(&self) -> PortableIdentitySource {
        match self {
            Self::UsbSerial { .. } => PortableIdentitySource::UsbSerial,
            Self::MacAddress { .. } => PortableIdentitySource::MacAddress,
            Self::HueBridgeId { .. } => PortableIdentitySource::HueBridgeId,
            Self::NanoleafSerial { .. } => PortableIdentitySource::NanoleafSerial,
        }
    }

    /// The value as the device reported it, before normalization.
    #[must_use]
    pub fn raw(&self) -> &str {
        match self {
            Self::UsbSerial { raw, .. }
            | Self::MacAddress { raw, .. }
            | Self::HueBridgeId { raw, .. }
            | Self::NanoleafSerial { raw, .. } => raw,
        }
    }
}

/// Canonicalization the host has reviewed for one `(vendor, product)` pair.
///
/// The default for an unregistered pair is refusal, because trimming and
/// preserving case cannot support a cross-OS claim in general: NUL padding,
/// non-ASCII whitespace, and OS stacks that disagree on case each produce
/// different keys for one device, while lowercasing everything would merge
/// devices whose serials legitimately differ only by case. Registering a
/// pair is an assertion that its serial behavior has been checked on the
/// OS stacks we support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialNormalization {
    /// Trim ASCII whitespace and NUL padding, require at least four
    /// printable ASCII characters, preserve case. For vendors verified to
    /// report byte-identical serials on every supported OS stack.
    TrimmedAscii,
    /// [`Self::TrimmedAscii`], then lowercase. For vendors whose OS stacks
    /// disagree only on case.
    LowercasedAscii,
}

/// Host-owned registry mapping `(vendor, product)` pairs to their reviewed
/// serial normalization.
#[derive(Debug, Clone, Default)]
pub struct SerialNormalizerRegistry {
    entries: HashMap<(u16, u16), SerialNormalization>,
}

impl SerialNormalizerRegistry {
    /// An empty registry, which refuses every serial.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the reviewed normalization for one `(vendor, product)`
    /// pair, replacing any previous entry.
    pub fn register(
        &mut self,
        vendor_id: u16,
        product_id: u16,
        normalization: SerialNormalization,
    ) {
        self.entries.insert((vendor_id, product_id), normalization);
    }

    /// The registered normalization for a pair, if any.
    #[must_use]
    pub fn get(&self, vendor_id: u16, product_id: u16) -> Option<SerialNormalization> {
        self.entries.get(&(vendor_id, product_id)).copied()
    }

    /// Canonicalizes a raw serial for a pair, refusing placeholders first
    /// and unregistered pairs always.
    #[must_use]
    pub fn normalize(&self, vendor_id: u16, product_id: u16, raw_serial: &str) -> Option<String> {
        let plausible = plausible_serial(raw_serial)?;
        let normalization = self.get(vendor_id, product_id)?;
        let trimmed: String = plausible
            .trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0')
            .to_owned();
        if trimmed.len() < MIN_SERIAL_LEN || !trimmed.chars().all(|c| c.is_ascii_graphic()) {
            return None;
        }

        match normalization {
            SerialNormalization::TrimmedAscii => Some(trimmed),
            SerialNormalization::LowercasedAscii => Some(trimmed.to_lowercase()),
        }
    }
}

const MIN_SERIAL_LEN: usize = 4;

/// Vendors ship production runs with a constant serial often enough that
/// accepting one would merge two physical devices into a single identity.
const PLACEHOLDER_SERIALS: &[&str] = &[
    "unknown",
    "default",
    "none",
    "n/a",
    "null",
    "123456789",
    "0123456789",
    "1234567890",
];

/// Refuses placeholder serials before any normalizer runs. Returns the
/// trimmed value when it survives.
fn plausible_serial(raw: &str) -> Option<&str> {
    let trimmed = raw.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    if trimmed.len() < MIN_SERIAL_LEN {
        return None;
    }
    if trimmed.chars().all(|c| c == '0') {
        return None;
    }
    let lowered = trimmed.to_lowercase();
    if PLACEHOLDER_SERIALS.contains(&lowered.as_str()) {
        return None;
    }

    Some(trimmed)
}

/// Normalizes every accepted MAC encoding into twelve lowercase hex
/// characters, or refuses it.
fn canonical_mac(raw: &str) -> Option<String> {
    let hex: String = raw
        .trim()
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.'))
        .collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let canonical = hex.to_lowercase();
    if canonical.chars().all(|c| c == '0') {
        return None;
    }

    let first_octet = u8::from_str_radix(canonical.get(0..2)?, 16).ok()?;
    // Multicast (bit 0) can never name one device; locally administered
    // (bit 1) is frequently randomized per network or per boot.
    if first_octet & 0b0000_0011 != 0 {
        return None;
    }

    Some(canonical)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    const PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40));

    fn registry() -> SerialNormalizerRegistry {
        let mut registry = SerialNormalizerRegistry::new();
        registry.register(0x1532, 0x0226, SerialNormalization::TrimmedAscii);
        registry.register(0x1b1c, 0x0c10, SerialNormalization::LowercasedAscii);
        registry
    }

    #[test]
    fn linux_and_windows_usb_serial_reads_land_on_one_key() {
        // Linux hidraw preserves NUL padding; Windows pads with spaces.
        let linux = PortableIdentityClaim::usb_serial(
            0x1532,
            0x0226,
            "PM2332H12345678\0\0",
            "usb-1.4.2",
            &registry(),
        )
        .expect("registered vendor with a real serial");
        let windows = PortableIdentityClaim::usb_serial(
            0x1532,
            0x0226,
            "PM2332H12345678  ",
            "USB\\VID_1532&PID_0226\\7&2f",
            &registry(),
        )
        .expect("registered vendor with a real serial");

        assert_eq!(linux.key(), windows.key());
        assert_eq!(linux.key().as_str(), "usb:1532:0226:PM2332H12345678");
        assert_eq!(linux.raw(), "PM2332H12345678\0\0");
    }

    #[test]
    fn case_folding_is_per_vendor_not_global() {
        let folded =
            PortableIdentityClaim::usb_serial(0x1b1c, 0x0c10, "AB12CD34", "usb-1.2", &registry())
                .expect("registered vendor");
        let preserved =
            PortableIdentityClaim::usb_serial(0x1532, 0x0226, "AB12CD34", "usb-1.2", &registry())
                .expect("registered vendor");

        assert_eq!(folded.key().as_str(), "usb:1b1c:0c10:ab12cd34");
        assert_eq!(preserved.key().as_str(), "usb:1532:0226:AB12CD34");
    }

    #[test]
    fn unregistered_vendor_pairs_yield_no_claim() {
        let claim = PortableIdentityClaim::usb_serial(
            0xffff,
            0x0001,
            "REAL-SERIAL-1",
            "usb-1.4",
            &registry(),
        );

        assert!(claim.is_none());
    }

    #[test]
    fn placeholder_serials_are_refused_for_registered_vendors() {
        for placeholder in ["0123456789", "unknown", "None", "0000000", "AB1", "n/a"] {
            let claim = PortableIdentityClaim::usb_serial(
                0x1532,
                0x0226,
                placeholder,
                "usb-1.4",
                &registry(),
            );

            assert!(claim.is_none(), "accepted placeholder {placeholder:?}");
        }
    }

    #[test]
    fn every_mac_encoding_lands_on_one_key() {
        let colons =
            PortableIdentityClaim::mac_address("2C:F4:32:11:22:33", NetworkAttachment::Peer(PEER));
        let bare =
            PortableIdentityClaim::mac_address("2CF432112233", NetworkAttachment::Peer(PEER));
        let hyphens = PortableIdentityClaim::mac_address(
            "2c-f4-32-11-22-33",
            NetworkAttachment::CloudInventory,
        );
        let dotted =
            PortableIdentityClaim::mac_address("2cf4.3211.2233", NetworkAttachment::Peer(PEER));

        let keys: Vec<String> = [colons, bare, hyphens, dotted]
            .into_iter()
            .map(|claim| claim.expect("valid unicast MAC").key().as_str().to_owned())
            .collect();

        assert!(keys.iter().all(|key| key == "net:2cf432112233"));
    }

    #[test]
    fn unstable_and_malformed_macs_are_refused() {
        for raw in [
            "00:00:00:00:00:00",
            "01:00:5e:11:22:33",
            "02:f4:32:11:22:33",
            "2C:F4:32:11:22",
            "2C:F4:32:11:22:33:44",
            "zz:f4:32:11:22:33",
        ] {
            let claim = PortableIdentityClaim::mac_address(raw, NetworkAttachment::Peer(PEER));

            assert!(claim.is_none(), "accepted {raw:?}");
        }
    }

    #[test]
    fn hue_and_nanoleaf_ids_lowercase_into_stable_keys() {
        let hue =
            PortableIdentityClaim::hue_bridge_id("001788FFFE23A4B5", PEER).expect("real bridge id");
        let nanoleaf =
            PortableIdentityClaim::nanoleaf_serial("S19122A1234", PEER).expect("real serial");

        assert_eq!(hue.key().as_str(), "hue:001788fffe23a4b5");
        assert_eq!(hue.source(), PortableIdentitySource::HueBridgeId);
        assert_eq!(nanoleaf.key().as_str(), "nanoleaf:s19122a1234");
    }

    #[test]
    fn claims_round_trip_through_serde() {
        let claim = PortableIdentityClaim::mac_address(
            "2C:F4:32:11:22:33",
            NetworkAttachment::CloudInventory,
        )
        .expect("valid unicast MAC");
        let json = serde_json::to_string(&claim).expect("serializes");
        let back: PortableIdentityClaim = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, claim);
    }
}
