//! Top-level Corsair descriptor aggregation.

use std::sync::LazyLock;

use crate::registry::DeviceDescriptor;

use super::{lcd, lighting_node, link, peripheral};

/// All Corsair device descriptors currently supported by HAL.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    static DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
        let mut all = Vec::new();
        all.extend_from_slice(link::devices::descriptors());
        all.extend_from_slice(lcd::devices::descriptors());
        all.extend_from_slice(lighting_node::devices::descriptors());
        all.extend_from_slice(peripheral::devices::descriptors());
        all
    });

    DESCRIPTORS.as_slice()
}
