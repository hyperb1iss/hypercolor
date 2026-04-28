//! `SMBus` protocol registry for HAL-managed controllers.

use std::path::Path;

pub use crate::drivers::asus::{AuraSmBusProbeError as SmBusProbeError, SmBusProbe};
use crate::protocol::Protocol;

pub const ASUS_AURA_SMBUS_PROTOCOL_ID: &str = "asus/aura-smbus";

pub async fn probe_smbus_devices_in_root(
    dev_root: &Path,
) -> Result<Vec<SmBusProbe>, SmBusProbeError> {
    crate::drivers::asus::probe_asus_smbus_devices_in_root(dev_root).await
}

#[must_use]
pub fn build_smbus_protocol(protocol_id: &str) -> Option<Box<dyn Protocol>> {
    match protocol_id {
        ASUS_AURA_SMBUS_PROTOCOL_ID => Some(crate::drivers::asus::build_aura_smbus_protocol()),
        _ => None,
    }
}
