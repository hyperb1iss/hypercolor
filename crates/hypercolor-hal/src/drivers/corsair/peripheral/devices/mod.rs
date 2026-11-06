//! Corsair peripheral device descriptors.
//!
//! Every peripheral here is a keyboard, mouse, or desk accessory that may
//! share its USB device with live input collections, so each descriptor
//! binds a vendor lighting collection through a host-managed HID transport
//! (HIDAPI or hidraw). None claims a USB interface, and none detaches the
//! kernel's input driver.

use std::sync::LazyLock;

use crate::registry::DeviceDescriptor;

pub mod bragi;
pub mod nxp;

pub use bragi::*;
pub use nxp::*;

/// Every registered Corsair peripheral descriptor, Bragi then NXP/CUE.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    static DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
        let mut all =
            Vec::with_capacity(bragi::BRAGI_DESCRIPTORS.len() + nxp::NXP_DESCRIPTORS.len());
        all.extend_from_slice(bragi::BRAGI_DESCRIPTORS);
        all.extend_from_slice(nxp::NXP_DESCRIPTORS);
        all
    });

    DESCRIPTORS.as_slice()
}
